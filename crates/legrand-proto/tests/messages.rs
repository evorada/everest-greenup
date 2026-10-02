//! Parsing of lines received from the board. Sample lines come from
//! greenup-link's parser (`src/serial.rs:99-235`), its REST doc examples and
//! the session-report example in `docs/PROTOCOLE_SERIE_LEGRAND.md`.

use std::time::Duration;

use legrand_proto::{Message, Plug, SessionReport, StateCode};

fn malformed(key: &'static str, value: &str) -> Message {
    Message::Malformed {
        key,
        value: value.to_owned(),
    }
}

#[test]
fn ping_request() {
    assert_eq!(Message::parse("RaspberryPi?"), Message::PingRequest);
}

#[test]
fn identity_values_are_kept_verbatim() {
    let cases = [
        (
            "SoftwareVersion:V01;18;10",
            Message::SoftwareVersion("V01;18;10".into()),
        ),
        (
            "HardwareVersion:V01",
            Message::HardwareVersion("V01".into()),
        ),
        (
            "SerialNumber:12345678",
            Message::SerialNumber("12345678".into()),
        ),
        ("Reference:058001", Message::Reference("058001".into())),
        (
            "WeekYearProduction:42/20",
            Message::WeekYearProduction("42/20".into()),
        ),
    ];
    for (line, expected) in cases {
        assert_eq!(Message::parse(line), expected, "{line}");
    }
}

#[test]
fn state_letters() {
    let cases = [
        ("State:A", StateCode::A),
        ("State:C", StateCode::C),
        ("State:E", StateCode::E),
        ("State:L", StateCode::L),
        ("State:M", StateCode::M),
        ("State:W", StateCode::W),
        ("State:Y", StateCode::Y),
        ("State:F", StateCode::Fault('F')),
        ("State:V", StateCode::Fault('V')),
        ("State:Q", StateCode::Other('Q')),
    ];
    for (line, expected) in cases {
        assert_eq!(Message::parse(line), Message::State(expected), "{line}");
    }
}

#[test]
fn state_must_be_one_uppercase_letter() {
    assert_eq!(Message::parse("State:"), malformed("State", ""));
    assert_eq!(Message::parse("State:AB"), malformed("State", "AB"));
    assert_eq!(Message::parse("State:e"), malformed("State", "e"));
    assert_eq!(Message::parse("State:1"), malformed("State", "1"));
}

#[test]
fn error_code_clear_only_when_all_zeros() {
    let Message::Error(ok) = Message::parse("E:0000") else {
        panic!("E:0000 not parsed as error code");
    };
    assert!(ok.is_clear());
    let Message::Error(fault) = Message::parse("E:0040") else {
        panic!("E:0040 not parsed as error code");
    };
    assert!(!fault.is_clear());
    assert_eq!(fault.as_str(), "0040");
    assert_eq!(Message::parse("E:"), malformed("E", ""));
}

#[test]
fn keys_match_exactly() {
    // greenup-link's starts_with chain had to order CCI before CC; exact
    // keys make that impossible to get wrong.
    assert_eq!(Message::parse("CC:16"), Message::CurrentLimit(16));
    assert_eq!(Message::parse("CCI:15.6"), Message::Current(15.6));
    assert_eq!(Message::parse("CCTIC:32"), Message::TicCurrentLimit(32));
    assert_eq!(Message::parse("CC:15.6"), malformed("CC", "15.6"));
}

#[test]
fn measurements() {
    assert_eq!(Message::parse("Volt:232.0"), Message::Voltage(232.0));
    assert_eq!(Message::parse("Freq:50.0"), Message::Frequency(50.0));
    assert_eq!(Message::parse("Ener:12.5"), Message::Energy(12.5));
    assert_eq!(Message::parse("Volt:abc"), malformed("Volt", "abc"));
    assert_eq!(Message::parse("Volt:inf"), malformed("Volt", "inf"));
    assert_eq!(Message::parse("CCI:NaN"), malformed("CCI", "NaN"));
}

#[test]
fn flags_accept_only_0_and_1() {
    assert_eq!(Message::parse("BT:1"), Message::Bluetooth(true));
    assert_eq!(Message::parse("BT:0"), Message::Bluetooth(false));
    assert_eq!(Message::parse("Slp:1"), Message::Sleep(true));
    assert_eq!(Message::parse("BT:2"), malformed("BT", "2"));
}

#[test]
fn modes_and_reports() {
    assert_eq!(Message::parse("FM:1F"), Message::FmMode("1F".into()));
    assert_eq!(
        Message::parse("FCS:"),
        Message::ChargeComplete(String::new())
    );
    assert_eq!(Message::parse("TICTestB:9600"), Message::TicTestBaud(9600));
    assert_eq!(Message::parse("TICTestB:0"), Message::TicTestBaud(0));
    assert_eq!(
        Message::parse("TICTestC:Init"),
        Message::TicTestStatus("Init".into())
    );
    assert_eq!(Message::parse("Side:2"), Message::Side(2));
}

#[test]
fn session_report_from_protocol_doc_example() {
    assert_eq!(
        Message::parse("WT:0:0:5:CT:0:6:58:EVplug:4485.20:0.00:"),
        Message::SessionReport(SessionReport {
            waiting: Duration::from_secs(5),
            charging: Duration::from_secs(6 * 60 + 58),
            plug: Plug::Type2,
            mean_power_peak_w: 4485.2,
            mean_power_offpeak_w: 0.0,
        })
    );
}

#[test]
fn session_report_variants() {
    let Message::SessionReport(report) = Message::parse("WT:1:2:3:CT:4:5:6:DOMplug:2000:150")
    else {
        panic!("report without trailing colon not parsed");
    };
    assert_eq!(report.waiting, Duration::from_secs(3723));
    assert_eq!(report.charging, Duration::from_secs(4 * 3600 + 5 * 60 + 6));
    assert_eq!(report.plug, Plug::Domestic);

    let malformed_reports = [
        "WT:0:0:5:CT:0:6:58:EVplug:4485.20",    // off-peak power missing
        "WT:0:61:5:CT:0:6:58:EVplug:1:0:",      // 61 minutes
        "WT:0:0:5:XX:0:6:58:EVplug:1:0:",       // CT marker missing
        "WT:0:0:5:CT:0:6:58::1:0:",             // empty plug
        "WT:0:0:5:CT:0:6:58:EVplug:1:0:extra:", // trailing fields
        "WT:99999999999999999:0:0:CT:0:0:0:EVplug:1:0:", // overflow
    ];
    for line in malformed_reports {
        assert!(
            matches!(Message::parse(line), Message::Malformed { key: "WT", .. }),
            "{line}"
        );
    }
}

#[test]
fn unknown_lines_are_kept_verbatim() {
    assert_eq!(Message::parse("Hello"), Message::Unknown("Hello".into()));
    assert_eq!(
        Message::parse("Foo:bar"),
        Message::Unknown("Foo:bar".into())
    );
}

#[test]
fn surrounding_whitespace_and_terminators_are_ignored() {
    assert_eq!(Message::parse("  State:A \r"), Message::State(StateCode::A));
    assert_eq!(Message::parse("CC: 16"), Message::CurrentLimit(16));
}
