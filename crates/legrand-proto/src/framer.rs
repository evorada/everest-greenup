/// Splits raw byte chunks read from the port into complete lines.
///
/// Both `\r` and `\n` end a line and empty lines are dropped, so `\r\n`,
/// `\n` and bare `\r` endings all work. Bytes are decoded per line,
/// lossily: one corrupt byte damages only its own line, never its
/// neighbours. A line longer than the cap is discarded up to its next
/// terminator and counted in [`LineFramer::overflows`].
#[derive(Debug, Clone)]
pub struct LineFramer {
    buf: Vec<u8>,
    max_line: usize,
    discarding: bool,
    overflows: u64,
}

impl LineFramer {
    /// Longest line kept, in bytes. The longest known message (the
    /// end-of-session report) is about 45 bytes.
    pub const DEFAULT_MAX_LINE: usize = 256;

    /// Framer with [`Self::DEFAULT_MAX_LINE`].
    pub fn new() -> Self {
        Self::with_max_line(Self::DEFAULT_MAX_LINE)
    }

    /// Framer with a custom cap (at least 1 byte).
    pub fn with_max_line(max_line: usize) -> Self {
        LineFramer {
            buf: Vec::new(),
            max_line: max_line.max(1),
            discarding: false,
            overflows: 0,
        }
    }

    /// Feeds one chunk; returns the lines it completed, in order, without
    /// their terminators.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in bytes {
            if b == b'\r' || b == b'\n' {
                if !self.discarding && !self.buf.is_empty() {
                    lines.push(String::from_utf8_lossy(&self.buf).into_owned());
                }
                self.buf.clear();
                self.discarding = false;
            } else if self.discarding {
                // Rest of an over-long line.
            } else if self.buf.len() >= self.max_line {
                self.buf.clear();
                self.discarding = true;
                self.overflows += 1;
            } else {
                self.buf.push(b);
            }
        }
        lines
    }

    /// Number of over-long lines discarded so far.
    pub fn overflows(&self) -> u64 {
        self.overflows
    }
}

impl Default for LineFramer {
    fn default() -> Self {
        Self::new()
    }
}
