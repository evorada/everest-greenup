//! The fake board against the protocol crate: every line it emits parses,
//! and the profiles differ where the plan says they may.

use std::time::{Duration, Instant};

use board_model::{Board, Profile};
use legrand_proto::{Message, StateCode};

fn parses(lines: &[String]) {
    for line in lines {
        let parsed = Message::parse(line);
        assert!(
            !matches!(parsed, Message::Unknown(_) | Message::Malformed { .. }),
            "{line:?} -> {parsed:?}"
        );
    }
}

fn state(lines: &[String]) -> Option<char> {
    lines.iter().rev().find_map(|l| match Message::parse(l) {
        Message::State(s) => Some(s.letter()),
        _ => None,
    })
}

#[test]
fn plug_authorize_draw_unplug_on_every_profile() {
    for (name, profile) in Profile::all() {
        let t0 = Instant::now();
        let mut b = Board::new(profile.clone(), t0);
        assert_eq!(b.state(), 'A', "{name}");
        let out = b.input_line("State?\r", t0);
        assert_eq!(out, ["State:A"], "{name}");
        let out = b.plug(t0);
        assert_eq!(state(&out), Some('B'), "{name}");
        // Drawing without authorization changes nothing.
        let out = b.ev_draw(true, t0);
        assert!(out.is_empty(), "{name}: {out:?}");
        let out = b.input_line("T2COK", t0);
        assert_eq!(state(&out), Some(profile.delivering_letter), "{name}");
        let out = b.ev_draw(false, t0);
        assert_eq!(state(&out), Some('C'), "{name}");
        b.ev_draw(true, t0);
        let out = b.tick(t0 + Duration::from_secs(2));
        parses(&out);
        assert!(out.iter().any(|l| l.starts_with("CCI:")), "{name}");
        let out = b.unplug(t0 + Duration::from_secs(3));
        parses(&out);
        assert!(
            matches!(Message::parse(&out[0]), Message::SessionReport(_)),
            "{name}: {out:?}"
        );
        assert_eq!(state(&out), Some('A'), "{name}");
        assert!(b.dangerous_commands().is_empty());
    }
}

#[test]
fn block_mid_charge_stops_and_resume_depends_on_the_profile() {
    for resumes in [true, false] {
        let profile = Profile {
            resumes_after_block: resumes,
            ..Profile::default()
        };
        let t0 = Instant::now();
        let mut b = Board::new(profile, t0);
        b.plug(t0);
        b.input_line("T2COK", t0);
        b.ev_draw(true, t0);
        let out = b.input_line("T2CNOK", t0);
        assert_eq!(state(&out), Some('W'));
        let out = b.tick(t0 + Duration::from_secs(2));
        assert_eq!(state(&out), Some('M'), "{out:?}");
        assert!(out.iter().any(|l| l.starts_with("WT:")));
        let out = b.input_line("T2COK", t0 + Duration::from_secs(3));
        if resumes {
            assert_eq!(state(&out), Some('D'));
        } else {
            assert!(out.is_empty(), "{out:?}");
            assert_eq!(b.state(), 'M');
        }
    }
}

#[test]
fn current_limit_respects_the_minimum() {
    let t0 = Instant::now();
    let mut b = Board::new(
        Profile {
            min_current: 10,
            ..Profile::default()
        },
        t0,
    );
    assert_eq!(b.input_line("CC:16", t0), ["CC:16"]);
    assert_eq!(b.input_line("CC:06", t0), ["CC:16"]);
    assert_eq!(b.input_line("CC:40", t0), ["CC:16"]);
    assert_eq!(b.input_line("CC:10", t0), ["CC:10"]);
}

#[test]
fn unanswered_pings_block_charging_on_a_strict_board() {
    let t0 = Instant::now();
    let mut b = Board::new(
        Profile {
            ping_timeout: Some(Duration::from_secs(30)),
            ping_interval: Duration::from_secs(10),
            ..Profile::default()
        },
        t0,
    );
    b.plug(t0);
    b.input_line("T2COK", t0);
    b.ev_draw(true, t0);
    let mut t = t0;
    let mut saw_ping = false;
    for _ in 0..5 {
        t += Duration::from_secs(10);
        let out = b.tick(t);
        saw_ping |= out.iter().any(|l| l == "RaspberryPi?");
    }
    assert!(saw_ping);
    assert!(!b.t2_allowed());
    assert_ne!(b.state(), 'D');
    // Answering keeps a strict board happy.
    let mut b = Board::new(b.profile().clone(), t0);
    b.plug(t0);
    b.input_line("T2COK", t0);
    b.ev_draw(true, t0);
    let mut t = t0;
    for _ in 0..10 {
        t += Duration::from_secs(10);
        for line in b.tick(t) {
            if line == "RaspberryPi?" {
                b.input_line("RaspberryPiModeOK", t);
            }
        }
    }
    assert!(b.t2_allowed());
}

#[test]
fn vehicle_full_reports_fcs_and_the_board_may_end_the_session() {
    let t0 = Instant::now();
    let mut b = Board::new(Profile::default(), t0);
    b.plug(t0);
    b.input_line("T2COK", t0);
    b.ev_draw(true, t0);
    let out = b.ev_full(t0);
    assert_eq!(out[0], "FCS:1");
    assert_eq!(state(&out), Some('W'));
    let out = b.tick(t0 + Duration::from_secs(2));
    assert_eq!(state(&out), Some('M'));
    let mut b = Board::new(
        Profile {
            ends_session_when_full: false,
            ..Profile::default()
        },
        t0,
    );
    b.plug(t0);
    b.input_line("T2COK", t0);
    b.ev_draw(true, t0);
    let out = b.ev_full(t0);
    assert_eq!(state(&out), Some('C'));
}

#[test]
fn identity_mode_and_bluetooth_answers() {
    let t0 = Instant::now();
    let mut b = Board::new(
        Profile {
            side: 2,
            native_fm: 4,
            ..Profile::default()
        },
        t0,
    );
    assert_eq!(b.input_line("Side?", t0), ["Side:2"]);
    assert_eq!(b.input_line("FM?", t0), ["FM:4"]);
    assert_eq!(b.input_line("FM:1", t0), ["FM:1"]);
    assert_eq!(b.input_line("FM:9", t0), ["FM:1"]);
    assert_eq!(b.input_line("BTNOK", t0), ["BT:0"]);
    assert!(!b.bluetooth());
    assert_eq!(b.input_line("E?", t0), ["E:0000"]);
    assert_eq!(b.input_line("Reference?", t0), ["Reference:058049"]);
    assert_eq!(b.input_line("Unlock", t0), Vec::<String>::new());
    assert_eq!(b.unlocks(), 1);
    assert_eq!(b.state(), 'A');
    assert_eq!(StateCode::parse("A"), Some(StateCode::A));
}

#[test]
fn dangerous_commands_are_recorded_not_executed() {
    let t0 = Instant::now();
    let mut b = Board::new(Profile::default(), t0);
    b.input_line("Z", t0);
    b.input_line("Reset", t0);
    assert_eq!(b.dangerous_commands(), ["Z", "Reset"]);
    assert_eq!(b.state(), 'A');
}

#[test]
fn sleep_and_wake() {
    let t0 = Instant::now();
    let mut b = Board::new(Profile::default(), t0);
    let out = b.input_line("SOK", t0);
    assert_eq!(out, ["Slp:1", "State:Y"]);
    let out = b.input_line("SNOK", t0);
    assert_eq!(out, ["Slp:0", "State:A"]);
}
