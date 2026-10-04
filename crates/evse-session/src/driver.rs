use std::time::Instant;

use crate::{Fact, Hint};

/// An order from the session to the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DriverAction {
    /// Allow (`true`) or block (`false`) charging on the vehicle socket.
    Authorize(bool),
    /// Maximum charge current per phase, amps.
    SetCurrent(u8),
    /// Release the cable lock.
    Unlock,
}

/// Something the board driver learned.
#[derive(Debug, Clone, PartialEq)]
pub enum DriverEvent {
    /// The board's state, reduced to a session fact. Sent on change only.
    Fact(Fact),
    /// The driver's aggregated fault: `Some(detail)` while anything is
    /// wrong (board fault, error code, link down, settings that will not
    /// stick, unauthorized delivery), `None` once everything is fine again.
    /// Sent on change only.
    Fault(Option<String>),
    /// Extra information that is not a state change.
    Hint(Hint),
    /// New measurements are available from [`ChargerDriver::meter`].
    MeterUpdated,
}

/// Latest measurements, in the units EVerest's `powermeter` type uses.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterSnapshot {
    /// Imported energy, watt-hours. Never decreases; survives restarts.
    pub energy_wh_import: f64,
    /// Instantaneous power, watts.
    pub power_w: f64,
    /// Phase voltage, volts, when known.
    pub voltage_v: Option<f64>,
    /// Grid frequency, hertz, when known.
    pub frequency_hz: Option<f64>,
    /// Current, amps, when known.
    pub current_a: Option<f64>,
}

/// What [`ChargerDriver::poll`] produced.
#[derive(Debug, Default, PartialEq)]
pub struct Poll {
    /// Bytes to write to the board, in order, already terminated.
    pub commands: Vec<Vec<u8>>,
    /// Events raised by timers (link down, orders that were ignored).
    pub events: Vec<DriverEvent>,
}

/// A board driver: pure logic between serial lines and session facts.
///
/// The runtime owns one per port. It feeds received lines through
/// [`on_line`](Self::on_line), forwards the session's
/// [`DriverAction`]s through [`apply`](Self::apply) and calls
/// [`poll`](Self::poll) regularly to get bytes to write and timer events.
pub trait ChargerDriver {
    /// The port was (re)opened. Starts the handshake.
    fn connected(&mut self, now: Instant);
    /// The port went away.
    fn disconnected(&mut self, now: Instant);
    /// One complete line from the board.
    fn on_line(&mut self, line: &str, now: Instant) -> Vec<DriverEvent>;
    /// The session wants something from the board.
    fn apply(&mut self, action: DriverAction, now: Instant);
    /// Commands due now and timer-driven events. Call every 50 ms or so.
    fn poll(&mut self, now: Instant) -> Poll;
    /// Latest measurements.
    fn meter(&self) -> MeterSnapshot;
}
