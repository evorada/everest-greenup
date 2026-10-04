use crate::driver::DriverAction;
use crate::types::{
    EnableSource, EnableState, Event, EvseState, Fact, Hint, PauseReason, SessionEvent,
    StartReason, StopReason,
};

/// What the session wants done after handling an input.
#[derive(Debug, Clone, PartialEq)]
pub enum Output<T> {
    /// Publish this session event.
    Event(SessionEvent<T>),
    /// Tell the board.
    Driver(DriverAction),
    /// Raise `evse_manager/Inoperative` with this message.
    FaultRaised(String),
    /// Clear `evse_manager/Inoperative`.
    FaultCleared,
}

/// Tunables of a [`Session`].
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Gate G2: after we blocked the board and it reported the session as
    /// ended (`M`), does `T2COK` resume it? When false, a resume ends the
    /// transaction instead and the vehicle has to be replugged.
    pub resume_from_ended: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            resume_from_ended: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Enabled, no vehicle, no session.
    Idle,
    /// Authorized before a vehicle arrived; session open, no transaction.
    AuthorizedUnplugged,
    /// Vehicle plugged, waiting for an authorization.
    WaitingForAuth,
    /// Transaction running, board told to allow, vehicle not drawing yet.
    Preparing,
    /// Power flowing.
    Charging,
    /// Vehicle stopped drawing on its own.
    PausedEv,
    /// We told the board to block (user, energy or fault).
    PausedEvse,
    /// The board reported it is winding down.
    Stopping,
    /// Transaction over, vehicle still plugged.
    Finished,
    /// Disabled by an enable source.
    Disabled,
}

#[derive(Debug, Clone)]
struct Auth<T> {
    token: T,
    reservation_id: Option<i64>,
}

/// One connector's session state machine.
///
/// Inputs are facts from the board, commands from EVerest and energy
/// availability. Outputs are events in EvseManager's grammar, driver
/// actions and fault changes. `T` is the id token type; the session only
/// stores and echoes it.
#[derive(Debug, Clone)]
pub struct Session<T> {
    config: SessionConfig,
    last_fact: Option<Fact>,
    table: Vec<EnableSource>,
    active_source: EnableSource,
    enabled: bool,
    phase: Phase,
    plugged: bool,
    uuid: String,
    session_active: bool,
    pending: Option<Auth<T>>,
    transaction: Option<Auth<T>>,
    pause_reasons: Vec<PauseReason>,
    reservation: Option<i64>,
    fault: Option<String>,
    energy_available: bool,
    charge_complete: bool,
    timed_out: bool,
    priority: bool,
}

impl<T: Clone> Default for Session<T> {
    fn default() -> Self {
        Self::new(SessionConfig::default())
    }
}

impl<T: Clone> Session<T> {
    /// A fresh, enabled, unplugged session with no energy available yet
    /// (the energy manager has to grant some first).
    pub fn new(config: SessionConfig) -> Self {
        Session {
            config,
            last_fact: None,
            table: vec![EnableSource::INITIAL],
            active_source: EnableSource::INITIAL,
            enabled: true,
            phase: Phase::Idle,
            plugged: false,
            uuid: new_uuid(),
            session_active: false,
            pending: None,
            transaction: None,
            pause_reasons: Vec::new(),
            reservation: None,
            fault: None,
            energy_available: false,
            charge_complete: false,
            timed_out: false,
            priority: true,
        }
    }

    /// Events to publish once the module is ready: the initial `Enabled`,
    /// as EvseManager does.
    pub fn startup(&mut self) -> Vec<Output<T>> {
        let mut out = Vec::new();
        self.emit(&mut out, Event::Enabled(self.active_source));
        self.priority = true;
        out
    }

