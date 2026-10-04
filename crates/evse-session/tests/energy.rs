//! Limit enforcement and `CC` rate limiting.

use std::time::{Duration, Instant};

use evse_session::{EnergyConfig, EnergyNode, LimitRes};

fn node() -> EnergyNode {
    EnergyNode::new(EnergyConfig {
        max_current_a: 32.0,
        min_current_a: 10.0,
        phases: 3,
        nominal_voltage_v: 230.0,
    })
}

fn amps(a: f64) -> LimitRes {
    LimitRes {
        ac_max_current_a: Some(a),
        ..LimitRes::default()
    }
}

const TEN_S: Duration = Duration::from_secs(10);

#[test]
fn nothing_is_available_before_the_first_limit() {
    let mut n = node();
    let t0 = Instant::now();
    assert_eq!(n.effective(t0), 0.0);
    // The first tick reports the (unavailable) state once, then stays quiet.
    assert!(n.tick(t0).is_some_and(|d| !d.available));
    assert!(n.tick(t0).is_none());
}

#[test]
fn first_limit_is_sent_at_once_and_floored() {
    let mut n = node();
    let t0 = Instant::now();
    let d = n.enforce(&amps(16.7), TEN_S, t0);
    assert!(d.available);
    assert_eq!(d.effective_a, 16.7);
    assert_eq!(d.set_current, Some(16));
}

#[test]
fn changes_are_rate_limited_to_one_per_five_seconds() {
    let mut n = node();
    let t0 = Instant::now();
    n.enforce(&amps(16.0), TEN_S, t0);
    let d = n.enforce(&amps(20.0), TEN_S, t0 + Duration::from_secs(1));
    assert_eq!(d.set_current, None);
    assert!(n.tick(t0 + Duration::from_secs(4)).is_none());
    let d = n.tick(t0 + Duration::from_secs(5));
    assert_eq!(d.map(|d| d.set_current), Some(Some(20)));
    assert!(n.tick(t0 + Duration::from_secs(6)).is_none());
}

#[test]
fn sub_amp_changes_are_not_sent() {
    let mut n = node();
    let t0 = Instant::now();
    n.enforce(&amps(16.0), TEN_S, t0);
    let d = n.enforce(&amps(16.9), TEN_S, t0 + Duration::from_secs(6));
    assert_eq!(d.set_current, None);
}

#[test]
fn below_the_minimum_nothing_is_available_and_nothing_is_sent() {
    let mut n = node();
    let t0 = Instant::now();
    let d = n.enforce(&amps(5.0), TEN_S, t0);
    assert!(!d.available);
    assert_eq!(d.set_current, None);
    let d = n.enforce(&amps(10.0), TEN_S, t0 + Duration::from_secs(1));
    assert!(d.available);
    assert_eq!(d.set_current, Some(10));
}

#[test]
fn an_expired_limit_falls_to_zero() {
    let mut n = node();
    let t0 = Instant::now();
    n.enforce(&amps(16.0), Duration::from_secs(2), t0);
    assert!(n.tick(t0 + Duration::from_secs(2)).is_none());
    let d = n.tick(t0 + Duration::from_secs(3));
    assert!(d.is_some_and(|d| !d.available && d.effective_a == 0.0));
    assert!(n.tick(t0 + Duration::from_secs(4)).is_none());
    // A fresh limit brings it back, and the same CC value is not repeated.
    let d = n.enforce(&amps(16.0), TEN_S, t0 + Duration::from_secs(5));
    assert!(d.available);
    assert_eq!(d.set_current, None);
}

#[test]
fn power_limits_convert_through_voltage_and_phases() {
    let mut n = node();
    let t0 = Instant::now();
    let d = n.enforce(
        &LimitRes {
            total_power_w: Some(11040.0),
            ..LimitRes::default()
        },
        TEN_S,
        t0,
    );
    assert_eq!(d.effective_a, 16.0);
    let d = n.enforce(
        &LimitRes {
            total_power_w: Some(11040.0),
            ac_max_phase_count: Some(1),
            ..LimitRes::default()
        },
        TEN_S,
        t0 + Duration::from_secs(6),
    );
    assert_eq!(d.effective_a, 32.0);
    // The tighter of current and power wins.
    let d = n.enforce(
        &LimitRes {
            ac_max_current_a: Some(12.0),
            total_power_w: Some(11040.0),
            ..LimitRes::default()
        },
        TEN_S,
        t0 + Duration::from_secs(12),
    );
    assert_eq!(d.effective_a, 12.0);
}

#[test]
fn limits_are_clamped_to_the_rating() {
    let mut n = node();
    let t0 = Instant::now();
    assert_eq!(n.enforce(&amps(63.0), TEN_S, t0).effective_a, 32.0);
    assert_eq!(n.enforce(&LimitRes::default(), TEN_S, t0).effective_a, 32.0);
    assert_eq!(n.enforce(&amps(-3.0), TEN_S, t0).effective_a, 0.0);
}
