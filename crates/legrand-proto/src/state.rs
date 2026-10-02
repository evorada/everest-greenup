use std::fmt;

/// State letter reported by the board in `State:X`.
///
/// These are Legrand's own letters, not IEC 61851 CP states (Legrand `C`
/// is "authorized, waiting for the vehicle", not IEC "charging").
/// Meanings come from greenup-link's UI (`src/index.html:491-573`); the
/// interpretation into charging facts belongs to the driver, which is
/// tuned against hardware traces (plan gate G8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateCode {
    /// Free / ready, no vehicle.
    A,
    /// Vehicle plugged, charging not authorized.
    B,
    /// Authorized, waiting for the vehicle to draw power.
    C,
    /// Charging (difference from `E` *unverified*).
    D,
    /// Charging (difference from `D` *unverified*).
    E,
    /// *Unverified*; greenup-link treats it like `A`.
    L,
    /// Stopping.
    W,
    /// Stopped: the board ended the session, vehicle still plugged.
    M,
    /// Deep sleep (entered with `SOK`, left with `SNOK`).
    Y,
    /// Error states `F`, `G`, `H`, `R`, `S`, `T`, `U`, `V`.
    Fault(char),
    /// Any other uppercase letter (not seen so far).
    Other(char),
}

impl StateCode {
    /// Letter to state; `None` unless `c` is an ASCII uppercase letter.
    pub fn from_letter(c: char) -> Option<Self> {
        Some(match c {
            'A' => StateCode::A,
            'B' => StateCode::B,
            'C' => StateCode::C,
            'D' => StateCode::D,
            'E' => StateCode::E,
            'L' => StateCode::L,
            'W' => StateCode::W,
            'M' => StateCode::M,
            'Y' => StateCode::Y,
            'F' | 'G' | 'H' | 'R' | 'S' | 'T' | 'U' | 'V' => StateCode::Fault(c),
            c if c.is_ascii_uppercase() => StateCode::Other(c),
            _ => return None,
        })
    }

    /// Parses the value of a `State:` line: exactly one letter.
    pub fn parse(value: &str) -> Option<Self> {
        let mut chars = value.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Self::from_letter(c),
            _ => None,
        }
    }

    /// The letter as sent by the board.
    pub fn letter(self) -> char {
        match self {
            StateCode::A => 'A',
            StateCode::B => 'B',
            StateCode::C => 'C',
            StateCode::D => 'D',
            StateCode::E => 'E',
            StateCode::L => 'L',
            StateCode::W => 'W',
            StateCode::M => 'M',
            StateCode::Y => 'Y',
            StateCode::Fault(c) | StateCode::Other(c) => c,
        }
    }
}

impl fmt::Display for StateCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.letter())
    }
}
