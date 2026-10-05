//! [`ChargerDriver`] for the Legrand Green'Up power board.
//!
//! Sits between the serial line and the session core: lines in, commands
//! out, facts and faults up. Pure, so the whole thing runs against the
//! fake board in tests.
//!
//! What it does, per `docs/plan.md` ("Board side"):
//!
//! - on (re)connect: `RaspberryPiModeOK`, `BTNOK`, `2PCNOK` (or `2PCOK`),
//!   `Side?`, `FM?` (forcing `FM:1`, direct charge), `FM2?` (forcing
//!   `FM2:0`, no eco-start), `State?`, `CC?`, `E?`, then `T2CNOK` so a
//!   board found mid-charge starts a fresh session;
//! - polling: `State?` every 5 s, `E?`/`CC?`/`FM?`/`FM2?`/`BT?` every 60 s;
//! - scheduling: one command per 150 ms, ping replies first, `CC` before
//!   `T2COK`, no duplicates in the queue;
//! - facts: the board's state letters mapped to [`Fact`]s;
//! - faults: link down for 3 s, side mismatch, board fault letters,
//!   `E:xxxx`, `FM:1` or `FM2:0` that will not stick, `T2CNOK` ignored for 10 s,
//!   delivery without authorization (which also gets `T2CNOK` every 2 s);
//! - metering: `Volt`/`CCI`/`Freq` into a power figure and a monotonic
//!   energy register integrated while delivering (provisional until gate
//!   G9 says what `Ener` and `CCI` mean).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use evse_session::{ChargerDriver, DriverAction, DriverEvent, Fact, Hint, MeterSnapshot, Poll};
use legrand_proto::{Command, Current, FunctioningMode, Message, Query, StateCode};

/// Tunables; the defaults are the plan's numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct DriverConfig {
    /// The side the board must report in `Side:n`; `None` skips the check.
    pub expected_side: Option<u8>,
    /// Allow the domestic socket (`2PCOK`) instead of blocking it.
    pub domestic_socket: bool,
    /// Phases wired to the socket, for the power figure.
    pub phases: u8,
    /// Interval of `State?` polls.
    pub state_poll: Duration,
    /// Interval of `E?`, `CC?`, `FM?`, `BT?` polls.
    pub slow_poll: Duration,
    /// Silence on the line that counts as link down.
    pub link_timeout: Duration,
    /// How long the board may keep delivering after `T2CNOK` before that
    /// is a fault.
    pub block_ignored_timeout: Duration,
    /// Minimum spacing between commands.
    pub command_spacing: Duration,
    /// How often `T2CNOK` is repeated while the board delivers without
    /// our authorization.
    pub unauthorized_block_interval: Duration,
}

impl Default for DriverConfig {
    fn default() -> Self {
        DriverConfig {
            expected_side: None,
            domestic_socket: false,
            phases: 3,
            state_poll: Duration::from_secs(5),
            slow_poll: Duration::from_secs(60),
            link_timeout: Duration::from_secs(3),
            block_ignored_timeout: Duration::from_secs(10),
            command_spacing: Duration::from_millis(150),
            unauthorized_block_interval: Duration::from_secs(2),
        }
    }
}

/// How many `FM:1` (or `FM2:0`) orders go unheeded before it is a fault.
const FM_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, Default)]
struct Faults {
    link_down: bool,
    side_mismatch: Option<u8>,
    board_fault: Option<char>,
    error_code: Option<String>,
    fm_stuck: bool,
    eco_start_stuck: bool,
    block_ignored: bool,
    unauthorized: bool,
}

impl Faults {
    fn message(&self) -> Option<String> {
        if self.link_down {
            return Some("link to the power board is down".to_owned());
        }
        if let Some(side) = self.side_mismatch {
            return Some(format!(
                "board reports Side:{side}, another side was expected"
            ));
        }
        if let Some(letter) = self.board_fault {
            return Some(format!("board fault state {letter}"));
        }
        if let Some(code) = &self.error_code {
            return Some(format!("board error E:{code}"));
        }
        if self.fm_stuck {
            return Some("board refuses FM:1 (direct charge mode)".to_owned());
        }
        if self.eco_start_stuck {
            return Some("board refuses FM2:0 (eco-start stays on)".to_owned());
        }
        if self.block_ignored {
            return Some("board keeps delivering after T2CNOK".to_owned());
        }
        if self.unauthorized {
            return Some("board delivers power without authorization".to_owned());
        }
        None
    }
}

