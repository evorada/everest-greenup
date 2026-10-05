//! The driver against the fake board, on every profile.

use std::time::{Duration, Instant};

use board_model::{Board, Profile};
use evse_session::{ChargerDriver, DriverAction, DriverEvent, Fact, Hint};
use legrand_driver::{DriverConfig, LegrandDriver};

/// Board and driver wired together with a simulated clock.
struct Rig {
    board: Board,
    driver: LegrandDriver,
    now: Instant,
    sent: Vec<String>,
    events: Vec<DriverEvent>,
}

impl Rig {
    fn new(profile: Profile, cfg: DriverConfig) -> Self {
        let now = Instant::now();
        let mut driver = LegrandDriver::new(cfg);
        driver.connected(now);
        Rig {
            board: Board::new(profile, now),
            driver,
            now,
            sent: Vec::new(),
            events: Vec::new(),
        }
    }

    fn feed(&mut self, lines: Vec<String>) {
        for line in lines {
            let events = self.driver.on_line(&line, self.now);
            self.events.extend(events);
        }
    }

    /// Run the loop for `d`, in 50 ms steps.
    fn run(&mut self, d: Duration) {
        let end = self.now + d;
        while self.now < end {
            let poll = self.driver.poll(self.now);
            self.events.extend(poll.events);
            for bytes in poll.commands {
                let text = String::from_utf8_lossy(&bytes).trim_end().to_owned();
                self.sent.push(text.clone());
                let replies = self.board.input_line(&text, self.now);
                self.feed(replies);
            }
            let lines = self.board.tick(self.now);
            self.feed(lines);
            self.now += Duration::from_millis(50);
        }
    }

    fn facts(&self) -> Vec<Fact> {
        self.events
            .iter()
            .filter_map(|e| match e {
                DriverEvent::Fact(f) => Some(*f),
                _ => None,
            })
            .collect()
    }

    fn faults(&self) -> Vec<Option<String>> {
        self.events
            .iter()
            .filter_map(|e| match e {
                DriverEvent::Fault(f) => Some(f.clone()),
                _ => None,
            })
            .collect()
    }
}

const SECOND: Duration = Duration::from_secs(1);

#[test]
fn handshake_order_and_pacing() {
    let mut rig = Rig::new(Profile::default(), DriverConfig::default());
    rig.run(Duration::from_secs(2));
    assert_eq!(
        rig.sent[..10],
        [
            "RaspberryPiModeOK",
            "BTNOK",
            "2PCNOK",
            "Side?",
            "FM?",
            "FM2?",
            "State?",
            "CC?",
            "E?",
            "T2CNOK"
        ]
    );
    assert!(!rig.board.bluetooth());
    assert_eq!(rig.facts(), [Fact::Unplugged]);
    assert!(rig.faults().is_empty(), "{:?}", rig.faults());
    // At 150 ms spacing no more than seven commands fit in the first second.
    let mut rig = Rig::new(Profile::default(), DriverConfig::default());
    rig.run(Duration::from_millis(1000));
    assert!(rig.sent.len() <= 7, "{:?}", rig.sent);
}

#[test]
fn plug_authorize_draw_unplug_facts_on_every_profile() {
    for (name, profile) in Profile::all() {
        let delivering = profile.delivering_letter;
        let mut rig = Rig::new(profile, DriverConfig::default());
        rig.run(Duration::from_secs(2));
        let lines = rig.board.plug(rig.now);
        rig.feed(lines);
        rig.driver.apply(DriverAction::SetCurrent(16), rig.now);
        rig.driver.apply(DriverAction::Authorize(true), rig.now);
        rig.run(SECOND);
        assert!(rig.board.t2_allowed(), "{name}");
        assert_eq!(rig.board.cc(), 16, "{name}");
        // CC goes out before T2COK.
        let cc = rig.sent.iter().position(|c| c == "CC:16");
        let ok = rig.sent.iter().position(|c| c == "T2COK");
        assert!(cc < ok, "{name}: {:?}", rig.sent);
        let lines = rig.board.ev_draw(true, rig.now);
        rig.feed(lines);
        rig.run(SECOND);
        assert_eq!(
            rig.facts(),
            [
                Fact::Unplugged,
                Fact::Plugged,
                Fact::BoardAuthorized,
                Fact::Delivering
            ],
            "{name} (delivering letter {delivering})"
        );
        assert!(rig.driver.meter().power_w > 0.0, "{name}");
        let before = rig.driver.meter().energy_wh_import;
        rig.run(Duration::from_secs(10));
        assert!(rig.driver.meter().energy_wh_import > before, "{name}");
        rig.driver.apply(DriverAction::Authorize(false), rig.now);
        rig.run(Duration::from_secs(3));
        assert!(!rig.board.t2_allowed(), "{name}");
        let lines = rig.board.unplug(rig.now);
        rig.feed(lines);
        rig.run(SECOND);
        assert_eq!(rig.facts().last(), Some(&Fact::Unplugged), "{name}");
        assert!(
            rig.events
                .iter()
                .any(|e| matches!(e, DriverEvent::Hint(Hint::SessionReport { .. }))),
            "{name}"
        );
        assert!(rig.board.dangerous_commands().is_empty(), "{name}");
        assert!(rig.faults().is_empty(), "{name}: {:?}", rig.faults());
    }
}

