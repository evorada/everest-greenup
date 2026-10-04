//! Between the EVerest-agnostic crates and the generated EVerest types.

use evse_session::{
    EnableSource, EnableSourceKind, EnableState, Event, EvseState, MeterSnapshot, PauseReason,
    SessionEvent, StartReason, StopReason,
};
use greenup_runtime::EnergyRequest;

use crate::generated::types::authorization::ProvidedIdToken;
use crate::generated::types::energy::{
    EnergyFlowRequest, EvseState as EvEvseState, IntegerWithSource, LimitsReq, NodeType,
    NumberWithSource, ScheduleReqEntry,
};
use crate::generated::types::evse_manager::{
    AuthorizationEvent, ChargingPausedEVSEReasons, ChargingStateChangedEvent, EnableDisableSource,
    EnableDisableSourceAutoGenEnableSource as EvSource,
    EnableDisableSourceAutoGenEnableState as EvState, PauseChargingEVSEReasonEnum,
    SessionEvent as EvSessionEvent, SessionEventEnum, SessionFinished, SessionStarted,
    StartSessionReason, StopTransactionReason, TransactionFinished, TransactionStarted,
};
use crate::generated::types::powermeter::Powermeter;
use crate::generated::types::units::{Energy, Frequency, Power, Voltage};

/// RFC 3339 UTC with milliseconds, as EVerest expects in timestamps.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Measurements as a `powermeter` object. Per-phase currents are held
/// back until gate G9 shows what `CCI` means on a three-phase board.
pub fn powermeter(m: MeterSnapshot) -> Powermeter {
    Powermeter {
        var: None,
        var_signed: None,
        current_a: None,
        current_a_signed: None,
        energy_wh_export: None,
        energy_wh_export_signed: None,
        energy_wh_import: Energy {
            l_1: None,
            l_2: None,
            l_3: None,
            total: m.energy_wh_import,
        },
        energy_wh_import_signed: None,
        frequency_hz: m.frequency_hz.map(|f| Frequency {
            l_1: f,
            l_2: None,
            l_3: None,
        }),
        frequency_hz_signed: None,
        meter_id: None,
        phase_seq_error: None,
        power_w: Some(Power {
            l_1: None,
            l_2: None,
            l_3: None,
            total: m.power_w,
        }),
        power_w_signed: None,
        signed_meter_value: None,
        temperatures: None,
        timestamp: now_rfc3339(),
        voltage_v: m.voltage_v.map(|v| Voltage {
            dc: None,
            l_1: Some(v),
            l_2: None,
            l_3: None,
        }),
        voltage_v_signed: None,
    }
}

pub fn enable_source_from(source: &EnableDisableSource) -> EnableSource {
    EnableSource {
        kind: match source.enable_source {
            EvSource::Unspecified => EnableSourceKind::Unspecified,
            EvSource::LocalAPI => EnableSourceKind::LocalApi,
            EvSource::LocalKeyLock => EnableSourceKind::LocalKeyLock,
            EvSource::ServiceTechnician => EnableSourceKind::ServiceTechnician,
            EvSource::RemoteKeyLock => EnableSourceKind::RemoteKeyLock,
            EvSource::MobileApp => EnableSourceKind::MobileApp,
            EvSource::FirmwareUpdate => EnableSourceKind::FirmwareUpdate,
            EvSource::CSMS => EnableSourceKind::Csms,
        },
        state: match source.enable_state {
            EvState::Unassigned => EnableState::Unassigned,
            EvState::Disable => EnableState::Disable,
            EvState::Enable => EnableState::Enable,
        },
        priority: source.enable_priority,
    }
}

pub fn enable_source_to(source: EnableSource) -> EnableDisableSource {
    EnableDisableSource {
        enable_priority: source.priority,
        enable_source: match source.kind {
            EnableSourceKind::Unspecified => EvSource::Unspecified,
            EnableSourceKind::LocalApi => EvSource::LocalAPI,
            EnableSourceKind::LocalKeyLock => EvSource::LocalKeyLock,
            EnableSourceKind::ServiceTechnician => EvSource::ServiceTechnician,
            EnableSourceKind::RemoteKeyLock => EvSource::RemoteKeyLock,
            EnableSourceKind::MobileApp => EvSource::MobileApp,
            EnableSourceKind::FirmwareUpdate => EvSource::FirmwareUpdate,
            EnableSourceKind::Csms => EvSource::CSMS,
        },
        enable_state: match source.state {
            EnableState::Unassigned => EvState::Unassigned,
            EnableState::Disable => EvState::Disable,
            EnableState::Enable => EvState::Enable,
        },
    }
}

