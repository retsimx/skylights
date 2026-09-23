//! Window actuation travel-time state machine.
//!
//! Provides a pure, hardware-agnostic model of a skylight window driven by
//! timed pulses. Full travel between 0 % (closed) and 100 % (open) is assumed
//! to take [`TRAVEL_TIME_MS`]; partial targets interpolate linearly. The state
//! machine only uses integer millisecond ticks — no clocks, sleeps, floats, or
//! hardware dependencies — so it runs identically on the ESP32 target and on
//! the host test harness.

use crate::{Position, WindowState};

/// Full travel time between the closed (0 %) and open (100 %) boundaries, in
/// milliseconds.
pub const TRAVEL_TIME_MS: u64 = 71_000;

/// Duration the actuation line is held low for each pulse, in milliseconds.
pub const PULSE_LOW_MS: u64 = 75;

/// Duration the actuation line is held high for each pulse, in milliseconds.
pub const PULSE_HIGH_MS: u64 = 75;

/// Settling delay after the final pulse before the window is considered
/// actuated, in milliseconds.
pub const PULSE_SETTLE_MS: u64 = 400;

/// Duration of the two low/high pulse cycles (the pulse burst), in milliseconds.
pub const PULSE_CYCLE_MS: u64 = 2 * (PULSE_LOW_MS + PULSE_HIGH_MS);

/// Total duration of the two-pulse sequence plus settle time, in milliseconds.
pub const PULSE_SEQUENCE_MS: u64 = PULSE_CYCLE_MS + PULSE_SETTLE_MS;

/// Physical actuation line to pulse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actuation {
    /// Drive the window open.
    Open,
    /// Drive the window closed.
    Close,
    /// Stop/hold the window in place.
    Stop,
}

/// Direction in which the window is travelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Travelling towards 100 % (open).
    Opening,
    /// Travelling towards 0 % (closed).
    Closing,
}

/// A request to the actuation driver describing how to reach a target position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TravelPlan {
    /// Actuation line to pulse to begin travel.
    pub actuation: Actuation,
    /// Direction the window will travel.
    pub direction: Direction,
    /// Duration to hold travel before completing or stopping, in milliseconds.
    pub travel_ms: u64,
    /// Target position this plan drives towards.
    pub target: Position,
    /// Whether an explicit [`Actuation::Stop`] pulse is required after
    /// `travel_ms` (true for partial targets, false for hard-seat boundaries).
    pub stop_after: bool,
}

/// Active motion tracked by [`WindowPositioner`] so it can complete or
/// interpolate an interruption.
#[derive(Debug, Clone, Copy)]
struct Motion {
    start: Position,
    direction: Direction,
    target: Position,
}

/// Calculates the travel duration in milliseconds between two positions.
///
/// The result is linear in the percentage delta: a full 0↔100 travel takes
/// exactly [`TRAVEL_TIME_MS`].
pub fn travel_duration_ms(from: Position, to: Position) -> u64 {
    (from.percent().abs_diff(to.percent()) as u64) * TRAVEL_TIME_MS / 100
}

/// Calculates the position reached after `elapsed_ms` of travel in `direction`
/// starting from `start`.
///
/// The result is saturated and clamped to the 0..=100 range, so an oversized
/// elapsed time can never overflow past the boundary or underflow below it.
pub fn interrupted_position(start: Position, direction: Direction, elapsed_ms: u64) -> Position {
    let traveled = elapsed_ms.saturating_mul(100) / TRAVEL_TIME_MS;
    let start_pct = start.percent() as u64;
    let pct = match direction {
        Direction::Opening => start_pct.saturating_add(traveled),
        Direction::Closing => start_pct.saturating_sub(traveled),
    };
    Position::new(pct.min(100) as u8)
}