#[test]
fn native_mode_is_forced_to_direct_charge() {
    let mut rig = Rig::new(
        Profile {
            native_fm: 4,
            ..Profile::default()
        },
        DriverConfig::default(),
    );
    rig.run(Duration::from_secs(3));
    assert_eq!(rig.board.fm(), 1);
    assert!(rig.sent.iter().any(|c| c == "FM:1"));
    assert!(rig.faults().is_empty());
}

#[test]
fn eco_start_is_switched_off_and_a_stuck_one_is_a_fault() {
    let mut rig = Rig::new(
        Profile {
            native_eco_start: true,
            ..Profile::default()
        },
        DriverConfig::default(),
    );
    rig.run(Duration::from_secs(3));
    assert!(!rig.board.eco_start());
    assert!(rig.sent.iter().any(|c| c == "FM2:0"));
    assert!(rig.faults().is_empty());
    let mut driver = LegrandDriver::new(DriverConfig::default());
    let t0 = Instant::now();
    driver.connected(t0);
    let mut faults = Vec::new();
    for i in 0..6 {
        let events = driver.on_line("FM2:1", t0 + Duration::from_secs(i));
        faults.extend(
            events
                .into_iter()
                .filter(|e| matches!(e, DriverEvent::Fault(_))),
        );
    }
    assert!(matches!(faults.as_slice(), [DriverEvent::Fault(Some(m))] if m.contains("FM2:0")));
}

#[test]
fn a_mode_that_will_not_stick_is_a_fault() {
    let mut driver = LegrandDriver::new(DriverConfig::default());
    let t0 = Instant::now();
    driver.connected(t0);
    let mut faults = Vec::new();
    for i in 0..6 {
        let events = driver.on_line("FM:4", t0 + Duration::from_secs(i));
        faults.extend(
            events
                .into_iter()
                .filter(|e| matches!(e, DriverEvent::Fault(_))),
        );
    }
    assert_eq!(faults.len(), 1);
    assert!(matches!(&faults[0], DriverEvent::Fault(Some(m)) if m.contains("FM:1")));
    let events = driver.on_line("FM:1", t0 + Duration::from_secs(7));
    assert!(events.contains(&DriverEvent::Fault(None)));
}

#[test]
fn wrong_side_is_a_fault() {
    let mut rig = Rig::new(
        Profile {
            side: 2,
            ..Profile::default()
        },
        DriverConfig {
            expected_side: Some(1),
            ..DriverConfig::default()
        },
    );
    rig.run(Duration::from_secs(2));
    assert!(matches!(rig.faults().as_slice(), [Some(m)] if m.contains("Side:2")));
    let mut rig = Rig::new(
        Profile {
            side: 2,
            ..Profile::default()
        },
        DriverConfig {
            expected_side: Some(2),
            ..DriverConfig::default()
        },
    );
    rig.run(Duration::from_secs(2));
    assert!(rig.faults().is_empty());
}

#[test]
fn silence_is_a_link_down_fault_that_clears_on_the_next_line() {
    let mut driver = LegrandDriver::new(DriverConfig::default());
    let t0 = Instant::now();
    driver.connected(t0);
    let poll = driver.poll(t0 + Duration::from_secs(2));
    assert!(poll.events.is_empty());
    let poll = driver.poll(t0 + Duration::from_secs(4));
    assert!(matches!(poll.events.as_slice(), [DriverEvent::Fault(Some(m))] if m.contains("link")));
    let events = driver.on_line("State:A", t0 + Duration::from_secs(5));
    assert!(events.contains(&DriverEvent::Fault(None)));
    driver.disconnected(t0 + Duration::from_secs(6));
    let poll = driver.poll(t0 + Duration::from_secs(6));
    assert!(matches!(
        poll.events.as_slice(),
        [DriverEvent::Fault(Some(_))]
    ));
    assert!(poll.commands.is_empty());
}

