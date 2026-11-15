//! Wi-Fi station bring-up, embassy-net stack, and reconnect supervision.
//!
//! This module owns the hardware/network glue only: it starts the `esp-wifi`
//! station, builds the `embassy-net` DHCPv4 stack, and runs a background
//! supervisor that re-associates forever with bounded backoff. The bounded
//! reconnect and liveness policy itself lives in [`skylights_core`] so it can be
//! host-tested.
//!
//! The supervisor resets the chip under exactly two conditions, OR'd, because
//! "associated" is a belief and not a proof that anything gets through:
//!
//! * Clock B — no association and no DHCP lease recovered for
//!   `NO_IP_REBOOT_MS` ([`NoIpWatchdog`]). This is the radio-recovery reset.
//! * Clock A — no *completed MQTT dispatch* for `NO_IP_REBOOT_MS`, even while
//!   the driver still reports the association as up. This is the end-to-end
//!   reset; without it "alive but unreachable" stays reachable forever.
//!
//! A broker outage therefore resets the device about every ten minutes. That is
//! deliberate: end-to-end health is the guarantee, and a chip that cannot reach
//! its broker is not doing its job.
//!
//! Two accepted limits of this budget, stated so nobody re-derives them:
//!
//! * The runtime hardware watchdog is unarmed until `self_test_task` first
//!   runs, so an early-boot hang (`esp_wifi::init`) is not covered.
//! * A QoS-0 publish the socket accepts proves the write, not delivery, so the
//!   real worst case is ~10 min plus one 60 s heartbeat in flight.
//!
//! Link status is published on [`WIFI_CONNECTED`] as an `embassy-sync`
//! `Watch`, so its consumers (MQTT and the post-swap self-test) can observe the
//! latest state, including late subscribers.

use core::net::Ipv4Addr;

use embassy_executor::Spawner;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_net::dns::{DnsQueryType, DnsSocket};
use embassy_net::{Config, Runner, Stack, StackResources};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use embassy_time::{with_timeout, Duration, Instant, Ticker, Timer};
use esp_hal::peripherals::{RADIO_CLK, TIMG0, WIFI};
use esp_hal::reset::software_reset;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use esp_wifi::wifi::{
    ClientConfiguration, Configuration, WifiController, WifiDevice, WifiEvent, WifiStaDevice,
};
use esp_wifi::EspWifiController;
use static_cell::StaticCell;

use skylights_core::wifi::{
    progress_fresh, progress_stale, NoIpWatchdog, DHCP_ATTEMPTS, DHCP_TIMEOUT_MS, DNS_TIMEOUT_MS,
    JOIN_TIMEOUT_MS, LEAVE_TIMEOUT_MS, MQTT_STALE_MS, NO_IP_REBOOT_MS,
};
use skylights_core::{LinkState, ReconnectBackoff};

/// Latest link state, retained for late subscribers.
///
/// Consumers read the current value via [`Watch::receiver`].
pub static WIFI_CONNECTED: Watch<CriticalSectionRawMutex, LinkState, 2> = Watch::new();

/// Last moment the MQTT task made progress: a dispatch completed, which proves
/// the socket accepted a write end-to-end.
///
/// The supervisor and the runtime watchdog age this stamp; it is the only
/// liveness signal in the firmware that stops advancing when traffic stops
/// flowing. It is deliberately separate from `MQTT_HEALTHY`, which gates OTA
/// self-test confirmation: a wedged client can leave that flag stuck at its
/// last value, while this stamp keeps ageing.
static MQTT_PROGRESS: Watch<CriticalSectionRawMutex, Instant, 2> = Watch::new();

static WIFI_INIT: StaticCell<EspWifiController<'static>> = StaticCell::new();

/// Four socket slots. `embassy-net` holds one for DHCPv4 while the link is up
/// and [`resolve`] briefly uses one for DNS, leaving room for MQTT + OTA.
static STACK_RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

/// Raised by the MQTT task after repeated connect failures to force a rejoin,
/// covering the "associated but no traffic" case the link state never reports.
pub static WIFI_REJOIN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// How often the supervisor re-evaluates end-to-end liveness while the driver
/// still reports the association as up.
const LIVENESS_PERIOD_SECS: u64 = 30;

/// Stamps [`MQTT_PROGRESS`] with the current instant.
pub fn note_progress() {
    MQTT_PROGRESS.sender().send(Instant::now());
}

/// Age of the newest completed MQTT dispatch, or `fallback` when the MQTT task
/// has never completed one.
///
/// Reads the `Watch` directly instead of going through a receiver: no receiver
/// slot is consumed, so this can never be starved by capacity, and a direct
/// read always returns the newest stamp however long ago it was written.
pub fn progress_age(fallback: Duration) -> Duration {
    MQTT_PROGRESS.try_get().map_or(fallback, |at| at.elapsed())
}

