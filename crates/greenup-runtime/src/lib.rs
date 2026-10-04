//! Threads around the pure crates.
//!
//! One [`Runtime`] per side. It owns the port, a reader thread, and an
//! actor thread that runs the [`LegrandDriver`], the [`Session`] and the
//! [`EnergyNode`]. The EVerest module talks to it through a cloneable
//! [`Handle`] (commands, answered synchronously) and a channel of
//! [`Notification`]s (events, measurements, faults, energy requests).
//!
//! The actor also persists the energy counter and the open transaction
//! (see [`PersistedState`]), reconnects a port that went away, and writes
//! an RX/TX trace when asked to.

mod persist;
mod port;

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use evse_session::{
    ChargerDriver, DriverEvent, EnableSource, EnergyConfig, EnergyNode, Event, EvseState, LimitRes,
    MeterSnapshot, Output, Session, SessionConfig, SessionEvent, StopReason,
};
use legrand_driver::{DriverConfig, LegrandDriver};
use legrand_proto::LineFramer;

pub use persist::PersistedState;

/// Everything a side needs.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Serial device path, or `tcp:host:port` for `legrand-sim`.
    pub port: String,
    /// Driver tunables.
    pub driver: DriverConfig,
    /// Static limits for the energy tree.
    pub energy: EnergyConfig,
    /// Session tunables (gate G2 answer).
    pub session: SessionConfig,
    /// Where to persist state; `None` keeps nothing across restarts.
    pub state_file: Option<PathBuf>,
    /// Append every line sent and received here; `None` disables.
    pub trace_file: Option<PathBuf>,
    /// Wait between attempts to open a port that failed or went away.
    pub reconnect_interval: Duration,
    /// Measurement publish interval while delivering ...
    pub meter_interval_charging: Duration,
    /// ... and otherwise.
    pub meter_interval_idle: Duration,
    /// How often the energy request goes out.
    pub energy_request_interval: Duration,
    /// How often persisted state is written when it changed.
    pub persist_interval: Duration,
}

impl RuntimeConfig {
    /// Defaults from the plan for everything but the port and the limits.
    pub fn new(port: impl Into<String>, driver: DriverConfig, energy: EnergyConfig) -> Self {
        RuntimeConfig {
            port: port.into(),
            driver,
            energy,
            session: SessionConfig::default(),
            state_file: None,
            trace_file: None,
            reconnect_interval: Duration::from_secs(2),
            meter_interval_charging: Duration::from_secs(1),
            meter_interval_idle: Duration::from_secs(10),
            energy_request_interval: Duration::from_secs(1),
            persist_interval: Duration::from_secs(5),
        }
    }
}

/// A command from the EVerest side.
#[derive(Debug, Clone)]
pub enum Request<T> {
    /// The module is ready: publish the initial events and recover a
    /// transaction left open by a crash.
    Startup,
    /// An accepted authorization.
    Authorize {
        /// The token.
        token: T,
        /// Reservation it belongs to, if any.
        reservation_id: Option<i64>,
    },
    /// Auth withdrew the authorization.
    WithdrawAuthorization,
    /// Stop the transaction.
    StopTransaction {
        /// Why.
        reason: StopReason,
        /// Token used to stop locally.
        id_tag: Option<T>,
    },
    /// Pause on the user's request.
    PauseCharging,
    /// Resume after a user pause.
    ResumeCharging,
    /// Update one enable source.
    EnableDisable(EnableSource),
    /// Reserve the connector.
    Reserve(i64),
    /// Drop the reservation.
    CancelReservation,
    /// Unlock the cable now.
    ForceUnlock,
    /// The energy manager enforced a limit.
    EnforceLimits {
        /// Root-side limits.
        limits: LimitRes,
        /// Validity.
        valid_for: Duration,
    },
    /// Block the board, persist, stop the threads.
    Shutdown,
}

/// The answer to a [`Request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// Nothing to say.
    Done,
    /// The command's boolean result.
    Accepted(bool),
}