#[derive(Debug, Clone, Default)]
struct Meter {
    voltage: Option<f64>,
    current: Option<f64>,
    frequency: Option<f64>,
    energy_wh: f64,
    last_integration: Option<Instant>,
}

/// The driver. See the crate docs.
#[derive(Debug, Clone)]
pub struct LegrandDriver {
    cfg: DriverConfig,
    connected: bool,
    queue: VecDeque<Command>,
    last_tx: Option<Instant>,
    last_rx: Option<Instant>,
    last_state_poll: Option<Instant>,
    last_slow_poll: Option<Instant>,
    wanted_authorized: bool,
    sent_authorized: Option<bool>,
    wanted_current: Option<Current>,
    unlock_wanted: bool,
    state: Option<StateCode>,
    reported_cc: Option<u8>,
    fm_attempts: u8,
    eco_start_attempts: u8,
    last_fact: Option<Fact>,
    faults: Faults,
    last_fault: Option<String>,
    delivering_unauthorized_since: Option<Instant>,
    last_unauthorized_block: Option<Instant>,
    meter: Meter,
}

impl LegrandDriver {
    /// A driver for a port that is not open yet.
    pub fn new(cfg: DriverConfig) -> Self {
        LegrandDriver {
            cfg,
            connected: false,
            queue: VecDeque::new(),
            last_tx: None,
            last_rx: None,
            last_state_poll: None,
            last_slow_poll: None,
            wanted_authorized: false,
            sent_authorized: None,
            wanted_current: None,
            unlock_wanted: false,
            state: None,
            reported_cc: None,
            fm_attempts: 0,
            eco_start_attempts: 0,
            last_fact: None,
            faults: Faults::default(),
            last_fault: None,
            delivering_unauthorized_since: None,
            last_unauthorized_block: None,
            meter: Meter::default(),
        }
    }

    /// Restore the energy register from persisted state.
    pub fn set_energy_wh(&mut self, wh: f64) {
        self.meter.energy_wh = self.meter.energy_wh.max(wh);
    }

    /// Last state letter the board reported.
    pub fn board_state(&self) -> Option<StateCode> {
        self.state
    }

    /// Last `CC` the board reported.
    pub fn reported_current(&self) -> Option<u8> {
        self.reported_cc
    }

    /// The current fault message, if any.
    pub fn fault(&self) -> Option<String> {
        self.faults.message()
    }

    fn delivering(&self) -> bool {
        matches!(self.state, Some(StateCode::D | StateCode::E))
    }

    fn enqueue(&mut self, cmd: Command) {
        if !self.queue.contains(&cmd) {
            self.queue.push_back(cmd);
        }
    }

    fn enqueue_front(&mut self, cmd: Command) {
        self.queue.retain(|c| *c != cmd);
        self.queue.push_front(cmd);
    }

    fn fact_for(state: StateCode) -> Option<Fact> {
        Some(match state {
            StateCode::A | StateCode::L | StateCode::Y => Fact::Unplugged,
            StateCode::B => Fact::Plugged,
            StateCode::C => Fact::BoardAuthorized,
            StateCode::D | StateCode::E => Fact::Delivering,
            StateCode::W => Fact::Stopping,
            StateCode::M => Fact::EndedByBoard,
            StateCode::Fault(_) | StateCode::Other(_) => return None,
        })
    }

    fn integrate(&mut self, now: Instant) {
        let power = self.power_w();
        if let Some(last) = self.meter.last_integration {
            let hours = now.saturating_duration_since(last).as_secs_f64() / 3600.0;
            self.meter.energy_wh += power * hours;
        }
        self.meter.last_integration = Some(now);
    }

    fn power_w(&self) -> f64 {
        if !self.delivering() {
            return 0.0;
        }
        match (self.meter.voltage, self.meter.current) {
            (Some(v), Some(i)) => v * i * f64::from(self.cfg.phases),
            _ => 0.0,
        }
    }