pub fn stop_reason_from(reason: StopTransactionReason) -> StopReason {
    match reason {
        StopTransactionReason::EmergencyStop => StopReason::EmergencyStop,
        StopTransactionReason::EVDisconnected => StopReason::EvDisconnected,
        StopTransactionReason::HardReset => StopReason::HardReset,
        StopTransactionReason::Local => StopReason::Local,
        StopTransactionReason::Other => StopReason::Other,
        StopTransactionReason::PowerLoss => StopReason::PowerLoss,
        StopTransactionReason::Reboot => StopReason::Reboot,
        StopTransactionReason::Remote => StopReason::Remote,
        StopTransactionReason::SoftReset => StopReason::SoftReset,
        StopTransactionReason::UnlockCommand => StopReason::UnlockCommand,
        StopTransactionReason::DeAuthorized => StopReason::DeAuthorized,
        StopTransactionReason::EnergyLimitReached => StopReason::EnergyLimitReached,
        StopTransactionReason::GroundFault => StopReason::GroundFault,
        StopTransactionReason::LocalOutOfCredit => StopReason::LocalOutOfCredit,
        StopTransactionReason::MasterPass => StopReason::MasterPass,
        StopTransactionReason::OvercurrentFault => StopReason::OvercurrentFault,
        StopTransactionReason::PowerQuality => StopReason::PowerQuality,
        StopTransactionReason::SOCLimitReached => StopReason::SocLimitReached,
        StopTransactionReason::StoppedByEV => StopReason::StoppedByEv,
        StopTransactionReason::TimeLimitReached => StopReason::TimeLimitReached,
        StopTransactionReason::Timeout => StopReason::Timeout,
        StopTransactionReason::ReqEnergyTransferRejected => StopReason::ReqEnergyTransferRejected,
        StopTransactionReason::EVSEDisabled => StopReason::EvseDisabled,
    }
}

pub fn stop_reason_to(reason: StopReason) -> StopTransactionReason {
    match reason {
        StopReason::EmergencyStop => StopTransactionReason::EmergencyStop,
        StopReason::EvDisconnected => StopTransactionReason::EVDisconnected,
        StopReason::HardReset => StopTransactionReason::HardReset,
        StopReason::Local => StopTransactionReason::Local,
        StopReason::Other => StopTransactionReason::Other,
        StopReason::PowerLoss => StopTransactionReason::PowerLoss,
        StopReason::Reboot => StopTransactionReason::Reboot,
        StopReason::Remote => StopTransactionReason::Remote,
        StopReason::SoftReset => StopTransactionReason::SoftReset,
        StopReason::UnlockCommand => StopTransactionReason::UnlockCommand,
        StopReason::DeAuthorized => StopTransactionReason::DeAuthorized,
        StopReason::EnergyLimitReached => StopTransactionReason::EnergyLimitReached,
        StopReason::GroundFault => StopTransactionReason::GroundFault,
        StopReason::LocalOutOfCredit => StopTransactionReason::LocalOutOfCredit,
        StopReason::MasterPass => StopTransactionReason::MasterPass,
        StopReason::OvercurrentFault => StopTransactionReason::OvercurrentFault,
        StopReason::PowerQuality => StopTransactionReason::PowerQuality,
        StopReason::SocLimitReached => StopTransactionReason::SOCLimitReached,
        StopReason::StoppedByEv => StopTransactionReason::StoppedByEV,
        StopReason::TimeLimitReached => StopTransactionReason::TimeLimitReached,
        StopReason::Timeout => StopTransactionReason::Timeout,
        StopReason::ReqEnergyTransferRejected => StopTransactionReason::ReqEnergyTransferRejected,
        StopReason::EvseDisabled => StopTransactionReason::EVSEDisabled,
    }
}

