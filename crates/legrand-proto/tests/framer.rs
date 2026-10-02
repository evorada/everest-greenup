//! Line framing of raw serial reads.

use legrand_proto::LineFramer;

const STREAM: &[u8] = b"State:B\r\nVolt:231.5\nCCI:15.9\rRaspberryPi?\r\n\r\nE:0000\r\n";
const LINES: [&str; 5] = [
    "State:B",
    "Volt:231.5",
    "CCI:15.9",
    "RaspberryPi?",
    "E:0000",
];

#[test]
fn crlf_lf_and_bare_cr_all_end_lines_and_empty_lines_are_dropped() {
    assert_eq!(LineFramer::new().push(STREAM), LINES);
}

#[test]
fn chunk_boundaries_do_not_change_the_lines() {
    for split in 0..=STREAM.len() {
        let mut framer = LineFramer::new();
        let mut lines = framer.push(&STREAM[..split]);
        lines.extend(framer.push(&STREAM[split..]));
        assert_eq!(lines, LINES, "split at {split}");
    }
    let mut framer = LineFramer::new();
    let byte_by_byte: Vec<String> = STREAM.iter().flat_map(|b| framer.push(&[*b])).collect();
    assert_eq!(byte_by_byte, LINES);
}

#[test]
fn partial_line_waits_for_its_terminator() {
    let mut framer = LineFramer::new();
    assert!(framer.push(b"Sta").is_empty());
    assert_eq!(framer.push(b"te:C\n"), ["State:C"]);
}

#[test]
fn invalid_utf8_only_damages_its_own_line() {
    // greenup-link dropped the whole read chunk on one bad byte.
    let lines = LineFramer::new().push(b"State:A\r\n\xffVolt:230\r\nCC:16\r\n");
    assert_eq!(lines, ["State:A", "\u{FFFD}Volt:230", "CC:16"]);
}

#[test]
fn over_long_line_is_discarded_up_to_its_terminator() {
    let mut framer = LineFramer::with_max_line(8);
    let lines = framer.push(b"0123456789ABCDEF\r\n01234567\r\nState:A\r\n");
    assert_eq!(lines, ["01234567", "State:A"]);
    assert_eq!(framer.overflows(), 1);
}