    /// Id of the current (or last) session.
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Simplified state for the energy tree.
    pub fn evse_state(&self) -> EvseState {
        if !self.enabled {
            return EvseState::Disabled;
        }
        match self.phase {
            Phase::Idle => EvseState::Unplugged,
            Phase::AuthorizedUnplugged | Phase::WaitingForAuth => EvseState::WaitForAuth,
            Phase::Preparing => EvseState::PrepareCharging,
            Phase::Charging | Phase::Stopping => EvseState::Charging,
            Phase::PausedEv => EvseState::PausedEv,
            Phase::PausedEvse => {
                if self.pause_reasons == [PauseReason::NoEnergy] {
                    EvseState::WaitForEnergy
                } else {
                    EvseState::PausedEvse
                }
            }
            Phase::Finished => EvseState::Finished,
            Phase::Disabled => EvseState::Disabled,
        }
    }

    /// True once after startup and after every change that the energy
    /// manager should react to right away.
    pub fn take_priority(&mut self) -> bool {
        std::mem::take(&mut self.priority)
    }

    /// Current fault message, if any.
    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    /// True while a transaction runs.
    pub fn transaction_active(&self) -> bool {
        self.transaction.is_some()
    }

    /// True while the board reports a vehicle.
    pub fn plugged(&self) -> bool {
        self.plugged
    }

    /// True unless an enable source disabled the EVSE.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The winning enable/disable source.
    pub fn active_source(&self) -> EnableSource {
        self.active_source
    }

    /// Active EVSE pause reasons, in order.
    pub fn pause_reasons(&self) -> &[PauseReason] {
        &self.pause_reasons
    }

    /// The board reported a (new) state.
    ///
    /// Any fact other than `Unplugged` means a vehicle is present, so a
    /// board that skips `Plugged` (straight from A to C or D) still starts
    /// the session or the transaction.
    pub fn on_fact(&mut self, fact: Fact) -> Vec<Output<T>> {
        let mut out = Vec::new();
        self.last_fact = Some(fact);
        if fact == Fact::Unplugged {
            self.plugged = false;
            if self.transaction.is_some() {
                self.finish_transaction(&mut out, StopReason::EvDisconnected, None);
            }
            if self.session_active && self.phase != Phase::AuthorizedUnplugged {
                self.end_session(&mut out);
            }
            return out;
        }
        self.plugged = true;
        match self.phase {
            Phase::Idle => self.start_plugged_session(&mut out),
            Phase::AuthorizedUnplugged => {
                if let Some(auth) = self.pending.take() {
                    self.start_transaction(&mut out, auth);
                }
            }
            _ => {}
        }
        match fact {
            Fact::Unplugged | Fact::Plugged => {}
            Fact::BoardAuthorized => {
                if self.phase == Phase::Charging {
                    self.emit(&mut out, Event::ChargingPausedEv);
                    self.phase = Phase::PausedEv;
                    self.priority = true;
                }
            }
            Fact::Delivering => {
                if matches!(
                    self.phase,
                    Phase::Preparing | Phase::PausedEv | Phase::Stopping
                ) {
                    self.emit(&mut out, Event::ChargingStarted);
                    self.phase = Phase::Charging;
                    self.priority = true;
                }
            }
            // After our own T2CNOK the board winds down too (W, then M or
            // B); that is the pause taking effect, not the board ending
            // the transaction on its own.
            Fact::Stopping => {
                if matches!(self.phase, Phase::Charging | Phase::PausedEv) {
                    self.emit(&mut out, Event::StoppingCharging);
                    self.phase = Phase::Stopping;
                }
            }
            Fact::EndedByBoard => {
                if self.transaction.is_some() && self.phase != Phase::PausedEvse {
                    let reason = if self.charge_complete {
                        StopReason::StoppedByEv
                    } else {
                        StopReason::Other
                    };
                    self.finish_transaction(&mut out, reason, None);
                }
            }
        }
        out
    }

    /// Extra information from the board.
    pub fn on_hint(&mut self, hint: &Hint) -> Vec<Output<T>> {
        if matches!(hint, Hint::ChargeComplete) && self.transaction.is_some() {
            self.charge_complete = true;
        }
        Vec::new()
    }

