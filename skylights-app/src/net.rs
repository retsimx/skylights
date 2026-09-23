//! Wi-Fi station bring-up, embassy-net stack, and reconnect supervision.
//!
//! This module owns the hardware/network glue only: it starts the `esp-wifi`
//! station, builds the `embassy-net` DHCPv4 stack, and runs a background
//! supervisor that re-associates forever without ever resetting the chip. The
//! bounded reconnect policy itself lives in [`skylights_core::net`] so it can
//! be host-tested.
//!
//! Link status is published on [`WIFI_CONNECTED`] as an `embassy-sync`
//! `Watch`, so any number of future consumers (MQTT, OTA) can observe the
//! latest state, including late subscribers.

use core::net::Ipv4Addr;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_net::dns::{DnsQueryType, DnsSocket};
use embassy_net::{Config, Runner, Stack, StackResources};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Watch;
use embassy_time::{with_timeout, Duration, Timer};
use esp_hal::peripherals::{RADIO_CLK, RNG, TIMG0, WIFI};
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use esp_wifi::wifi::{
    ClientConfiguration, Configuration, WifiController, WifiDevice, WifiEvent, WifiStaDevice,
};
use esp_wifi::EspWifiController;
use static_cell::StaticCell;

use skylights_core::{LinkState, ReconnectBackoff};

/// Latest link state, retained for late subscribers.
///
/// Consumers read the current value via [`Watch::receiver`].
pub static WIFI_CONNECTED: Watch<CriticalSectionRawMutex, LinkState, 2> = Watch::new();

static WIFI_INIT: StaticCell<EspWifiController<'static>> = StaticCell::new();

/// Four socket slots. `embassy-net` holds one for DHCPv4 while the link is up
/// and [`resolve`] briefly uses one for DNS, leaving room for MQTT + OTA.
static STACK_RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

/// Upper bound on DHCP address acquisition. A stalled DHCP server must not
/// block the supervisor forever, so the wait is abandoned and routed through
/// the reconnect backoff like any other failed attempt.
const DHCP_TIMEOUT: Duration = Duration::from_secs(30);

/// Brings up the Wi-Fi station and embassy-net stack, spawns the network
/// runner and the reconnect supervisor, and returns the stack handle.
///
/// `timg0` drives `esp-wifi`; embassy time is driven by a different timer
/// group (TIMG1) initialized in `main`. The two timer groups are never reused.
pub fn init(
    spawner: &Spawner,
    timg0: TIMG0,
    rng: RNG,
    radio_clk: RADIO_CLK,
    wifi: WIFI,
) -> Stack<'static> {
    let timg0 = TimerGroup::new(timg0);
    let mut rng = Rng::new(rng);

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

/// Performs one association attempt and waits for a DHCP lease.
///
/// Returns `true` when the station is associated and the stack has an IPv4
/// configuration; `false` on any failure (start, connect, DHCP timeout, or
/// link loss during DHCP), in which case the caller applies the backoff.
async fn associate(controller: &mut WifiController<'static>, stack: Stack<'static>) -> bool {
    if !matches!(controller.is_started(), Ok(true)) && controller.start_async().await.is_err() {
        println!("Wi-Fi: start failed, will retry");
        return false;
    }

    // `connect_async` waits for a fresh `StaConnected`, so a station that is
    // still associated must be disassociated first or the wait could stall.
    if matches!(controller.is_connected(), Ok(true)) {
        let _ = controller.disconnect_async().await;
    }

    match controller.connect_async().await {
        Ok(()) => {
            let dhcp = with_timeout(DHCP_TIMEOUT, stack.wait_config_up());
            match select(
                dhcp,
                controller.wait_for_events(WifiEvent::StaDisconnected.into(), false),
            )
            .await
            {
                Either::First(Ok(())) => true,
                Either::First(Err(_)) => {
                    println!("Wi-Fi: DHCP timed out, will retry");
                    false
                }
                Either::Second(_) => {
                    println!("Wi-Fi: link lost during DHCP, will retry");
                    false
                }
            }
        }
        Err(_) => {
            println!("Wi-Fi: connect failed, will retry");
            false
        }
    }
}

/// Station connection supervisor. Re-associates forever with bounded backoff
/// and never resets or restarts the chip.
#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>, stack: Stack<'static>) {
    let client_config = Configuration::Client(ClientConfiguration {
        ssid: crate::secrets::WIFI_SSID
            .try_into()
            .expect("WIFI_SSID exceeds 32 bytes"),
        password: crate::secrets::WIFI_PASS
            .try_into()
            .expect("WIFI_PASS exceeds 64 bytes"),
        ..Default::default()
    });
    controller.set_configuration(&client_config).unwrap();

    let mut backoff = ReconnectBackoff::new();

    loop {
        WIFI_CONNECTED.sender().send(LinkState::Connecting);

        if associate(&mut controller, stack).await {
            backoff.reset();
            WIFI_CONNECTED.sender().send(LinkState::Connected);
            if let Some(config) = stack.config_v4() {
                println!("Wi-Fi connected, IPv4: {}", config.address);
            }
            match resolve(stack, crate::secrets::MQTT_HOST).await {
                Some(addr) => {
                    println!("DNS resolved {} -> {}", crate::secrets::MQTT_HOST, addr)
                }
                None => println!("DNS lookup failed for {}", crate::secrets::MQTT_HOST),
            }
            let _ = controller
                .wait_for_events(WifiEvent::StaDisconnected.into(), false)
                .await;
        }

        WIFI_CONNECTED.sender().send(LinkState::Disconnected);
        Timer::after(Duration::from_millis(backoff.next_delay_ms())).await;
    }
}
