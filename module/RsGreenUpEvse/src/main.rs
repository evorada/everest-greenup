//! RsGreenUpEvse: `evse_manager` and `energy` provider for Legrand
//! Green'Up wallboxes (058049 and the rest of the family).
//!
//! The power board runs its own IEC 61851 state machine and only takes
//! session-level orders, so this module replaces EvseManager rather than
//! sitting under it as a BSP. One instance per side (per power board, per
//! serial port). All the logic lives in the `evse-session`,
//! `legrand-driver` and `greenup-runtime` crates; this file is the
//! everestrs adapter.
#![allow(non_snake_case, non_camel_case_types)]

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

mod convert;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use everestrs::log;
use evse_session::{EnergyConfig, LimitRes, MeterSnapshot, SessionConfig, StopReason};
use generated::errors::evse_manager::{Error, EvseManagerError};
use generated::types::authorization::{AuthorizationStatus, ProvidedIdToken, ValidationResult};
use generated::types::energy::EnforcedLimits;
use generated::types::evse_board_support::{
    HardwareCapabilities, HardwareCapabilitiesAutoGenConnectorType,
};
use generated::types::evse_manager::{
    ChargeMode, Connector, ConnectorTypeEnum, EnableDisableSource, Evse, Limits,
    PlugAndChargeConfiguration, SetDerAvailableResult, StopTransactionRequest,
    UpdateAllowedEnergyTransferModesResult,
};
use generated::types::iso15118::EnergyTransferMode;
use generated::{
    Context, EnergyServiceSubscriber, EvseManagerServicePublisher, EvseManagerServiceSubscriber,
    Module, ModuleConfig, ModulePublisher, OnReadySubscriber,
};
use greenup_runtime::{Handle, Notification, Reply, Request, Runtime, RuntimeConfig};
use legrand_driver::DriverConfig;

type Token = ProvidedIdToken;

/// Everything derived from the manifest config, once.
struct Settings {
    evse_id: i64,
    energy: EnergyConfig,
    wait_for_external_ready: bool,
    runtime: RuntimeConfig,
    /// Our node's uuid in the energy tree; EnergyManager echoes it in
    /// `enforce_limits`.
    energy_uuid: String,
}

impl Settings {
    fn from_config(config: &ModuleConfig, module_id: String) -> Self {
        let phases = u8::try_from(config.phase_count.clamp(1, 3)).unwrap_or(3);
        let side = u8::try_from(config.side.clamp(0, 255)).unwrap_or(0);
        let evse_id = if config.evse_id > 0 {
            config.evse_id
        } else {
            i64::from(side.max(1))
        };
        let energy = EnergyConfig {
            max_current_a: config.max_current_a,
            min_current_a: config.min_current_a,
            phases,
            nominal_voltage_v: config.nominal_voltage_v,
        };
        let driver = DriverConfig {
            expected_side: (side > 0).then_some(side),
            domestic_socket: config.domestic_socket_enabled,
            phases,
            ..DriverConfig::default()
        };
        let mut runtime = RuntimeConfig::new(config.port.clone(), driver, energy);
        runtime.session = SessionConfig {
            resume_from_ended: config.resume_from_ended,
        };
        if !config.state_dir.is_empty() {
            runtime.state_file =
                Some(PathBuf::from(&config.state_dir).join(format!("evse{evse_id}.json")));
        }
        if !config.trace_file.is_empty() {
            runtime.trace_file = Some(PathBuf::from(&config.trace_file));
        }
        Settings {
            evse_id,
            energy,
            wait_for_external_ready: config.wait_for_external_ready,
            runtime,
            energy_uuid: module_id,
        }
    }
}

struct GreenUp {
    settings: Settings,
    handle: Mutex<Option<Arc<Handle<Token>>>>,
    runtime: Mutex<Option<Runtime<Token>>>,
}

impl GreenUp {
    fn handle(&self) -> Option<Arc<Handle<Token>>> {
        self.handle.lock().ok().and_then(|h| h.clone())
    }