/// Brings up the Wi-Fi station and embassy-net stack, spawns the network
/// runner and the reconnect supervisor, and returns the stack handle.
///
/// `timg0` drives `esp-wifi`; embassy time is driven by a different timer
/// group (TIMG1) initialized in `main`. The two timer groups are never reused.
///
/// The caller owns the [`Rng`] and passes it in: [`Rng`] is `Copy` over the
/// (phantom) `RNG` peripheral, so `main` can hand the same instance to both
/// this function and the OTA task without re-initialising the peripheral.
pub fn init(
    spawner: &Spawner,
    timg0: TIMG0,
    mut rng: Rng,
    radio_clk: RADIO_CLK,
    wifi: WIFI,
) -> Stack<'static> {
    let timg0 = TimerGroup::new(timg0);

    let ctrl = WIFI_INIT.init(esp_wifi::init(timg0.timer0, rng, radio_clk).unwrap());
    let (wifi_interface, controller) =
        esp_wifi::wifi::new_with_mode(ctrl, wifi, WifiStaDevice).unwrap();

    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        Config::dhcpv4(Default::default()),
        STACK_RESOURCES.init(StackResources::new()),
        seed,
    );

    if spawner.spawn(net_task(runner)).is_err() {
        println!("ERROR: failed to spawn net_task; network stack will not run");
    }
    if spawner.spawn(connection_task(controller, stack)).is_err() {
        println!("ERROR: failed to spawn connection_task; Wi-Fi will not connect");
    }

    stack
}

/// Resolves `host` to an IPv4 address using the stack's DNS client.
pub async fn resolve(stack: Stack<'static>, host: &str) -> Option<Ipv4Addr> {
    let addrs = DnsSocket::new(stack)
        .query(host, DnsQueryType::A)
        .await
        .ok()?;

    addrs
        .iter()
        .find_map(|addr| match core::net::IpAddr::from(*addr) {
            core::net::IpAddr::V4(v4) => Some(v4),
            core::net::IpAddr::V6(_) => None,
        })
}

/// Drives the embassy-net interface forever.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static, WifiStaDevice>>) {
    runner.run().await
}

/// Why one association attempt did not succeed.
enum JoinFailure {
    /// The association call did not complete within `JOIN_TIMEOUT_MS`.
    Timeout,
    /// The radio reported an association failure.
    Error,
}

/// Starts the station (if needed) and performs one bounded association attempt.
///
/// `connect_async` waits for a fresh `StaConnected`, so a station that is still
/// associated is disconnected first; the disconnect is bounded so a stalled
/// leave cannot wedge the supervisor.
async fn attempt_join(controller: &mut WifiController<'static>) -> Result<(), JoinFailure> {
    if !matches!(controller.is_started(), Ok(true)) {
        match with_timeout(
            Duration::from_millis(JOIN_TIMEOUT_MS),
            controller.start_async(),
        )
        .await
        {
            Ok(Ok(())) => {}
            _ => {
                println!("Wi-Fi: start failed, will retry");
                return Err(JoinFailure::Error);
            }
        }
    }

    if matches!(controller.is_connected(), Ok(true))
        && with_timeout(
            Duration::from_millis(LEAVE_TIMEOUT_MS),
            controller.disconnect_async(),
        )
        .await
        .is_err()
    {
        println!("Wi-Fi: disconnect did not complete; continuing");
    }

    match with_timeout(
        Duration::from_millis(JOIN_TIMEOUT_MS),
        controller.connect_async(),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(JoinFailure::Error),
        Err(_) => Err(JoinFailure::Timeout),
    }
}