/// What the energy request should say right now.
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyRequest {
    /// Simplified connector state.
    pub evse_state: EvseState,
    /// Ask the energy manager to run now.
    pub priority: bool,
    /// Rated current per phase.
    pub max_current_a: f64,
    /// Minimum usable current.
    pub min_current_a: f64,
    /// Phases.
    pub phases: u8,
    /// Latest measurements.
    pub meter: MeterSnapshot,
}

/// Something for the EVerest side to publish.
#[derive(Debug, Clone, PartialEq)]
pub enum Notification<T> {
    /// A session event, with the measurements at that moment.
    Session {
        /// The event.
        event: SessionEvent<T>,
        /// Measurements for the event's context object.
        meter: MeterSnapshot,
    },
    /// Periodic measurements.
    Meter(MeterSnapshot),
    /// The enforced limit changed.
    Limits {
        /// Current available to the vehicle, amps per phase.
        max_current_a: f64,
        /// Phases available.
        phases: u8,
    },
    /// Time to publish the energy flow request.
    EnergyRequest(EnergyRequest),
    /// Raise `Inoperative` with this message.
    FaultRaised(String),
    /// Clear `Inoperative`.
    FaultCleared,
    /// A transaction was open when the process last died. Close it
    /// (TransactionFinished with PowerLoss, then SessionFinished).
    RecoveredTransaction {
        /// Its session uuid.
        uuid: String,
        /// Measurements now.
        meter: MeterSnapshot,
    },
}

enum Inbox<T> {
    Rx { generation: u64, line: String },
    Disconnected { generation: u64 },
    Call(Request<T>, Sender<Reply>),
}

/// Cloneable, thread-safe way to send commands to the actor.
pub struct Handle<T> {
    tx: Mutex<Sender<Inbox<T>>>,
}

impl<T> Handle<T> {
    /// Send a request and wait for its answer. A dead actor answers
    /// `Done`/`Accepted(false)`.
    pub fn call(&self, request: Request<T>) -> Reply {
        let (reply_tx, reply_rx) = mpsc::channel();
        let sent = match self.tx.lock() {
            Ok(tx) => tx.send(Inbox::Call(request, reply_tx)).is_ok(),
            Err(_) => false,
        };
        if !sent {
            return Reply::Accepted(false);
        }
        reply_rx.recv().unwrap_or(Reply::Accepted(false))
    }

    /// [`Self::call`] for commands that answer a boolean.
    pub fn accepted(&self, request: Request<T>) -> bool {
        matches!(self.call(request), Reply::Accepted(true))
    }
}

/// A side's threads.
pub struct Runtime<T> {
    handle: Arc<Handle<T>>,
    notifications: Option<Receiver<Notification<T>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl<T: Clone + Send + 'static> Runtime<T> {
    /// Read persisted state, start the actor thread. The port is opened
    /// from the actor; failures there are faults, not errors here.
    pub fn start(config: RuntimeConfig) -> io::Result<Self> {
        let persisted = match &config.state_file {
            Some(path) => PersistedState::load(path)?,
            None => PersistedState::default(),
        };
        let trace = match &config.trace_file {
            Some(path) => Some(OpenOptions::new().create(true).append(true).open(path)?),
            None => None,
        };
        let (tx, rx) = mpsc::channel();
        let (notify_tx, notify_rx) = mpsc::channel();
        let name = format!("greenup {}", config.port);
        let actor_tx = tx.clone();
        let thread = thread::Builder::new()
            .name(name)
            .spawn(move || Actor::new(config, persisted, trace, actor_tx, notify_tx).run(rx))?;
        Ok(Runtime {
            handle: Arc::new(Handle { tx: Mutex::new(tx) }),
            notifications: Some(notify_rx),
            thread: Some(thread),
        })
    }

    /// The command side.
    pub fn handle(&self) -> Arc<Handle<T>> {
        self.handle.clone()
    }

    /// The notification side; available once.
    pub fn take_notifications(&mut self) -> Option<Receiver<Notification<T>>> {
        self.notifications.take()
    }