    fn call(&self, request: Request<Token>) -> Reply {
        match self.handle() {
            Some(handle) => handle.call(request),
            None => {
                log::warn!("command before the runtime is up: {request:?}");
                Reply::Accepted(false)
            }
        }
    }

    fn accepted(&self, request: Request<Token>) -> bool {
        matches!(self.call(request), Reply::Accepted(true))
    }

    fn take_runtime(&self) -> Option<Runtime<Token>> {
        self.runtime.lock().ok().and_then(|mut r| r.take())
    }
}

fn report(what: &str, result: ::everestrs::Result<()>) {
    if let Err(e) = result {
        log::warn!("publishing {what} failed: {e:?}");
    }
}

fn inoperative(message: String) -> ::everestrs::ErrorType<Error> {
    let mut error: ::everestrs::ErrorType<Error> =
        Error::EvseManager(EvseManagerError::Inoperative).into();
    error.description = "Charging is not possible on this connector".to_owned();
    error.message = message;
    error
}

/// Turns runtime notifications into published variables and errors.
fn forward(
    rx: Receiver<Notification<Token>>,
    evse: EvseManagerServicePublisher,
    energy: generated::EnergyServicePublisher,
    energy_uuid: String,
    phases: u8,
) {
    let source = format!("{energy_uuid}/hardware");
    while let Ok(notification) = rx.recv() {
        match notification {
            Notification::Session { event, meter } => {
                let name = event.event.name();
                log::info!("session event {name} ({})", event.uuid);
                report(
                    name,
                    evse.session_event(convert::session_event(event, meter)),
                );
            }
            Notification::Meter(meter) => {
                report("powermeter", evse.powermeter(convert::powermeter(meter)));
            }
            Notification::Limits {
                max_current_a,
                phases,
            } => {
                report(
                    "limits",
                    evse.limits(Limits {
                        max_current: max_current_a,
                        nr_of_phases_available: i64::from(phases),
                        uuid: Some(energy_uuid.clone()),
                    }),
                );
            }
            Notification::EnergyRequest(request) => {
                let request = convert::energy_flow_request(&energy_uuid, &source, &request);
                report("energy_flow_request", energy.energy_flow_request(request));
            }
            Notification::FaultRaised(message) => {
                log::error!("Inoperative: {message}");
                evse.raise_error(inoperative(message));
            }
            Notification::FaultCleared => {
                log::info!("Inoperative cleared");
                evse.clear_error(Error::EvseManager(EvseManagerError::Inoperative));
            }
            Notification::RecoveredTransaction { uuid, meter } => {
                log::warn!("closing transaction {uuid} left open by the previous run");
                report(
                    "TransactionFinished",
                    evse.session_event(convert::transaction_finished(
                        uuid.clone(),
                        StopReason::PowerLoss,
                        None,
                        meter,
                    )),
                );
                report(
                    "SessionFinished",
                    evse.session_event(convert::session_finished(uuid, meter)),
                );
            }
        }
    }
    let _ = phases;
}