fn pause_reason_to(reason: PauseReason) -> PauseChargingEVSEReasonEnum {
    match reason {
        PauseReason::UserPause => PauseChargingEVSEReasonEnum::UserPause,
        PauseReason::Error => PauseChargingEVSEReasonEnum::Error,
        PauseReason::NoEnergy => PauseChargingEVSEReasonEnum::NoEnergy,
    }
}

fn evse_state_to(state: EvseState) -> EvEvseState {
    match state {
        EvseState::Unplugged => EvEvseState::Unplugged,
        EvseState::WaitForAuth => EvEvseState::WaitForAuth,
        EvseState::WaitForEnergy => EvEvseState::WaitForEnergy,
        EvseState::PrepareCharging => EvEvseState::PrepareCharging,
        EvseState::PausedEv => EvEvseState::PausedEV,
        EvseState::PausedEvse => EvEvseState::PausedEVSE,
        EvseState::Charging => EvEvseState::Charging,
        EvseState::Finished => EvEvseState::Finished,
        EvseState::Disabled => EvEvseState::Disabled,
    }
}

fn empty_session_event(uuid: String, event: SessionEventEnum) -> EvSessionEvent {
    EvSessionEvent {
        authorization_event: None,
        charging_paused_evse: None,
        charging_state_changed_event: None,
        connector_id: Some(1),
        event,
        session_finished: None,
        session_started: None,
        source: None,
        timestamp: now_rfc3339(),
        transaction_finished: None,
        transaction_started: None,
        uuid,
    }
}

/// A session event with the context object its consumers dereference.
pub fn session_event(event: SessionEvent<ProvidedIdToken>, meter: MeterSnapshot) -> EvSessionEvent {
    let uuid = event.uuid;
    match event.event {
        Event::Enabled(source) => {
            let mut e = empty_session_event(uuid, SessionEventEnum::Enabled);
            e.source = Some(enable_source_to(source));
            e
        }
        Event::Disabled(source) => {
            let mut e = empty_session_event(uuid, SessionEventEnum::Disabled);
            e.source = Some(enable_source_to(source));
            e
        }
        Event::SessionStarted {
            reason,
            id_tag,
            reservation_id,
        } => {
            let mut e = empty_session_event(uuid, SessionEventEnum::SessionStarted);
            e.session_started = Some(SessionStarted {
                id_tag,
                logging_path: None,
                meter_value: powermeter(meter),
                reason: match reason {
                    StartReason::EvConnected => StartSessionReason::EVConnected,
                    StartReason::Authorized => StartSessionReason::Authorized,
                },
                reservation_id,
                signed_meter_value: None,
            });
            e
        }
        Event::AuthRequired => empty_session_event(uuid, SessionEventEnum::AuthRequired),
        Event::Authorized => {
            let mut e = empty_session_event(uuid, SessionEventEnum::Authorized);
            e.authorization_event = Some(AuthorizationEvent {
                meter_value: powermeter(meter),
            });
            e
        }
        Event::Deauthorized => {
            let mut e = empty_session_event(uuid, SessionEventEnum::Deauthorized);
            e.authorization_event = Some(AuthorizationEvent {
                meter_value: powermeter(meter),
            });
            e
        }
        Event::TransactionStarted {
            id_tag,
            reservation_id,
        } => {
            let mut e = empty_session_event(uuid, SessionEventEnum::TransactionStarted);
            e.transaction_started = Some(TransactionStarted {
                id_tag,
                meter_value: powermeter(meter),
                reservation_id,
                signed_meter_value: None,
            });
            e
        }
        Event::PrepareCharging => empty_session_event(uuid, SessionEventEnum::PrepareCharging),
        Event::ChargingStarted => {
            let mut e = empty_session_event(uuid, SessionEventEnum::ChargingStarted);
            e.charging_state_changed_event = Some(ChargingStateChangedEvent {
                meter_value: powermeter(meter),
            });
            e
        }
        Event::ChargingPausedEv => {
            let mut e = empty_session_event(uuid, SessionEventEnum::ChargingPausedEV);
            e.charging_state_changed_event = Some(ChargingStateChangedEvent {
                meter_value: powermeter(meter),
            });
            e
        }
        Event::ChargingPausedEvse(reasons) => {
            let mut e = empty_session_event(uuid, SessionEventEnum::ChargingPausedEVSE);
            e.charging_state_changed_event = Some(ChargingStateChangedEvent {
                meter_value: powermeter(meter),
            });
            e.charging_paused_evse = Some(ChargingPausedEVSEReasons {
                reasons: reasons.into_iter().map(pause_reason_to).collect(),
            });
            e
        }
        Event::StoppingCharging => empty_session_event(uuid, SessionEventEnum::StoppingCharging),
        Event::ChargingFinished => empty_session_event(uuid, SessionEventEnum::ChargingFinished),
        Event::TransactionFinished { reason, id_tag } => {
            transaction_finished(uuid, reason, id_tag, meter)
        }
        Event::SessionFinished => session_finished(uuid, meter),
        Event::ReservationStart => empty_session_event(uuid, SessionEventEnum::ReservationStart),
        Event::ReservationEnd => empty_session_event(uuid, SessionEventEnum::ReservationEnd),
        Event::PluginTimeout => empty_session_event(uuid, SessionEventEnum::PluginTimeout),
    }
}