    /// The driver's aggregated fault changed. Raises or clears
    /// `Inoperative` and pauses or resumes a running transaction.
    pub fn set_fault(&mut self, fault: Option<String>) -> Vec<Output<T>> {
        let mut out = Vec::new();
        if fault == self.fault {
            return out;
        }
        if self.fault.is_some() {
            out.push(Output::FaultCleared);
        }
        self.fault = fault;
        match &self.fault {
            Some(message) => {
                out.push(Output::FaultRaised(message.clone()));
                if self.in_active_transaction() {
                    self.add_pause(&mut out, PauseReason::Error);
                }
            }
            None => {
                if self.in_active_transaction() {
                    self.remove_pause(&mut out, PauseReason::Error);
                }
            }
        }
        out
    }

    /// An accepted authorization for this connector. Rejected ones must
    /// not reach the session.
    pub fn authorize(&mut self, token: T, reservation_id: Option<i64>) -> Vec<Output<T>> {
        let mut out = Vec::new();
        if !self.enabled || self.fault.is_some() {
            return out;
        }
        let reservation_id = reservation_id.or(self.reservation);
        match self.phase {
            Phase::Idle => {
                self.start_session(
                    &mut out,
                    StartReason::Authorized,
                    Some(token.clone()),
                    reservation_id,
                );
                self.emit(&mut out, Event::Authorized);
                self.pending = Some(Auth {
                    token,
                    reservation_id,
                });
                self.phase = Phase::AuthorizedUnplugged;
            }
            Phase::WaitingForAuth => {
                self.emit(&mut out, Event::Authorized);
                self.start_transaction(
                    &mut out,
                    Auth {
                        token,
                        reservation_id,
                    },
                );
            }
            _ => {}
        }
        out
    }

    /// Auth withdrew the authorization (typically a connection timeout).
    pub fn withdraw_authorization(&mut self) -> Vec<Output<T>> {
        let mut out = Vec::new();
        match self.phase {
            Phase::AuthorizedUnplugged => {
                self.emit(&mut out, Event::Deauthorized);
                self.pending = None;
                self.end_session(&mut out);
            }
            Phase::WaitingForAuth if self.session_active && !self.timed_out => {
                self.timed_out = true;
                self.emit(&mut out, Event::Deauthorized);
                self.emit(&mut out, Event::PluginTimeout);
            }
            _ => {}
        }
        out
    }

    /// Stop the running transaction. Returns false when there is none.
    pub fn stop_transaction(
        &mut self,
        reason: StopReason,
        id_tag: Option<T>,
    ) -> (bool, Vec<Output<T>>) {
        let mut out = Vec::new();
        if self.transaction.is_none() {
            return (false, out);
        }
        self.finish_transaction(&mut out, reason, id_tag);
        (true, out)
    }

    /// Pause the running transaction on the user's request.
    pub fn pause_charging(&mut self) -> (bool, Vec<Output<T>>) {
        let mut out = Vec::new();
        if !self.in_active_transaction() {
            return (false, out);
        }
        self.add_pause(&mut out, PauseReason::UserPause);
        (true, out)
    }

    /// Resume after a user pause. False when the pause was the vehicle's,
    /// or when the board ended the session meanwhile and cannot resume
    /// (see [`SessionConfig::resume_from_ended`]).
    pub fn resume_charging(&mut self) -> (bool, Vec<Output<T>>) {
        let mut out = Vec::new();
        if self.phase != Phase::PausedEvse || !self.pause_reasons.contains(&PauseReason::UserPause)
        {
            return (false, out);
        }
        let resumed = self.remove_pause(&mut out, PauseReason::UserPause);
        (resumed, out)
    }

    /// The energy component decided whether the enforced limit allows
    /// charging at all.
    pub fn set_energy_available(&mut self, available: bool) -> Vec<Output<T>> {
        let mut out = Vec::new();
        self.energy_available = available;
        if self.in_active_transaction() {
            if available {
                self.remove_pause(&mut out, PauseReason::NoEnergy);
            } else {
                self.add_pause(&mut out, PauseReason::NoEnergy);
            }
        }
        out
    }

