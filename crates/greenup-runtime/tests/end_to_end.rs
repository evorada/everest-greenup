//! The runtime against `legrand-sim` over TCP: the whole pipe from socket
//! to session events, on a Mac, in a few seconds.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use board_model::Profile;
use evse_session::{EnergyConfig, Event, EvseState, LimitRes, StopReason};
use greenup_runtime::{Notification, Reply, Request, Runtime, RuntimeConfig};
use legrand_driver::DriverConfig;
use legrand_sim::Simulator;

type Token = String;

fn start(profile: Profile, state_file: Option<std::path::PathBuf>) -> (Simulator, Runtime<Token>) {
    let sim = Simulator::start(
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
        profile,
        Vec::new(),
    )
    .unwrap();
    let mut config = RuntimeConfig::new(
        format!("tcp:{}", sim.board_addr()),
        DriverConfig {
            expected_side: Some(1),
            ..DriverConfig::default()
        },
        EnergyConfig {
            max_current_a: 32.0,
            min_current_a: 10.0,
            phases: 3,
            nominal_voltage_v: 230.0,
        },
    );
    config.state_file = state_file;
    config.meter_interval_idle = Duration::from_millis(500);
    config.persist_interval = Duration::from_millis(200);
    let runtime = Runtime::start(config).unwrap();
    (sim, runtime)
}

/// Collect event names until `last` shows up or the timeout passes.
fn wait_for(rx: &Receiver<Notification<Token>>, last: &str, timeout: Duration) -> Vec<String> {
    let deadline = Instant::now() + timeout;
    let mut names = Vec::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Notification::Session { event, .. }) => {
                names.push(event.event.name().to_owned());
                if event.event.name() == last {
                    return names;
                }
            }
            Ok(Notification::FaultRaised(m)) => names.push(format!("Inoperative({m})")),
            Ok(Notification::FaultCleared) => names.push("Cleared".to_owned()),
            Ok(_) | Err(_) => {}
        }
    }
    names
}

fn grant(runtime: &Runtime<Token>, amps: f64) {
    runtime.handle().call(Request::EnforceLimits {
        limits: LimitRes {
            ac_max_current_a: Some(amps),
            ..LimitRes::default()
        },
        valid_for: Duration::from_secs(10),
    });
}

