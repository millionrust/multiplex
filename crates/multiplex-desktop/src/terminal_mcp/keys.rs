//! Named keys an agent sends, such as `Enter`, `Ctrl+C`, `C-d`, `^Z`, `Shift+Tab`, `Up` or
//! `F5`. They are encoded when sent, because cursor keys follow the mode the program asked for.

use crate::ui::keys::{TerminalModifiers, encode_terminal_key};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedKey {
    /// The key as [`encode_terminal_key`] names it, or `f1` to `f12`.
    key: String,
    /// What a printable key types, with its case kept.
    text: Option<String>,
    modifiers: TerminalModifiers,
}

const NAMED_KEYS: [(&str, &str); 28] = [
    ("enter", "enter"),
    ("return", "enter"),
    ("ret", "enter"),
    ("cr", "enter"),
    ("tab", "tab"),
    ("escape", "escape"),
    ("esc", "escape"),
    ("backspace", "backspace"),
    ("bs", "backspace"),
    ("delete", "delete"),
    ("del", "delete"),
    ("insert", "insert"),
    ("ins", "insert"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("home", "home"),
    ("end", "end"),
    ("pageup", "pageup"),
    ("page_up", "pageup"),
    ("pgup", "pageup"),
    ("pagedown", "pagedown"),
    ("page_down", "pagedown"),
    ("pgdn", "pagedown"),
    ("space", "space"),
    ("spc", "space"),
    ("minus", "-"),
];

impl ParsedKey {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("a key name is empty".to_owned());
        }
        // `^C` is Control-C.
        if let Some(rest) = spec.strip_prefix('^')
            && rest.chars().count() == 1
        {
            let mut key = Self::single(rest)?;
            key.modifiers.control = true;
            key.text = Some(key.key.clone());
            return Ok(key);
        }
        let parts = split_modifiers(spec);
        let (name, modifier_names) = parts
            .split_last()
            .ok_or_else(|| format!("unknown key `{spec}`"))?;
        let mut modifiers = TerminalModifiers::default();
        for modifier in modifier_names {
            match modifier.to_ascii_lowercase().as_str() {
                "c" | "ctrl" | "control" => modifiers.control = true,
                "m" | "alt" | "meta" | "opt" | "option" => modifiers.alt = true,
                "s" | "shift" => modifiers.shift = true,
                _ => return Err(format!("unknown modifier `{modifier}` in `{spec}`")),
            }
        }
        let mut key = if name.chars().count() == 1 {
            Self::single(name)?
        } else {
            Self::named(name).ok_or_else(|| format!("unknown key `{spec}`"))?
        };
        key.modifiers = TerminalModifiers {
            shift: modifiers.shift || key.modifiers.shift,
            control: modifiers.control,
            alt: modifiers.alt,
        };
        if let Some(text) = key.text.as_mut() {
            // Control sends the same code whatever the case, so `C-c` and `Ctrl+C` are equal.
            if modifiers.control {
                *text = key.key.clone();
            } else if modifiers.shift {
                *text = text.to_uppercase();
            }
        }
        if key.modifiers.control
            && key.function_number().is_none()
            && crate::ui::keys::encode_control_char(&key.key).is_none()
            && !matches!(
                key.key.as_str(),
                "up" | "down" | "left" | "right" | "home" | "end"
            )
        {
            return Err(format!("`{spec}` has no control code"));
        }
        Ok(key)
    }

    fn single(text: &str) -> Result<Self, String> {
        let lower = text.to_lowercase();
        Ok(Self {
            key: lower,
            text: Some(text.to_owned()),
            modifiers: TerminalModifiers::default(),
        })
    }

    fn named(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if let Some(number) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
            && (1..=12).contains(&number)
        {
            return Some(Self {
                key: lower,
                text: None,
                modifiers: TerminalModifiers::default(),
            });
        }
        let (_, key) = NAMED_KEYS.iter().find(|(alias, _)| *alias == lower)?;
        Some(Self {
            key: (*key).to_owned(),
            text: (key.chars().count() == 1).then(|| (*key).to_owned()),
            modifiers: TerminalModifiers::default(),
        })
    }

    fn function_number(&self) -> Option<u8> {
        self.key
            .strip_prefix('f')
            .and_then(|number| number.parse::<u8>().ok())
            .filter(|number| (1..=12).contains(number))
    }

    /// The bytes the key sends, following the program's cursor-key mode.
    pub fn encode(&self, application_cursor: bool) -> Option<Vec<u8>> {
        if let Some(number) = self.function_number() {
            return Some(function_key(number, self.modifiers));
        }
        encode_terminal_key(
            &self.key,
            self.text.as_deref(),
            self.modifiers,
            application_cursor,
        )
    }
}

