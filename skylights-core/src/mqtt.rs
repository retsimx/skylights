//! MQTT protocol contract for the Skylights controller.
//!
//! This module is pure and hardware-agnostic: it owns the topic names, the
//! validated command/telemetry data types, and the zero-allocation JSON
//! parse/format functions that implement the normative protocol in issue #7.
//! It performs no I/O and depends on no transport, async-runtime, or hardware
//! crates, so every rule below is exercised by host unit tests.
//!
//! Payloads are parsed with [`serde_json_core::from_slice`] into private raw
//! structs with unvalidated `u8` fields, then range-checked into the
//! [`Index`]/[`Percentage`] newtypes. Formatting uses
//! [`serde_json_core::to_slice`] and writes into the caller's buffer.

use serde::{Deserialize, Serialize};

/// Topic carrying window target commands: `{"index":1..3,"percentage":0..100}`.
pub const TOPIC_SET: &str = "skylight/set";

/// Topic requesting a window's current position: `{"index":1..3}`.
pub const TOPIC_GET: &str = "skylight/get";

/// Topic halting a window immediately: `{"index":1..3}`.
pub const TOPIC_STOP: &str = "skylight/stop";

/// Topic triggering an OTA check. Deliberately permissive payload.
pub const TOPIC_RESET: &str = "skylight/reset";

/// Topic carrying the reply to [`TOPIC_GET`]: `{"index":N,"percentage":M}`.
pub const TOPIC_GET_RESPONSE: &str = "skylight/get/response";

/// Topic carrying periodic and event-driven telemetry.
pub const TOPIC_STATE: &str = "skylight/state";

/// Subscription wildcard covering every `skylight/*` command topic.
pub const TOPIC_WILDCARD: &str = "skylight/+";

/// Version reported in the `version` field of [`StateMessage`].
pub const STATE_VERSION: u8 = 1;

/// A window index in the inclusive range `1..=3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Index(u8);

