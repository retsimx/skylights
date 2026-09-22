#![cfg_attr(not(test), no_std)]

//! skylights-core: Hardware-agnostic core logic for Skylights controller.
//!
//! Contains window timing models, state representations, position calculations,
//! and utilities with zero hardware or HAL dependencies.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Current operational state of a skylight window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum WindowState {
    /// Fully closed and idle.
    Closed,
    /// Actively driving open.
    Opening,
    /// Fully open and idle.
    Open,
    /// Actively driving closed.
    Closing,
    /// Stopped at an intermediate position.
    Stopped,
}

impl WindowState {
    /// Returns true if the window is actively moving.
    pub const fn is_moving(&self) -> bool {
        matches!(self, Self::Opening | Self::Closing)
    }

    /// Returns true if the window is completely closed.
    pub const fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }

    /// Returns true if the window is completely open.
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Open)
    }
}

/// Percentage position of a skylight window (0% = fully closed, 100% = fully open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(from = "u8"))]
pub struct Position(u8);

impl Position {
    /// Fully closed position (0%).
    pub const CLOSED: Self = Self(0);

    /// Fully open position (100%).
    pub const OPEN: Self = Self(100);

    /// Creates a new `Position`, clamping values above 100 to 100.
    pub const fn new(percent: u8) -> Self {
        if percent > 100 {
            Self(100)
        } else {
            Self(percent)
        }
    }

    /// Returns the raw percentage value (0..=100).
    pub const fn percent(&self) -> u8 {
        self.0
    }

    /// Returns true if this position represents fully closed (0%).
    pub const fn is_closed(&self) -> bool {
        self.0 == 0
    }

    /// Returns true if this position represents fully open (100%).
    pub const fn is_open(&self) -> bool {
        self.0 == 100
    }
}

impl From<u8> for Position {
    fn from(val: u8) -> Self {
        Self::new(val)
    }
}

/// Calculates the interpolated position during travel.
///
/// * `start`: Starting position percentage (0..=100)
/// * `target`: Target position percentage (0..=100)
/// * `elapsed_ms`: Elapsed time in milliseconds since motion began
/// * `total_duration_ms`: Travel duration in milliseconds from `start` to `target`
///
/// Returns the current estimated `Position`.
pub fn calculate_travel_position(
    start: Position,
    target: Position,
    elapsed_ms: u64,
    total_duration_ms: u64,
) -> Position {
    if total_duration_ms == 0 || elapsed_ms >= total_duration_ms {
        return target;
    }

    let start_pct = start.percent() as u64;
    let target_pct = target.percent() as u64;

    if start_pct <= target_pct {
        // Moving open / forward
        let delta = target_pct - start_pct;
        let progress = (delta * elapsed_ms) / total_duration_ms;
        Position::new((start_pct + progress) as u8)
    } else {
        // Moving closed / reverse
        let delta = start_pct - target_pct;
        let progress = (delta * elapsed_ms) / total_duration_ms;
        Position::new((start_pct - progress) as u8)
    }
}

/// Library version identifier.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_available() {
        assert_eq!(CORE_VERSION, "0.1.0");
    }

    #[test]
    fn test_position_clamping() {
        assert_eq!(Position::new(0).percent(), 0);
        assert_eq!(Position::new(50).percent(), 50);
        assert_eq!(Position::new(100).percent(), 100);
        assert_eq!(Position::new(150).percent(), 100);
        assert!(Position::CLOSED.is_closed());
        assert!(!Position::CLOSED.is_open());
        assert!(Position::OPEN.is_open());
        assert!(!Position::OPEN.is_closed());
    }

    #[test]
    fn test_window_state_predicates() {
        assert!(!WindowState::Closed.is_moving());
        assert!(WindowState::Closed.is_closed());
        assert!(!WindowState::Closed.is_open());

        assert!(WindowState::Opening.is_moving());
        assert!(!WindowState::Opening.is_closed());

        assert!(!WindowState::Open.is_moving());
        assert!(WindowState::Open.is_open());

        assert!(WindowState::Closing.is_moving());
        assert!(!WindowState::Stopped.is_moving());
    }

    #[test]
    fn test_travel_position_opening() {
        let start = Position::CLOSED;
        let target = Position::OPEN;
        let total_ms = 10_000;

        assert_eq!(
            calculate_travel_position(start, target, 0, total_ms),
            Position::new(0)
        );
        assert_eq!(
            calculate_travel_position(start, target, 2_500, total_ms),
            Position::new(25)
        );
        assert_eq!(
            calculate_travel_position(start, target, 5_000, total_ms),
            Position::new(50)
        );
        assert_eq!(
            calculate_travel_position(start, target, 7_500, total_ms),
            Position::new(75)
        );
        assert_eq!(
            calculate_travel_position(start, target, 10_000, total_ms),
            Position::new(100)
        );
        assert_eq!(
            calculate_travel_position(start, target, 15_000, total_ms),
            Position::new(100)
        );
    }

    #[test]
    fn test_travel_position_closing() {
        let start = Position::OPEN;
        let target = Position::CLOSED;
        let total_ms = 10_000;

        assert_eq!(
            calculate_travel_position(start, target, 0, total_ms),
            Position::new(100)
        );
        assert_eq!(
            calculate_travel_position(start, target, 5_000, total_ms),
            Position::new(50)
        );
        assert_eq!(
            calculate_travel_position(start, target, 10_000, total_ms),
            Position::new(0)
        );
        assert_eq!(
            calculate_travel_position(start, target, 20_000, total_ms),
            Position::new(0)
        );
    }

    #[test]
    fn test_travel_position_partial_travel() {
        let start = Position::new(20);
        let target = Position::new(80);
        let duration = 6_000;

        assert_eq!(
            calculate_travel_position(start, target, 3_000, duration),
            Position::new(50)
        );
        assert_eq!(
            calculate_travel_position(start, target, 6_000, duration),
            Position::new(80)
        );
    }

    #[test]
    fn test_travel_zero_duration() {
        let start = Position::CLOSED;
        let target = Position::OPEN;
        assert_eq!(
            calculate_travel_position(start, target, 500, 0),
            Position::OPEN
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_serde_serialization() {
        let state = WindowState::Opening;
        let serialized = serde_json_core::to_string::<_, 32>(&state).unwrap();
        assert_eq!(serialized.as_str(), "\"Opening\"");

        let (deserialized, _): (WindowState, _) = serde_json_core::from_str(&serialized).unwrap();
        assert_eq!(deserialized, state);

        // Verify Position serde deserialization invariant enforcement via serde(from = "u8")
        let (pos, _): (Position, _) = serde_json_core::from_str("150").unwrap();
        assert_eq!(pos, Position::OPEN);
    }
}
