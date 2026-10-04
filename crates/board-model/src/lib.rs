//! A fake Legrand Green'Up power board.
//!
//! Speaks the board's line protocol (greenup-link's
//! `docs/PROTOCOLE_SERIE_LEGRAND.md`) well enough to drive the real driver
//! and session code in tests and in `legrand-sim`. Where the real board's
//! behaviour is still unknown (the gates in `docs/plan.md`), a [`Profile`]
//! field picks one answer, so every test runs against every answer.
//!
//! Pure: time comes in through `now`, lines go out as `Vec<String>` without
//! terminators. The vehicle is simulated through [`Board::plug`],
//! [`Board::unplug`], [`Board::ev_draw`] and [`Board::ev_full`].

use std::time::{Duration, Instant};

/// One answer to every open hardware question.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// Side the board reports (`Side:n`).
    pub side: u8,
    /// Gate G2: after `T2CNOK` mid-charge, does `T2COK` resume without a
    /// replug? When false the board ends the session (`W` then `M`).
    pub resumes_after_block: bool,
    /// Gate G4: lowest `CC` the board accepts; lower orders keep the old
    /// value.
    pub min_current: u8,
    /// Gate G5: how long the board tolerates missing `RaspberryPiModeOK`
    /// answers before it blocks charging on its own. `None` means never.
    pub ping_timeout: Option<Duration>,
    /// How often the board sends `RaspberryPi?`.
    pub ping_interval: Duration,
    /// Gate G8: the letter the board reports while delivering.
    pub delivering_letter: char,
    /// Gate G8: when the vehicle is full, does the board end the session
    /// (`W`, `M`) or just sit in `C`?
    pub ends_session_when_full: bool,
    /// How often measurements go out.
    pub telemetry_interval: Duration,
    /// The functioning mode the board boots with (`FM?`).
    pub native_fm: u8,
    /// Grid voltage reported.
    pub voltage: f32,
}

impl Default for Profile {
    /// The behaviour the plan hopes for.
    fn default() -> Self {
        Profile {
            side: 1,
            resumes_after_block: true,
            min_current: 6,
            ping_timeout: None,
            ping_interval: Duration::from_secs(10),
            delivering_letter: 'D',
            ends_session_when_full: true,
            telemetry_interval: Duration::from_secs(1),
            native_fm: 1,
            voltage: 230.0,
        }
    }
}

impl Profile {
    /// Every profile worth testing against, with a name.
    pub fn all() -> Vec<(&'static str, Profile)> {
        vec![
            ("default", Profile::default()),
            (
                "no-resume",
                Profile {
                    resumes_after_block: false,
                    ..Profile::default()
                },
            ),
            (
                "min-10a",
                Profile {
                    min_current: 10,
                    ..Profile::default()
                },
            ),
            (
                "strict-ping",
                Profile {
                    ping_timeout: Some(Duration::from_secs(30)),
                    ..Profile::default()
                },
            ),
            (
                "delivering-e",
                Profile {
                    delivering_letter: 'E',
                    ..Profile::default()
                },
            ),
            (
                "full-stays-c",
                Profile {
                    ends_session_when_full: false,
                    ..Profile::default()
                },
            ),
            (
                "native-fm-4",
                Profile {
                    native_fm: 4,
                    ..Profile::default()
                },
            ),
        ]
    }
}

/// Where the vehicle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ev {
    Absent,
    Idle,
    Drawing,
    Full,
}

/// The fake board.
#[derive(Debug, Clone)]
pub struct Board {
    profile: Profile,
    ev: Ev,
    t2_allowed: bool,
    domestic_allowed: bool,
    cc: u8,
    fm: u8,
    bt: bool,
    sleeping: bool,
    ended: bool,
    stopping_until: Option<Instant>,
    pi_mode: bool,
    last_ping: Instant,
    ping_outstanding: Option<Instant>,
    last_telemetry: Instant,
    last_tick: Instant,
    reported_state: Option<char>,
    session_wh: f64,
    charging_secs: f64,
    waiting_secs: f64,
    unlocks: u32,
    dangerous: Vec<String>,
    log: Vec<String>,
}

impl Board {
    /// A board that just powered up: unplugged, nothing allowed.
    pub fn new(profile: Profile, now: Instant) -> Self {
        Board {
            fm: profile.native_fm,
            profile,
            ev: Ev::Absent,
            t2_allowed: false,
            domestic_allowed: true,
            cc: 32,
            bt: true,
            sleeping: false,
            ended: false,
            stopping_until: None,
            pi_mode: false,
            last_ping: now,
            ping_outstanding: None,
            last_telemetry: now,
            last_tick: now,
            reported_state: None,
            session_wh: 0.0,
            charging_secs: 0.0,
            waiting_secs: 0.0,
            unlocks: 0,
            dangerous: Vec::new(),
            log: Vec::new(),
        }
    }

