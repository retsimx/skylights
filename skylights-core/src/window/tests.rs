use super::*;

/// Builds a positioner that has settled at `pct` via a request + finish.
fn positioner_at(pct: u8) -> WindowPositioner {
    let mut positioner = WindowPositioner::new();
    positioner.request(Position::new(pct));
    positioner.finish();
    positioner
}

#[test]
fn test_new_boot_state() {
    let positioner = WindowPositioner::new();
    assert_eq!(positioner.position(), Position::CLOSED);
    assert_eq!(positioner.position().percent(), 0);
    assert_eq!(positioner.state(), WindowState::Closed);
    assert!(!positioner.is_moving());

    let default_positioner = WindowPositioner::default();
    assert_eq!(default_positioner.position(), Position::CLOSED);
    assert_eq!(default_positioner.state(), WindowState::Closed);
}

#[test]
fn test_request_close_always_full_travel() {
    for start in [0u8, 50, 100] {
        let mut positioner = positioner_at(start);
        let plan = positioner.request(Position::CLOSED).unwrap();
        assert_eq!(plan.actuation, Actuation::Close);
        assert_eq!(plan.direction, Direction::Closing);
        assert_eq!(plan.travel_ms, TRAVEL_TIME_MS);
        assert_eq!(plan.travel_ms, 71_000);
        assert_eq!(plan.target, Position::CLOSED);
        assert!(!plan.stop_after);
        assert_eq!(positioner.state(), WindowState::Closing);
        assert!(positioner.is_moving());
    }
}

#[test]
fn test_request_open_always_full_travel() {
    for start in [0u8, 50, 100] {
        let mut positioner = positioner_at(start);
        let plan = positioner.request(Position::OPEN).unwrap();
        assert_eq!(plan.actuation, Actuation::Open);
        assert_eq!(plan.direction, Direction::Opening);
        assert_eq!(plan.travel_ms, TRAVEL_TIME_MS);
        assert_eq!(plan.travel_ms, 71_000);
        assert_eq!(plan.target, Position::OPEN);
        assert!(!plan.stop_after);
        assert_eq!(positioner.state(), WindowState::Opening);
        assert!(positioner.is_moving());
    }
}

#[test]
fn test_request_boundary_from_boundary_recalibrates() {
    let mut positioner = positioner_at(0);
    let plan = positioner.request(Position::CLOSED).unwrap();
    assert_eq!(plan.travel_ms, TRAVEL_TIME_MS);
    assert!(!plan.stop_after);
    assert_eq!(positioner.state(), WindowState::Closing);

    let mut positioner = positioner_at(100);
    let plan = positioner.request(Position::OPEN).unwrap();
    assert_eq!(plan.travel_ms, TRAVEL_TIME_MS);
    assert!(!plan.stop_after);
    assert_eq!(positioner.state(), WindowState::Opening);
}

#[test]
fn test_partial_opening_0_to_50() {
    let mut positioner = WindowPositioner::new();
    let plan = positioner.request(Position::new(50)).unwrap();
    assert_eq!(plan.actuation, Actuation::Open);
    assert_eq!(plan.direction, Direction::Opening);
    assert_eq!(plan.travel_ms, 35_500);
    assert_eq!(plan.target, Position::new(50));
    assert!(plan.stop_after);
    assert_eq!(positioner.state(), WindowState::Opening);
    assert_eq!(positioner.position(), Position::CLOSED);
}

#[test]
fn test_partial_opening_50_to_75() {
    let mut positioner = positioner_at(50);
    let plan = positioner.request(Position::new(75)).unwrap();
    assert_eq!(plan.actuation, Actuation::Open);
    assert_eq!(plan.direction, Direction::Opening);
    assert_eq!(plan.travel_ms, 17_750);
    assert_eq!(plan.target, Position::new(75));
    assert!(plan.stop_after);
    assert_eq!(positioner.state(), WindowState::Opening);
}

#[test]
fn test_partial_closing_100_to_25() {
    let mut positioner = positioner_at(100);
    let plan = positioner.request(Position::new(25)).unwrap();
    assert_eq!(plan.actuation, Actuation::Close);
    assert_eq!(plan.direction, Direction::Closing);
    assert_eq!(plan.travel_ms, 53_250);
    assert_eq!(plan.target, Position::new(25));
    assert!(plan.stop_after);
    assert_eq!(positioner.state(), WindowState::Closing);
}

#[test]
fn test_partial_request_at_current_is_noop() {
    let mut positioner = positioner_at(50);
    assert!(positioner.request(Position::new(50)).is_none());
    assert_eq!(positioner.position(), Position::new(50));
    assert_eq!(positioner.state(), WindowState::Stopped);
    assert!(!positioner.is_moving());

    // Still a no-op while a different motion is in flight: requesting the
    // last known position leaves the moving state unchanged.
    let mut opening = positioner_at(50);
    opening.request(Position::new(75));
    assert_eq!(opening.state(), WindowState::Opening);
    assert!(opening.request(Position::new(50)).is_none());
    assert_eq!(opening.state(), WindowState::Opening);
    assert!(opening.is_moving());
}

#[test]
fn test_finish_full_close() {
    let mut positioner = positioner_at(100);
    positioner.request(Position::CLOSED);
    positioner.finish();
    assert_eq!(positioner.position(), Position::CLOSED);
    assert_eq!(positioner.state(), WindowState::Closed);
    assert!(!positioner.is_moving());
}

