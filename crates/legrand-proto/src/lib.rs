//! Serial protocol of the Legrand Green'Up wallbox power board.
//!
//! The power board (an ATmega2560 running Legrand firmware) talks to the
//! communication kit over USB serial at 115200 baud: ASCII lines, commands
//! terminated by `\r`. This crate is the pure, I/O-free half of that link:
//!
//! - [`Command`]: everything we are willing to send, encoded for the wire;
//! - [`Message`]: one line received from the board, parsed by exact key;
//! - [`StateCode`]: the board's own state letters (not IEC 61851 letters);
//! - [`LineFramer`]: raw byte chunks from the port into complete lines.
//!
//! # Commands that are deliberately missing
//!
//! `Z` (reboots the board into its AVR bootloader), `Reset`, `Test`
//! (factory mode), `SOK` (deep sleep) and arbitrary raw strings have no
//! representation here, so nothing built on this crate can send them.
//!
//! Protocol knowledge comes from greenup-link (MIT, see `NOTICE`) and its
//! reverse-engineered `docs/PROTOCOLE_SERIE_LEGRAND.md`; the functioning
//! modes ([`FunctioningMode`], `FM2` eco-start) come from BolzaW's reading
//! of the ATmega firmware (greenup-link issue #9). Meanings marked
//! *unverified* still need confirming on hardware (`docs/plan.md`, Phase 1).

mod command;
mod framer;
mod message;
mod state;

pub use command::{Command, Current, CurrentOutOfRange, FunctioningMode, Query};
pub use framer::LineFramer;
pub use message::{ErrorCode, Message, Plug, SessionReport};
pub use state::StateCode;