pub fn transaction_finished(
    uuid: String,
    reason: StopReason,
    id_tag: Option<ProvidedIdToken>,
    meter: MeterSnapshot,
) -> EvSessionEvent {
    let mut e = empty_session_event(uuid, SessionEventEnum::TransactionFinished);
    e.transaction_finished = Some(TransactionFinished {
        id_tag,
        meter_value: powermeter(meter),
        reason: Some(stop_reason_to(reason)),
        signed_meter_value: None,
        start_signed_meter_value: None,
    });
    e
}

pub fn session_finished(uuid: String, meter: MeterSnapshot) -> EvSessionEvent {
    let mut e = empty_session_event(uuid, SessionEventEnum::SessionFinished);
    e.session_finished = Some(SessionFinished {
        meter_value: powermeter(meter),
    });
    e
}

/// The energy tree leaf: one import entry valid from now, no export, no
/// setpoints.
pub fn energy_flow_request(uuid: &str, source: &str, req: &EnergyRequest) -> EnergyFlowRequest {
    let number = |value: f64| NumberWithSource {
        source: source.to_owned(),
        value,
    };
    let integer = |value: i64| IntegerWithSource {
        source: source.to_owned(),
        value,
    };
    let phases = i64::from(req.phases);
    let limits = LimitsReq {
        ac_max_current_a: Some(number(req.max_current_a)),
        ac_max_phase_count: Some(integer(phases)),
        ac_min_current_a: Some(number(req.min_current_a)),
        ac_min_phase_count: Some(integer(phases)),
        ac_number_of_active_phases: Some(phases),
        ac_supports_changing_phases_during_charging: Some(false),
        total_power_w: None,
    };
    let no_export = LimitsReq {
        ac_max_current_a: Some(number(0.0)),
        ac_max_phase_count: None,
        ac_min_current_a: None,
        ac_min_phase_count: None,
        ac_number_of_active_phases: None,
        ac_supports_changing_phases_during_charging: None,
        total_power_w: Some(number(0.0)),
    };
    let entry = |limits: LimitsReq| ScheduleReqEntry {
        conversion_efficiency: None,
        limits_to_leaves: limits.clone(),
        limits_to_root: limits,
        price_per_kwh: None,
        timestamp: now_rfc3339(),
    };
    EnergyFlowRequest {
        children: Vec::new(),
        energy_usage_leaves: None,
        energy_usage_root: Some(powermeter(req.meter)),
        evse_state: Some(evse_state_to(req.evse_state)),
        node_type: NodeType::Evse,
        optimizer_target: None,
        priority_request: Some(req.priority),
        schedule_export: vec![entry(no_export)],
        schedule_import: vec![entry(limits)],
        schedule_setpoints: Vec::new(),
        uuid: uuid.to_owned(),
    }
}