    /// Block the board, persist, stop the threads.
    pub fn shutdown(mut self) {
        self.handle.call(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Connection {
    io: Box<dyn port::PortIo>,
    generation: u64,
    stop: Arc<AtomicBool>,
}

struct Actor<T> {
    config: RuntimeConfig,
    driver: LegrandDriver,
    session: Session<T>,
    energy: EnergyNode,
    connection: Option<Connection>,
    generation: u64,
    next_connect: Instant,
    inbox_tx: Sender<Inbox<T>>,
    notify: Sender<Notification<T>>,
    persisted: PersistedState,
    persist_dirty: bool,
    last_persist: Instant,
    last_meter: Instant,
    last_energy_request: Instant,
    last_limits: Option<f64>,
    trace: Option<File>,
    started: bool,
}

impl<T: Clone + Send + 'static> Actor<T> {
    fn new(
        config: RuntimeConfig,
        persisted: PersistedState,
        trace: Option<File>,
        inbox_tx: Sender<Inbox<T>>,
        notify: Sender<Notification<T>>,
    ) -> Self {
        let now = Instant::now();
        let mut driver = LegrandDriver::new(config.driver.clone());
        driver.set_energy_wh(persisted.energy_wh);
        let energy = EnergyNode::new(config.energy);
        Actor {
            driver,
            session: Session::new(config.session.clone()),
            energy,
            connection: None,
            generation: 0,
            next_connect: now,
            inbox_tx,
            notify,
            persisted,
            persist_dirty: false,
            last_persist: now,
            last_meter: now,
            last_energy_request: now,
            last_limits: None,
            trace,
            config,
            started: false,
        }
    }

    fn run(mut self, rx: Receiver<Inbox<T>>) {
        loop {
            let now = Instant::now();
            self.maintain_connection(now);
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Inbox::Rx { generation, line }) => {
                    if self
                        .connection
                        .as_ref()
                        .is_some_and(|c| c.generation == generation)
                    {
                        self.trace("RX", &line);
                        let events = self.driver.on_line(&line, Instant::now());
                        self.handle_driver_events(events);
                    }
                }
                Ok(Inbox::Disconnected { generation }) => {
                    if self
                        .connection
                        .as_ref()
                        .is_some_and(|c| c.generation == generation)
                    {
                        self.drop_connection(Instant::now());
                    }
                }
                Ok(Inbox::Call(Request::Shutdown, reply)) => {
                    self.shutdown();
                    let _ = reply.send(Reply::Done);
                    return;
                }
                Ok(Inbox::Call(request, reply)) => {
                    let answer = self.handle_request(request, Instant::now());
                    let _ = reply.send(answer);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.shutdown();
                    return;
                }
            }
            let now = Instant::now();
            self.pump_driver(now);
            self.pump_energy(now);
            self.periodic(now);
        }
    }

    fn maintain_connection(&mut self, now: Instant) {
        if self.connection.is_some() || now < self.next_connect {
            return;
        }
        self.next_connect = now + self.config.reconnect_interval;
        match port::open(&self.config.port, Duration::from_millis(100)) {
            Ok(io) => {
                self.generation += 1;
                let generation = self.generation;
                let stop = Arc::new(AtomicBool::new(false));
                match io.try_clone_box() {
                    Ok(reader) => {
                        spawn_reader(reader, generation, stop.clone(), self.inbox_tx.clone());
                        self.connection = Some(Connection {
                            io,
                            generation,
                            stop,
                        });
                        self.driver.connected(now);
                        self.trace("--", &format!("opened {}", self.config.port));
                    }
                    Err(e) => self.trace("--", &format!("cannot clone port handle: {e}")),
                }
            }
            Err(e) => {
                self.trace("--", &format!("cannot open {}: {e}", self.config.port));
                // Not connected is a fault like any other; the driver
                // reports it through its poll.
                self.driver.disconnected(now);
                let poll = self.driver.poll(now);
                self.handle_driver_events(poll.events);
            }
        }
    }

    fn drop_connection(&mut self, now: Instant) {
        if let Some(connection) = self.connection.take() {
            connection.stop.store(true, Ordering::SeqCst);
            connection.io.shutdown();
            self.trace("--", "connection lost");
        }
        self.driver.disconnected(now);
        let poll = self.driver.poll(now);
        self.handle_driver_events(poll.events);
    }

