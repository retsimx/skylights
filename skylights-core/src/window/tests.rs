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
    assert_eq!(plan.travel_ms, 32_467);
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
    assert_eq!(plan.travel_ms, 15_558);
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
    assert_eq!(plan.travel_ms, 48_275);
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
    assert_eq!(position, Position::new(55));
    assert_eq!(positioner.position(), Position::new(55));
    assert_eq!(positioner.state(), WindowState::Stopped);
    assert!(!positioner.is_moving());
}

#[test]
fn test_stop_while_closing_interpolates() {
    let mut positioner = positioner_at(100);
    positioner.request(Position::CLOSED);
    assert!(positioner.is_moving());
    let position = positioner.stop(35_500);
    assert_eq!(position, Position::new(44));
    assert_eq!(positioner.position(), Position::new(44));
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
fn test_travel_duration_calibrated_cases() {
    // Requirement table (design 014 §4.3). The ±200 ms tolerance covers the
    // integer-truncation difference on the closing case: the arithmetic gives
    // 15 250 ms where the measured table rounds to 15 251 ms.
    let cases = [
        (0u8, 20u8, 13_111u64), // opening from the closed seat
        (0, 1, 852),            // opening from the closed seat
        (10, 20, 5_399),        // opening mid-travel
        (1, 20, 11_494),        // opening mid-travel
        (50, 25, 15_250),       // closing mid-travel (table: 15 251)
        (50, 51, MIN_PULSE_MS), // mid-open 1 %: clamped, not a no-op
    ];
    for (from, to, expected) in cases {
        let actual = travel_duration_ms(Position::new(from), Position::new(to));
        assert!(
            actual.abs_diff(expected) <= 200,
            "travel_duration_ms({from}, {to}) = {actual}, expected {expected} +/-200"
        );
    }

    // Exact: 8 522 tenths truncates cleanly to 852. The ±200 band above also
    // accepts the retired 710 ms flat model, so assert this row exactly too.
    assert_eq!(travel_duration_ms(Position::new(0), Position::new(1)), 852);

    // The clamp is exact: the mid-travel 1 % step needs negative drive time
    // and must land on the floor rather than on zero.
    assert_eq!(
        travel_duration_ms(Position::new(50), Position::new(51)),
        MIN_PULSE_MS
    );
}

#[test]
fn test_travel_duration_equal_positions_are_zero() {
    // A partial no-op never commands travel; `request` rejects these before
    // calling the helper.
    for pct in 0..=100u8 {
        assert_eq!(
            travel_duration_ms(Position::new(pct), Position::new(pct)),
            0,
            "equal positions at {pct} % must not command travel"
        );
    }
}

#[test]
fn test_boundary_plan_from_any_position() {
    let targets = [
        (Position::CLOSED, Actuation::Close, Direction::Closing),
        (Position::OPEN, Actuation::Open, Direction::Opening),
    ];
    for start in [0u8, 50, 100] {
        for (target, actuation, direction) in targets {
            let mut positioner = positioner_at(start);
            let plan = positioner
                .request(target)
                .expect("boundary targets always emit a plan");
            assert_eq!(plan.travel_ms, TRAVEL_TIME_MS);
            assert_eq!(plan.travel_ms, 71_000);
            assert_eq!(plan.target, target);
            assert_eq!(plan.actuation, actuation);
            assert_eq!(plan.direction, direction);
            assert!(!plan.stop_after, "boundary drives never need a Stop pulse");
        }
    }
}

#[test]
fn test_min_pulse_is_not_a_noop() {
    let mut positioner = positioner_at(50);
    let plan = positioner
        .request(Position::new(51))
        .expect("a 1 % mid-travel step still pulses");
    assert_eq!(plan.travel_ms, MIN_PULSE_MS);
    assert_eq!(plan.travel_ms, 200);
    assert!(plan.stop_after);
    assert_eq!(plan.direction, Direction::Opening);
    assert_eq!(plan.target, Position::new(51));
    assert_eq!(positioner.state(), WindowState::Opening);
}

#[test]
fn test_interrupted_position_round_trips() {
    // Mid-travel opening 50 -> 70: the commanded time must invert back to the
    // 20-point delta (this is the requirement's round-trip, ±1).
    let travel_ms = travel_duration_ms(Position::new(50), Position::new(70));
    assert_eq!(travel_ms, 12_172);

    let stopped = interrupted_position(Position::new(50), Direction::Opening, travel_ms);
    let delta = stopped.percent() as i32 - 50;
    assert!(
        (delta - 20).abs() <= 1,
        "expected a 20 point delta +/-1, got {delta} ({stopped:?})"
    );
    assert!(
        (stopped.percent() as i32 - 70).abs() <= 1,
        "expected 70 % +/-1, got {stopped:?}"
    );
}

#[test]
fn test_interrupted_position_zero_elapsed_is_model_value() {
    // Spec-literal (design D5): the mid curves carry the ~2 % per-move
    // over-travel as a negative intercept, so zero elapsed reads +2 / -2. The
    // seat curve's positive intercept cancels, so it still reads 0.
    assert_eq!(
        interrupted_position(Position::new(50), Direction::Opening, 0),
        Position::new(52)
    );
    assert_eq!(
        interrupted_position(Position::new(50), Direction::Closing, 0),
        Position::new(48)
    );
    assert_eq!(
        interrupted_position(Position::new(0), Direction::Opening, 0),
        Position::new(0)
    );
}

#[test]
fn test_interrupted_position_mid_and_bounds() {
    assert_eq!(
        interrupted_position(Position::new(0), Direction::Opening, 35_500),
        Position::new(55)
    );
    assert_eq!(
        interrupted_position(Position::new(100), Direction::Closing, 35_500),
        Position::new(44)
    );
    assert_eq!(
        interrupted_position(Position::new(50), Direction::Opening, 0),
        Position::new(52)
    );
    assert_eq!(
        interrupted_position(Position::new(50), Direction::Closing, 0),
        Position::new(48)
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
    assert_eq!(PULSE_LOW_MS, 100);
    assert_eq!(PULSE_HIGH_MS, 800);
    assert_eq!(PULSE_SETTLE_MS, 400);
    assert_eq!(PULSE_CYCLE_MS, 1800);
    assert_eq!(PULSE_SEQUENCE_MS, 2200);
}
