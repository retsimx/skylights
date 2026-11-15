//! Pure Wi-Fi recovery policy: bounded timing thresholds and the small
//! decision helpers the network tasks use.
//!
//! Kept free of esp-hal/embassy types so every threshold and decision boundary
//! is host-tested. Mirror of the sibling garagedoorcontroller/garagelight
//! recovery policy, with the one deliberate reboot escalation documented here.

/// Maximum time to wait for one association attempt before treating it as
/// failed.
///
/// A stalled radio call would otherwise wedge the supervisor task forever; the
/// task is not visible to the hardware watchdog the way a stalled executor is.
pub const JOIN_TIMEOUT_MS: u64 = 15_000;

/// Maximum time to wait for one DHCP acquisition window.
pub const DHCP_TIMEOUT_MS: u64 = 15_000;

/// Number of DHCP windows to try within a single association before leaving.
///
/// Re-issuing DHCP on the existing association recovers faster than a full
/// disassociate/rejoin when the access point is up but slow to lease.
pub const DHCP_ATTEMPTS: u32 = 3;

/// Maximum time to wait for a `disconnect`/leave call before continuing.
pub const LEAVE_TIMEOUT_MS: u64 = 5_000;

/// Consecutive MQTT connect failures that request a Wi-Fi rejoin.
///
/// Covers the "associated but no traffic" case, where the link never reports
/// down and so the supervisor alone would never re-associate.
pub const MQTT_REJOIN_FAILURES: u32 = 5;

/// Duration without an IP lease after which the MCU resets to recover a wedged
/// radio.
///
/// This is the single deliberate exception to the "network loss never resets"
/// contract: it is bounded, fires only after [`NO_IP_REBOOT_MS`] with no lease,
/// and is harmless because the device cannot reach the network in that state.
pub const NO_IP_REBOOT_MS: u64 = 10 * 60 * 1000;

/// Tracks how long the device has been without an IP lease.
#[derive(Debug, Clone, Copy)]
pub struct NoIpWatchdog {
    last_good_ms: Option<u64>,
}

impl NoIpWatchdog {
    /// Creates a watchdog that is "due" once [`NO_IP_REBOOT_MS`] has elapsed
    /// since boot if no lease is ever acquired.
    pub const fn new() -> Self {
        Self { last_good_ms: None }
    }

    /// Records `now_ms` as the last moment the link was known good: a lease was
    /// held then.
    ///
    /// The no-lease countdown is measured from this instant, so it is refreshed
    /// both when a lease is acquired and when a held link is lost. Refreshing on
    /// the loss is what keeps a healthy association from being charged to the
    /// next outage: without it a link that stayed up longer than
    /// [`NO_IP_REBOOT_MS`] looks overdue the moment it drops, and the following
    /// re-association escalates to a reset.
    pub fn note_last_good(&mut self, now_ms: u64) {
        self.last_good_ms = Some(now_ms);
    }

    /// Returns `true` when [`NO_IP_REBOOT_MS`] has elapsed since the last
    /// known-good instant (a lease acquisition or a link loss).
    pub fn reboot_due(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_good_ms.unwrap_or(0)) >= NO_IP_REBOOT_MS
    }
}

impl Default for NoIpWatchdog {
    fn default() -> Self {
        Self::new()
    }
}

/// Counts consecutive MQTT connect failures and trips at a threshold.
#[derive(Debug, Clone, Copy)]
pub struct RejoinCounter {
    failures: u32,
}

impl RejoinCounter {
    /// Creates a counter with no recorded failures.
    pub const fn new() -> Self {
        Self { failures: 0 }
    }

    /// Records a failed connect attempt. Returns `true` exactly when the
    /// threshold is reached; the counter then resets so the next trip needs a
    /// fresh run of failures.
    pub fn failure(&mut self) -> bool {
        self.failures += 1;
        if self.failures >= MQTT_REJOIN_FAILURES {
            self.failures = 0;
            true
        } else {
            false
        }
    }

    /// Clears the run of failures after a successful connect.
    pub fn success(&mut self) {
        self.failures = 0;
    }
}

impl Default for RejoinCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_trips_only_after_threshold_without_ip() {
        let watchdog = NoIpWatchdog::new();
        assert!(!watchdog.reboot_due(0));
        assert!(!watchdog.reboot_due(NO_IP_REBOOT_MS - 1));
        assert!(watchdog.reboot_due(NO_IP_REBOOT_MS));
        assert!(watchdog.reboot_due(NO_IP_REBOOT_MS * 3));
    }

    #[test]
    fn watchdog_note_last_good_restarts_the_window() {
        let mut watchdog = NoIpWatchdog::new();
        watchdog.note_last_good(5_000);
        assert!(!watchdog.reboot_due(5_000 + NO_IP_REBOOT_MS - 1));
        assert!(watchdog.reboot_due(5_000 + NO_IP_REBOOT_MS));
        watchdog.note_last_good(5_000 + NO_IP_REBOOT_MS);
        assert!(!watchdog.reboot_due(5_000 + NO_IP_REBOOT_MS));
    }

    /// Documents the trap this fix closes: a lease stamped at association time
    /// is overdue as soon as the association outlives [`NO_IP_REBOOT_MS`], so a
    /// drop after a long association used to reset the MCU. Recording the link
    /// loss restarts the window from the drop.
    #[test]
    fn link_loss_after_a_long_association_does_not_reboot() {
        const ELEVEN_HOURS_MS: u64 = 11 * 60 * 60 * 1000;

        let mut watchdog = NoIpWatchdog::new();
        watchdog.note_last_good(0);
        // Pre-fix behaviour: association age alone trips the watchdog.
        assert!(watchdog.reboot_due(ELEVEN_HOURS_MS));

        // Recording the drop restarts the countdown from the loss.
        watchdog.note_last_good(ELEVEN_HOURS_MS);
        assert!(!watchdog.reboot_due(ELEVEN_HOURS_MS + NO_IP_REBOOT_MS - 1));
        assert!(watchdog.reboot_due(ELEVEN_HOURS_MS + NO_IP_REBOOT_MS));
    }

    #[test]
    fn rejoin_counter_trips_at_threshold_and_resets() {
        let mut counter = RejoinCounter::new();
        for _ in 0..MQTT_REJOIN_FAILURES - 1 {
            assert!(!counter.failure());
        }
        assert!(counter.failure());
        // Reset after tripping: the next trip needs a fresh full run.
        for _ in 0..MQTT_REJOIN_FAILURES - 1 {
            assert!(!counter.failure());
        }
        assert!(counter.failure());
    }

    #[test]
    fn rejoin_counter_success_clears_the_run() {
        let mut counter = RejoinCounter::new();
        assert!(!counter.failure());
        assert!(!counter.failure());
        counter.success();
        for _ in 0..MQTT_REJOIN_FAILURES - 1 {
            assert!(!counter.failure());
        }
        assert!(counter.failure());
    }
}
