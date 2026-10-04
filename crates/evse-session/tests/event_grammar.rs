//! Property tests on the event grammar: random inputs, invariants that the
//! consumers (Auth, OCPP, API) rely on.

use evse_session::{
    DriverAction, EnableSource, EnableSourceKind, EnableState, Event, Fact, Hint, Output, Session,
    StopReason,
};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Fact(Fact),
    Hint,
    Fault(Option<String>),
    Authorize(u8, Option<i64>),
    Withdraw,
    Stop,
    Pause,
    Resume,
    Energy(bool),
    Enable(EnableSource),
    Reserve(i64),
    CancelReservation,
    ForceUnlock,
}

fn fact() -> impl Strategy<Value = Fact> {
    prop_oneof![
        Just(Fact::Unplugged),
        Just(Fact::Plugged),
        Just(Fact::BoardAuthorized),
        Just(Fact::Delivering),
        Just(Fact::Stopping),
        Just(Fact::EndedByBoard),
    ]
}

fn source() -> impl Strategy<Value = EnableSource> {
    (
        prop_oneof![
            Just(EnableSourceKind::LocalApi),
            Just(EnableSourceKind::Csms),
            Just(EnableSourceKind::LocalKeyLock),
        ],
        prop_oneof![
            Just(EnableState::Unassigned),
            Just(EnableState::Disable),
            Just(EnableState::Enable),
        ],
        0i64..3,
    )
        .prop_map(|(kind, state, priority)| EnableSource {
            kind,
            state,
            priority,
        })
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => fact().prop_map(Op::Fact),
        1 => Just(Op::Hint),
        1 => prop_oneof![Just(None), Just(Some("f1".to_owned())), Just(Some("f2".to_owned()))]
            .prop_map(Op::Fault),
        3 => (0u8..3, prop_oneof![Just(None), Just(Some(5i64))]).prop_map(|(t, r)| Op::Authorize(t, r)),
        1 => Just(Op::Withdraw),
        1 => Just(Op::Stop),
        1 => Just(Op::Pause),
        1 => Just(Op::Resume),
        2 => any::<bool>().prop_map(Op::Energy),
        1 => source().prop_map(Op::Enable),
        1 => (1i64..4).prop_map(Op::Reserve),
        1 => Just(Op::CancelReservation),
        1 => Just(Op::ForceUnlock),
    ]
}

#[derive(Default)]
struct Check {
    enabled: bool,
    session: Option<String>,
    in_transaction: bool,
    charging: bool,
    fault_raised: bool,
    last_fact: Option<Fact>,
    events: Vec<&'static str>,
}

impl Check {
    fn event(&mut self, uuid: &str, event: &Event<u8>, op: &Op, plugged: bool) {
        assert!(uuid.len() <= 36 && uuid.is_ascii(), "uuid {uuid:?}");
        self.events.push(event.name());
        match event {
            Event::Enabled(_) => {
                assert!(
                    !self.enabled || self.events.len() == 1,
                    "Enabled without change"
                );
                self.enabled = true;
            }
            Event::Disabled(_) => {
                assert!(self.enabled, "Disabled without change");
                self.enabled = false;
            }
            Event::SessionStarted { .. } => {
                assert!(self.session.is_none(), "nested session");
                assert!(self.enabled, "session while disabled");
                self.session = Some(uuid.to_owned());
            }
            Event::TransactionStarted { .. } => {
                assert_eq!(
                    self.session.as_deref(),
                    Some(uuid),
                    "transaction outside session"
                );
                assert!(!self.in_transaction, "nested transaction");
                self.in_transaction = true;
            }
            Event::ChargingStarted => {
                assert!(self.in_transaction, "charging without transaction");
                self.charging = true;
            }
            Event::ChargingPausedEv | Event::ChargingPausedEvse(_) => {
                assert!(self.in_transaction, "pause without transaction");
                self.charging = false;
            }
            Event::StoppingCharging | Event::ChargingFinished => {
                assert!(self.in_transaction, "{} without transaction", event.name());
            }
            Event::TransactionFinished { reason, .. } => {
                assert!(self.in_transaction, "finish without transaction");
                let n = self.events.len();
                assert_eq!(self.events[n - 2], "ChargingFinished");
                self.in_transaction = false;
                self.charging = false;
                if *reason == StopReason::EvDisconnected {
                    assert!(matches!(op, Op::Fact(Fact::Unplugged)));
                }
            }
            Event::SessionFinished => {
                assert_eq!(
                    self.session.as_deref(),
                    Some(uuid),
                    "finish outside session"
                );
                assert!(!self.in_transaction, "session finished during transaction");
                assert!(
                    !plugged || matches!(op, Op::Enable(_)),
                    "SessionFinished with a vehicle plugged in, op {op:?}"
                );
                self.session = None;
            }
            Event::Authorized
            | Event::Deauthorized
            | Event::AuthRequired
            | Event::PrepareCharging
            | Event::PluginTimeout => {
                assert_eq!(
                    self.session.as_deref(),
                    Some(uuid),
                    "{} outside session",
                    event.name()
                );
            }
            Event::ReservationStart | Event::ReservationEnd => {}
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, failure_persistence: None, ..ProptestConfig::default() })]
    #[test]
    fn random_inputs_keep_the_grammar(ops in prop::collection::vec(op(), 1..60)) {
        let mut s = Session::<u8>::default();
        let mut check = Check::default();
        for o in s.startup() {
            if let Output::Event(e) = o {
                check.event(&e.uuid, &e.event, &Op::Hint, false);
            }
        }
        for op in &ops {
            let out = match op.clone() {
                Op::Fact(f) => { check.last_fact = Some(f); s.on_fact(f) }
                Op::Hint => s.on_hint(&Hint::ChargeComplete),
                Op::Fault(f) => s.set_fault(f),
                Op::Authorize(t, r) => s.authorize(t, r),
                Op::Withdraw => s.withdraw_authorization(),
                Op::Stop => s.stop_transaction(StopReason::Remote, None).1,
                Op::Pause => s.pause_charging().1,
                Op::Resume => s.resume_charging().1,
                Op::Energy(a) => s.set_energy_available(a),
                Op::Enable(src) => s.enable_disable(src).1,
                Op::Reserve(id) => s.reserve(id).1,
                Op::CancelReservation => s.cancel_reservation(),
                Op::ForceUnlock => s.force_unlock().1,
            };
            for o in &out {
                match o {
                    Output::Event(e) => check.event(&e.uuid, &e.event, op, s.plugged()),
                    Output::Driver(DriverAction::Authorize(true)) => {
                        prop_assert!(s.plugged(), "T2COK while unplugged (board state A)");
                        prop_assert!(check.in_transaction, "T2COK without a transaction");
                        prop_assert!(s.fault().is_none(), "T2COK while faulted");
                    }
                    Output::Driver(_) => {}
                    Output::FaultRaised(_) => {
                        prop_assert!(!check.fault_raised, "raised twice");
                        check.fault_raised = true;
                    }
                    Output::FaultCleared => {
                        prop_assert!(check.fault_raised, "cleared without raise");
                        check.fault_raised = false;
                    }
                }
            }
            prop_assert_eq!(check.enabled, s.enabled());
            prop_assert_eq!(check.in_transaction, s.transaction_active());
            prop_assert_eq!(check.fault_raised, s.fault().is_some());
            if check.session.is_some() {
                prop_assert_eq!(check.session.as_deref(), Some(s.uuid()));
            }
        }
    }
}
