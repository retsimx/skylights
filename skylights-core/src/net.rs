//! Network link-state and reconnect-backoff policy.
//!
//! Provides a pure, hardware-agnostic model of the link lifecycle and the
//! exponential backoff used between reconnection attempts. The backoff only
//! uses integer millisecond arithmetic — no clocks, sleeps, floats, or hardware
//! dependencies — so it runs identically on the ESP32 target and on the host
//! test harness. The firmware supervisor owns the radio; this module owns only
//! the delay schedule.

use core::net::Ipv4Addr;

/// Default MQTT broker port used when `MQTT_BROKER` omits one.
pub const BROKER_PORT: u16 = 1883;

/// Parses `MQTT_BROKER`: an IPv4 literal with an optional `mqtt://` scheme and
/// an optional `:port` defaulting to [`BROKER_PORT`].
///
/// Hostnames, IPv6 literals, malformed addresses, and port `0` are rejected, so
/// the device never depends on DNS to reach the broker.
pub fn parse_broker(value: &str) -> Option<(Ipv4Addr, u16)> {
    let rest = value.strip_prefix("mqtt://").unwrap_or(value);
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (rest, BROKER_PORT),
    };
    if port == 0 {
        return None;
    }
    Some((host.parse::<Ipv4Addr>().ok()?, port))
}

/// First backoff delay after a drop, in milliseconds.
pub const BACKOFF_INITIAL_MS: u64 = 1_000;

/// Upper bound the backoff delay saturates at, in milliseconds.
pub const BACKOFF_MAX_MS: u64 = 30_000;

/// Multiplier applied to the delay after each attempt.
pub const BACKOFF_FACTOR: u32 = 2;

/// Current state of the network link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// No link; the supervisor is idle or waiting to retry.
    Disconnected,
    /// Association or address acquisition is in progress.
    Connecting,
    /// Link is up and usable.
    Connected,
}

/// Exponential reconnect backoff with a saturating cap.
///
/// Starts at [`BACKOFF_INITIAL_MS`], doubles after every [`ReconnectBackoff::next_delay_ms`]
/// call, and saturates at [`BACKOFF_MAX_MS`]. [`ReconnectBackoff::reset`] returns
/// the schedule to its initial delay after a successful connection.
#[derive(Debug, Clone, Copy)]
pub struct ReconnectBackoff {
    current_ms: u64,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconnectBackoff {
    /// Creates a backoff whose first delay is [`BACKOFF_INITIAL_MS`].
    pub const fn new() -> Self {
        Self {
            current_ms: BACKOFF_INITIAL_MS,
        }
    }

    /// Restarts the schedule at [`BACKOFF_INITIAL_MS`].
    pub fn reset(&mut self) {
        self.current_ms = BACKOFF_INITIAL_MS;
    }

    /// Returns the current delay and advances towards [`BACKOFF_MAX_MS`].
    ///
    /// The first call returns [`BACKOFF_INITIAL_MS`]; each subsequent call first
    /// returns the current delay, then doubles it, saturating at
    /// [`BACKOFF_MAX_MS`]. The exact sequence is
    /// `1000, 2000, 4000, 8000, 16000, 30000, 30000, ...`.
    pub fn next_delay_ms(&mut self) -> u64 {
        let delay = self.current_ms;
        self.current_ms = self
            .current_ms
            .saturating_mul(BACKOFF_FACTOR as u64)
            .min(BACKOFF_MAX_MS);
        delay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_broker_accepts_scheme_ipv4_and_ports() {
        assert_eq!(
            parse_broker("mqtt://10.0.0.45:1883"),
            Some((Ipv4Addr::new(10, 0, 0, 45), 1883))
        );
        assert_eq!(
            parse_broker("mqtt://192.168.1.10"),
            Some((Ipv4Addr::new(192, 168, 1, 10), BROKER_PORT))
        );
        assert_eq!(
            parse_broker("10.0.0.45:1884"),
            Some((Ipv4Addr::new(10, 0, 0, 45), 1884))
        );
        assert_eq!(
            parse_broker("10.0.0.45"),
            Some((Ipv4Addr::new(10, 0, 0, 45), BROKER_PORT))
        );
    }

    #[test]
    fn parse_broker_rejects_hostnames_ipv6_and_bad_ports() {
        assert_eq!(parse_broker("mqtt://broker.example.com:1883"), None);
        assert_eq!(parse_broker("mqtt://10.0.0.45:0"), None);
        assert_eq!(parse_broker("mqtt://10.0.0.45:abc"), None);
        assert_eq!(parse_broker("mqtt://::1:1883"), None);
        assert_eq!(parse_broker(""), None);
    }

    #[test]
    fn test_backoff_exact_sequence() {
        let mut backoff = ReconnectBackoff::new();
        let expected = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000];
        for want in expected {
            assert_eq!(backoff.next_delay_ms(), want);
        }
    }

    #[test]
    fn test_backoff_stays_capped() {
        let mut backoff = ReconnectBackoff::new();
        for _ in 0..6 {
            backoff.next_delay_ms();
        }
        for _ in 0..100 {
            assert_eq!(backoff.next_delay_ms(), BACKOFF_MAX_MS);
        }
    }

    #[test]
    fn test_backoff_reset_restarts_at_initial() {
        let mut backoff = ReconnectBackoff::new();
        for _ in 0..5 {
            backoff.next_delay_ms();
        }
        assert_eq!(backoff.next_delay_ms(), BACKOFF_MAX_MS);

        backoff.reset();
        assert_eq!(backoff.next_delay_ms(), BACKOFF_INITIAL_MS);
        assert_eq!(backoff.next_delay_ms(), BACKOFF_INITIAL_MS * 2);
    }

    #[test]
    fn test_backoff_default_starts_at_initial() {
        let mut backoff = ReconnectBackoff::default();
        assert_eq!(backoff.next_delay_ms(), BACKOFF_INITIAL_MS);
    }

    #[test]
    fn test_link_state_equality() {
        assert_eq!(LinkState::Disconnected, LinkState::Disconnected);
        assert_ne!(LinkState::Disconnected, LinkState::Connecting);
        assert_ne!(LinkState::Connecting, LinkState::Connected);
        assert_eq!(format!("{:?}", LinkState::Connected), "Connected");
    }
}
