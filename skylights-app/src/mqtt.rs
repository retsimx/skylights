//! Asynchronous MQTT client backed by `minimq` over an `embassy-net` TCP socket.
//!
//! [`mqtt_task`] owns one long-lived `minimq::Session` and reconnects forever
//! without ever resetting the chip: it waits for [`WIFI_CONNECTED`], resolves
//! the broker, opens a TCP socket, performs the MQTT handshake, and subscribes
//! to `skylight/+` on every fresh session. While connected it runs a `select3`
//! over inbound publishes, a 60-second telemetry ticker, and the
//! [`STATE_CHANGED`] signal from the window controllers.
//!
//! Protocol parsing/formatting is pure and lives in `skylights-core::mqtt`;
//! this module only moves bytes and dispatches owned [`Action`]s to the shared
//! window command channels and state snapshot.

use core::net::SocketAddrV4;

use embassy_executor::task;
use embassy_futures::select::{select3, Either3};
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Receiver as WatchReceiver;
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_hal::efuse::Efuse;
use esp_println::println;
use minimq::{Buffers, ConfigBuilder, ConnectEvent, Connection, Publication, Session, TopicFilter};
use static_cell::StaticCell;

use skylights_core::mqtt::{
    format_get_response, format_state, parse_get, parse_reset, parse_set, parse_stop, Index,
    Percentage, StateMessage, WindowTelemetry, STATE_VERSION, TOPIC_GET, TOPIC_GET_RESPONSE,
    TOPIC_RESET, TOPIC_SET, TOPIC_STATE, TOPIC_STOP, TOPIC_WILDCARD,
};
use skylights_core::{LinkState, Position, ReconnectBackoff};

use crate::net::{self, WIFI_CONNECTED};
use crate::ota::OTA_TRIGGER;
use crate::secrets;
use crate::window::{WindowCommand, WindowSnapshot, STATE_CHANGED, WINDOW_COMMANDS, WINDOW_STATES};

/// Advertised MQTT keepalive, in seconds.
const KEEPALIVE_SECS: u16 = 60;

/// Telemetry heartbeat period, in seconds.
const HEARTBEAT_SECS: u64 = 60;

/// Placeholder connected-AP RSSI; `0` means "unknown" (design 007 §10).
const WIFI_RSSI_PLACEHOLDER: i8 = 0;

/// Capacity of the `skylight/get/response` encoding buffer.
const RESPONSE_BUFFER_SIZE: usize = 64;

/// Capacity of the `skylight/state` encoding buffer.
const STATE_BUFFER_SIZE: usize = 256;

/// 2048-byte class TCP receive buffer, initialised once.
static TCP_RX: StaticCell<[u8; 2048]> = StaticCell::new();

/// 2048-byte class TCP transmit buffer, initialised once.
static TCP_TX: StaticCell<[u8; 2048]> = StaticCell::new();

/// minimq inbound packet buffer, initialised once.
static MQTT_RX: StaticCell<[u8; 512]> = StaticCell::new();

/// minimq outbound packet arena, initialised once.
static MQTT_TX: StaticCell<[u8; 1024]> = StaticCell::new();

/// Storage backing the `&'static str` client identifier.
static CLIENT_ID_BYTES: StaticCell<[u8; 32]> = StaticCell::new();

/// An owned command derived from an inbound publish.
///
/// Extracting owned values lets the borrowed [`minimq::InboundPublish`] drop
/// before the connection is used again to publish a reply.
enum Action {
    /// Drive a window to a target percentage.
    Set(Index, Percentage),
    /// Reply with a window's current percentage.
    Get(Index),
    /// Halt a window.
    Stop(Index),
    /// Raise [`OTA_TRIGGER`].
    Reset,
    /// Publish periodic telemetry.
    Heartbeat,
    /// Publish telemetry after a window state change.
    StateChanged,
    /// The connection failed; leave the inner loop and reconnect.
    Disconnect,
    /// Unrecognised topic or malformed payload.
    Ignore,
}

/// Runs the MQTT client forever, reconnecting with bounded exponential backoff
/// and never resetting the chip.
#[task]
pub async fn mqtt_task(stack: Stack<'static>) -> ! {
    let tcp_rx = TCP_RX.init([0; 2048]);
    let tcp_tx = TCP_TX.init([0; 2048]);
    let mqtt_rx = MQTT_RX.init([0; 512]);
    let mqtt_tx = MQTT_TX.init([0; 1024]);
    let client_id = build_client_id(CLIENT_ID_BYTES.init([0; 32]));

    let mut session = Session::new(build_config(mqtt_rx, mqtt_tx, client_id));
    let mut socket = TcpSocket::new(stack, tcp_rx, tcp_tx);
    let boot = Instant::now();

    let Some(mut link) = WIFI_CONNECTED.receiver() else {
        println!("MQTT: WIFI_CONNECTED has no free receiver slot; MQTT disabled");
        loop {
            Timer::after(Duration::from_secs(HEARTBEAT_SECS)).await;
        }
    };

    println!("MQTT: starting with client id {client_id}");

    let mut backoff = ReconnectBackoff::new();
    loop {
        wait_for_link(&mut link).await;
        if serve_connection(&mut session, &mut socket, stack, boot).await {
            backoff.reset();
        }
        Timer::after(Duration::from_millis(backoff.next_delay_ms())).await;
    }
}

