//! Keystroke and mouse encoding helpers shared by the terminal UI.

use gpui::{Keystroke, Modifiers, ScrollDelta};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

#[derive(Clone, Copy)]
pub enum MouseEventKind {
    Press,
    Move { dragging: bool },
    Release,
    Wheel { delta: ScrollDelta },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCellPos {
    pub row: u16,
    pub col: u16,
}

pub fn encode_terminal_input(keystroke: &Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    if keystroke.modifiers.function || keystroke.key.is_empty() {
        return None;
    }

    let mut bytes = if keystroke.modifiers.control {
        vec![encode_control_char(&keystroke.key)?]
    } else {
        match keystroke.key.as_str() {
            "enter" => b"\r".to_vec(),
            "backspace" => vec![0x7f],
            "tab" if keystroke.modifiers.shift => b"\x1b[Z".to_vec(),
            "tab" => b"\t".to_vec(),
            "escape" => vec![0x1b],
            "up" => {
                if application_cursor {
                    b"\x1bOA".to_vec()
                } else {
                    b"\x1b[A".to_vec()
                }
            }
            "down" => {
                if application_cursor {
                    b"\x1bOB".to_vec()
                } else {
                    b"\x1b[B".to_vec()
                }
            }
            "right" => {
                if application_cursor {
                    b"\x1bOC".to_vec()
                } else {
                    b"\x1b[C".to_vec()
                }
            }
            "left" => {
                if application_cursor {
                    b"\x1bOD".to_vec()
                } else {
                    b"\x1b[D".to_vec()
                }
            }
            "home" => {
                if application_cursor {
                    b"\x1bOH".to_vec()
                } else {
                    b"\x1b[H".to_vec()
                }
            }
            "end" => {
                if application_cursor {
                    b"\x1bOF".to_vec()
                } else {
                    b"\x1b[F".to_vec()
                }
            }
            "insert" => b"\x1b[2~".to_vec(),
            "delete" => b"\x1b[3~".to_vec(),
            "pageup" => b"\x1b[5~".to_vec(),
            "pagedown" => b"\x1b[6~".to_vec(),
            "space" => b" ".to_vec(),
            _ => keystroke.key_char.as_ref()?.as_bytes().to_vec(),
        }
    };

    if keystroke.modifiers.alt {
        let mut prefixed = Vec::with_capacity(bytes.len() + 1);
        prefixed.push(0x1b);
        prefixed.extend(bytes);
        bytes = prefixed;
    }

    Some(bytes)
}

pub fn encode_control_char(key: &str) -> Option<u8> {
    if key.len() == 1 {
        let ch = key.as_bytes()[0];
        return match ch {
            b'a'..=b'z' => Some(ch & 0x1f),
            b'2' | b'@' => Some(0),
            b'3' | b'[' => Some(27),
            b'4' | b'\\' => Some(28),
            b'5' | b']' => Some(29),
            b'6' | b'^' => Some(30),
            b'7' | b'_' | b'/' => Some(31),
            _ => None,
        };
    }

    match key {
        "space" => Some(0),
        "enter" => Some(b'\r'),
        "backspace" => Some(0x7f),
        _ => None,
    }
}

pub fn encode_mouse_report(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    kind: MouseEventKind,
    pos: TerminalCellPos,
    modifiers: Modifiers,
) -> Option<Vec<u8>> {
    if mode == MouseProtocolMode::None {
        return None;
    }

    let mut button_code = modifier_bits(modifiers);
    let terminator = match kind {
        MouseEventKind::Press => {
            if mode == MouseProtocolMode::Press
                || mode == MouseProtocolMode::PressRelease
                || mode == MouseProtocolMode::ButtonMotion
                || mode == MouseProtocolMode::AnyMotion
            {
                button_code += 0;
                'M'
            } else {
                return None;
            }
        }
        MouseEventKind::Release => {
            if mode == MouseProtocolMode::PressRelease
                || mode == MouseProtocolMode::ButtonMotion
                || mode == MouseProtocolMode::AnyMotion
            {
                button_code += 3;
                'm'
            } else {
                return None;
            }
        }
        MouseEventKind::Move { dragging } => match mode {
            MouseProtocolMode::ButtonMotion if dragging => {
                button_code += 32;
                'M'
            }
            MouseProtocolMode::AnyMotion => {
                button_code += 35;
                'M'
            }
            _ => return None,
        },
        MouseEventKind::Wheel { delta } => {
            let direction = match delta {
                ScrollDelta::Lines(delta) => delta.y,
                ScrollDelta::Pixels(delta) => {
                    let value: f32 = delta.y.into();
                    value
                }
            };
            if direction < 0.0 {
                button_code += 64;
            } else if direction > 0.0 {
                button_code += 65;
            } else {
                return None;
            }
            'M'
        }
    };

    let x = pos.col as u32 + 1;
    let y = pos.row as u32 + 1;

    match encoding {
        MouseProtocolEncoding::Sgr => {
            Some(format!("\x1b[<{};{};{}{}", button_code, x, y, terminator).into_bytes())
        }
        MouseProtocolEncoding::Default => Some(vec![
            0x1b,
            b'[',
            b'M',
            (button_code + 32) as u8,
            (x + 32) as u8,
            (y + 32) as u8,
        ]),
        MouseProtocolEncoding::Utf8 => {
            let mut bytes = b"\x1b[M".to_vec();
            bytes.extend(char::from_u32(button_code + 32)?.to_string().into_bytes());
            bytes.extend(char::from_u32(x + 32)?.to_string().into_bytes());
            bytes.extend(char::from_u32(y + 32)?.to_string().into_bytes());
            Some(bytes)
        }
    }
}

pub fn modifier_bits(modifiers: Modifiers) -> u32 {
    let mut bits = 0;
    if modifiers.shift {
        bits += 4;
    }
    if modifiers.alt {
        bits += 8;
    }
    if modifiers.control {
        bits += 16;
    }
    bits
}
