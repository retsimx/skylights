//! Post-swap self-test state machine.
//!
//! Pure and allocation-free. [`SelfTestTracker`] latches the three operational
//! signals (`gpio_safe`, `wifi_connected`, `mqtt_healthy`) and adjudicates them
//! against a [`Clock`] over [`SELF_TEST_WINDOW_MS`]: all three seen within the
//! window confirms the slot, an invariant violation or expiry fails it.

/// Observation window after a trial boot, in milliseconds.
pub const SELF_TEST_WINDOW_MS: u64 = 30_000;

/// Monotonic millisecond clock seam (host tests inject a mock).
pub trait Clock {
    fn now_ms(&self) -> u64;
}

/// The three operational signals the self-test requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SelfTestSignals {
    pub gpio_safe: bool,
    pub wifi_connected: bool,
    pub mqtt_healthy: bool,
}

/// Outcome of evaluating the self-test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfTestVerdict {
    Pending,
    Passed,
    Failed(&'static str),
}

/// Pure self-test tracker: latches positive signals, adjudicates the window.
pub struct SelfTestTracker {
    start_ms: u64,
    signals: SelfTestSignals,
    verdict: SelfTestVerdict,
}

impl SelfTestTracker {
    /// Creates a tracker starting at `start_ms` with the boot GPIO probe result.
    pub const fn new(start_ms: u64, gpio_safe: bool) -> Self {
        Self {
            start_ms,
            signals: SelfTestSignals {
                gpio_safe,
                wifi_connected: false,
                mqtt_healthy: false,
            },
            verdict: SelfTestVerdict::Pending,
        }
    }

    /// Latches positive Wi-Fi/MQTT observations (a signal that goes false after
    /// being true stays latched; the window only requires it to have been seen).
    pub fn observe(&mut self, wifi_connected: bool, mqtt_healthy: bool) {
        self.signals.wifi_connected |= wifi_connected;
        self.signals.mqtt_healthy |= mqtt_healthy;
    }

    /// Records a critical-invariant failure (e.g. spurious low pin), latching
    /// `Failed(reason)`.
    pub fn fail(&mut self, reason: &'static str) {
        self.verdict = SelfTestVerdict::Failed(reason);
    }

    /// Advances the state machine against `clock` and returns the verdict.
    pub fn evaluate(&mut self, clock: &impl Clock) -> SelfTestVerdict {
        match self.verdict {
            SelfTestVerdict::Passed | SelfTestVerdict::Failed(_) => return self.verdict,
            SelfTestVerdict::Pending => {}
        }
        if !self.signals.gpio_safe {
            self.verdict = SelfTestVerdict::Failed("gpio");
        } else if self.signals.wifi_connected && self.signals.mqtt_healthy {
            self.verdict = SelfTestVerdict::Passed;
        } else if clock.now_ms().saturating_sub(self.start_ms) >= SELF_TEST_WINDOW_MS {
            self.verdict = SelfTestVerdict::Failed("timeout");
        }
        self.verdict
    }

    /// Returns the latched signals (diagnostics/tests).
    pub fn signals(&self) -> SelfTestSignals {
        self.signals
    }

    /// Returns the current verdict without advancing.
    pub fn verdict(&self) -> SelfTestVerdict {
        self.verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock monotonic clock advanced explicitly by tests.
    struct MockClock {
        now_ms: u64,
    }

    impl MockClock {
        fn new(now_ms: u64) -> Self {
            Self { now_ms }
        }

        fn advance(&mut self, ms: u64) {
            self.now_ms += ms;
        }
    }

    impl Clock for MockClock {
        fn now_ms(&self) -> u64 {
            self.now_ms
        }
    }

    #[test]
    fn wifi_then_mqtt_within_window_passes() {
        let mut clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        tracker.observe(true, false);
        clock.advance(1_000);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Pending);
        tracker.observe(true, true);
        clock.advance(1_000);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Passed);
    }

    #[test]
    fn mqtt_then_wifi_within_window_passes() {
        let mut clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        tracker.observe(false, true);
        clock.advance(1_000);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Pending);
        tracker.observe(true, true);
        clock.advance(1_000);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Passed);
    }

    #[test]
    fn both_signals_at_once_within_window_passes() {
        let mut clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        tracker.observe(true, true);
        clock.advance(1_000);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Passed);
    }

    #[test]
    fn partial_pass_at_window_expiry_times_out() {
        let mut clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        tracker.observe(true, false);
        clock.advance(SELF_TEST_WINDOW_MS);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Failed("timeout"));
    }

    #[test]
    fn gpio_unsafe_fails_immediately_at_zero() {
        let clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, false);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Failed("gpio"));
        assert_eq!(tracker.verdict(), SelfTestVerdict::Failed("gpio"));
    }

    #[test]
    fn all_signals_true_exactly_at_window_passes() {
        let mut clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        tracker.observe(true, true);
        clock.advance(SELF_TEST_WINDOW_MS);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Passed);
    }

    #[test]
    fn incomplete_signals_at_exact_window_times_out() {
        let mut clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        clock.advance(SELF_TEST_WINDOW_MS);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Failed("timeout"));
    }

    #[test]
    fn terminal_verdict_latches_across_later_evaluations() {
        let mut clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        tracker.observe(true, true);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Passed);

        tracker.observe(false, false);
        clock.advance(SELF_TEST_WINDOW_MS + 1);
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Passed);
        assert_eq!(tracker.verdict(), SelfTestVerdict::Passed);

        let mut failed = SelfTestTracker::new(0, false);
        assert_eq!(failed.evaluate(&clock), SelfTestVerdict::Failed("gpio"));
        clock.advance(SELF_TEST_WINDOW_MS);
        assert_eq!(failed.evaluate(&clock), SelfTestVerdict::Failed("gpio"));
    }

    #[test]
    fn positive_signals_latch_across_later_false_observation() {
        let clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        tracker.observe(true, true);
        tracker.observe(false, false);
        assert_eq!(
            tracker.signals(),
            SelfTestSignals {
                gpio_safe: true,
                wifi_connected: true,
                mqtt_healthy: true,
            }
        );
        assert_eq!(tracker.evaluate(&clock), SelfTestVerdict::Passed);
    }

    #[test]
    fn fail_latches_reason() {
        let clock = MockClock::new(0);
        let mut tracker = SelfTestTracker::new(0, true);
        tracker.fail("spurious_low");
        assert_eq!(tracker.verdict(), SelfTestVerdict::Failed("spurious_low"));
        assert_eq!(
            tracker.evaluate(&clock),
            SelfTestVerdict::Failed("spurious_low")
        );
    }
}