    fn pump_driver(&mut self, now: Instant) {
        let poll = self.driver.poll(now);
        self.handle_driver_events(poll.events);
        for bytes in poll.commands {
            let text = String::from_utf8_lossy(&bytes).trim_end().to_owned();
            self.trace("TX", &text);
            let failed = match self.connection.as_mut() {
                Some(connection) => connection.io.write_all(&bytes).is_err(),
                None => true,
            };
            if failed {
                self.drop_connection(now);
                break;
            }
        }
    }

    fn pump_energy(&mut self, now: Instant) {
        if let Some(decision) = self.energy.tick(now) {
            self.apply_decision(decision, now);
        }
    }

    fn apply_decision(&mut self, decision: evse_session::Decision, now: Instant) {
        if let Some(amps) = decision.set_current {
            self.driver
                .apply(evse_session::DriverAction::SetCurrent(amps), now);
        }
        let outputs = self.session.set_energy_available(decision.available);
        self.handle_outputs(outputs, now);
        if self.last_limits != Some(decision.effective_a) {
            self.last_limits = Some(decision.effective_a);
            let _ = self.notify.send(Notification::Limits {
                max_current_a: decision.effective_a,
                phases: self.config.energy.phases,
            });
        }
    }

    fn periodic(&mut self, now: Instant) {
        if !self.started {
            return;
        }
        let meter_interval = if self.driver.meter().power_w > 0.0 {
            self.config.meter_interval_charging
        } else {
            self.config.meter_interval_idle
        };
        if now.duration_since(self.last_meter) >= meter_interval {
            self.last_meter = now;
            let meter = self.driver.meter();
            let _ = self.notify.send(Notification::Meter(meter));
        }
        if now.duration_since(self.last_energy_request) >= self.config.energy_request_interval {
            self.last_energy_request = now;
            let energy = self.config.energy;
            let request = EnergyRequest {
                evse_state: self.session.evse_state(),
                priority: self.session.take_priority(),
                max_current_a: energy.max_current_a,
                min_current_a: energy.min_current_a,
                phases: energy.phases,
                meter: self.driver.meter(),
            };
            let _ = self.notify.send(Notification::EnergyRequest(request));
        }
        let energy_wh = self.driver.meter().energy_wh_import;
        if energy_wh > self.persisted.energy_wh {
            self.persisted.energy_wh = energy_wh;
            self.persist_dirty = true;
        }
        if self.persist_dirty
            && now.duration_since(self.last_persist) >= self.config.persist_interval
        {
            self.persist(now);
        }
    }

    fn persist(&mut self, now: Instant) {
        self.last_persist = now;
        self.persist_dirty = false;
        if let Some(path) = &self.config.state_file {
            if let Err(e) = self.persisted.save(path) {
                self.trace("--", &format!("cannot persist state: {e}"));
            }
        }
    }

