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

/// Maximum time to wait for a DNS lookup before giving up on it.
///
/// Bounds the post-association DNS probe so an unresponsive resolver cannot
/// wedge the Wi-Fi supervisor before it enters link supervision.
pub const DNS_TIMEOUT_MS: u64 = 10_000;

/// Maximum time to wait for one MQTT TCP connect, broker handshake, or
/// subscribe attempt.
///
/// Bounds every pre-serve phase so a stalled broker or dead link records a
/// failure, advances reconnect backoff, and can trigger a Wi-Fi rejoin instead
/// of hanging forever.
pub const MQTT_CONNECT_TIMEOUT_MS: u64 = 10_000;

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

/// Age of the newest completed MQTT dispatch at which the Wi-Fi supervisor stops
/// trusting the association and abandons it to force a rejoin.
///
/// Must comfortably exceed the worst legitimate silent gap. Heartbeats are
/// every 60 s and a failed attempt is bounded by one backoff ceiling (30 s)
/// plus connect and handshake timeouts (10 s each), so 3 minutes leaves margin
/// without delaying recovery much. Abandoning at this age is what recovers a
/// black-holed association long before [`NO_IP_REBOOT_MS`] resets the chip.
pub const MQTT_STALE_MS: u64 = 180_000;

/// Returns `true` when MQTT progress is old enough to abandon the association.
pub const fn progress_stale(age_ms: u64) -> bool {
    age_ms >= MQTT_STALE_MS
}

/// Returns `true` when MQTT progress is old enough to reset the chip.
///
/// A QoS-0 publish the socket accepted proves the write, not delivery, so the
/// real detection bound is this threshold plus one heartbeat in flight.
pub const fn progress_reset_due(age_ms: u64) -> bool {
    age_ms >= NO_IP_REBOOT_MS
}

/// Returns `true` while MQTT progress is recent enough to keep feeding the
/// runtime hardware watchdog. Exactly the negation of [`progress_reset_due`],
/// so the reset and the watchdog-starvation recovery cannot drift apart.
pub const fn progress_fresh(age_ms: u64) -> bool {
    !progress_reset_due(age_ms)
}

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
    fn progress_turns_stale_only_at_the_threshold() {
        assert!(!progress_stale(0));
        assert!(!progress_stale(MQTT_STALE_MS - 1));
        assert!(progress_stale(MQTT_STALE_MS));
        assert!(progress_stale(MQTT_STALE_MS * 2));
        // Abandoning the association fires strictly before the chip reset.
        assert!(!progress_reset_due(MQTT_STALE_MS));
    }

    #[test]
    fn progress_reset_fires_only_at_the_threshold() {
        assert!(!progress_reset_due(0));
        assert!(!progress_reset_due(MQTT_STALE_MS));
        assert!(!progress_reset_due(NO_IP_REBOOT_MS - 1));
        assert!(progress_reset_due(NO_IP_REBOOT_MS));
        assert!(progress_reset_due(NO_IP_REBOOT_MS * 3));
    }

    /// The runtime watchdog feeds on `progress_fresh` while the supervisor
    /// resets on `progress_reset_due`: they must stay exact opposites, or one
    /// recovery path silently outlives the other.
    #[test]
    fn progress_fresh_is_the_negation_of_reset_due() {
        for age in [
            0,
            1,
            MQTT_STALE_MS,
            NO_IP_REBOOT_MS - 1,
            NO_IP_REBOOT_MS,
            NO_IP_REBOOT_MS + 1,
            NO_IP_REBOOT_MS * 10,
        ] {
            assert_eq!(progress_fresh(age), !progress_reset_due(age), "age {age}");
        }
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

    #[test]
    fn timeout_constants_are_positive_and_reexported() {
        const {
            assert!(DNS_TIMEOUT_MS > 0);
            assert!(MQTT_CONNECT_TIMEOUT_MS > 0);
            assert!(DNS_TIMEOUT_MS < MQTT_STALE_MS);
            assert!(MQTT_CONNECT_TIMEOUT_MS < MQTT_STALE_MS);
            assert!(MQTT_STALE_MS < NO_IP_REBOOT_MS);
        }
        assert_eq!(crate::DNS_TIMEOUT_MS, DNS_TIMEOUT_MS);
        assert_eq!(crate::MQTT_CONNECT_TIMEOUT_MS, MQTT_CONNECT_TIMEOUT_MS);
    }

    #[test]
    fn association_dhcp_and_dns_probe_fit_well_below_reboot_budget() {
        // One attempt_join (start + leave + connect) + DHCP_ATTEMPTS + DNS probe.
        let worst_case_bringup_ms = JOIN_TIMEOUT_MS
            + LEAVE_TIMEOUT_MS
            + JOIN_TIMEOUT_MS
            + (DHCP_ATTEMPTS as u64) * DHCP_TIMEOUT_MS
            + DNS_TIMEOUT_MS;
        assert!(worst_case_bringup_ms < NO_IP_REBOOT_MS / 2);
    }

    #[test]
    fn mqtt_rejoin_run_with_bounded_phases_and_backoff_fits_recovery_budgets() {
        use crate::net::ReconnectBackoff;

        let mut backoff = ReconnectBackoff::new();
        let mut counter = RejoinCounter::new();
        let mut single_phase_run_ms = 0u64;
        let mut all_phases_run_ms = 0u64;
        let mut tripped = false;

        for _ in 0..MQTT_REJOIN_FAILURES {
            let delay = backoff.next_delay_ms();
            // One phase hanging (TCP connect, MQTT handshake, or subscribe).
            single_phase_run_ms += MQTT_CONNECT_TIMEOUT_MS + delay;
            // Worst case: all three pre-serve phases nearly exhaust their bounds.
            all_phases_run_ms += 3 * MQTT_CONNECT_TIMEOUT_MS + delay;
            tripped = counter.failure();
        }

        assert!(tripped);
        // Normal hung-step run triggers Wi-Fi rejoin before stale abandonment.
        assert!(single_phase_run_ms < MQTT_STALE_MS);
        // Even a 3-phase worst-case run stays well inside the chip-reset budget,
        // and MQTT_STALE_MS abandons the association long before NO_IP_REBOOT_MS.
        assert!(all_phases_run_ms < NO_IP_REBOOT_MS);
        assert!(progress_stale(MQTT_STALE_MS));
        assert!(!progress_reset_due(MQTT_STALE_MS));
    }
}