/// Travel-time state machine for a single skylight window.
///
/// The machine tracks the last known [`Position`] and [`WindowState`] and
/// emits a [`TravelPlan`] for each movement request. Movement is completed with
/// [`WindowPositioner::finish`] or interrupted with [`WindowPositioner::stop`].
#[derive(Debug, Clone, Copy)]
pub struct WindowPositioner {
    position: Position,
    state: WindowState,
    motion: Option<Motion>,
}

impl Default for WindowPositioner {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowPositioner {
    /// Creates a positioner at the fully closed (0 %) boundary, idle.
    pub const fn new() -> Self {
        Self {
            position: Position::CLOSED,
            state: WindowState::Closed,
            motion: None,
        }
    }

    /// Returns the last known position.
    pub const fn position(&self) -> Position {
        self.position
    }

    /// Returns the current operational state.
    pub const fn state(&self) -> WindowState {
        self.state
    }

    /// Returns true while the window is travelling (`Opening` or `Closing`).
    pub const fn is_moving(&self) -> bool {
        self.state.is_moving()
    }

    /// Requests travel to `target` and returns the resulting [`TravelPlan`].
    ///
    /// The 0 % and 100 % boundaries always produce a full [`TRAVEL_TIME_MS`]
    /// plan with `stop_after = false`, even when already at the boundary, so
    /// that a hard-seat request re-calibrates the window. A partial target
    /// equal to the current position yields `None` and leaves the state
    /// unchanged.
    pub fn request(&mut self, target: Position) -> Option<TravelPlan> {
        let target_pct = target.percent();

        if target_pct == 0 {
            self.start_motion(Direction::Closing, Position::CLOSED);
            return Some(TravelPlan {
                actuation: Actuation::Close,
                direction: Direction::Closing,
                travel_ms: TRAVEL_TIME_MS,
                target: Position::CLOSED,
                stop_after: false,
            });
        }

        if target_pct == 100 {
            self.start_motion(Direction::Opening, Position::OPEN);
            return Some(TravelPlan {
                actuation: Actuation::Open,
                direction: Direction::Opening,
                travel_ms: TRAVEL_TIME_MS,
                target: Position::OPEN,
                stop_after: false,
            });
        }

        if target == self.position {
            return None;
        }

        let (direction, actuation) = if target > self.position {
            (Direction::Opening, Actuation::Open)
        } else {
            (Direction::Closing, Actuation::Close)
        };
        let travel_ms = travel_duration_ms(self.position, target);
        self.start_motion(direction, target);
        Some(TravelPlan {
            actuation,
            direction,
            travel_ms,
            target,
            stop_after: true,
        })
    }

    /// Completes the active motion, snapping the position to its target.
    ///
    /// Boundary targets resolve to `Closed`/`Open`; intermediate targets
    /// resolve to `Stopped`. A no-op when the window is idle.
    pub fn finish(&mut self) {
        if let Some(motion) = self.motion.take() {
            self.position = motion.target;
            self.state = if motion.target.is_closed() {
                WindowState::Closed
            } else if motion.target.is_open() {
                WindowState::Open
            } else {
                WindowState::Stopped
            };
        }
    }

    /// Interrupts the active motion after `elapsed_ms`, interpolating the
    /// position reached, marking the window `Stopped`, and returning the new
    /// position.
    ///
    /// A no-op returning the current position when the window is idle.
    pub fn stop(&mut self, elapsed_ms: u64) -> Position {
        if let Some(motion) = self.motion.take() {
            self.position = interrupted_position(motion.start, motion.direction, elapsed_ms);
            self.state = WindowState::Stopped;
        }
        self.position
    }

    /// Records a new motion and transitions to the matching moving state.
    fn start_motion(&mut self, direction: Direction, target: Position) {
        self.motion = Some(Motion {
            start: self.position,
            direction,
            target,
        });
        self.state = match direction {
            Direction::Opening => WindowState::Opening,
            Direction::Closing => WindowState::Closing,
        };
    }
}

#[cfg(test)]
mod tests;