    /// The profile in use.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The state letter the board would answer to `State?`.
    pub fn state(&self) -> char {
        if self.sleeping {
            return 'Y';
        }
        if self.stopping_until.is_some() {
            return 'W';
        }
        match self.ev {
            Ev::Absent => 'A',
            _ if self.ended => 'M',
            _ if !self.t2_allowed => 'B',
            Ev::Drawing => self.profile.delivering_letter,
            Ev::Idle | Ev::Full => 'C',
        }
    }

    /// Configured current limit.
    pub fn cc(&self) -> u8 {
        self.cc
    }

    /// True while power flows.
    pub fn delivering(&self) -> bool {
        self.state() == self.profile.delivering_letter
    }

    /// `T2COK` is in force.
    pub fn t2_allowed(&self) -> bool {
        self.t2_allowed
    }

    /// Current functioning mode.
    pub fn fm(&self) -> u8 {
        self.fm
    }

    /// Bluetooth module on.
    pub fn bluetooth(&self) -> bool {
        self.bt
    }

    /// Times `Unlock` was received.
    pub fn unlocks(&self) -> u32 {
        self.unlocks
    }

    /// Commands that would have bricked or reset a real board.
    pub fn dangerous_commands(&self) -> &[String] {
        &self.dangerous
    }

    /// Every command line received, in order.
    pub fn received(&self) -> &[String] {
        &self.log
    }

    /// A vehicle plugs in.
    pub fn plug(&mut self, now: Instant) -> Vec<String> {
        if self.ev == Ev::Absent {
            self.ev = Ev::Idle;
            self.ended = false;
            self.session_wh = 0.0;
            self.charging_secs = 0.0;
            self.waiting_secs = 0.0;
        }
        self.state_change(now)
    }

    /// The vehicle leaves.
    pub fn unplug(&mut self, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        if self.ev != Ev::Absent {
            let was_in_session = self.t2_allowed || self.ended || self.charging_secs > 0.0;
            self.ev = Ev::Absent;
            self.ended = false;
            self.stopping_until = None;
            // Legrand's firmware withdraws its authorization on unplug.
            self.t2_allowed = false;
            if was_in_session {
                out.push(self.session_report());
            }
        }
        out.extend(self.state_change(now));
        out
    }

    /// The vehicle starts or stops drawing power (it only can while the
    /// board allows it).
    pub fn ev_draw(&mut self, draw: bool, now: Instant) -> Vec<String> {
        match (self.ev, draw) {
            (Ev::Absent, _) => {}
            (_, true) => self.ev = Ev::Drawing,
            (_, false) => self.ev = Ev::Idle,
        }
        self.state_change(now)
    }

    /// The vehicle's battery is full: it stops drawing and the board reports
    /// `FCS`. Depending on the profile the board then ends the session.
    pub fn ev_full(&mut self, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        if self.ev == Ev::Absent {
            return out;
        }
        self.ev = Ev::Full;
        out.push("FCS:1".to_owned());
        if self.profile.ends_session_when_full && self.t2_allowed {
            self.stopping_until = Some(now + Duration::from_secs(1));
        }
        out.extend(self.state_change(now));
        out
    }

