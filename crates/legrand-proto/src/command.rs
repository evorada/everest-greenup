use std::fmt;

/// Maximum charge current for the Type 2 socket, as sent in `CC:NN`.
///
/// Construction is range-checked so an out-of-range value can never reach
/// the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Current(u8);

impl Current {
    /// IEC 61851 minimum. Whether the board accepts 6..=9 A is
    /// *unverified*: greenup-link only ever sends 10..=32.
    pub const MIN: Current = Current(6);
    /// Rating of both the 7 kW single-phase and the 22 kW three-phase boards.
    pub const MAX: Current = Current(32);

    /// Range-checked constructor (`MIN..=MAX`).
    pub fn new(amps: u8) -> Result<Self, CurrentOutOfRange> {
        if (Self::MIN.0..=Self::MAX.0).contains(&amps) {
            Ok(Current(amps))
        } else {
            Err(CurrentOutOfRange(amps))
        }
    }

    /// The current in amps.
    pub fn amps(self) -> u8 {
        self.0
    }
}

/// Rejected [`Current`] value, in amps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentOutOfRange(pub u8);

impl fmt::Display for CurrentOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "charge current {} A outside {}..={} A",
            self.0,
            Current::MIN.0,
            Current::MAX.0
        )
    }
}

impl std::error::Error for CurrentOutOfRange {}

/// Value for `FM:`. Only `1` is known: greenup-link forces it whenever the
/// board reports anything other than `1`/`1F`. Meaning *unverified*
/// (upstream issue BolzaW/greenup-link#9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FmMode {
    /// `FM:1`.
    One,
}

/// Read-only queries; the board answers with the matching [`crate::Message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Query {
    /// `SoftwareVersion?`, firmware version (e.g. `V01;18;10`).
    SoftwareVersion,
    /// `HardwareVersion?`.
    HardwareVersion,
    /// `SerialNumber?`.
    SerialNumber,
    /// `Reference?`, the Legrand catalogue reference.
    Reference,
    /// `WeekYearProduction?`.
    WeekYearProduction,
    /// `Side?`, which side of a two-sided unit this board drives.
    Side,
    /// `State?`.
    State,
    /// `E?`, current error code (`E:0000` when healthy).
    Error,
    /// `CC?`, configured Type 2 current limit.
    CurrentLimit,
    /// `CCTIC?`, current limit derived from the Linky TIC.
    TicCurrentLimit,
    /// `FM?`.
    FmMode,
    /// `BT?`, Bluetooth module on/off.
    Bluetooth,
}

impl Query {
    fn as_str(self) -> &'static str {
        match self {
            Query::SoftwareVersion => "SoftwareVersion?",
            Query::HardwareVersion => "HardwareVersion?",
            Query::SerialNumber => "SerialNumber?",
            Query::Reference => "Reference?",
            Query::WeekYearProduction => "WeekYearProduction?",
            Query::Side => "Side?",
            Query::State => "State?",
            Query::Error => "E?",
            Query::CurrentLimit => "CC?",
            Query::TicCurrentLimit => "CCTIC?",
            Query::FmMode => "FM?",
            Query::Bluetooth => "BT?",
        }
    }
}

/// A command for the power board.
///
/// [`fmt::Display`] gives the text without terminator (for traces);
/// [`Command::encode`] gives the bytes to write, `\r`-terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// `RaspberryPiModeOK`: handshake after opening the port, and the
    /// mandatory answer to every `RaspberryPi?` ping.
    PiModeOk,
    /// A read-only query.
    Query(Query),
    /// `CC:NN`: Type 2 maximum charge current.
    SetCurrent(Current),
    /// `T2COK` (allow) / `T2CNOK` (block) charging on the Type 2 socket.
    Type2Charging(bool),
    /// `2PCOK` (allow) / `2PCNOK` (block) charging on the domestic socket.
    DomesticCharging(bool),
    /// `BTOK` / `BTNOK`: Bluetooth module on/off. Off stops the Legrand
    /// phone app from overriding our orders.
    Bluetooth(bool),
    /// `TICTM:1` / `TICTM:0`: Linky TIC detection test mode on/off.
    TicTestMode(bool),
    /// `FM:<mode>`.
    SetFmMode(FmMode),
    /// `SNOK`: wake the board from deep sleep (state `Y`).
    Wake,
    /// `Unlock`: release the Type 2 cable lock.
    Unlock,
}

impl Command {
    /// Bytes to write to the port, `\r`-terminated.
    pub fn encode(&self) -> Vec<u8> {
        format!("{self}\r").into_bytes()
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::PiModeOk => f.write_str("RaspberryPiModeOK"),
            Command::Query(q) => f.write_str(q.as_str()),
            Command::SetCurrent(c) => write!(f, "CC:{:02}", c.amps()),
            Command::Type2Charging(true) => f.write_str("T2COK"),
            Command::Type2Charging(false) => f.write_str("T2CNOK"),
            Command::DomesticCharging(true) => f.write_str("2PCOK"),
            Command::DomesticCharging(false) => f.write_str("2PCNOK"),
            Command::Bluetooth(true) => f.write_str("BTOK"),
            Command::Bluetooth(false) => f.write_str("BTNOK"),
            Command::TicTestMode(on) => write!(f, "TICTM:{}", u8::from(*on)),
            Command::SetFmMode(FmMode::One) => f.write_str("FM:1"),
            Command::Wake => f.write_str("SNOK"),
            Command::Unlock => f.write_str("Unlock"),
        }
    }
}
