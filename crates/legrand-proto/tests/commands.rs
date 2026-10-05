//! Wire encodings. Expected strings are the ones greenup-link sends
//! (`src/serial.rs`, `src/api.rs`), so these tests double as parity checks.

use legrand_proto::{Command, Current, CurrentOutOfRange, FunctioningMode, Query};

fn wire(cmd: Command) -> String {
    // Lossy is fine: non-ASCII output would fail the comparisons anyway.
    String::from_utf8_lossy(&cmd.encode()).into_owned()
}

fn all_queries() -> Vec<Query> {
    use Query::*;
    let all = vec![
        SoftwareVersion,
        HardwareVersion,
        SerialNumber,
        Reference,
        WeekYearProduction,
        Side,
        State,
        Error,
        CurrentLimit,
        TicCurrentLimit,
        FunctioningMode,
        EcoStart,
        Bluetooth,
    ];
    // Exhaustiveness guard: a new variant fails to compile here until it is
    // added to the list above.
    for q in &all {
        match q {
            SoftwareVersion | HardwareVersion | SerialNumber | Reference | WeekYearProduction
            | Side | State | Error | CurrentLimit | TicCurrentLimit | FunctioningMode
            | EcoStart | Bluetooth => {}
        }
    }
    all
}

/// Every command the public API can construct.
fn all_commands() -> Vec<Command> {
    let mut all = vec![Command::PiModeOk, Command::Wake, Command::Unlock];
    all.extend(all_queries().into_iter().map(Command::Query));
    all.extend(all_modes().into_iter().map(Command::SetFunctioningMode));
    let currents: Vec<Current> = (Current::MIN.amps()..=Current::MAX.amps())
        .filter_map(|a| Current::new(a).ok())
        .collect();
    assert_eq!(
        currents.len(),
        27,
        "every amp value in MIN..=MAX is constructible"
    );
    all.extend(currents.into_iter().map(Command::SetCurrent));
    for on in [true, false] {
        all.extend([
            Command::Type2Charging(on),
            Command::DomesticCharging(on),
            Command::Bluetooth(on),
            Command::TicTestMode(on),
            Command::EcoStart(on),
        ]);
    }
    all
}

fn all_modes() -> Vec<FunctioningMode> {
    use FunctioningMode::*;
    let all = vec![
        DirectCharge,
        RemoteControls,
        SmartMeter,
        Programming,
        Modbus,
        Ocpp,
    ];
    for m in &all {
        match m {
            DirectCharge | RemoteControls | SmartMeter | Programming | Modbus | Ocpp => {}
        }
    }
    all
}

#[test]
fn encodings_match_greenup_link() {
    let cases = [
        (Command::PiModeOk, "RaspberryPiModeOK\r"),
        (Command::Query(Query::SoftwareVersion), "SoftwareVersion?\r"),
        (Command::Query(Query::HardwareVersion), "HardwareVersion?\r"),
        (Command::Query(Query::SerialNumber), "SerialNumber?\r"),
        (Command::Query(Query::Reference), "Reference?\r"),
        (
            Command::Query(Query::WeekYearProduction),
            "WeekYearProduction?\r",
        ),
        (Command::Query(Query::Side), "Side?\r"),
        (Command::Query(Query::State), "State?\r"),
        (Command::Query(Query::Error), "E?\r"),
        (Command::Query(Query::CurrentLimit), "CC?\r"),
        (Command::Query(Query::TicCurrentLimit), "CCTIC?\r"),
        (Command::Query(Query::FunctioningMode), "FM?\r"),
        (Command::Query(Query::EcoStart), "FM2?\r"),
        (Command::Query(Query::Bluetooth), "BT?\r"),
        (Command::SetCurrent(Current::new(16).unwrap()), "CC:16\r"),
        (Command::SetCurrent(Current::MAX), "CC:32\r"),
        (Command::SetCurrent(Current::MIN), "CC:06\r"),
        (Command::Type2Charging(true), "T2COK\r"),
        (Command::Type2Charging(false), "T2CNOK\r"),
        (Command::DomesticCharging(true), "2PCOK\r"),
        (Command::DomesticCharging(false), "2PCNOK\r"),
        (Command::Bluetooth(true), "BTOK\r"),
        (Command::Bluetooth(false), "BTNOK\r"),
        (Command::TicTestMode(true), "TICTM:1\r"),
        (Command::TicTestMode(false), "TICTM:0\r"),
        (
            Command::SetFunctioningMode(FunctioningMode::DirectCharge),
            "FM:1\r",
        ),
        (Command::SetFunctioningMode(FunctioningMode::Ocpp), "FM:6\r"),
        (Command::EcoStart(false), "FM2:0\r"),
        (Command::EcoStart(true), "FM2:1\r"),
        (Command::Wake, "SNOK\r"),
        (Command::Unlock, "Unlock\r"),
    ];
    for (cmd, expected) in cases {
        assert_eq!(wire(cmd), expected, "{cmd:?}");
    }
}

#[test]
fn display_is_the_wire_text_without_terminator() {
    for cmd in all_commands() {
        assert_eq!(format!("{cmd}\r"), wire(cmd));
    }
}

#[test]
fn functioning_mode_numbers_round_trip() {
    for mode in all_modes() {
        assert_eq!(FunctioningMode::from_number(mode.number()), Some(mode));
    }
    assert_eq!(FunctioningMode::DirectCharge.number(), 1);
    assert_eq!(FunctioningMode::from_number(0), None);
    assert_eq!(FunctioningMode::from_number(7), None);
}

#[test]
fn current_rejects_values_outside_6_to_32_amps() {
    assert_eq!(Current::new(0), Err(CurrentOutOfRange(0)));
    assert_eq!(Current::new(5), Err(CurrentOutOfRange(5)));
    assert_eq!(Current::new(6).map(Current::amps), Ok(6));
    assert_eq!(Current::new(32).map(Current::amps), Ok(32));
    assert_eq!(Current::new(33), Err(CurrentOutOfRange(33)));
}

#[test]
fn no_constructible_command_is_a_dangerous_one() {
    // Z = reboot into bootloader, Reset, Test = factory mode, SOK = sleep.
    const FORBIDDEN: [&str; 4] = ["Z", "Reset", "Test", "SOK"];
    for cmd in all_commands() {
        let text = cmd.to_string();
        assert!(
            !FORBIDDEN.contains(&text.as_str()),
            "{cmd:?} encodes to {text}"
        );
        assert!(!text.contains(['\r', '\n']), "{cmd:?} embeds a terminator");
    }
}