#[test]
fn plug_authorize_charge_unplug_through_the_socket() {
    let dir = std::env::temp_dir().join(format!("greenup-rt-{}", std::process::id()));
    let state_file = dir.join("side1.json");
    let (sim, mut runtime) = start(Profile::default(), Some(state_file.clone()));
    let rx = runtime.take_notifications().unwrap();
    let handle = runtime.handle();
    handle.call(Request::Startup);
    assert_eq!(
        wait_for(&rx, "Enabled", Duration::from_secs(2)),
        ["Enabled"]
    );
    grant(&runtime, 16.0);
    assert_eq!(
        sim.control("plug").unwrap().split(' ').nth(1),
        Some("state=B")
    );
    assert_eq!(
        wait_for(&rx, "AuthRequired", Duration::from_secs(5)),
        ["SessionStarted", "AuthRequired"]
    );
    handle.call(Request::Authorize {
        token: "tok".into(),
        reservation_id: None,
    });
    assert_eq!(
        wait_for(&rx, "PrepareCharging", Duration::from_secs(5)),
        ["Authorized", "TransactionStarted", "PrepareCharging"]
    );
    // The board must have been told CC:16 then T2COK.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = sim.control("state").unwrap();
        if state.contains("t2=1") && state.contains("cc=16") {
            break;
        }
        assert!(Instant::now() < deadline, "board not authorized: {state}");
        std::thread::sleep(Duration::from_millis(100));
    }
    sim.control("draw").unwrap();
    assert_eq!(
        wait_for(&rx, "ChargingStarted", Duration::from_secs(5)),
        ["ChargingStarted"]
    );
    assert!(matches!(
        handle.call(Request::PauseCharging),
        Reply::Accepted(true)
    ));
    assert_eq!(
        wait_for(&rx, "ChargingPausedEVSE", Duration::from_secs(5)),
        ["ChargingPausedEVSE"]
    );
    assert!(handle.accepted(Request::ResumeCharging));
    assert_eq!(
        wait_for(&rx, "PrepareCharging", Duration::from_secs(5)),
        ["PrepareCharging"]
    );
    assert_eq!(
        wait_for(&rx, "ChargingStarted", Duration::from_secs(8)),
        ["ChargingStarted"]
    );
    // A stale limit pauses with NoEnergy; a fresh one resumes.
    std::thread::sleep(Duration::from_secs(1));
    grant(&runtime, 5.0);
    assert_eq!(
        wait_for(&rx, "ChargingPausedEVSE", Duration::from_secs(5)),
        ["ChargingPausedEVSE"]
    );
    grant(&runtime, 20.0);
    assert_eq!(
        wait_for(&rx, "PrepareCharging", Duration::from_secs(5)),
        ["PrepareCharging"]
    );
    sim.control("unplug").unwrap();
    assert_eq!(
        wait_for(&rx, "SessionFinished", Duration::from_secs(8)),
        [
            "StoppingCharging",
            "ChargingFinished",
            "TransactionFinished",
            "SessionFinished"
        ]
    );
    runtime.shutdown();
    let persisted = greenup_runtime::PersistedState::load(&state_file).unwrap();
    assert!(persisted.energy_wh > 0.0, "{persisted:?}");
    assert_eq!(persisted.open_transaction, None);
    sim.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_open_transaction_is_recovered_at_startup() {
    let dir = std::env::temp_dir().join(format!("greenup-rt-rec-{}", std::process::id()));
    let state_file = dir.join("side1.json");
    greenup_runtime::PersistedState {
        energy_wh: 4321.0,
        open_transaction: Some("old-uuid".into()),
    }
    .save(&state_file)
    .unwrap();
    let (sim, mut runtime) = start(Profile::default(), Some(state_file.clone()));
    let rx = runtime.take_notifications().unwrap();
    runtime.handle().call(Request::Startup);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut recovered = None;
    while Instant::now() < deadline && recovered.is_none() {
        if let Ok(Notification::RecoveredTransaction { uuid, meter }) =
            rx.recv_timeout(Duration::from_millis(100))
        {
            recovered = Some((uuid, meter));
        }
    }
    let (uuid, meter) = recovered.expect("recovered transaction");
    assert_eq!(uuid, "old-uuid");
    assert_eq!(meter.energy_wh_import, 4321.0);
    runtime.shutdown();
    sim.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_board_is_a_fault_until_it_appears() {
    let sim = Simulator::start(
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
        Profile::default(),
        Vec::new(),
    )
    .unwrap();
    let addr = sim.board_addr();
    sim.stop();
    let mut config = RuntimeConfig::new(
        format!("tcp:{addr}"),
        DriverConfig::default(),
        EnergyConfig {
            max_current_a: 32.0,
            min_current_a: 10.0,
            phases: 3,
            nominal_voltage_v: 230.0,
        },
    );
    config.reconnect_interval = Duration::from_millis(300);
    let mut runtime = Runtime::<Token>::start(config).unwrap();
    let rx = runtime.take_notifications().unwrap();
    runtime.handle().call(Request::Startup);
    let names = wait_for(&rx, "never", Duration::from_secs(2));
    assert!(
        names.iter().any(|n| n.starts_with("Inoperative(")),
        "{names:?}"
    );
    assert_eq!(
        runtime.handle().call(Request::Reserve(1)),
        Reply::Accepted(false)
    );
    let sim = Simulator::start(
        addr,
        "127.0.0.1:0".parse().unwrap(),
        Profile::default(),
        Vec::new(),
    )
    .unwrap();
    let names = wait_for(&rx, "Cleared", Duration::from_secs(5));
    assert_eq!(
        names.last().map(String::as_str),
        Some("Cleared"),
        "{names:?}"
    );
    runtime.shutdown();
    sim.stop();
}

#[test]
fn stop_transaction_and_disable_through_the_handle() {
    let (sim, mut runtime) = start(Profile::default(), None);
    let rx = runtime.take_notifications().unwrap();
    let handle = runtime.handle();
    handle.call(Request::Startup);
    wait_for(&rx, "Enabled", Duration::from_secs(2));
    grant(&runtime, 16.0);
    sim.control("plug").unwrap();
    wait_for(&rx, "AuthRequired", Duration::from_secs(5));
    assert!(!handle.accepted(Request::StopTransaction {
        reason: StopReason::Remote,
        id_tag: None
    }));
    handle.call(Request::Authorize {
        token: "tok".into(),
        reservation_id: Some(3),
    });
    wait_for(&rx, "PrepareCharging", Duration::from_secs(5));
    assert!(handle.accepted(Request::StopTransaction {
        reason: StopReason::Remote,
        id_tag: None
    }));
    assert_eq!(
        wait_for(&rx, "TransactionFinished", Duration::from_secs(5)),
        [
            "StoppingCharging",
            "ChargingFinished",
            "TransactionFinished"
        ]
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while sim.control("state").unwrap().contains("t2=1") {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(100));
    }
    let disable = evse_session::EnableSource {
        kind: evse_session::EnableSourceKind::Csms,
        state: evse_session::EnableState::Disable,
        priority: 10,
    };
    assert!(!handle.accepted(Request::EnableDisable(disable)));
    assert_eq!(
        wait_for(&rx, "Disabled", Duration::from_secs(5)),
        ["SessionFinished", "Disabled"]
    );
    // Energy requests keep flowing and report the state.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut saw = false;
    while Instant::now() < deadline && !saw {
        if let Ok(Notification::EnergyRequest(req)) = rx.recv_timeout(Duration::from_millis(100)) {
            saw = req.evse_state == EvseState::Disabled;
        }
    }
    assert!(saw);
    runtime.shutdown();
    sim.stop();
    let _ = Event::<String>::AuthRequired;
}