    fn on_state(&mut self, state: StateCode, now: Instant, events: &mut Vec<DriverEvent>) {
        self.integrate(now);
        self.state = Some(state);
        self.faults.board_fault = match state {
            StateCode::Fault(c) => Some(c),
            _ => None,
        };
        if state == StateCode::Y {
            self.enqueue(Command::Wake);
        }
        if let Some(fact) = Self::fact_for(state) {
            if self.last_fact != Some(fact) {
                self.last_fact = Some(fact);
                events.push(DriverEvent::Fact(fact));
            }
        }
        // The board reports what it is doing; our last order is only a
        // guess until it does. After T2CNOK the board reaches B or M; after
        // T2COK it reaches C, D or E.
        let blocked = matches!(
            state,
            StateCode::B | StateCode::M | StateCode::A | StateCode::L | StateCode::Y
        );
        if blocked && self.sent_authorized == Some(true) && !self.wanted_authorized {
            self.sent_authorized = Some(false);
        }
        if self.delivering() && !self.wanted_authorized {
            if self.delivering_unauthorized_since.is_none() {
                self.delivering_unauthorized_since = Some(now);
            }
        } else {
            self.delivering_unauthorized_since = None;
            self.faults.block_ignored = false;
            self.faults.unauthorized = false;
        }
    }

    fn fault_changes(&mut self, events: &mut Vec<DriverEvent>) {
        let fault = self.faults.message();
        if fault != self.last_fault {
            self.last_fault = fault.clone();
            events.push(DriverEvent::Fault(fault));
        }
    }
}

impl ChargerDriver for LegrandDriver {
    fn connected(&mut self, now: Instant) {
        self.connected = true;
        self.last_rx = Some(now);
        self.last_tx = None;
        self.sent_authorized = None;
        self.reported_cc = None;
        self.fm_attempts = 0;
        self.eco_start_attempts = 0;
        self.faults.link_down = false;
        self.queue.clear();
        for cmd in [
            Command::PiModeOk,
            Command::Bluetooth(false),
            Command::DomesticCharging(self.cfg.domestic_socket),
            Command::Query(Query::Side),
            Command::Query(Query::FunctioningMode),
            Command::Query(Query::EcoStart),
            Command::Query(Query::State),
            Command::Query(Query::CurrentLimit),
            Command::Query(Query::Error),
            Command::Type2Charging(false),
        ] {
            self.enqueue(cmd);
        }
        self.sent_authorized = Some(false);
        self.last_state_poll = Some(now);
        self.last_slow_poll = Some(now);
    }

    fn disconnected(&mut self, now: Instant) {
        self.integrate(now);
        self.connected = false;
        self.queue.clear();
        self.faults.link_down = true;
        self.state = None;
    }

    fn on_line(&mut self, line: &str, now: Instant) -> Vec<DriverEvent> {
        let mut events = Vec::new();
        self.last_rx = Some(now);
        if self.faults.link_down && self.connected {
            self.faults.link_down = false;
        }
        match Message::parse(line) {
            Message::PingRequest => self.enqueue_front(Command::PiModeOk),
            Message::State(state) => self.on_state(state, now, &mut events),
            Message::Error(code) => {
                self.faults.error_code = if code.is_clear() {
                    None
                } else {
                    Some(code.as_str().to_owned())
                };
            }
            Message::CurrentLimit(amps) => self.reported_cc = Some(amps),
            Message::FunctioningMode(mode) => {
                if mode == FunctioningMode::DirectCharge {
                    self.fm_attempts = 0;
                    self.faults.fm_stuck = false;
                } else if self.fm_attempts >= FM_ATTEMPTS {
                    self.faults.fm_stuck = true;
                } else {
                    self.fm_attempts += 1;
                    self.enqueue(Command::SetFunctioningMode(FunctioningMode::DirectCharge));
                    self.enqueue(Command::Query(Query::FunctioningMode));
                }
            }
            Message::EcoStart(on) => {
                if !on {
                    self.eco_start_attempts = 0;
                    self.faults.eco_start_stuck = false;
                } else if self.eco_start_attempts >= FM_ATTEMPTS {
                    self.faults.eco_start_stuck = true;
                } else {
                    self.eco_start_attempts += 1;
                    self.enqueue(Command::EcoStart(false));
                    self.enqueue(Command::Query(Query::EcoStart));
                }
            }
            Message::Side(side) => {
                self.faults.side_mismatch = match self.cfg.expected_side {
                    Some(expected) if expected != side => Some(side),
                    _ => None,
                };
            }
            Message::Bluetooth(true) => self.enqueue(Command::Bluetooth(false)),
            Message::Sleep(true) => self.enqueue(Command::Wake),
            Message::Voltage(v) => {
                self.integrate(now);
                self.meter.voltage = Some(f64::from(v));
                events.push(DriverEvent::MeterUpdated);
            }
            Message::Current(a) => {
                self.integrate(now);
                self.meter.current = Some(f64::from(a));
                events.push(DriverEvent::MeterUpdated);
            }
            Message::Frequency(hz) => {
                self.meter.frequency = Some(f64::from(hz));
                events.push(DriverEvent::MeterUpdated);
            }
            Message::ChargeComplete(_) => events.push(DriverEvent::Hint(Hint::ChargeComplete)),
            Message::SessionReport(report) => events.push(DriverEvent::Hint(Hint::SessionReport {
                charging_secs: report.charging.as_secs(),
                mean_power_w: report.mean_power_peak_w.max(report.mean_power_offpeak_w),
            })),
            _ => {}
        }
        self.fault_changes(&mut events);
        events
    }

