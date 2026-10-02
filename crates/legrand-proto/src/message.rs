use std::str::FromStr;
use std::time::Duration;

use crate::StateCode;

/// Error code from `E:xxxx`. `0000` means no error; the meaning of other
/// codes is *unverified*.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ErrorCode(String);

impl ErrorCode {
    /// The code as sent, e.g. `0000`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True for an all-zero code (board healthy).
    pub fn is_clear(&self) -> bool {
        self.0.bytes().all(|b| b == b'0')
    }
}

/// Socket named in an end-of-session report.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Plug {
    /// `EVplug`: the Type 2 socket.
    Type2,
    /// `DOMplug`: the domestic (Mode 2) socket.
    Domestic,
    /// Any other identifier.
    Other(String),
}

/// End-of-session report the board sends on its own when a session ends:
/// `WT:h:m:s:CT:h:m:s:<plug>:<mean W peak>:<mean W off-peak>:`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionReport {
    /// Time plugged in without drawing power.
    pub waiting: Duration,
    /// Time drawing power.
    pub charging: Duration,
    /// Socket used.
    pub plug: Plug,
    /// Mean power during peak-tariff hours (French "HP"), watts.
    pub mean_power_peak_w: f32,
    /// Mean power during off-peak hours (French "HC"), watts.
    pub mean_power_offpeak_w: f32,
}

impl SessionReport {
    fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split(':');
        if parts.next()? != "WT" {
            return None;
        }
        let waiting = hms(&mut parts)?;
        if parts.next()? != "CT" {
            return None;
        }
        let charging = hms(&mut parts)?;
        let plug = match parts.next()? {
            "" => return None,
            "EVplug" => Plug::Type2,
            "DOMplug" => Plug::Domestic,
            other => Plug::Other(other.to_owned()),
        };
        let mean_power_peak_w = finite(parts.next()?)?;
        let mean_power_offpeak_w = finite(parts.next()?)?;
        // A trailing ':' leaves one empty field; anything more is unknown.
        match (parts.next(), parts.next()) {
            (None, _) | (Some(""), None) => {}
            _ => return None,
        }
        Some(SessionReport {
            waiting,
            charging,
            plug,
            mean_power_peak_w,
            mean_power_offpeak_w,
        })
    }
}

/// One line received from the board.
///
/// Keys are matched exactly (`CC` and `CCI` are different messages). A
/// known key with an unparseable value becomes [`Message::Malformed`]; an
/// unknown key becomes [`Message::Unknown`]. Neither is dropped, so traces
/// keep everything the board said.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// `RaspberryPi?`: watchdog ping, must be answered with
    /// [`crate::Command::PiModeOk`].
    PingRequest,
    /// `SoftwareVersion:…`, firmware version (e.g. `V01;18;10`).
    SoftwareVersion(String),
    /// `HardwareVersion:…`.
    HardwareVersion(String),
    /// `SerialNumber:…`.
    SerialNumber(String),
    /// `Reference:…`, Legrand catalogue reference (e.g. `058001`).
    Reference(String),
    /// `WeekYearProduction:…`.
    WeekYearProduction(String),
    /// `Side:n`, side of a two-sided unit (format *unverified*).
    Side(u8),
    /// `State:X`.
    State(StateCode),
    /// `E:xxxx`.
    Error(ErrorCode),
    /// `CC:n`, configured Type 2 current limit, amps.
    CurrentLimit(u8),
    /// `CCTIC:n`, limit derived from the Linky TIC, amps (format
    /// *unverified*).
    TicCurrentLimit(u8),
    /// `CCI:x`, measured current, amps. Per-phase meaning on three-phase
    /// boards *unverified*.
    Current(f32),
    /// `Volt:x`, measured voltage, volts.
    Voltage(f32),
    /// `Freq:x`, grid frequency, hertz.
    Frequency(f32),
    /// `Ener:x`, energy counter. Unit and lifetime-vs-session semantics
    /// *unverified*.
    Energy(f32),
    /// `FCS:…`, "charge complete" (vehicle stopped drawing); payload
    /// *unverified*.
    ChargeComplete(String),
    /// `FM:…`, see [`crate::FmMode`]. Seen values: `1`, `1F`.
    FmMode(String),
    /// `BT:0|1`, Bluetooth module off/on.
    Bluetooth(bool),
    /// `Slp:0|1`, deep sleep off/on.
    Sleep(bool),
    /// `TICTestB:n`, TIC test result: baud rate, `0` when no TIC.
    TicTestBaud(u32),
    /// `TICTestC:…`, TIC test status (e.g. `Init`).
    TicTestStatus(String),
    /// `WT:…:CT:…`, end-of-session report.
    SessionReport(SessionReport),
    /// Known key, unparseable value.
    Malformed {
        /// The key, e.g. `State`.
        key: &'static str,
        /// The raw value after the first `:`.
        value: String,
    },
    /// Line with no known key, kept verbatim.
    Unknown(String),
}