    /// Update one enable source and re-arbitrate. Returns whether the EVSE
    /// is enabled afterwards. Events only when the outcome changes.
    pub fn enable_disable(&mut self, source: EnableSource) -> (bool, Vec<Output<T>>) {
        let mut out = Vec::new();
        match self.table.iter_mut().find(|row| row.kind == source.kind) {
            Some(row) => *row = source,
            None => self.table.push(source),
        }
        let last = self.active_source;
        let (enabled, winner) = arbitrate(&self.table);
        self.active_source = winner;
        self.enabled = enabled;
        let changed = (winner.state == EnableState::Disable || last.state == EnableState::Disable)
            && winner.state != last.state;
        if changed {
            if enabled {
                self.phase = Phase::Idle;
                self.emit(&mut out, Event::Enabled(winner));
                if self.plugged {
                    self.start_plugged_session(&mut out);
                }
            } else {
                if self.transaction.is_some() {
                    self.finish_transaction(&mut out, StopReason::EvseDisabled, None);
                }
                if self.session_active {
                    self.end_session(&mut out);
                }
                self.pending = None;
                self.phase = Phase::Disabled;
                self.emit(&mut out, Event::Disabled(winner));
            }
            self.priority = true;
        }
        (enabled, out)
    }

    /// Reserve the connector. Only an idle, enabled, healthy connector
    /// accepts.
    pub fn reserve(&mut self, reservation_id: i64) -> (bool, Vec<Output<T>>) {
        let mut out = Vec::new();
        if self.phase != Phase::Idle || !self.enabled || self.fault.is_some() {
            return (false, out);
        }
        if self.reservation.is_none() {
            self.emit(&mut out, Event::ReservationStart);
        }
        self.reservation = Some(reservation_id);
        (true, out)
    }

    /// Drop the reservation, if any.
    pub fn cancel_reservation(&mut self) -> Vec<Output<T>> {
        let mut out = Vec::new();
        if self.reservation.take().is_some() {
            self.emit(&mut out, Event::ReservationEnd);
        }
        out
    }

    /// Unlock the cable now, ending any transaction first.
    pub fn force_unlock(&mut self) -> (bool, Vec<Output<T>>) {
        let mut out = Vec::new();
        if self.transaction.is_some() {
            self.finish_transaction(&mut out, StopReason::UnlockCommand, None);
        }
        out.push(Output::Driver(DriverAction::Unlock));
        (true, out)
    }

    fn in_active_transaction(&self) -> bool {
        self.transaction.is_some()
            && matches!(
                self.phase,
                Phase::Preparing | Phase::Charging | Phase::PausedEv | Phase::PausedEvse
            )
    }

    fn emit(&self, out: &mut Vec<Output<T>>, event: Event<T>) {
        out.push(Output::Event(SessionEvent {
            uuid: self.uuid.clone(),
            event,
        }));
    }

    fn start_session(
        &mut self,
        out: &mut Vec<Output<T>>,
        reason: StartReason,
        id_tag: Option<T>,
        reservation_id: Option<i64>,
    ) {
        self.uuid = new_uuid();
        self.session_active = true;
        self.charge_complete = false;
        self.timed_out = false;
        self.emit(
            out,
            Event::SessionStarted {
                reason,
                id_tag,
                reservation_id,
            },
        );
        self.priority = true;
    }

    fn start_plugged_session(&mut self, out: &mut Vec<Output<T>>) {
        self.start_session(out, StartReason::EvConnected, None, self.reservation);
        self.emit(out, Event::AuthRequired);
        self.phase = Phase::WaitingForAuth;
    }