#[test]
fn test_finish_full_open() {
    let mut positioner = WindowPositioner::new();
    positioner.request(Position::OPEN);
    positioner.finish();
    assert_eq!(positioner.position(), Position::OPEN);
    assert_eq!(positioner.state(), WindowState::Open);
    assert!(!positioner.is_moving());
}

#[test]
fn test_finish_partial_lands_stopped() {
    let mut positioner = WindowPositioner::new();
    positioner.request(Position::new(50));
    positioner.finish();
    assert_eq!(positioner.position(), Position::new(50));
    assert_eq!(positioner.state(), WindowState::Stopped);
    assert!(!positioner.is_moving());
}

#[test]
fn test_finish_idle_is_noop() {
    let mut positioner = positioner_at(25);
    positioner.finish();
    assert_eq!(positioner.position(), Position::new(25));
    assert_eq!(positioner.state(), WindowState::Stopped);
}

#[test]
fn test_stop_while_opening_interpolates() {
    let mut positioner = WindowPositioner::new();
    positioner.request(Position::OPEN);
    assert!(positioner.is_moving());
    let position = positioner.stop(35_500);
    assert_eq!(position, Position::new(50));
    assert_eq!(positioner.position(), Position::new(50));
    assert_eq!(positioner.state(), WindowState::Stopped);
    assert!(!positioner.is_moving());
}

#[test]
fn test_stop_while_closing_interpolates() {
    let mut positioner = positioner_at(100);
    positioner.request(Position::CLOSED);
    assert!(positioner.is_moving());
    let position = positioner.stop(35_500);
    assert_eq!(position, Position::new(50));
    assert_eq!(positioner.position(), Position::new(50));
    assert_eq!(positioner.state(), WindowState::Stopped);
    assert!(!positioner.is_moving());
}

#[test]
fn test_stop_while_idle_is_noop() {
    let mut positioner = positioner_at(25);
    let position = positioner.stop(35_500);
    assert_eq!(position, Position::new(25));
    assert_eq!(positioner.position(), Position::new(25));
    assert_eq!(positioner.state(), WindowState::Stopped);
}

#[test]
fn test_stop_clamps_at_both_bounds() {
    let mut opening = WindowPositioner::new();
    opening.request(Position::OPEN);
    assert_eq!(opening.stop(TRAVEL_TIME_MS * 2), Position::OPEN);
    assert_eq!(opening.position(), Position::OPEN);

    let mut closing = positioner_at(100);
    closing.request(Position::CLOSED);
    assert_eq!(closing.stop(TRAVEL_TIME_MS * 2), Position::CLOSED);
    assert_eq!(closing.position(), Position::CLOSED);

    // Oversized elapsed must not overflow.
    let mut saturated = WindowPositioner::new();
    saturated.request(Position::OPEN);
    assert_eq!(saturated.stop(u64::MAX), Position::OPEN);

    let mut underflow = positioner_at(100);
    underflow.request(Position::CLOSED);
    assert_eq!(underflow.stop(u64::MAX), Position::CLOSED);
}

#[test]
fn test_travel_duration_symmetry_and_zero() {
    let samples = [
        (Position::new(0), Position::new(0)),
        (Position::new(25), Position::new(25)),
        (Position::new(0), Position::new(100)),
        (Position::new(20), Position::new(80)),
        (Position::new(75), Position::new(25)),
    ];
    for (from, to) in samples {
        assert_eq!(travel_duration_ms(from, to), travel_duration_ms(to, from));
    }
    assert_eq!(travel_duration_ms(Position::new(0), Position::new(0)), 0);
    assert_eq!(travel_duration_ms(Position::new(60), Position::new(60)), 0);
    assert_eq!(
        travel_duration_ms(Position::new(0), Position::new(100)),
        TRAVEL_TIME_MS
    );
    assert_eq!(travel_duration_ms(Position::new(0), Position::new(1)), 710);
}

#[test]
fn test_interrupted_position_mid_and_bounds() {
    assert_eq!(
        interrupted_position(Position::new(0), Direction::Opening, 35_500),
        Position::new(50)
    );
    assert_eq!(
        interrupted_position(Position::new(100), Direction::Closing, 35_500),
        Position::new(50)
    );
    assert_eq!(
        interrupted_position(Position::new(50), Direction::Opening, 0),
        Position::new(50)
    );
    assert_eq!(
        interrupted_position(Position::new(50), Direction::Closing, 0),
        Position::new(50)
    );
    assert_eq!(
        interrupted_position(Position::new(0), Direction::Opening, TRAVEL_TIME_MS * 2),
        Position::OPEN
    );
    assert_eq!(
        interrupted_position(Position::new(100), Direction::Closing, TRAVEL_TIME_MS * 2),
        Position::CLOSED
    );
    assert_eq!(
        interrupted_position(Position::new(0), Direction::Opening, u64::MAX),
        Position::OPEN
    );
    assert_eq!(
        interrupted_position(Position::new(100), Direction::Closing, u64::MAX),
        Position::CLOSED
    );
}

#[test]
fn test_pulse_constants() {
    assert_eq!(PULSE_LOW_MS, 75);
    assert_eq!(PULSE_HIGH_MS, 75);
    assert_eq!(PULSE_SETTLE_MS, 400);
    assert_eq!(PULSE_CYCLE_MS, 300);
    assert_eq!(PULSE_SEQUENCE_MS, 700);
}