    /// One command line from the Pi (terminator optional).
    pub fn input_line(&mut self, line: &str, now: Instant) -> Vec<String> {
        let line = line.trim_end_matches(['\r', '\n']).trim();
        self.log.push(line.to_owned());
        let mut out = Vec::new();
        match line {
            "RaspberryPiModeOK" => {
                self.pi_mode = true;
                self.ping_outstanding = None;
            }
            "Z" | "Reset" | "Test" => self.dangerous.push(line.to_owned()),
            "SoftwareVersion?" => out.push("SoftwareVersion:V01;18;04".to_owned()),
            "HardwareVersion?" => out.push("HardwareVersion:V02".to_owned()),
            "SerialNumber?" => out.push("SerialNumber:2026000042".to_owned()),
            "Reference?" => out.push("Reference:058049".to_owned()),
            "WeekYearProduction?" => out.push("WeekYearProduction:12/26".to_owned()),
            "Side?" => out.push(format!("Side:{}", self.profile.side)),
            "State?" => out.push(format!("State:{}", self.state())),
            "E?" => out.push("E:0000".to_owned()),
            "CC?" => out.push(format!("CC:{}", self.cc)),
            "CCTIC?" => out.push("CCTIC:32".to_owned()),
            "FM?" => out.push(format!("FM:{}", self.fm)),
            "BT?" => out.push(format!("BT:{}", u8::from(self.bt))),
            "BTOK" => {
                self.bt = true;
                out.push("BT:1".to_owned());
            }
            "BTNOK" => {
                self.bt = false;
                out.push("BT:0".to_owned());
            }
            "2PCOK" => self.domestic_allowed = true,
            "2PCNOK" => self.domestic_allowed = false,
            "T2COK" => {
                if !self.t2_allowed && (self.profile.resumes_after_block || !self.ended) {
                    self.t2_allowed = true;
                    self.ended = false;
                    self.stopping_until = None;
                }
                out.extend(self.state_change(now));
            }
            "T2CNOK" => {
                if self.t2_allowed {
                    self.t2_allowed = false;
                    if self.ev != Ev::Absent {
                        self.stopping_until = Some(now + Duration::from_secs(1));
                    }
                }
                out.extend(self.state_change(now));
            }
            "SNOK" => {
                self.sleeping = false;
                out.push("Slp:0".to_owned());
                out.extend(self.state_change(now));
            }
            "SOK" => {
                if self.ev == Ev::Absent {
                    self.sleeping = true;
                    out.push("Slp:1".to_owned());
                    out.extend(self.state_change(now));
                }
            }
            "Unlock" => self.unlocks += 1,
            "TICTM:1" => {
                out.push("TICTestC:Init".to_owned());
                out.push("TICTestB:0".to_owned());
            }
            "TICTM:0" => {}
            _ => {
                if let Some(value) = line.strip_prefix("CC:") {
                    if let Ok(amps) = value.parse::<u8>() {
                        if amps >= self.profile.min_current && amps <= 32 {
                            self.cc = amps;
                        }
                    }
                    out.push(format!("CC:{}", self.cc));
                } else if let Some(value) = line.strip_prefix("FM:") {
                    if let Ok(mode) = value.parse::<u8>() {
                        if (1..=6).contains(&mode) {
                            self.fm = mode;
                        }
                    }
                    out.push(format!("FM:{}", self.fm));
                }
            }
        }
        out
    }

    /// Time passes: pings, telemetry, delayed state changes.
    pub fn tick(&mut self, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        let dt = now.saturating_duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        if self.delivering() {
            let power_w = f64::from(self.profile.voltage) * f64::from(self.cc) * 3.0;
            self.session_wh += power_w * dt / 3600.0;
            self.charging_secs += dt;
        } else if self.ev != Ev::Absent {
            self.waiting_secs += dt;
        }
        if let Some(until) = self.stopping_until {
            if now >= until {
                self.stopping_until = None;
                self.ended = true;
                out.push(self.session_report());
            }
        }
        if now.duration_since(self.last_ping) >= self.profile.ping_interval {
            self.last_ping = now;
            if self.ping_outstanding.is_none() {
                self.ping_outstanding = Some(now);
            }
            out.push("RaspberryPi?".to_owned());
        }
        if let (Some(timeout), Some(since)) = (self.profile.ping_timeout, self.ping_outstanding) {
            if now.duration_since(since) >= timeout && self.t2_allowed {
                self.t2_allowed = false;
                self.pi_mode = false;
            }
        }
        if now.duration_since(self.last_telemetry) >= self.profile.telemetry_interval {
            self.last_telemetry = now;
            let current = if self.delivering() {
                f32::from(self.cc)
            } else {
                0.0
            };
            out.push(format!("Volt:{:.1}", self.profile.voltage));
            out.push(format!("CCI:{current:.1}"));
            out.push("Freq:50.0".to_owned());
            out.push(format!("Ener:{:.1}", self.session_wh));
        }
        out.extend(self.state_change(now));
        out
    }

    fn state_change(&mut self, _now: Instant) -> Vec<String> {
        let state = self.state();
        if self.reported_state == Some(state) {
            return Vec::new();
        }
        self.reported_state = Some(state);
        vec![format!("State:{state}")]
    }

    fn session_report(&self) -> String {
        let hms = |secs: f64| {
            // Durations here are seconds of simulated time; truncation is intended.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let s = secs.max(0.0) as u64;
            format!("{}:{}:{}", s / 3600, (s % 3600) / 60, s % 60)
        };
        let mean_w = if self.charging_secs > 0.0 {
            self.session_wh * 3600.0 / self.charging_secs
        } else {
            0.0
        };
        format!(
            "WT:{}:CT:{}:EVplug:{mean_w:.2}:0.00:",
            hms(self.waiting_secs),
            hms(self.charging_secs)
        )
    }
}
