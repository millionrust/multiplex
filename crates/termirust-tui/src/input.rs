use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub const LEADER_TIMEOUT: Duration = Duration::from_millis(750);
pub const MAX_PASTE_BYTES: usize = 64 * 1024;
pub const PASTE_CONFIRM_BYTES: usize = 4 * 1024;
const LEADER_BYTE: u8 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiFocus {
    Fleet,
    Terminal,
    Leader,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractiveLease {
    ViewOnly,
    Interactive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputDecision {
    None,
    Send(Vec<u8>),
    Detach,
    ConfirmPaste { bytes: usize, multiline: bool },
    PasteRejected,
}

#[derive(Clone, Debug)]
pub struct TerminalInputModel {
    focus: TuiFocus,
    lease: InteractiveLease,
    leader_deadline: Option<Instant>,
    pending_paste: Option<Vec<u8>>,
}

impl Default for TerminalInputModel {
    fn default() -> Self {
        Self {
            focus: TuiFocus::Terminal,
            lease: InteractiveLease::ViewOnly,
            leader_deadline: None,
            pending_paste: None,
        }
    }
}

impl TerminalInputModel {
    pub const fn focus(&self) -> TuiFocus {
        self.focus
    }

    pub const fn lease(&self) -> InteractiveLease {
        self.lease
    }

    pub fn pending_paste(&self) -> Option<&[u8]> {
        self.pending_paste.as_deref()
    }

    pub fn leader_deadline(&self) -> Option<Instant> {
        self.leader_deadline
    }

    pub fn cancel_leader(&mut self) {
        if self.focus == TuiFocus::Leader {
            self.focus = TuiFocus::Terminal;
            self.leader_deadline = None;
        }
    }

    pub fn set_lease(&mut self, held: bool) {
        self.lease = if held {
            InteractiveLease::Interactive
        } else {
            self.leader_deadline = None;
            self.pending_paste = None;
            self.focus = TuiFocus::Terminal;
            InteractiveLease::ViewOnly
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent, now: Instant) -> InputDecision {
        if self.pending_paste.is_some() {
            return match key.code {
                KeyCode::Enter if self.lease == InteractiveLease::Interactive => {
                    self.focus = TuiFocus::Terminal;
                    InputDecision::Send(self.pending_paste.take().unwrap_or_default())
                }
                KeyCode::Esc => {
                    self.pending_paste = None;
                    self.focus = TuiFocus::Terminal;
                    InputDecision::None
                }
                _ => InputDecision::None,
            };
        }

        match self.focus {
            TuiFocus::Fleet => InputDecision::None,
            TuiFocus::Terminal if is_leader(key) => {
                self.focus = TuiFocus::Leader;
                self.leader_deadline = Some(now + LEADER_TIMEOUT);
                InputDecision::None
            }
            TuiFocus::Terminal => self.send_if_interactive(encode_key(key)),
            TuiFocus::Leader => {
                self.focus = TuiFocus::Terminal;
                self.leader_deadline = None;
                match key.code {
                    KeyCode::Esc => {
                        self.focus = TuiFocus::Fleet;
                        InputDecision::Detach
                    }
                    KeyCode::Char(' ') if key.modifiers.is_empty() => {
                        self.send_if_interactive(Some(vec![LEADER_BYTE]))
                    }
                    _ => {
                        let mut bytes = vec![LEADER_BYTE];
                        if let Some(key_bytes) = encode_key(key) {
                            bytes.extend(key_bytes);
                        }
                        self.send_if_interactive(Some(bytes))
                    }
                }
            }
        }
    }

    pub fn expire_leader(&mut self, now: Instant) -> InputDecision {
        if self.focus != TuiFocus::Leader
            || self.leader_deadline.is_none_or(|deadline| deadline > now)
        {
            return InputDecision::None;
        }
        self.focus = TuiFocus::Terminal;
        self.leader_deadline = None;
        self.send_if_interactive(Some(vec![LEADER_BYTE]))
    }

    pub fn handle_paste(&mut self, value: String, bracketed: bool) -> InputDecision {
        if self.lease != InteractiveLease::Interactive {
            return InputDecision::None;
        }
        let multiline = value.contains(['\r', '\n']);
        let mut bytes = value.into_bytes();
        if bracketed {
            let mut framed = Vec::with_capacity(bytes.len().saturating_add(12));
            framed.extend_from_slice(b"\x1b[200~");
            framed.append(&mut bytes);
            framed.extend_from_slice(b"\x1b[201~");
            bytes = framed;
        }
        if bytes.len() > MAX_PASTE_BYTES {
            return InputDecision::PasteRejected;
        }
        if multiline || bytes.len() > PASTE_CONFIRM_BYTES {
            let size = bytes.len();
            self.pending_paste = Some(bytes);
            self.focus = TuiFocus::Fleet;
            return InputDecision::ConfirmPaste {
                bytes: size,
                multiline,
            };
        }
        InputDecision::Send(bytes)
    }

    fn send_if_interactive(&self, bytes: Option<Vec<u8>>) -> InputDecision {
        if self.lease != InteractiveLease::Interactive {
            return InputDecision::None;
        }
        bytes.map_or(InputDecision::None, InputDecision::Send)
    }
}

pub fn is_leader(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(
            key.code,
            KeyCode::Char(' ') | KeyCode::Char('@') | KeyCode::Null
        )
}

pub fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let mut bytes = match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let lower = character.to_ascii_lowercase();
            if lower.is_ascii_lowercase() {
                vec![(lower as u8) - b'a' + 1]
            } else {
                match character {
                    ' ' | '@' => vec![0],
                    '[' => vec![0x1b],
                    '\\' => vec![0x1c],
                    ']' => vec![0x1d],
                    '^' => vec![0x1e],
                    '_' => vec![0x1f],
                    '?' => vec![0x7f],
                    _ => return None,
                }
            }
        }
        KeyCode::Char(character) => character.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::F(number) if (1..=4).contains(&number) => {
            vec![0x1b, b'O', b'P' + number - 1]
        }
        KeyCode::F(number) if (5..=12).contains(&number) => {
            let code = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(number - 5)];
            format!("\x1b[{code}~").into_bytes()
        }
        KeyCode::Null => vec![0],
        _ => return None,
    };
    if key.modifiers.contains(KeyModifiers::ALT) && !bytes.starts_with(b"\x1b") {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn terminal_keys_are_raw_and_leader_escape_detaches() {
        let now = Instant::now();
        let mut model = TerminalInputModel::default();
        model.set_lease(true);
        assert_eq!(
            model.handle_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), now),
            InputDecision::Send(vec![3])
        );
        assert_eq!(
            model.handle_key(key(KeyCode::Tab, KeyModifiers::NONE), now),
            InputDecision::Send(vec![b'\t'])
        );
        assert_eq!(
            model.handle_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL), now),
            InputDecision::None
        );
        assert_eq!(model.focus(), TuiFocus::Leader);
        assert_eq!(
            model.handle_key(key(KeyCode::Esc, KeyModifiers::NONE), now),
            InputDecision::Detach
        );
    }

    #[test]
    fn leader_space_timeout_and_unknown_key_preserve_literal_nul() {
        let now = Instant::now();
        let mut model = TerminalInputModel::default();
        model.set_lease(true);
        model.handle_key(key(KeyCode::Null, KeyModifiers::CONTROL), now);
        assert_eq!(
            model.handle_key(key(KeyCode::Char(' '), KeyModifiers::NONE), now),
            InputDecision::Send(vec![0])
        );
        model.handle_key(key(KeyCode::Null, KeyModifiers::CONTROL), now);
        assert_eq!(
            model.handle_key(key(KeyCode::Char('x'), KeyModifiers::NONE), now),
            InputDecision::Send(vec![0, b'x'])
        );
        model.handle_key(key(KeyCode::Null, KeyModifiers::CONTROL), now);
        assert_eq!(
            model.expire_leader(now + LEADER_TIMEOUT),
            InputDecision::Send(vec![0])
        );
    }

    #[test]
    fn paste_confirmation_is_bounded_and_lease_loss_discards_it() {
        let mut model = TerminalInputModel::default();
        model.set_lease(true);
        assert!(matches!(
            model.handle_paste("one\ntwo".into(), false),
            InputDecision::ConfirmPaste {
                multiline: true,
                ..
            }
        ));
        assert!(model.pending_paste().is_some());
        model.set_lease(false);
        assert!(model.pending_paste().is_none());
        model.set_lease(true);
        assert_eq!(
            model.handle_paste("x".repeat(MAX_PASTE_BYTES + 1), false),
            InputDecision::PasteRejected
        );
    }
}
