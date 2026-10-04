/// Who asked for an enable or disable (mirrors EVerest's `EnableDisableSource.enable_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnableSourceKind {
    /// No particular source; the initial state.
    Unspecified,
    /// The local API module.
    LocalApi,
    /// A physical key lock on the charger.
    LocalKeyLock,
    /// A service technician.
    ServiceTechnician,
    /// A remote key lock.
    RemoteKeyLock,
    /// A mobile app.
    MobileApp,
    /// A firmware update in progress.
    FirmwareUpdate,
    /// The OCPP central system.
    Csms,
}

/// Requested state of an enable source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnableState {
    /// The source does not care; others decide.
    Unassigned,
    /// The source wants the EVSE disabled.
    Disable,
    /// The source wants the EVSE enabled.
    Enable,
}

/// One row of the enable/disable arbitration table.
///
/// Priority 0 is the highest, 10000 the lowest. At equal priority, disable
/// wins. With every source unassigned the EVSE is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnableSource {
    /// Who.
    pub kind: EnableSourceKind,
    /// What.
    pub state: EnableState,
    /// How strongly.
    pub priority: i64,
}

impl EnableSource {
    /// The row every session starts with: unspecified, unassigned, lowest priority.
    pub const INITIAL: EnableSource = EnableSource {
        kind: EnableSourceKind::Unspecified,
        state: EnableState::Unassigned,
        priority: 10000,
    };
}

/// Why a session started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartReason {
    /// A vehicle was plugged in.
    EvConnected,
    /// An authorization arrived before a vehicle.
    Authorized,
}

/// Why a transaction stopped (mirrors EVerest's `StopTransactionReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum StopReason {
    EmergencyStop,
    EvDisconnected,
    HardReset,
    Local,
    Other,
    PowerLoss,
    Reboot,
    Remote,
    SoftReset,
    UnlockCommand,
    DeAuthorized,
    EnergyLimitReached,
    GroundFault,
    LocalOutOfCredit,
    MasterPass,
    OvercurrentFault,
    PowerQuality,
    SocLimitReached,
    StoppedByEv,
    TimeLimitReached,
    Timeout,
    ReqEnergyTransferRejected,
    EvseDisabled,
}

/// Why the EVSE paused charging. Several can be active at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PauseReason {
    /// `pause_charging` was called.
    UserPause,
    /// A fault prevents charging; it resumes once the fault clears.
    Error,
    /// The energy manager left no energy for this connector.
    NoEnergy,
}

/// Simplified EVSE state for the energy tree (mirrors EVerest's `EvseState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum EvseState {
    Unplugged,
    WaitForAuth,
    WaitForEnergy,
    PrepareCharging,
    PausedEv,
    PausedEvse,
    Charging,
    Finished,
    Disabled,
}

/// What the board reports, reduced to what the session needs.
///
/// The driver maps the board's own state letters to these; the mapping is
/// tuned against hardware traces. Faults are separate (see
/// [`DriverEvent::Fault`](crate::DriverEvent::Fault)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fact {
    /// No vehicle.
    Unplugged,
    /// Vehicle plugged, the board is not authorizing it.
    Plugged,
    /// The board authorized the vehicle; it is not drawing power yet.
    BoardAuthorized,
    /// Power is flowing.
    Delivering,
    /// The board is winding the session down.
    Stopping,
    /// The board ended the session on its own; the vehicle is still plugged.
    EndedByBoard,
}

/// Extra information from the board that is not a state change.
#[derive(Debug, Clone, PartialEq)]
pub enum Hint {
    /// The vehicle stopped drawing because its battery is full.
    ChargeComplete,
    /// The board's end-of-session report.
    SessionReport {
        /// Seconds spent delivering.
        charging_secs: u64,
        /// Mean power over the session, watts.
        mean_power_w: f32,
    },
}

/// A session event, in EvseManager's vocabulary. `T` is the id token type.
#[derive(Debug, Clone, PartialEq)]
pub enum Event<T> {
    /// The EVSE became enabled; carries the winning source.
    Enabled(EnableSource),
    /// The EVSE became disabled; carries the winning source.
    Disabled(EnableSource),
    /// A session began.
    SessionStarted {
        /// Plug-in or authorization first.
        reason: StartReason,
        /// The token, when authorization came first.
        id_tag: Option<T>,
        /// The reservation this session consumes, if any.
        reservation_id: Option<i64>,
    },
    /// Waiting for an authorization.
    AuthRequired,
    /// An authorization was accepted.
    Authorized,
    /// The authorization was withdrawn.
    Deauthorized,
    /// Vehicle and authorization are both present; energy may flow.
    TransactionStarted {
        /// The token that authorized the transaction.
        id_tag: T,
        /// The reservation used, if any.
        reservation_id: Option<i64>,
    },
    /// Waiting for the vehicle to draw power.
    PrepareCharging,
    /// Power is flowing.
    ChargingStarted,
    /// The vehicle stopped drawing.
    ChargingPausedEv,
    /// The EVSE stopped delivering; carries every active reason.
    ChargingPausedEvse(Vec<PauseReason>),
    /// The transaction is winding down.
    StoppingCharging,
    /// Delivery for this transaction is over.
    ChargingFinished,
    /// The transaction is over.
    TransactionFinished {
        /// Why.
        reason: StopReason,
        /// The token used to stop it locally, if any.
        id_tag: Option<T>,
    },
    /// The vehicle left (or the EVSE was disabled); the connector is free.
    SessionFinished,
    /// A reservation was placed.
    ReservationStart,
    /// The reservation was used or cancelled.
    ReservationEnd,
    /// A vehicle was plugged in but nobody authorized it in time.
    PluginTimeout,
}

impl<T> Event<T> {
    /// The event name as EVerest spells it (`SessionEventEnum`).
    pub fn name(&self) -> &'static str {
        match self {
            Event::Enabled(_) => "Enabled",
            Event::Disabled(_) => "Disabled",
            Event::SessionStarted { .. } => "SessionStarted",
            Event::AuthRequired => "AuthRequired",
            Event::Authorized => "Authorized",
            Event::Deauthorized => "Deauthorized",
            Event::TransactionStarted { .. } => "TransactionStarted",
            Event::PrepareCharging => "PrepareCharging",
            Event::ChargingStarted => "ChargingStarted",
            Event::ChargingPausedEv => "ChargingPausedEV",
            Event::ChargingPausedEvse(_) => "ChargingPausedEVSE",
            Event::StoppingCharging => "StoppingCharging",
            Event::ChargingFinished => "ChargingFinished",
            Event::TransactionFinished { .. } => "TransactionFinished",
            Event::SessionFinished => "SessionFinished",
            Event::ReservationStart => "ReservationStart",
            Event::ReservationEnd => "ReservationEnd",
            Event::PluginTimeout => "PluginTimeout",
        }
    }
}

/// An event together with the session it belongs to.
///
/// `uuid` is stable for the whole session and at most 36 ASCII characters;
/// OCPP 2.x uses it as the transaction id. Events outside a session
/// (Enabled, Disabled, reservations) carry the uuid of the last session or a
/// fresh one.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEvent<T> {
    /// Session id.
    pub uuid: String,
    /// What happened.
    pub event: Event<T>,
}