#[test]
fn unauthorized_delivery_is_blocked_every_two_seconds_and_reported() {
    let mut rig = Rig::new(Profile::default(), DriverConfig::default());
    rig.run(Duration::from_secs(2));
    let lines = rig.board.plug(rig.now);
    rig.feed(lines);
    // Someone else (the Legrand app over Bluetooth, say) lets the board go.
    let lines = rig.board.input_line("T2COK", rig.now);
    rig.feed(lines);
    let lines = rig.board.ev_draw(true, rig.now);
    rig.feed(lines);
    rig.run(Duration::from_secs(1));
    assert!(!rig.board.t2_allowed(), "driver blocked it: {:?}", rig.sent);
    assert!(
        rig.faults().iter().any(|f| f
            .as_deref()
            .is_some_and(|m| m.contains("without authorization"))),
        "{:?}",
        rig.faults()
    );
    rig.run(Duration::from_secs(3));
    assert_eq!(rig.faults().last(), Some(&None), "{:?}", rig.faults());
}

#[test]
fn ping_replies_jump_the_queue() {
    let mut driver = LegrandDriver::new(DriverConfig::default());
    let t0 = Instant::now();
    driver.connected(t0);
    driver.on_line("RaspberryPi?", t0);
    let poll = driver.poll(t0);
    assert_eq!(poll.commands, [b"RaspberryPiModeOK\r".to_vec()]);
    // Already queued from the handshake: not sent twice in a row.
    let poll = driver.poll(t0 + Duration::from_millis(200));
    assert_eq!(poll.commands, [b"BTNOK\r".to_vec()]);
}

#[test]
fn state_is_polled_every_five_seconds_and_the_rest_every_minute() {
    let mut rig = Rig::new(Profile::default(), DriverConfig::default());
    rig.run(Duration::from_secs(61));
    let count = |cmd: &str| rig.sent.iter().filter(|c| c.as_str() == cmd).count();
    assert!((11..=14).contains(&count("State?")), "{}", count("State?"));
    assert_eq!(count("E?"), 2);
    assert_eq!(count("FM2?"), 2);
    assert_eq!(count("BT?"), 1);
}

#[test]
fn unlock_waits_until_the_board_stops_delivering() {
    let mut rig = Rig::new(Profile::default(), DriverConfig::default());
    rig.run(Duration::from_secs(2));
    let lines = rig.board.plug(rig.now);
    rig.feed(lines);
    rig.driver.apply(DriverAction::Authorize(true), rig.now);
    rig.run(SECOND);
    let lines = rig.board.ev_draw(true, rig.now);
    rig.feed(lines);
    rig.run(SECOND);
    rig.driver.apply(DriverAction::Authorize(false), rig.now);
    rig.driver.apply(DriverAction::Unlock, rig.now);
    rig.run(Duration::from_secs(3));
    let block = rig.sent.iter().rposition(|c| c == "T2CNOK");
    let unlock = rig.sent.iter().position(|c| c == "Unlock");
    assert!(unlock.is_some() && block < unlock, "{:?}", rig.sent);
    assert_eq!(rig.board.unlocks(), 1);
}

#[test]
fn board_fault_letters_and_error_codes_raise_and_clear() {
    let mut driver = LegrandDriver::new(DriverConfig::default());
    let t0 = Instant::now();
    driver.connected(t0);
    let events = driver.on_line("State:F", t0);
    assert!(matches!(events.as_slice(), [DriverEvent::Fault(Some(m))] if m.contains('F')));
    let events = driver.on_line("State:B", t0);
    assert_eq!(
        events,
        [DriverEvent::Fact(Fact::Plugged), DriverEvent::Fault(None)]
    );
    let events = driver.on_line("E:0040", t0);
    assert!(matches!(events.as_slice(), [DriverEvent::Fault(Some(m))] if m.contains("0040")));
    let events = driver.on_line("E:0000", t0);
    assert_eq!(events, [DriverEvent::Fault(None)]);
}

#[test]
fn persisted_energy_is_restored_and_never_lowered() {
    let mut driver = LegrandDriver::new(DriverConfig::default());
    driver.set_energy_wh(1234.5);
    driver.set_energy_wh(1000.0);
    assert_eq!(driver.meter().energy_wh_import, 1234.5);
}