impl Index {
    /// Creates an `Index`, returning `None` when `value` is outside `1..=3`.
    pub fn new(value: u8) -> Option<Self> {
        if (1..=3).contains(&value) {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the raw index value (`1..=3`).
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A percentage position in the inclusive range `0..=100`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Percentage(u8);

impl Percentage {
    /// Creates a `Percentage`, returning `None` when `value` is outside `0..=100`.
    pub fn new(value: u8) -> Option<Self> {
        if (0..=100).contains(&value) {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the raw percentage value (`0..=100`).
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Command delivered on [`TOPIC_SET`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCommand {
    /// Target window.
    pub index: Index,
    /// Target position percentage.
    pub percentage: Percentage,
}

/// Command delivered on [`TOPIC_GET`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetCommand {
    /// Window whose position is requested.
    pub index: Index,
}

/// Command delivered on [`TOPIC_STOP`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopCommand {
    /// Window to halt.
    pub index: Index,
}

/// Per-window telemetry inside [`StateMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WindowTelemetry {
    /// Window number (`1..=3`).
    pub index: u8,
    /// Last settled/origin position percentage.
    pub percentage: u8,
    /// Whether the window is currently travelling.
    pub moving: bool,
}

/// Telemetry published on [`TOPIC_STATE`].
///
/// Field declaration order is normative: `windows`, `version`, `wifi_rssi`,
/// `uptime_secs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StateMessage {
    /// State of each of the three windows.
    pub windows: [WindowTelemetry; 3],
    /// Protocol version, currently [`STATE_VERSION`].
    pub version: u8,
    /// Connected-AP RSSI in dBm; placeholder `0` means "unknown".
    pub wifi_rssi: i8,
    /// Seconds elapsed since boot.
    pub uptime_secs: u32,
}

/// Errors produced while parsing or formatting protocol payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttError {
    /// The payload was not valid JSON for the expected shape.
    Json,
    /// A field was syntactically valid but outside its permitted range.
    Range,
    /// The caller-provided output buffer was too small.
    Buffer,
}

#[derive(Deserialize)]
struct RawSet {
    index: u8,
    percentage: u8,
}

#[derive(Deserialize)]
struct RawIndex {
    index: u8,
}

#[derive(Serialize)]
struct RawGetResponse {
    index: u8,
    percentage: u8,
}

fn map_serialize_error(_: serde_json_core::ser::Error) -> MqttError {
    MqttError::Buffer
}

/// Parses a [`TOPIC_SET`] payload into a validated [`SetCommand`].
pub fn parse_set(payload: &[u8]) -> Result<SetCommand, MqttError> {
    let (raw, _) = serde_json_core::from_slice::<RawSet>(payload).map_err(|_| MqttError::Json)?;
    let index = Index::new(raw.index).ok_or(MqttError::Range)?;
    let percentage = Percentage::new(raw.percentage).ok_or(MqttError::Range)?;
    Ok(SetCommand { index, percentage })
}

/// Parses a [`TOPIC_GET`] payload into a validated [`GetCommand`].
pub fn parse_get(payload: &[u8]) -> Result<GetCommand, MqttError> {
    let (raw, _) = serde_json_core::from_slice::<RawIndex>(payload).map_err(|_| MqttError::Json)?;
    let index = Index::new(raw.index).ok_or(MqttError::Range)?;
    Ok(GetCommand { index })
}

/// Parses a [`TOPIC_STOP`] payload into a validated [`StopCommand`].
pub fn parse_stop(payload: &[u8]) -> Result<StopCommand, MqttError> {
    let (raw, _) = serde_json_core::from_slice::<RawIndex>(payload).map_err(|_| MqttError::Json)?;
    let index = Index::new(raw.index).ok_or(MqttError::Range)?;
    Ok(StopCommand { index })
}

/// Parses a [`TOPIC_RESET`] payload.
///
/// Per the issue #7 contract the reset payload is `reset` **or any string**;
/// its content is ignored. This function is therefore deliberately permissive
/// and always returns `Ok(())`, including for empty payloads.
pub fn parse_reset(_payload: &[u8]) -> Result<(), MqttError> {
    Ok(())
}

/// Formats a [`TOPIC_GET_RESPONSE`] payload as `{"index":N,"percentage":M}`.
///
/// Returns the number of bytes written into `out`.
pub fn format_get_response(
    index: Index,
    percentage: Percentage,
    out: &mut [u8],
) -> Result<usize, MqttError> {
    let raw = RawGetResponse {
        index: index.get(),
        percentage: percentage.get(),
    };
    serde_json_core::to_slice(&raw, out).map_err(map_serialize_error)
}

/// Formats a [`TOPIC_STATE`] payload from `state`.
///
/// Returns the number of bytes written into `out`.
pub fn format_state(state: &StateMessage, out: &mut [u8]) -> Result<usize, MqttError> {
    serde_json_core::to_slice(state, out).map_err(map_serialize_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(value: u8) -> Index {
        Index::new(value).expect("test index in range")
    }

    fn percentage(value: u8) -> Percentage {
        Percentage::new(value).expect("test percentage in range")
    }

    #[test]
    fn parse_set_accepts_valid_payloads() {
        assert_eq!(
            parse_set(br#"{"index":1,"percentage":0}"#),
            Ok(SetCommand {
                index: index(1),
                percentage: percentage(0)
            })
        );
        assert_eq!(
            parse_set(br#"{"index":2,"percentage":50}"#),
            Ok(SetCommand {
                index: index(2),
                percentage: percentage(50)
            })
        );
        assert_eq!(
            parse_set(br#"{"index":3,"percentage":100}"#),
            Ok(SetCommand {
                index: index(3),
                percentage: percentage(100)
            })
        );
    }

    #[test]
    fn parse_get_and_stop_accept_valid_payloads() {
        assert_eq!(
            parse_get(br#"{"index":1}"#),
            Ok(GetCommand { index: index(1) })
        );
        assert_eq!(
            parse_get(br#"{"index":3}"#),
            Ok(GetCommand { index: index(3) })
        );
        assert_eq!(
            parse_stop(br#"{"index":2}"#),
            Ok(StopCommand { index: index(2) })
        );
    }

    #[test]
    fn parse_set_rejects_out_of_range_index() {
        for value in [0u8, 4, 255] {
            let payload = format!("{{\"index\":{value},\"percentage\":50}}");
            assert_eq!(parse_set(payload.as_bytes()), Err(MqttError::Range));
        }
    }

    #[test]
    fn parse_set_rejects_out_of_range_percentage() {
        for value in [101u8, 255] {
            let payload = format!("{{\"index\":1,\"percentage\":{value}}}");
            assert_eq!(parse_set(payload.as_bytes()), Err(MqttError::Range));
        }
    }

    #[test]
    fn parse_get_and_stop_reject_out_of_range_index() {
        for value in [0u8, 4, 255] {
            let payload = format!("{{\"index\":{value}}}");
            assert_eq!(parse_get(payload.as_bytes()), Err(MqttError::Range));
            assert_eq!(parse_stop(payload.as_bytes()), Err(MqttError::Range));
        }
    }

    #[test]
    fn parse_rejects_garbage_and_truncated_payloads() {
        assert_eq!(parse_set(b"not json"), Err(MqttError::Json));
        assert_eq!(parse_set(b""), Err(MqttError::Json));
        assert_eq!(parse_set(br#"{"index":1,"#), Err(MqttError::Json));
        assert_eq!(
            parse_set(br#"{"index":"a","percentage":1}"#),
            Err(MqttError::Json)
        );
        assert_eq!(
            parse_set(br#"{"index":1,"percentage":50}extra"#),
            Err(MqttError::Json)
        );
        assert_eq!(parse_set(br#"{"percentage":50}"#), Err(MqttError::Json));
        assert_eq!(parse_get(b"{"), Err(MqttError::Json));
        assert_eq!(parse_stop(b"[]"), Err(MqttError::Json));
    }

    #[test]
    fn parse_reset_accepts_empty_and_any_payload() {
        assert_eq!(parse_reset(b""), Ok(()));
        assert_eq!(parse_reset(b"reset"), Ok(()));
        assert_eq!(parse_reset(br#"{"anything":true}"#), Ok(()));
    }

    #[test]
    fn format_get_response_is_byte_exact() {
        let mut out = [0u8; 64];
        let len = format_get_response(index(1), percentage(50), &mut out).unwrap();
        assert_eq!(&out[..len], br#"{"index":1,"percentage":50}"#);
    }

    #[test]
    fn format_get_response_handles_bounds() {
        let mut out = [0u8; 64];
        let len = format_get_response(index(3), percentage(100), &mut out).unwrap();
        assert_eq!(&out[..len], br#"{"index":3,"percentage":100}"#);
    }

    #[test]
    fn format_state_is_byte_exact() {
        let state = StateMessage {
            windows: [
                WindowTelemetry {
                    index: 1,
                    percentage: 0,
                    moving: false,
                },
                WindowTelemetry {
                    index: 2,
                    percentage: 50,
                    moving: false,
                },
                WindowTelemetry {
                    index: 3,
                    percentage: 100,
                    moving: false,
                },
            ],
            version: STATE_VERSION,
            wifi_rssi: -65,
            uptime_secs: 120,
        };
        let mut out = [0u8; 256];
        let len = format_state(&state, &mut out).unwrap();
        let expected = concat!(
            r#"{"windows":["#,
            r#"{"index":1,"percentage":0,"moving":false},"#,
            r#"{"index":2,"percentage":50,"moving":false},"#,
            r#"{"index":3,"percentage":100,"moving":false}],"#,
            r#""version":1,"wifi_rssi":-65,"uptime_secs":120}"#
        );
        assert_eq!(&out[..len], expected.as_bytes());
    }

    #[test]
    fn format_state_serializes_moving_true() {
        let state = StateMessage {
            windows: [
                WindowTelemetry {
                    index: 1,
                    percentage: 10,
                    moving: true,
                },
                WindowTelemetry {
                    index: 2,
                    percentage: 20,
                    moving: false,
                },
                WindowTelemetry {
                    index: 3,
                    percentage: 30,
                    moving: true,
                },
            ],
            version: STATE_VERSION,
            wifi_rssi: 0,
            uptime_secs: 1,
        };
        let mut out = [0u8; 256];
        let len = format_state(&state, &mut out).unwrap();
        let expected = concat!(
            r#"{"windows":["#,
            r#"{"index":1,"percentage":10,"moving":true},"#,
            r#"{"index":2,"percentage":20,"moving":false},"#,
            r#"{"index":3,"percentage":30,"moving":true}],"#,
            r#""version":1,"wifi_rssi":0,"uptime_secs":1}"#
        );
        assert_eq!(&out[..len], expected.as_bytes());
    }

    #[test]
    fn format_get_response_reports_undersized_buffer() {
        let mut out = [0u8; 4];
        assert_eq!(
            format_get_response(index(1), percentage(50), &mut out),
            Err(MqttError::Buffer)
        );
    }

    #[test]
    fn format_state_reports_undersized_buffer() {
        let state = StateMessage {
            windows: [WindowTelemetry {
                index: 1,
                percentage: 0,
                moving: false,
            }; 3],
            version: STATE_VERSION,
            wifi_rssi: 0,
            uptime_secs: 0,
        };
        let mut out = [0u8; 8];
        assert_eq!(format_state(&state, &mut out), Err(MqttError::Buffer));
    }

    #[test]
    fn newtypes_enforce_ranges() {
        assert_eq!(Index::new(1).map(Index::get), Some(1));
        assert_eq!(Index::new(3).map(Index::get), Some(3));
        assert_eq!(Index::new(0), None);
        assert_eq!(Index::new(4), None);
        assert_eq!(Index::new(255), None);

        assert_eq!(Percentage::new(0).map(Percentage::get), Some(0));
        assert_eq!(Percentage::new(100).map(Percentage::get), Some(100));
        assert_eq!(Percentage::new(101), None);
        assert_eq!(Percentage::new(255), None);
    }

    #[test]
    fn topic_constants_match_contract() {
        assert_eq!(TOPIC_SET, "skylight/set");
        assert_eq!(TOPIC_GET, "skylight/get");
        assert_eq!(TOPIC_STOP, "skylight/stop");
        assert_eq!(TOPIC_RESET, "skylight/reset");
        assert_eq!(TOPIC_GET_RESPONSE, "skylight/get/response");
        assert_eq!(TOPIC_STATE, "skylight/state");
        assert_eq!(TOPIC_WILDCARD, "skylight/+");
        assert_eq!(STATE_VERSION, 1);
    }
}