impl OnReadySubscriber for GreenUp {
    fn on_ready(&self, publishers: &ModulePublisher) {
        let energy = self.settings.energy;
        let evse = publishers.evse.clone();
        report(
            "hw_capabilities",
            evse.hw_capabilities(HardwareCapabilities {
                connector_type: HardwareCapabilitiesAutoGenConnectorType::IEC62196Type2Socket,
                max_current_a_export: 0.0,
                max_current_a_import: energy.max_current_a,
                max_phase_count_export: 1,
                max_phase_count_import: i64::from(energy.phases),
                max_plug_temperature_c: None,
                min_current_a_export: 0.0,
                min_current_a_import: energy.min_current_a,
                min_phase_count_export: 1,
                min_phase_count_import: 1,
                supports_changing_phases_during_charging: false,
                supports_cp_state_e: false,
            }),
        );
        let mode = if energy.phases == 1 {
            EnergyTransferMode::AC_single_phase_core
        } else {
            EnergyTransferMode::AC_three_phase_core
        };
        report(
            "supported_energy_transfer_modes",
            evse.supported_energy_transfer_modes(vec![mode]),
        );
        report(
            "selected_protocol",
            evse.selected_protocol("IEC61851-1".to_owned()),
        );
        report(
            "limits",
            evse.limits(Limits {
                max_current: 0.0,
                nr_of_phases_available: i64::from(energy.phases),
                uuid: Some(self.settings.energy_uuid.clone()),
            }),
        );
        report(
            "powermeter",
            evse.powermeter(convert::powermeter(MeterSnapshot::default())),
        );
        if self.settings.wait_for_external_ready {
            report(
                "waiting_for_external_ready",
                evse.waiting_for_external_ready(true),
            );
        } else {
            report("ready", evse.ready(true));
        }

        match Runtime::start(self.settings.runtime.clone()) {
            Ok(mut runtime) => {
                let handle = runtime.handle();
                if let Some(rx) = runtime.take_notifications() {
                    let energy_pub = publishers.energy_grid.clone();
                    let uuid = self.settings.energy_uuid.clone();
                    let phases = energy.phases;
                    let evse = evse.clone();
                    if let Err(e) = thread::Builder::new()
                        .name("greenup publish".to_owned())
                        .spawn(move || forward(rx, evse, energy_pub, uuid, phases))
                    {
                        log::error!("cannot start the publishing thread: {e}");
                    }
                }
                if let Ok(mut slot) = self.handle.lock() {
                    *slot = Some(handle.clone());
                }
                if let Ok(mut slot) = self.runtime.lock() {
                    *slot = Some(runtime);
                }
                handle.call(Request::Startup);
                log::info!(
                    "RsGreenUpEvse ready: port {}, evse id {}",
                    self.settings.runtime.port,
                    self.settings.evse_id
                );
            }
            Err(e) => {
                log::error!("cannot start the runtime: {e}");
                evse.raise_error(inoperative(format!("cannot start the board runtime: {e}")));
            }
        }
    }
}

impl EvseManagerServiceSubscriber for GreenUp {
    fn authorize_response(
        &self,
        _context: &Context,
        provided_token: ProvidedIdToken,
        validation_result: ValidationResult,
    ) -> ::everestrs::Result<()> {
        if validation_result.authorization_status == AuthorizationStatus::Accepted {
            self.call(Request::Authorize {
                token: provided_token,
                reservation_id: validation_result.reservation_id,
            });
        } else {
            log::info!(
                "authorization not accepted ({:?}), ignored",
                validation_result.authorization_status
            );
        }
        Ok(())
    }

    fn cancel_reservation(&self, _context: &Context) -> ::everestrs::Result<()> {
        self.call(Request::CancelReservation);
        Ok(())
    }

    fn enable_disable(
        &self,
        _context: &Context,
        cmd_source: EnableDisableSource,
        connector_id: i64,
    ) -> ::everestrs::Result<bool> {
        log::info!("enable_disable connector {connector_id}: {cmd_source:?}");
        Ok(
            self.accepted(Request::EnableDisable(convert::enable_source_from(
                &cmd_source,
            ))),
        )
    }

    fn external_ready_to_start_charging(&self, context: &Context) -> ::everestrs::Result<bool> {
        report("ready", context.publisher.evse.ready(true));
        Ok(true)
    }

    fn force_unlock(&self, _context: &Context, _connector_id: i64) -> ::everestrs::Result<bool> {
        Ok(self.accepted(Request::ForceUnlock))
    }

    fn get_evse(&self, _context: &Context) -> ::everestrs::Result<Evse> {
        Ok(Evse {
            connectors: vec![Connector {
                charge_mode: ChargeMode::AC,
                hlc_capable: false,
                id: 1,
                r#type: Some(ConnectorTypeEnum::sType2),
            }],
            evse_id: None,
            evse_id_din: None,
            id: self.settings.evse_id,
        })
    }

