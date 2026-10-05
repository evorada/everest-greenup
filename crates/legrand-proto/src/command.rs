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

/// The board's functioning mode: value of `FM?` / `FM:n`.
///
/// Meanings come from BolzaW's reading of the ATmega firmware 18.04
/// (greenup-link `docs/PROTOCOLE_SERIE_LEGRAND.md` §7, issue #9). The
/// driver only ever sends [`DirectCharge`](Self::DirectCharge); the other
/// modes are here so the board's native mode can be read back and, if the
/// board is ever handed back to Legrand's software, restored. The board
/// never sends `1F` (an early greenup-link guess).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctioningMode {
    /// `1`, "Direct Charge (Permanent)": charge as soon as a vehicle is
    /// plugged in, no internal conditions. The mode this project needs.
    DirectCharge,
    /// `2`, "Remote controls (Auto-heures)": charge driven by the dry
    /// contact input or the TIC off-peak flags.
    RemoteControls,
    /// `3`, "Smart meter (TIC Linky)": not implemented in firmware 18.04.
    SmartMeter,
    /// `4`, "Programming (Planning)": the board's internal calendar.
    Programming,
    /// `5`, "Modbus (DLM)": load management by an external manager over
    /// RS485.
    Modbus,
    /// `6`, "OCPP": the board expects orders from an OCPP server. Changes
    /// the board's state machine (no TIC detection, a blocking 30 s RFID
    /// wait, different `Unlock` handling).
    Ocpp,
}

impl FunctioningMode {
    /// Mode from its wire number, `None` outside `1..=6`.
    pub fn from_number(n: u8) -> Option<Self> {
        Some(match n {
            1 => FunctioningMode::DirectCharge,
            2 => FunctioningMode::RemoteControls,
            3 => FunctioningMode::SmartMeter,
            4 => FunctioningMode::Programming,
            5 => FunctioningMode::Modbus,
            6 => FunctioningMode::Ocpp,
            _ => return None,
        })
    }

    /// The wire number, `1..=6`.
    pub fn number(self) -> u8 {
        match self {
            FunctioningMode::DirectCharge => 1,
            FunctioningMode::RemoteControls => 2,
            FunctioningMode::SmartMeter => 3,
            FunctioningMode::Programming => 4,
            FunctioningMode::Modbus => 5,
            FunctioningMode::Ocpp => 6,
        }
    }
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
    /// `FM?`, see [`FunctioningMode`].
    FunctioningMode,
    /// `FM2?`, eco-start on/off, see [`Command::EcoStart`].
    EcoStart,
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
            Query::FunctioningMode => "FM?",
            Query::EcoStart => "FM2?",
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
    /// `FM:n`: the board's functioning mode. The driver sends only
    /// [`FunctioningMode::DirectCharge`].
    SetFunctioningMode(FunctioningMode),
    /// `FM2:1` / `FM2:0`: eco-start ("Éco-démarrage") on/off. When on (the
    /// same as DIP switch 2 on the board), the board withholds charging
    /// until the Linky TIC flags say off-peak hours, which the Pi cannot
    /// observe. Must be off for this project.
    EcoStart(bool),
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
            Command::SetFunctioningMode(mode) => write!(f, "FM:{}", mode.number()),
            Command::EcoStart(on) => write!(f, "FM2:{}", u8::from(*on)),
            Command::Wake => f.write_str("SNOK"),
            Command::Unlock => f.write_str("Unlock"),
        }
    }
}