    fn end_session(&mut self, out: &mut Vec<Output<T>>) {
        self.emit(out, Event::SessionFinished);
        self.session_active = false;
        self.pending = None;
        self.charge_complete = false;
        self.pause_reasons.clear();
        if self.phase != Phase::Disabled {
            self.phase = Phase::Idle;
        }
        self.priority = true;
    }

    fn start_transaction(&mut self, out: &mut Vec<Output<T>>, auth: Auth<T>) {
        self.emit(
            out,
            Event::TransactionStarted {
                id_tag: auth.token.clone(),
                reservation_id: auth.reservation_id,
            },
        );
        if self.reservation.take().is_some() {
            self.emit(out, Event::ReservationEnd);
        }
        self.transaction = Some(auth);
        self.pause_reasons.clear();
        self.charge_complete = false;
        if self.fault.is_some() {
            self.pause_reasons.push(PauseReason::Error);
        }
        if !self.energy_available {
            self.pause_reasons.push(PauseReason::NoEnergy);
        }
        if self.pause_reasons.is_empty() {
            out.push(Output::Driver(DriverAction::Authorize(true)));
            self.emit(out, Event::PrepareCharging);
            self.phase = Phase::Preparing;
        } else {
            out.push(Output::Driver(DriverAction::Authorize(false)));
            self.emit(out, Event::ChargingPausedEvse(self.pause_reasons.clone()));
            self.phase = Phase::PausedEvse;
        }
        self.priority = true;
    }

    fn finish_transaction(
        &mut self,
        out: &mut Vec<Output<T>>,
        reason: StopReason,
        id_tag: Option<T>,
    ) {
        out.push(Output::Driver(DriverAction::Authorize(false)));
        if self.phase != Phase::Stopping {
            self.emit(out, Event::StoppingCharging);
        }
        self.emit(out, Event::ChargingFinished);
        self.emit(out, Event::TransactionFinished { reason, id_tag });
        self.transaction = None;
        self.pause_reasons.clear();
        self.phase = Phase::Finished;
        self.priority = true;
    }

    fn add_pause(&mut self, out: &mut Vec<Output<T>>, reason: PauseReason) {
        if self.pause_reasons.contains(&reason) {
            return;
        }
        self.pause_reasons.push(reason);
        self.pause_reasons.sort();
        out.push(Output::Driver(DriverAction::Authorize(false)));
        self.emit(out, Event::ChargingPausedEvse(self.pause_reasons.clone()));
        self.phase = Phase::PausedEvse;
        self.priority = true;
    }

    /// Returns false when the transaction had to end instead of resuming.
    fn remove_pause(&mut self, out: &mut Vec<Output<T>>, reason: PauseReason) -> bool {
        let Some(index) = self.pause_reasons.iter().position(|r| *r == reason) else {
            return true;
        };
        self.pause_reasons.remove(index);
        if self.pause_reasons.is_empty() {
            if self.last_fact == Some(Fact::EndedByBoard) && !self.config.resume_from_ended {
                self.finish_transaction(out, StopReason::Other, None);
                return false;
            }
            out.push(Output::Driver(DriverAction::Authorize(true)));
            self.emit(out, Event::PrepareCharging);
            self.phase = Phase::Preparing;
        } else {
            self.emit(out, Event::ChargingPausedEvse(self.pause_reasons.clone()));
        }
        self.priority = true;
        true
    }
}

/// EvseManager's rule: ignore unassigned rows; the lowest priority number
/// wins; at a tie, disable beats enable; nothing assigned means enabled.
fn arbitrate(table: &[EnableSource]) -> (bool, EnableSource) {
    let mut winner = EnableSource::INITIAL;
    let mut enabled = true;
    for row in table {
        if row.state == EnableState::Unassigned {
            continue;
        }
        let take = winner.state == EnableState::Unassigned
            || row.priority < winner.priority
            || (row.priority == winner.priority && row.state == EnableState::Disable);
        if take {
            winner = *row;
            enabled = row.state != EnableState::Disable;
        }
    }
    (enabled, winner)
}

fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
