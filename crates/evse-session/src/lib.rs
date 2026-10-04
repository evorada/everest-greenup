//! Session core for a wallbox that runs its own IEC 61851 state machine.
//!
//! EVerest's EvseManager drives a board at control-pilot level. The boards
//! this crate is written for (Legrand Green'Up and friends) only take
//! session-level orders: authorize, block, set the maximum current, unlock.
//! So this crate implements the layer EvseManager provides, the
//! `evse_manager` and `energy` interfaces, on top of a small
//! [`ChargerDriver`] contract instead of a BSP.
//!
//! Everything here is pure: no threads, no clocks, no I/O. Time comes in as
//! [`std::time::Instant`] arguments, decisions come out as [`Output`] values.
//! The EVerest module turns outputs into published variables and errors, and
//! turns interface commands into method calls.
//!
//! - [`Session`]: one connector's session, transaction and charging state;
//! - [`EnergyNode`]: the energy-tree leaf, limit enforcement and `CC`
//!   rate limiting;
//! - [`ChargerDriver`]: what a board driver must provide.
//!
//! The event grammar the session emits follows EvseManager's
//! (`modules/EVSE/EvseManager/docs/index.rst`), because Auth, OCPP and the
//! APIs are written against it. The tests pin it down.

mod driver;
mod energy;
mod session;
mod types;

pub use driver::{ChargerDriver, DriverAction, DriverEvent, MeterSnapshot, Poll};
pub use energy::{Decision, EnergyConfig, EnergyNode, LimitRes};
pub use session::{Output, Session, SessionConfig};
pub use types::{
    EnableSource, EnableSourceKind, EnableState, Event, EvseState, Fact, Hint, PauseReason,
    SessionEvent, StartReason, StopReason,
};