/// `Ctrl+Shift+Tab` and `C-S-tab` both split into modifiers and a key; a `+` or `-` that is
/// itself the key, as in `Ctrl+-` or `-`, is kept.
fn split_modifiers(spec: &str) -> Vec<&str> {
    let separator = if spec.contains('+') { '+' } else { '-' };
    let mut parts: Vec<&str> = spec.split(separator).collect();
    if parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
        if let Some(last) = parts.last_mut()
            && last.is_empty()
        {
            *last = if separator == '+' { "+" } else { "-" };
        } else {
            parts.push(if separator == '+' { "+" } else { "-" });
        }
    }
    parts.retain(|part| !part.is_empty());
    parts
}

fn function_key(number: u8, modifiers: TerminalModifiers) -> Vec<u8> {
    let modifier = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    match number {
        1..=4 => {
            let final_byte = b"PQRS"[usize::from(number - 1)] as char;
            if modifier > 1 {
                format!("\x1b[1;{modifier}{final_byte}").into_bytes()
            } else {
                format!("\x1bO{final_byte}").into_bytes()
            }
        }
        _ => {
            let code = match number {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                _ => 24,
            };
            if modifier > 1 {
                format!("\x1b[{code};{modifier}~").into_bytes()
            } else {
                format!("\x1b[{code}~").into_bytes()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(spec: &str, application_cursor: bool) -> Vec<u8> {
        ParsedKey::parse(spec)
            .unwrap_or_else(|error| panic!("{spec}: {error}"))
            .encode(application_cursor)
            .unwrap_or_else(|| panic!("{spec} encodes"))
    }

    #[test]
    fn control_keys_are_spelled_every_common_way() {
        for spec in ["Ctrl+C", "ctrl-c", "C-c", "^C", "control+c"] {
            assert_eq!(bytes(spec, false), vec![0x03], "{spec}");
        }
        assert_eq!(bytes("C-d", false), vec![0x04]);
        assert_eq!(bytes("Ctrl+[", false), vec![0x1b]);
    }

    #[test]
    fn named_keys_follow_the_cursor_key_mode() {
        assert_eq!(bytes("Enter", false), b"\r");
        assert_eq!(bytes("Return", false), b"\r");
        assert_eq!(bytes("Esc", false), b"\x1b");
        assert_eq!(bytes("Up", false), b"\x1b[A");
        assert_eq!(bytes("Up", true), b"\x1bOA");
        assert_eq!(bytes("Shift+Tab", false), b"\x1b[Z");
        assert_eq!(bytes("PageDown", false), b"\x1b[6~");
        assert_eq!(bytes("Ctrl+Left", false), b"\x1b[1;5D");
        assert_eq!(bytes("Alt+b", false), b"\x1bb");
        assert_eq!(bytes("M-x", false), b"\x1bx");
    }

    #[test]
    fn function_keys_use_xterm_sequences() {
        assert_eq!(bytes("F1", false), b"\x1bOP");
        assert_eq!(bytes("F5", false), b"\x1b[15~");
        assert_eq!(bytes("F12", false), b"\x1b[24~");
        assert_eq!(bytes("Shift+F5", false), b"\x1b[15;2~");
        assert_eq!(bytes("Ctrl+F1", false), b"\x1b[1;5P");
    }

    #[test]
    fn printable_keys_keep_their_case_and_separators_can_be_keys() {
        assert_eq!(bytes("q", false), b"q");
        assert_eq!(bytes("Q", false), b"Q");
        assert_eq!(bytes("-", false), b"-");
        assert_eq!(bytes("+", false), b"+");
        assert_eq!(bytes("Alt+-", false), b"\x1b-");
    }

    #[test]
    fn unknown_keys_and_modifiers_are_refused() {
        assert!(ParsedKey::parse("Hyper+x").is_err());
        assert!(ParsedKey::parse("Bogus").is_err());
        assert!(ParsedKey::parse("").is_err());
        assert!(ParsedKey::parse("F13").is_err());
        assert!(ParsedKey::parse("Ctrl+Tab").is_err());
    }
}