    fn apply(&mut self, action: DriverAction, _now: Instant) {
        match action {
            DriverAction::Authorize(allow) => self.wanted_authorized = allow,
            DriverAction::SetCurrent(amps) => {
                if let Ok(current) = Current::new(amps) {
                    self.wanted_current = Some(current);
                }
            }
            DriverAction::Unlock => self.unlock_wanted = true,
        }
    }

    fn poll(&mut self, now: Instant) -> Poll {
        let mut poll = Poll::default();
        if !self.connected {
            self.fault_changes(&mut poll.events);
            return poll;
        }
        if let Some(last_rx) = self.last_rx {
            if now.duration_since(last_rx) > self.cfg.link_timeout {
                self.faults.link_down = true;
            }
        }
        if let Some(since) = self.delivering_unauthorized_since {
            // Someone else let the board deliver, or it ignores our block:
            // keep blocking, and say so.
            if self.sent_authorized != Some(true) {
                self.faults.unauthorized = true;
            }
            if now.duration_since(since) > self.cfg.block_ignored_timeout {
                self.faults.block_ignored = true;
            }
            let due = self
                .last_unauthorized_block
                .is_none_or(|t| now.duration_since(t) >= self.cfg.unauthorized_block_interval);
            if due {
                self.last_unauthorized_block = Some(now);
                self.enqueue(Command::Type2Charging(false));
            }
        }
        if self
            .last_state_poll
            .is_none_or(|t| now.duration_since(t) >= self.cfg.state_poll)
        {
            self.last_state_poll = Some(now);
            self.enqueue(Command::Query(Query::State));
        }
        if self
            .last_slow_poll
            .is_none_or(|t| now.duration_since(t) >= self.cfg.slow_poll)
        {
            self.last_slow_poll = Some(now);
            for q in [
                Query::Error,
                Query::CurrentLimit,
                Query::FunctioningMode,
                Query::EcoStart,
                Query::Bluetooth,
            ] {
                self.enqueue(Command::Query(q));
            }
        }
        // Reconcile what the session wants with what we last sent: the
        // current limit goes first, so a T2COK never starts at an old CC.
        if let Some(current) = self.wanted_current {
            if self.reported_cc != Some(current.amps()) {
                self.enqueue(Command::SetCurrent(current));
            }
        }
        if self.sent_authorized != Some(self.wanted_authorized) {
            self.enqueue(Command::Type2Charging(self.wanted_authorized));
        }
        if self.unlock_wanted && !self.delivering() {
            self.unlock_wanted = false;
            self.enqueue(Command::Unlock);
        }
        let spacing_ok = self
            .last_tx
            .is_none_or(|t| now.duration_since(t) >= self.cfg.command_spacing);
        if spacing_ok {
            if let Some(cmd) = self.queue.pop_front() {
                self.last_tx = Some(now);
                match cmd {
                    Command::Type2Charging(allow) => self.sent_authorized = Some(allow),
                    Command::SetCurrent(current) => {
                        // Assume it took; the next CC? or echo corrects us.
                        self.reported_cc = Some(current.amps());
                    }
                    _ => {}
                }
                poll.commands.push(cmd.encode());
            }
        }
        self.fault_changes(&mut poll.events);
        poll
    }

    fn meter(&self) -> MeterSnapshot {
        MeterSnapshot {
            energy_wh_import: self.meter.energy_wh,
            power_w: self.power_w(),
            voltage_v: self.meter.voltage,
            frequency_hz: self.meter.frequency,
            current_a: self.meter.current,
        }
    }
}