/// Resolves, connects, subscribes, and serves one MQTT session.
///
/// Returns `true` when a connection was established (so the caller resets its
/// backoff) and `false` when a pre-serve step failed. Either way the caller
/// applies the next bounded backoff delay before retrying.
async fn serve_connection(
    session: &mut Session<'static>,
    socket: &mut TcpSocket<'static>,
    stack: Stack<'static>,
    boot: Instant,
) -> bool {
    let Some(address) = net::resolve(stack, secrets::MQTT_HOST).await else {
        println!("MQTT: DNS lookup failed for {}", secrets::MQTT_HOST);
        return false;
    };

    socket.abort();
    let endpoint = SocketAddrV4::new(address, secrets::MQTT_PORT);
    if socket.connect(endpoint).await.is_err() {
        println!("MQTT: TCP connect to {endpoint} failed");
        return false;
    }

    let mut connection = match session.connect(&mut *socket).await {
        Ok(connection) => connection,
        Err(_) => {
            println!("MQTT: broker handshake failed, retrying");
            return false;
        }
    };

    if !subscribe_if_fresh(&mut connection).await {
        return false;
    }

    run_connection(&mut connection, boot).await;
    println!("MQTT: connection lost, reconnecting");
    true
}

/// Builds `skylights-<4 hex of MAC[4],MAC[5]>` into `storage`.
fn build_client_id(storage: &'static mut [u8; 32]) -> &'static str {
    const PREFIX: &[u8] = b"skylights-";
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mac = Efuse::mac_address();
    storage[..PREFIX.len()].copy_from_slice(PREFIX);
    storage[PREFIX.len()] = HEX[(mac[4] >> 4) as usize];
    storage[PREFIX.len() + 1] = HEX[(mac[4] & 0x0f) as usize];
    storage[PREFIX.len() + 2] = HEX[(mac[5] >> 4) as usize];
    storage[PREFIX.len() + 3] = HEX[(mac[5] & 0x0f) as usize];

    core::str::from_utf8(&storage[..PREFIX.len() + 4]).unwrap_or("skylights")
}

/// Builds the minimq configuration, adding auth only when configured.
fn build_config<'a>(rx: &'a mut [u8], tx: &'a mut [u8], client_id: &str) -> ConfigBuilder<'a> {
    let builder = ConfigBuilder::new(Buffers::new(rx, tx))
        .client_id(client_id)
        .expect("client id fits in minimq storage")
        .keepalive_interval(KEEPALIVE_SECS)
        .session_expiry_interval(0);

    match secrets::MQTT_USER {
        Some(user) => builder
            .auth(user, secrets::MQTT_PASSWORD.unwrap_or("").as_bytes())
            .expect("auth configured once"),
        None => builder,
    }
}

/// Blocks until the Wi-Fi supervisor reports [`LinkState::Connected`].
async fn wait_for_link(link: &mut WatchReceiver<'_, CriticalSectionRawMutex, LinkState, 2>) {
    while link.get().await != LinkState::Connected {
        link.changed().await;
    }
}

/// Subscribes on a fresh session; keeps broker subscriptions on a resume.
///
/// Returns `false` when the fresh subscribe failed, so the caller reconnects.
async fn subscribe_if_fresh<IO: minimq::Io>(connection: &mut Connection<'_, '_, IO>) -> bool {
    match connection.connect_event() {
        ConnectEvent::Connected => {
            if connection
                .subscribe(&[TopicFilter::new(TOPIC_WILDCARD)], &[])
                .await
                .is_err()
            {
                println!("MQTT: subscribe to {} failed", TOPIC_WILDCARD);
                return false;
            }
            println!("MQTT: connected, subscribed to {}", TOPIC_WILDCARD);
            true
        }
        ConnectEvent::Reconnected => {
            // The broker retained our subscriptions across a session resume, so
            // re-subscribing is unnecessary. With `session_expiry_interval(0)`
            // the broker never resumes, so this arm is defensive correctness
            // for the MQTT session-resume case, not dead code.
            println!("MQTT: reconnected, broker subscriptions retained");
            true
        }
    }
}

/// Drives one live connection until it fails, then returns to reconnect.
async fn run_connection<IO: minimq::Io>(connection: &mut Connection<'_, '_, IO>, boot: Instant) {
    let mut heartbeat = Ticker::every(Duration::from_secs(HEARTBEAT_SECS));

    loop {
        let action = match select3(connection.recv(), heartbeat.next(), STATE_CHANGED.wait()).await
        {
            Either3::First(Ok(message)) => parse_action(message.topic(), message.payload()),
            Either3::First(Err(_)) => Action::Disconnect,
            Either3::Second(()) => Action::Heartbeat,
            Either3::Third(()) => Action::StateChanged,
        };

        if matches!(action, Action::Disconnect) {
            return;
        }

        dispatch(&action, connection, boot).await;
    }
}

/// Maps an inbound topic/payload pair to an owned [`Action`].
fn parse_action(topic: &str, payload: &[u8]) -> Action {
    match topic {
        TOPIC_SET => match parse_set(payload) {
            Ok(command) => Action::Set(command.index, command.percentage),
            Err(err) => {
                println!("MQTT: rejected malformed set command: {:?}", err);
                Action::Ignore
            }
        },
        TOPIC_GET => match parse_get(payload) {
            Ok(command) => Action::Get(command.index),
            Err(err) => {
                println!("MQTT: rejected malformed get command: {:?}", err);
                Action::Ignore
            }
        },
        TOPIC_STOP => match parse_stop(payload) {
            Ok(command) => Action::Stop(command.index),
            Err(err) => {
                println!("MQTT: rejected malformed stop command: {:?}", err);
                Action::Ignore
            }
        },
        TOPIC_RESET => {
            let _ = parse_reset(payload);
            Action::Reset
        }
        // `skylight/+` also matches our own `skylight/state` publishes, so this
        // arm deliberately ignores the self-echo without error (harmless, no loop).
        _ => Action::Ignore,
    }
}

/// Executes an owned action against the live connection and shared state.
async fn dispatch<IO: minimq::Io>(
    action: &Action,
    connection: &mut Connection<'_, '_, IO>,
    boot: Instant,
) {
    match action {
        Action::Set(index, percentage) => {
            send_command(
                *index,
                WindowCommand::SetTarget(Position::new(percentage.get())),
            );
        }
        Action::Stop(index) => send_command(*index, WindowCommand::Stop),
        Action::Get(index) => publish_get_response(connection, *index).await,
        Action::Reset => {
            OTA_TRIGGER.signal(());
            println!("MQTT: OTA reset requested");
        }
        Action::Heartbeat | Action::StateChanged => publish_state(connection, boot).await,
        Action::Disconnect | Action::Ignore => {}
    }
}

/// Non-blocking dispatch of `command` to the indexed window controller.
fn send_command(index: Index, command: WindowCommand) {
    let slot = index.get() as usize - 1;
    if WINDOW_COMMANDS[slot].try_send(command).is_err() {
        println!(
            "MQTT: window {} command queue full; command dropped",
            index.get()
        );
    }
}

/// Publishes `index`'s current position on [`TOPIC_GET_RESPONSE`] (QoS 0).
async fn publish_get_response<IO: minimq::Io>(
    connection: &mut Connection<'_, '_, IO>,
    index: Index,
) {
    let snapshot = {
        let states = WINDOW_STATES.lock().await;
        states[index.get() as usize - 1]
    };
    let Some(percentage) = Percentage::new(snapshot.percentage) else {
        println!(
            "MQTT: stored percentage {} out of range",
            snapshot.percentage
        );
        return;
    };

    let mut buffer = [0u8; RESPONSE_BUFFER_SIZE];
    let Ok(length) = format_get_response(index, percentage, &mut buffer) else {
        println!("MQTT: failed to encode get response");
        return;
    };
    publish_qos0(connection, TOPIC_GET_RESPONSE, &buffer[..length]).await;
}

/// Publishes a telemetry snapshot on [`TOPIC_STATE`] (QoS 0).
async fn publish_state<IO: minimq::Io>(connection: &mut Connection<'_, '_, IO>, boot: Instant) {
    let state = build_state(boot).await;
    let mut buffer = [0u8; STATE_BUFFER_SIZE];
    let Ok(length) = format_state(&state, &mut buffer) else {
        println!("MQTT: failed to encode state telemetry");
        return;
    };
    publish_qos0(connection, TOPIC_STATE, &buffer[..length]).await;
}

/// Publishes `payload` on `topic` at QoS 0, logging any transport failure.
///
/// A QoS-0 publish is never silently discarded: the write result is checked
/// and a failure is logged, even though no acknowledgement is expected.
async fn publish_qos0<IO: minimq::Io>(
    connection: &mut Connection<'_, '_, IO>,
    topic: &str,
    payload: &[u8],
) {
    if connection
        .publish(Publication::new(topic, payload))
        .await
        .is_err()
    {
        println!("MQTT: failed to publish {topic}");
    }
}

/// Builds the telemetry message from the shared snapshots and uptime.
async fn build_state(boot: Instant) -> StateMessage {
    let snapshots = *WINDOW_STATES.lock().await;
    StateMessage {
        windows: [
            snapshot_telemetry(1, snapshots[0]),
            snapshot_telemetry(2, snapshots[1]),
            snapshot_telemetry(3, snapshots[2]),
        ],
        version: STATE_VERSION,
        wifi_rssi: WIFI_RSSI_PLACEHOLDER,
        uptime_secs: boot.elapsed().as_secs() as u32,
    }
}

/// Converts a stored [`WindowSnapshot`] into wire telemetry for `index`.
fn snapshot_telemetry(index: u8, snapshot: WindowSnapshot) -> WindowTelemetry {
    WindowTelemetry {
        index,
        percentage: snapshot.percentage,
        moving: snapshot.moving,
    }
}