    fn pause_charging(&self, _context: &Context) -> ::everestrs::Result<bool> {
        Ok(self.accepted(Request::PauseCharging))
    }

    fn reserve(&self, _context: &Context, reservation_id: i64) -> ::everestrs::Result<bool> {
        Ok(self.accepted(Request::Reserve(reservation_id)))
    }

    fn resume_charging(&self, _context: &Context) -> ::everestrs::Result<bool> {
        Ok(self.accepted(Request::ResumeCharging))
    }

    fn set_der_available(
        &self,
        _context: &Context,
        _available: bool,
    ) -> ::everestrs::Result<SetDerAvailableResult> {
        Ok(SetDerAvailableResult::NoHlc)
    }

    fn set_plug_and_charge_configuration(
        &self,
        _context: &Context,
        _plug_and_charge_configuration: PlugAndChargeConfiguration,
    ) -> ::everestrs::Result<()> {
        Ok(())
    }

    fn stop_transaction(
        &self,
        _context: &Context,
        request: StopTransactionRequest,
    ) -> ::everestrs::Result<bool> {
        Ok(self.accepted(Request::StopTransaction {
            reason: convert::stop_reason_from(request.reason),
            id_tag: request.id_tag,
        }))
    }

    fn update_allowed_energy_transfer_modes(
        &self,
        _context: &Context,
        _allowed_energy_transfer_modes: Vec<EnergyTransferMode>,
    ) -> ::everestrs::Result<UpdateAllowedEnergyTransferModesResult> {
        Ok(UpdateAllowedEnergyTransferModesResult::NoHlc)
    }

    fn withdraw_authorization(&self, _context: &Context) -> ::everestrs::Result<()> {
        self.call(Request::WithdrawAuthorization);
        Ok(())
    }
}

impl EnergyServiceSubscriber for GreenUp {
    fn enforce_limits(&self, context: &Context, value: EnforcedLimits) -> ::everestrs::Result<()> {
        if value.uuid != self.settings.energy_uuid {
            return Ok(());
        }
        report(
            "enforced_limits",
            context.publisher.evse.enforced_limits(value.clone()),
        );
        let root = &value.limits_root_side;
        let valid_for = Duration::from_secs(u64::try_from(value.valid_for).unwrap_or(0));
        self.call(Request::EnforceLimits {
            limits: LimitRes {
                ac_max_current_a: root.ac_max_current_a.as_ref().map(|n| n.value),
                total_power_w: root.total_power_w.as_ref().map(|n| n.value),
                ac_max_phase_count: root.ac_max_phase_count.as_ref().map(|n| n.value),
            },
            valid_for,
        });
        Ok(())
    }
}

/// The manager starts every module with `--module <id>`; the id is unique
/// in the configuration, which makes it a good uuid for the energy tree.
fn module_id_from_args() -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--module" {
            return args.next();
        }
        if let Some(value) = arg.strip_prefix("--module=") {
            return Some(value.to_owned());
        }
    }
    None
}

#[everestrs::main]
fn main(module: &Module) {
    let config = module.get_config();
    let module_id =
        module_id_from_args().unwrap_or_else(|| format!("RsGreenUpEvse-side{}", config.side));
    let settings = Settings::from_config(&config, module_id);
    let class = Arc::new(GreenUp {
        settings,
        handle: Mutex::new(None),
        runtime: Mutex::new(None),
    });
    let _publishers = module.start(class.clone(), class.clone(), class.clone());

    // everestrs has no shutdown hook: block the board ourselves on SIGTERM.
    let term = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        if let Err(e) = signal_hook::flag::register(signal, term.clone()) {
            log::warn!("cannot register signal {signal}: {e}");
        }
    }
    while !term.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }
    log::info!("terminating: blocking the board and persisting state");
    if let Some(runtime) = class.take_runtime() {
        runtime.shutdown();
    }
    std::process::exit(0);
}
