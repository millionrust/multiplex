//! Recent pane output, kept so an agent can wait for what a command prints after it was sent.

use std::collections::VecDeque;

/// How much raw output each pane keeps while the server runs.
pub const OUTPUT_LOG_BYTES: usize = 256 * 1024;

/// The last [`OUTPUT_LOG_BYTES`] a pane received, and how many it has received in all. The
/// total is the cursor an agent waits from: output after cursor `n` is everything received
/// once `n` bytes had been.
#[derive(Debug, Default)]
pub struct OutputLog {
    bytes: VecDeque<u8>,
    total: u64,
}

pub struct OutputSince {
    pub bytes: Vec<u8>,
    /// Some of the output after the cursor was dropped, either from the log or by `max_bytes`.
    pub truncated: bool,
}

impl OutputLog {
    pub fn record(&mut self, data: &[u8]) {
        self.total = self.total.saturating_add(data.len() as u64);
        if data.len() >= OUTPUT_LOG_BYTES {
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - OUTPUT_LOG_BYTES..]);
            return;
        }
        let overflow = (self.bytes.len() + data.len()).saturating_sub(OUTPUT_LOG_BYTES);
        self.bytes.drain(..overflow);
        self.bytes.extend(data);
    }

    pub fn cursor(&self) -> u64 {
        self.total
    }

    /// Output received after `cursor`, keeping its last `max_bytes`.
    pub fn since(&self, cursor: u64, max_bytes: usize) -> OutputSince {
        let first_kept = self.total - self.bytes.len() as u64;
        let start = cursor.clamp(first_kept, self.total);
        let mut truncated = cursor < first_kept;
        let mut offset = (start - first_kept) as usize;
        let available = self.bytes.len() - offset;
        if available > max_bytes {
            offset += available - max_bytes;
            truncated = true;
        }
        OutputSince {
            bytes: self.bytes.range(offset..).copied().collect(),
            truncated,
        }
    }
}

/// Terminal output as plain text: escape sequences and control characters other than line
/// breaks and tabs removed, and a carriage return that is not part of a line break read as one.
pub fn strip_terminal_controls(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | '^' | '_' | 'X') => {
                    while let Some(next) = chars.next() {
                        if next == '\x07' {
                            break;
                        }
                        if next == '\x1b' {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                // Character set selection takes one more byte.
                Some('(' | ')' | '*' | '+' | '#' | '%') => {
                    chars.next();
                }
                _ => {}
            },
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    output.push('\n');
                }
            }
            '\n' | '\t' => output.push(character),
            character if character.is_control() => {}
            character => output.push(character),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_after_a_cursor_is_returned_and_old_output_is_dropped() {
        let mut log = OutputLog::default();
        log.record(b"hello ");
        let cursor = log.cursor();
        log.record(b"world");
        let since = log.since(cursor, 1024);
        assert_eq!(since.bytes, b"world");
        assert!(!since.truncated);
        assert_eq!(log.since(log.cursor(), 1024).bytes, b"");

        let limited = log.since(0, 3);
        assert_eq!(limited.bytes, b"rld");
        assert!(limited.truncated);

        log.record(&vec![b'x'; OUTPUT_LOG_BYTES]);
        let dropped = log.since(cursor, usize::MAX);
        assert!(dropped.truncated);
        assert_eq!(dropped.bytes.len(), OUTPUT_LOG_BYTES);
        assert_eq!(log.cursor(), 11 + OUTPUT_LOG_BYTES as u64);
    }

    #[test]
    fn control_sequences_are_stripped() {
        let raw = b"\x1b[1;32mok\x1b[0m\r\n\x1b]0;title\x07done\x1b(B\rprogress 2\x08!";
        assert_eq!(strip_terminal_controls(raw), "ok\ndone\nprogress 2!");
        assert_eq!(
            strip_terminal_controls(b"\x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\"),
            "link"
        );
    }
}