impl Message {
    /// Parses one line (terminators and surrounding whitespace ignored).
    pub fn parse(line: &str) -> Message {
        let line = line.trim();
        if line == "RaspberryPi?" {
            return Message::PingRequest;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Message::Unknown(line.to_owned());
        };
        let value = value.trim();
        match key {
            "SoftwareVersion" => text("SoftwareVersion", value, Message::SoftwareVersion),
            "HardwareVersion" => text("HardwareVersion", value, Message::HardwareVersion),
            "SerialNumber" => text("SerialNumber", value, Message::SerialNumber),
            "Reference" => text("Reference", value, Message::Reference),
            "WeekYearProduction" => text("WeekYearProduction", value, Message::WeekYearProduction),
            "Side" => number("Side", value, Message::Side),
            "State" => {
                StateCode::parse(value).map_or_else(|| malformed("State", value), Message::State)
            }
            "E" => text("E", value, |code| Message::Error(ErrorCode(code))),
            "CC" => number("CC", value, Message::CurrentLimit),
            "CCTIC" => number("CCTIC", value, Message::TicCurrentLimit),
            "CCI" => real("CCI", value, Message::Current),
            "Volt" => real("Volt", value, Message::Voltage),
            "Freq" => real("Freq", value, Message::Frequency),
            "Ener" => real("Ener", value, Message::Energy),
            "FCS" => Message::ChargeComplete(value.to_owned()),
            "FM" => text("FM", value, Message::FmMode),
            "BT" => flag("BT", value, Message::Bluetooth),
            "Slp" => flag("Slp", value, Message::Sleep),
            "TICTestB" => number("TICTestB", value, Message::TicTestBaud),
            "TICTestC" => text("TICTestC", value, Message::TicTestStatus),
            "WT" => SessionReport::parse(line)
                .map_or_else(|| malformed("WT", value), Message::SessionReport),
            _ => Message::Unknown(line.to_owned()),
        }
    }
}

fn malformed(key: &'static str, value: &str) -> Message {
    Message::Malformed {
        key,
        value: value.to_owned(),
    }
}

fn text(key: &'static str, value: &str, wrap: impl FnOnce(String) -> Message) -> Message {
    if value.is_empty() {
        malformed(key, value)
    } else {
        wrap(value.to_owned())
    }
}

fn number<T: FromStr>(key: &'static str, value: &str, wrap: impl FnOnce(T) -> Message) -> Message {
    value.parse().map_or_else(|_| malformed(key, value), wrap)
}

fn real(key: &'static str, value: &str, wrap: impl FnOnce(f32) -> Message) -> Message {
    finite(value).map_or_else(|| malformed(key, value), wrap)
}

fn flag(key: &'static str, value: &str, wrap: impl FnOnce(bool) -> Message) -> Message {
    match value {
        "0" => wrap(false),
        "1" => wrap(true),
        _ => malformed(key, value),
    }
}

fn finite(value: &str) -> Option<f32> {
    value.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn hms<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Option<Duration> {
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let s: u64 = parts.next()?.parse().ok()?;
    if m >= 60 || s >= 60 {
        return None;
    }
    let secs = h.checked_mul(3600)?.checked_add(m * 60 + s)?;
    Some(Duration::from_secs(secs))
}