    fn handle_request(&mut self, request: Request<T>, now: Instant) -> Reply {
        match request {
            Request::Startup => {
                self.started = true;
                let outputs = self.session.startup();
                self.handle_outputs(outputs, now);
                if let Some(uuid) = self.persisted.open_transaction.take() {
                    self.persist_dirty = true;
                    let _ = self.notify.send(Notification::RecoveredTransaction {
                        uuid,
                        meter: self.driver.meter(),
                    });
                }
                Reply::Done
            }
            Request::Authorize {
                token,
                reservation_id,
            } => {
                let outputs = self.session.authorize(token, reservation_id);
                self.handle_outputs(outputs, now);
                Reply::Done
            }
            Request::WithdrawAuthorization => {
                let outputs = self.session.withdraw_authorization();
                self.handle_outputs(outputs, now);
                Reply::Done
            }
            Request::StopTransaction { reason, id_tag } => {
                let (ok, outputs) = self.session.stop_transaction(reason, id_tag);
                self.handle_outputs(outputs, now);
                Reply::Accepted(ok)
            }
            Request::PauseCharging => {
                let (ok, outputs) = self.session.pause_charging();
                self.handle_outputs(outputs, now);
                Reply::Accepted(ok)
            }
            Request::ResumeCharging => {
                let (ok, outputs) = self.session.resume_charging();
                self.handle_outputs(outputs, now);
                Reply::Accepted(ok)
            }
            Request::EnableDisable(source) => {
                let (enabled, outputs) = self.session.enable_disable(source);
                self.handle_outputs(outputs, now);
                Reply::Accepted(enabled)
            }
            Request::Reserve(id) => {
                let (ok, outputs) = self.session.reserve(id);
                self.handle_outputs(outputs, now);
                Reply::Accepted(ok)
            }
            Request::CancelReservation => {
                let outputs = self.session.cancel_reservation();
                self.handle_outputs(outputs, now);
                Reply::Done
            }
            Request::ForceUnlock => {
                let (ok, outputs) = self.session.force_unlock();
                self.handle_outputs(outputs, now);
                Reply::Accepted(ok)
            }
            Request::EnforceLimits { limits, valid_for } => {
                let decision = self.energy.enforce(&limits, valid_for, now);
                self.apply_decision(decision, now);
                Reply::Done
            }
            Request::Shutdown => Reply::Done,
        }
    }

    fn handle_driver_events(&mut self, events: Vec<DriverEvent>) {
        let now = Instant::now();
        for event in events {
            let outputs = match event {
                DriverEvent::Fact(fact) => self.session.on_fact(fact),
                DriverEvent::Fault(fault) => self.session.set_fault(fault),
                DriverEvent::Hint(hint) => self.session.on_hint(&hint),
                DriverEvent::MeterUpdated => Vec::new(),
            };
            self.handle_outputs(outputs, now);
        }
    }

    fn handle_outputs(&mut self, outputs: Vec<Output<T>>, now: Instant) {
        for output in outputs {
            match output {
                Output::Event(event) => {
                    match &event.event {
                        Event::TransactionStarted { .. } => {
                            self.persisted.open_transaction = Some(event.uuid.clone());
                            self.persist(now);
                        }
                        Event::TransactionFinished { .. } => {
                            self.persisted.open_transaction = None;
                            self.persist(now);
                        }
                        _ => {}
                    }
                    let _ = self.notify.send(Notification::Session {
                        event,
                        meter: self.driver.meter(),
                    });
                }
                Output::Driver(action) => self.driver.apply(action, now),
                Output::FaultRaised(message) => {
                    let _ = self.notify.send(Notification::FaultRaised(message));
                }
                Output::FaultCleared => {
                    let _ = self.notify.send(Notification::FaultCleared);
                }
            }
        }
    }

    fn shutdown(&mut self) {
        let now = Instant::now();
        // Block the board before leaving; give the scheduler a moment to
        // get T2CNOK out.
        self.driver
            .apply(evse_session::DriverAction::Authorize(false), now);
        let deadline = now + Duration::from_millis(500);
        while Instant::now() < deadline && self.connection.is_some() {
            self.pump_driver(Instant::now());
            thread::sleep(Duration::from_millis(20));
        }
        self.persisted.energy_wh = self.driver.meter().energy_wh_import;
        self.persist(Instant::now());
        self.drop_connection(Instant::now());
        self.trace("--", "shutdown");
    }

    fn trace(&mut self, direction: &str, text: &str) {
        if let Some(file) = self.trace.as_mut() {
            let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let _ = writeln!(file, "{stamp} {direction} {text}");
        }
    }
}

fn spawn_reader<T: Send + 'static>(
    mut io: Box<dyn port::PortIo>,
    generation: u64,
    stop: Arc<AtomicBool>,
    tx: Sender<Inbox<T>>,
) {
    thread::spawn(move || {
        let mut framer = LineFramer::new();
        let mut buf = [0u8; 512];
        loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            match io.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    for line in framer.push(&buf[..n]) {
                        if tx.send(Inbox::Rx { generation, line }).is_err() {
                            return;
                        }
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => break,
            }
        }
        let _ = tx.send(Inbox::Disconnected { generation });
    });
}
