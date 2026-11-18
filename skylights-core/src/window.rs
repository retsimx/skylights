//! Window actuation travel-time state machine.
//!
//! Provides a pure, hardware-agnostic model of a skylight window driven by
//! timed pulses. [`TRAVEL_TIME_MS`] is the full-stroke time used for boundary
//! targets only (0 % and 100 %). Partial moves use three measured affine
//! inversions — opening from the closed seat, opening mid-travel, and closing
//! mid-travel — selected by the start position and direction, and
//! [`interrupted_position`] inverts the same relations. The state machine only
//! uses integer millisecond ticks — no clocks, sleeps, floats, or hardware
//! dependencies — so it runs identically on the ESP32 target and on the host
//! test harness.

use crate::{Position, WindowState};

/// Full travel time between the closed (0 %) and open (100 %) boundaries, in
/// milliseconds.
///
/// This is a boundary-only constant: partial moves use the calibrated affine
/// inversions in [`travel_duration_ms`].
pub const TRAVEL_TIME_MS: u64 = 71_000;

/// Smallest drive pulse for a partial move, in milliseconds.
///
/// The mid-travel inversions have negative intercepts: a 1–2 % step needs zero
/// or negative drive time. Clamp to this value instead of underflowing or
/// turning the move into a no-op. Tunable; 200 ms is the starting value.
pub const MIN_PULSE_MS: u64 = 200;

/// Duration the actuation line is held low for each button press, in
/// milliseconds.
///
/// The actuation line drives the OEM remote's button pad directly, so this is
/// the emulated "click" duration. 100 ms is deliberate: a shorter press was
/// observed to leave the remote occasionally unresponsive.
pub const PULSE_LOW_MS: u64 = 100;

/// Duration the actuation line is held high between the two button presses, in
/// milliseconds.
///
/// This is the gap between clicks; 800 ms gives the remote time to transmit and
/// reset before the second press.
pub const PULSE_HIGH_MS: u64 = 800;

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

// Measured inversions, stored as integer tenths of a millisecond per percent
// (slope) and tenths of a millisecond (intercept). A single `/ 10` at the end
// recovers milliseconds. Forward: travel_ms = (slope·ΔV + intercept) / 10.
// Inverse:  ΔV = (10·elapsed − intercept) / slope.

/// Opening from the closed seat: `travel_ms = 645.2·ΔV + 207`.
/// Fit `Δθ = 0.2170·D − 0.045`; residual sd 0.0335°, max 0.0506°, n = 6.
const SEAT_OPEN_SLOPE_TENTHS: i32 = 6_452;
/// Seat-opening intercept: +207 ms (0.045° of the fit).
const SEAT_OPEN_INTERCEPT_TENTHS: i32 = 2_070;

/// Opening mid-travel: `travel_ms = 677.3·ΔV − 1374`.
/// Fit `Δθ = 0.2067·D + 0.284`; residual sd 0.0307°, max 0.0814°, n = 11.
const MID_OPEN_SLOPE_TENTHS: i32 = 6_773;
/// Mid-opening intercept: −1374 ms (the ≈2 % per-move over-travel).
const MID_OPEN_INTERCEPT_TENTHS: i32 = -13_740;

/// Closing mid-travel: `travel_ms = 660.5·ΔV − 1262`.
/// Fit `Δθ = −0.2120·D − 0.267`; n = 2 (residuals not reported).
const MID_CLOSE_SLOPE_TENTHS: i32 = 6_605;
/// Mid-closing intercept: −1262 ms (the same per-move over-travel).
const MID_CLOSE_INTERCEPT_TENTHS: i32 = -12_620;

/// One measured affine relation between commanded drive time and travel.
#[derive(Clone, Copy)]
struct DriveCurve {
    slope_tenths: i32,
    intercept_tenths: i32,
}

const SEAT_OPEN: DriveCurve = DriveCurve {
    slope_tenths: SEAT_OPEN_SLOPE_TENTHS,
    intercept_tenths: SEAT_OPEN_INTERCEPT_TENTHS,
};

const MID_OPEN: DriveCurve = DriveCurve {
    slope_tenths: MID_OPEN_SLOPE_TENTHS,
    intercept_tenths: MID_OPEN_INTERCEPT_TENTHS,
};

const MID_CLOSE: DriveCurve = DriveCurve {
    slope_tenths: MID_CLOSE_SLOPE_TENTHS,
    intercept_tenths: MID_CLOSE_INTERCEPT_TENTHS,
};

/// Selects the measured curve for a move. Closing is always mid-travel: a
/// partial close cannot start at 0.
fn drive_curve(from_pct: u8, direction: Direction) -> DriveCurve {
    match direction {
        Direction::Closing => MID_CLOSE,
        Direction::Opening if from_pct == 0 => SEAT_OPEN,
        Direction::Opening => MID_OPEN,
    }
}

/// Calculates the drive duration in milliseconds for a partial move.
///
/// Uses the calibrated affine inversion for the start position and direction,
/// floored at [`MIN_PULSE_MS`]. An equal-position delta returns 0. Boundary
/// targets (0 % and 100 %) always use [`TRAVEL_TIME_MS`] via
/// [`WindowPositioner::request`] and must not call this helper.
pub fn travel_duration_ms(from: Position, to: Position) -> u64 {
    let delta_v = from.percent().abs_diff(to.percent()) as i32;
    if delta_v == 0 {
        return 0; // partial no-op; request() rejects these before calling
    }
    let direction = if to > from {
        Direction::Opening
    } else {
        Direction::Closing
    };
    let curve = drive_curve(from.percent(), direction);
    let ms = (curve.slope_tenths * delta_v + curve.intercept_tenths) / 10;
    ms.max(MIN_PULSE_MS as i32) as u64
}

/// Calculates the position reached after `elapsed_ms` of travel in `direction`
/// starting from `start`.
///
/// Inverts the same calibrated curve used by [`travel_duration_ms`], so a move
/// commanded for a given delta and stopped after its full drive time returns to
/// that delta within ±1 %. The result is clamped to the 0..=100 range, and the
/// elapsed time is bounded before the arithmetic, so an oversized value can
/// never overflow past the boundary or underflow below it.
pub fn interrupted_position(start: Position, direction: Direction, elapsed_ms: u64) -> Position {
    let curve = drive_curve(start.percent(), direction);

    // Bound the arithmetic; any longer drive is already beyond the far
    // boundary, so the result is clamped to 0/100 either way.
    let elapsed_ms = elapsed_ms.min(TRAVEL_TIME_MS * 2) as i32;

    // `+ slope/2` rounds to nearest, so a partial move does not systematically
    // lose a position point; a plain floor is therefore wrong for the inverse.
    let numerator = elapsed_ms * 10 - curve.intercept_tenths + curve.slope_tenths / 2;
    let delta_v = (numerator / curve.slope_tenths).clamp(0, 100);

    let pct = match direction {
        Direction::Opening => start.percent() as i32 + delta_v,
        Direction::Closing => start.percent() as i32 - delta_v,
    };
    Position::new(pct.clamp(0, 100) as u8)
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