/// Waits for a DHCP lease on the current association, then supervises the link.
///
/// DHCP is retried in place up to `DHCP_ATTEMPTS` times; the loop also returns
/// early on a rejoin request. A lease resets the backoff and refreshes the no-IP
/// watchdog. Returns when the link drops, a rejoin is requested, DHCP gives up,
/// or the association stops carrying MQTT traffic.
async fn handle_associated(
    controller: &mut WifiController<'static>,
    stack: Stack<'static>,
    backoff: &mut ReconnectBackoff,
    watchdog: &mut NoIpWatchdog,
) {
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match select(
            with_timeout(
                Duration::from_millis(DHCP_TIMEOUT_MS),
                stack.wait_config_up(),
            ),
            WIFI_REJOIN.wait(),
        )
        .await
        {
            Either::First(Ok(())) => break,
            Either::First(Err(_)) => {
                if attempts >= DHCP_ATTEMPTS {
                    println!("Wi-Fi: DHCP timed out, will retry");
                    return;
                }
                println!("Wi-Fi: DHCP not up yet; retrying on the same association");
            }
            Either::Second(()) => {
                println!("Wi-Fi: rejoin requested while awaiting DHCP");
                return;
            }
        }
    }

    backoff.reset();
    WIFI_CONNECTED.sender().send(LinkState::Connected);
    watchdog.note_last_good(Instant::now().as_millis());
    if let Some(config) = stack.config_v4() {
        println!("Wi-Fi connected, IPv4: {}", config.address);
    }

    let dns_probe =
        skylights_core::ota::parse_url(crate::secrets::OTA_URL, crate::secrets::OTA_PROJECT)
            .map(|uri| uri.host)
            .unwrap_or("");
    match with_timeout(
        Duration::from_millis(DNS_TIMEOUT_MS),
        resolve(stack, dns_probe),
    )
    .await
    {
        Ok(Some(addr)) => println!("DNS resolved {} -> {}", dns_probe, addr),
        Ok(None) => println!("DNS lookup failed for {}", dns_probe),
        Err(_) => println!("DNS lookup timed out for {}", dns_probe),
    }

    // End-to-end liveness, not link state: `WIFI_CONNECTED` is this task's own
    // belief and stays `Connected` through a link that has silently died, so it
    // cannot decide whether to keep trusting the association. MQTT progress is
    // the only signal here that stops advancing when traffic stops flowing, and
    // this ticker is what makes it reachable at all while the driver still
    // claims the association is up.
    let mut liveness = Ticker::every(Duration::from_secs(LIVENESS_PERIOD_SECS));
    let associated_at = Instant::now();

    loop {
        match select3(
            controller.wait_for_events(WifiEvent::StaDisconnected.into(), false),
            WIFI_REJOIN.wait(),
            liveness.next(),
        )
        .await
        {
            Either3::First(_) => {
                println!("Wi-Fi: network link dropped");
                break;
            }
            Either3::Second(()) => {
                println!("Wi-Fi: rejoin requested");
                break;
            }
            Either3::Third(()) => {
                // Clock A, end-to-end: a completed MQTT write proves the link
                // still carries traffic. A blocked write, a dead link and a
                // black-holed association all stop that stamp advancing, and
                // none of them raise a disconnect event.
                //
                // An unstamped clock (no dispatch has ever completed) counts
                // from association instead, so a client that wedges before its
                // first publish is still caught.
                let silent = progress_age(associated_at.elapsed());
                reset_if_silent(silent);
                if progress_stale(silent.as_millis()) {
                    println!(
                        "Wi-Fi: no MQTT progress for {}s; abandoning the association",
                        MQTT_STALE_MS / 1000
                    );
                    break;
                }
            }
        }
    }

    // The association is ending. Clock B starts here: `connection_task`
    // escalates to a reset if no association and lease are recovered within
    // `NO_IP_REBOOT_MS`, and stamping on the way out is what keeps a healthy
    // association from being charged to the outage that follows.
    //
    // Refreshing this stamp on every exit cannot postpone the reset that
    // matters: Clock A is stamped only by completed MQTT dispatches, which this
    // task never writes on an unhealthy association's behalf.
    watchdog.note_last_good(Instant::now().as_millis());
}

/// Clock A: resets the chip once the MQTT path has been silent for
/// [`NO_IP_REBOOT_MS`].
///
/// Both reboot clocks are OR'd and independent. This one is end-to-end, so it
/// fires for a radio that is associated with a lease while nothing gets through
/// — a state Clock B (no lease / no association) cannot see. The runtime
/// hardware watchdog is the last resort behind it, for when the executor itself
/// stops scheduling this task.
fn reset_if_silent(silent: Duration) {
    if progress_fresh(silent.as_millis()) {
        return;
    }
    println!(
        "Wi-Fi: no MQTT progress for {}s; resetting",
        NO_IP_REBOOT_MS / 1000
    );
    software_reset();
}

/// Station connection supervisor. Re-associates forever with bounded backoff;
/// resets the chip only through the two independent reboot clocks.
#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>, stack: Stack<'static>) {
    let client_config = Configuration::Client(ClientConfiguration {
        ssid: crate::secrets::WIFI_SSID
            .try_into()
            .expect("WIFI_SSID exceeds 32 bytes"),
        password: crate::secrets::WIFI_PASSWORD
            .try_into()
            .expect("WIFI_PASSWORD exceeds 64 bytes"),
        ..Default::default()
    });
    controller.set_configuration(&client_config).unwrap();

    let boot = Instant::now();
    let mut backoff = ReconnectBackoff::new();
    let mut watchdog = NoIpWatchdog::new();

    loop {
        WIFI_CONNECTED.sender().send(LinkState::Connecting);

        match attempt_join(&mut controller).await {
            Ok(()) => {
                WIFI_REJOIN.reset();
                handle_associated(&mut controller, stack, &mut backoff, &mut watchdog).await;
            }
            Err(JoinFailure::Timeout) => {
                println!("Wi-Fi: association attempt timed out, will retry")
            }
            Err(JoinFailure::Error) => println!("Wi-Fi: connect failed, will retry"),
        }

        // Clock A again, from the supervisor's own loop: a link that flaps
        // faster than the liveness ticker never reaches the in-association
        // check, and every pass here refreshes Clock B (`note_last_good`), so
        // Clock B alone could be postponed forever by a flapping access point.
        reset_if_silent(progress_age(boot.elapsed()));

        if watchdog.reboot_due(Instant::now().as_millis()) {
            println!(
                "Wi-Fi: no IP for {}s; resetting to recover the radio",
                NO_IP_REBOOT_MS / 1000
            );
            software_reset();
        }

        WIFI_CONNECTED.sender().send(LinkState::Disconnected);
        Timer::after(Duration::from_millis(backoff.next_delay_ms())).await;
    }
}
