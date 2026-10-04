//! Table-style scenarios against the session core. Expected event
//! sequences follow EvseManager (`modules/EVSE/EvseManager/docs/index.rst`).

use evse_session::{
    DriverAction, EnableSource, EnableSourceKind, EnableState, Event, EvseState, Fact, Hint,
    Output, PauseReason, Session, SessionConfig, StartReason, StopReason,
};

type S = Session<String>;

/// Event names and driver actions, in order, as a compact trace.
fn trace(out: &[Output<String>]) -> Vec<String> {
    out.iter()
        .map(|o| match o {
            Output::Event(e) => e.event.name().to_owned(),
            Output::Driver(DriverAction::Authorize(true)) => "T2COK".to_owned(),
            Output::Driver(DriverAction::Authorize(false)) => "T2CNOK".to_owned(),
            Output::Driver(DriverAction::SetCurrent(a)) => format!("CC:{a}"),
            Output::Driver(DriverAction::Unlock) => "Unlock".to_owned(),
            Output::FaultRaised(m) => format!("Inoperative({m})"),
            Output::FaultCleared => "Cleared".to_owned(),
        })
        .collect()
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// A session that is enabled, has energy and a vehicle plugged in.
fn plugged() -> S {
    let mut s = S::default();
    s.startup();
    s.set_energy_available(true);
    assert_eq!(
        trace(&s.on_fact(Fact::Plugged)),
        names(&["SessionStarted", "AuthRequired"])
    );
    s
}

/// Plugged, authorized and delivering.
fn charging() -> S {
    let mut s = plugged();
    assert_eq!(
        trace(&s.authorize("tok".into(), None)),
        names(&[
            "Authorized",
            "TransactionStarted",
            "T2COK",
            "PrepareCharging"
        ])
    );
    assert!(s.on_fact(Fact::BoardAuthorized).is_empty());
    assert_eq!(
        trace(&s.on_fact(Fact::Delivering)),
        names(&["ChargingStarted"])
    );
    s
}

fn uuids(out: &[Output<String>]) -> Vec<String> {
    out.iter()
        .filter_map(|o| match o {
            Output::Event(e) => Some(e.uuid.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn startup_publishes_the_initial_enabled() {
    let mut s = S::default();
    let out = s.startup();
    assert!(matches!(
        out.as_slice(),
        [Output::Event(e)] if e.event == Event::Enabled(EnableSource::INITIAL)
    ));
    assert!(s.take_priority());
    assert!(!s.take_priority());
    assert_eq!(s.evse_state(), EvseState::Unplugged);
}

#[test]
fn plug_authorize_charge_pause_by_ev_unplug() {
    let mut s = charging();
    let uuid = s.uuid().to_owned();
    assert_eq!(uuid.len(), 36);
    assert_eq!(s.evse_state(), EvseState::Charging);
    assert_eq!(
        trace(&s.on_fact(Fact::BoardAuthorized)),
        names(&["ChargingPausedEV"])
    );
    assert_eq!(s.evse_state(), EvseState::PausedEv);
    assert_eq!(
        trace(&s.on_fact(Fact::Delivering)),
        names(&["ChargingStarted"])
    );
    let out = s.on_fact(Fact::Unplugged);
    assert_eq!(
        trace(&out),
        names(&[
            "T2CNOK",
            "StoppingCharging",
            "ChargingFinished",
            "TransactionFinished",
            "SessionFinished"
        ])
    );
    assert!(uuids(&out).iter().all(|u| *u == uuid));
    assert!(!s.transaction_active());
    assert_eq!(s.evse_state(), EvseState::Unplugged);
    assert!(matches!(
        &out[3],
        Output::Event(e) if e.event == Event::TransactionFinished { reason: StopReason::EvDisconnected, id_tag: None }
    ));
}

#[test]
fn authorization_before_plug_in_starts_the_session_but_not_the_board() {
    let mut s = S::default();
    s.startup();
    s.set_energy_available(true);
    let out = s.authorize("tok".into(), None);
    assert_eq!(trace(&out), names(&["SessionStarted", "Authorized"]));
    assert!(matches!(
        &out[0],
        Output::Event(e) if e.event == Event::SessionStarted { reason: StartReason::Authorized, id_tag: Some("tok".into()), reservation_id: None }
    ));
    assert_eq!(s.evse_state(), EvseState::WaitForAuth);
    let uuid = s.uuid().to_owned();
    let out = s.on_fact(Fact::Plugged);
    assert_eq!(
        trace(&out),
        names(&["TransactionStarted", "T2COK", "PrepareCharging"])
    );
    assert_eq!(s.uuid(), uuid);
}

#[test]
fn withdraw_before_plug_in_ends_the_session() {
    let mut s = S::default();
    s.startup();
    s.set_energy_available(true);
    s.authorize("tok".into(), None);
    assert_eq!(
        trace(&s.withdraw_authorization()),
        names(&["Deauthorized", "SessionFinished"])
    );
    assert_eq!(s.evse_state(), EvseState::Unplugged);
    // A vehicle arriving later needs a fresh authorization.
    assert_eq!(
        trace(&s.on_fact(Fact::Plugged)),
        names(&["SessionStarted", "AuthRequired"])
    );
}

#[test]
fn withdraw_while_plugged_without_authorization_is_a_plugin_timeout() {
    let mut s = plugged();
    assert_eq!(
        trace(&s.withdraw_authorization()),
        names(&["Deauthorized", "PluginTimeout"])
    );
    assert!(s.withdraw_authorization().is_empty());
    assert_eq!(
        trace(&s.on_fact(Fact::Unplugged)),
        names(&["SessionFinished"])
    );
}

#[test]
fn stop_transaction_ends_it_and_keeps_the_session_until_unplug() {
    let mut s = charging();
    let (ok, out) = s.stop_transaction(StopReason::Remote, Some("stopper".into()));
    assert!(ok);
    assert_eq!(
        trace(&out),
        names(&[
            "T2CNOK",
            "StoppingCharging",
            "ChargingFinished",
            "TransactionFinished"
        ])
    );
    assert!(matches!(
        &out[3],
        Output::Event(e) if e.event == Event::TransactionFinished { reason: StopReason::Remote, id_tag: Some("stopper".into()) }
    ));
    assert_eq!(s.evse_state(), EvseState::Finished);
    assert!(!s.stop_transaction(StopReason::Remote, None).0);
    assert!(s.on_fact(Fact::EndedByBoard).is_empty());
    assert_eq!(
        trace(&s.on_fact(Fact::Unplugged)),
        names(&["SessionFinished"])
    );
}

#[test]
fn pause_and_resume() {
    let mut s = charging();
    let (ok, out) = s.pause_charging();
    assert!(ok);
    assert_eq!(trace(&out), names(&["T2CNOK", "ChargingPausedEVSE"]));
    assert_eq!(s.pause_reasons(), [PauseReason::UserPause]);
    assert_eq!(s.evse_state(), EvseState::PausedEvse);
    // Pausing again changes nothing but still succeeds.
    let (ok, out) = s.pause_charging();
    assert!(ok);
    assert!(out.is_empty());
    let (ok, out) = s.resume_charging();
    assert!(ok);
    assert_eq!(trace(&out), names(&["T2COK", "PrepareCharging"]));
    assert!(!s.resume_charging().0);
    assert_eq!(
        trace(&s.on_fact(Fact::Delivering)),
        names(&["ChargingStarted"])
    );
    // A vehicle pause cannot be resumed from our side.
    s.on_fact(Fact::BoardAuthorized);
    assert!(!s.resume_charging().0);
    assert!(!S::default().pause_charging().0);
}

#[test]
fn transaction_waits_for_energy_and_combines_pause_reasons() {
    let mut s = S::default();
    s.startup();
    s.on_fact(Fact::Plugged);
    let out = s.authorize("tok".into(), None);
    assert_eq!(
        trace(&out),
        names(&[
            "Authorized",
            "TransactionStarted",
            "T2CNOK",
            "ChargingPausedEVSE"
        ])
    );
    assert_eq!(s.evse_state(), EvseState::WaitForEnergy);
    assert!(s.pause_charging().0);
    assert_eq!(
        s.pause_reasons(),
        [PauseReason::UserPause, PauseReason::NoEnergy]
    );
    assert_eq!(s.evse_state(), EvseState::PausedEvse);
    assert_eq!(
        trace(&s.set_energy_available(true)),
        names(&["ChargingPausedEVSE"])
    );
    let (ok, out) = s.resume_charging();
    assert!(ok);
    assert_eq!(trace(&out), names(&["T2COK", "PrepareCharging"]));
    s.on_fact(Fact::Delivering);
    assert_eq!(
        trace(&s.set_energy_available(false)),
        names(&["T2CNOK", "ChargingPausedEVSE"])
    );
    assert_eq!(s.evse_state(), EvseState::WaitForEnergy);
    assert!(s.set_energy_available(false).is_empty());
}

#[test]
fn disable_tears_everything_down_and_enable_starts_over() {
    let mut s = charging();
    let disable = EnableSource {
        kind: EnableSourceKind::Csms,
        state: EnableState::Disable,
        priority: 100,
    };
    let (enabled, out) = s.enable_disable(disable);
    assert!(!enabled);
    assert_eq!(
        trace(&out),
        names(&[
            "T2CNOK",
            "StoppingCharging",
            "ChargingFinished",
            "TransactionFinished",
            "SessionFinished",
            "Disabled"
        ])
    );
    assert!(matches!(
        &out[3],
        Output::Event(e) if e.event == Event::TransactionFinished { reason: StopReason::EvseDisabled, id_tag: None }
    ));
    assert_eq!(s.evse_state(), EvseState::Disabled);
    assert!(s.authorize("tok".into(), None).is_empty());
    assert!(!s.reserve(1).0);
    // Same request again: no change, no event.
    assert!(s.enable_disable(disable).1.is_empty());
    let enable = EnableSource {
        state: EnableState::Enable,
        ..disable
    };
    let (enabled, out) = s.enable_disable(enable);
    assert!(enabled);
    assert_eq!(
        trace(&out),
        names(&["Enabled", "SessionStarted", "AuthRequired"])
    );
    assert!(matches!(&out[0], Output::Event(e) if e.event == Event::Enabled(enable)));
}

#[test]
fn enable_arbitration_follows_priority_and_disable_wins_ties() {
    let mut s = S::default();
    s.startup();
    let api_disable = EnableSource {
        kind: EnableSourceKind::LocalApi,
        state: EnableState::Disable,
        priority: 42,
    };
    let lock_enable = EnableSource {
        kind: EnableSourceKind::LocalKeyLock,
        state: EnableState::Enable,
        priority: 0,
    };
    assert!(!s.enable_disable(api_disable).0);
    assert!(s.enable_disable(lock_enable).0);
    assert_eq!(s.active_source(), lock_enable);
    let lock_unassigned = EnableSource {
        state: EnableState::Unassigned,
        ..lock_enable
    };
    assert!(!s.enable_disable(lock_unassigned).0);
    assert_eq!(s.active_source(), api_disable);
    let csms_enable_same_priority = EnableSource {
        kind: EnableSourceKind::Csms,
        state: EnableState::Enable,
        priority: 42,
    };
    assert!(!s.enable_disable(csms_enable_same_priority).0);
    let api_unassigned = EnableSource {
        state: EnableState::Unassigned,
        ..api_disable
    };
    assert!(s.enable_disable(api_unassigned).0);
}

#[test]
fn reservation_is_consumed_by_the_transaction() {
    let mut s = S::default();
    s.startup();
    s.set_energy_available(true);
    let (ok, out) = s.reserve(7);
    assert!(ok);
    assert_eq!(trace(&out), names(&["ReservationStart"]));
    let (ok, out) = s.reserve(8);
    assert!(ok);
    assert!(out.is_empty());
    let out = s.on_fact(Fact::Plugged);
    assert!(matches!(
        &out[0],
        Output::Event(e) if e.event == Event::SessionStarted { reason: StartReason::EvConnected, id_tag: None, reservation_id: Some(8) }
    ));
    assert!(!s.reserve(9).0);
    let out = s.authorize("tok".into(), None);
    assert_eq!(
        trace(&out),
        names(&[
            "Authorized",
            "TransactionStarted",
            "ReservationEnd",
            "T2COK",
            "PrepareCharging"
        ])
    );
    assert!(matches!(
        &out[1],
        Output::Event(e) if e.event == Event::TransactionStarted { id_tag: "tok".into(), reservation_id: Some(8) }
    ));
    assert!(s.cancel_reservation().is_empty());
}

#[test]
fn cancel_reservation_ends_it() {
    let mut s = S::default();
    s.startup();
    s.reserve(7);
    assert_eq!(trace(&s.cancel_reservation()), names(&["ReservationEnd"]));
    assert!(s.cancel_reservation().is_empty());
}

#[test]
fn faults_pause_the_transaction_and_clear_resumes_it() {
    let mut s = charging();
    assert_eq!(
        trace(&s.set_fault(Some("E:0040".into()))),
        names(&["Inoperative(E:0040)", "T2CNOK", "ChargingPausedEVSE"])
    );
    assert_eq!(s.pause_reasons(), [PauseReason::Error]);
    assert!(s.set_fault(Some("E:0040".into())).is_empty());
    assert_eq!(
        trace(&s.set_fault(Some("link down".into()))),
        names(&["Cleared", "Inoperative(link down)"])
    );
    assert_eq!(
        trace(&s.set_fault(None)),
        names(&["Cleared", "T2COK", "PrepareCharging"])
    );
    assert!(s.set_fault(None).is_empty());
}

#[test]
fn faulted_connector_refuses_authorization_and_reservation() {
    let mut s = plugged();
    s.set_fault(Some("fault".into()));
    assert!(s.authorize("tok".into(), None).is_empty());
    assert!(!s.reserve(1).0);
    s.set_fault(None);
    assert!(!s.authorize("tok".into(), None).is_empty());
}

#[test]
fn board_ended_session_after_charge_complete_is_stopped_by_ev() {
    let mut s = charging();
    s.on_hint(&Hint::ChargeComplete);
    assert_eq!(
        trace(&s.on_fact(Fact::Stopping)),
        names(&["StoppingCharging"])
    );
    let out = s.on_fact(Fact::EndedByBoard);
    assert_eq!(
        trace(&out),
        names(&["T2CNOK", "ChargingFinished", "TransactionFinished"])
    );
    assert!(matches!(
        &out[2],
        Output::Event(e) if e.event == Event::TransactionFinished { reason: StopReason::StoppedByEv, id_tag: None }
    ));
}

#[test]
fn board_ended_session_without_a_hint_is_other() {
    let mut s = charging();
    let out = s.on_fact(Fact::EndedByBoard);
    assert!(matches!(
        &out[3],
        Output::Event(e) if e.event == Event::TransactionFinished { reason: StopReason::Other, id_tag: None }
    ));
}

#[test]
fn force_unlock_stops_the_transaction_first() {
    let mut s = charging();
    let (ok, out) = s.force_unlock();
    assert!(ok);
    assert_eq!(
        trace(&out),
        names(&[
            "T2CNOK",
            "StoppingCharging",
            "ChargingFinished",
            "TransactionFinished",
            "Unlock"
        ])
    );
    let (ok, out) = S::default().force_unlock();
    assert!(ok);
    assert_eq!(trace(&out), names(&["Unlock"]));
}

#[test]
fn the_board_winding_down_after_our_block_is_not_a_stop() {
    let mut s = charging();
    s.pause_charging();
    assert!(s.on_fact(Fact::Stopping).is_empty());
    assert!(s.on_fact(Fact::EndedByBoard).is_empty());
    assert!(s.transaction_active());
    let (ok, out) = s.resume_charging();
    assert!(ok);
    assert_eq!(trace(&out), names(&["T2COK", "PrepareCharging"]));
    // The W from the earlier block may arrive late: still not a stop.
    assert!(s.on_fact(Fact::Stopping).is_empty());
    assert_eq!(
        trace(&s.on_fact(Fact::Delivering)),
        names(&["ChargingStarted"])
    );
}

#[test]
fn a_board_that_cannot_resume_ends_the_transaction_on_resume() {
    let mut s = S::new(SessionConfig {
        resume_from_ended: false,
    });
    s.startup();
    s.set_energy_available(true);
    s.on_fact(Fact::Plugged);
    s.authorize("tok".into(), None);
    s.on_fact(Fact::Delivering);
    s.pause_charging();
    s.on_fact(Fact::EndedByBoard);
    let (ok, out) = s.resume_charging();
    assert!(!ok);
    assert_eq!(
        trace(&out),
        names(&[
            "T2CNOK",
            "StoppingCharging",
            "ChargingFinished",
            "TransactionFinished"
        ])
    );
    assert_eq!(s.evse_state(), EvseState::Finished);
}
