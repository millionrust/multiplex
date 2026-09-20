//! Key caps show a shortcut next to its action.

use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div};

use crate::theme::{ActiveTheme, SlateStyled};

/// The keyboard layout convention to spell a shortcut in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyPlatform {
    /// Modifier glyphs with no separators: `⌘⇧B`.
    Mac,
    /// Spelled-out modifiers joined with `+`: `Ctrl+Shift+B`.
    Other,
}

impl KeyPlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

/// Formats a GPUI keystroke such as `cmd-shift-b` or `ctrl-alt-right`.
///
/// Chords separated by spaces are formatted one by one and joined with a
/// space.
pub fn format_keystroke(keystroke: &str, platform: KeyPlatform) -> String {
    keystroke
        .split_whitespace()
        .map(|chord| format_chord(chord, platform))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_chord(chord: &str, platform: KeyPlatform) -> String {
    let parts: Vec<&str> = chord.split('-').filter(|part| !part.is_empty()).collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return String::new();
    };
    // Modifiers print in the platform's canonical order, not the typed order.
    let order = ["ctrl", "alt", "shift", "cmd"];
    let mut sorted: Vec<&str> = order
        .iter()
        .copied()
        .filter(|name| modifiers.iter().any(|m| normalize(m) == *name))
        .collect();
    if platform == KeyPlatform::Other {
        sorted.sort_by_key(|name| {
            ["ctrl", "cmd", "alt", "shift"]
                .iter()
                .position(|o| o == name)
        });
    }
    let key = key_label(key, platform);
    match platform {
        KeyPlatform::Mac => {
            let mut out: String = sorted
                .iter()
                .map(|name| match *name {
                    "ctrl" => "⌃",
                    "alt" => "⌥",
                    "shift" => "⇧",
                    _ => "⌘",
                })
                .collect();
            out.push_str(&key);
            out
        }
        KeyPlatform::Other => {
            let mut out: Vec<String> = sorted
                .iter()
                .map(|name| {
                    match *name {
                        "ctrl" | "cmd" => "Ctrl",
                        "alt" => "Alt",
                        _ => "Shift",
                    }
                    .to_string()
                })
                .collect();
            out.dedup();
            out.push(key);
            out.join("+")
        }
    }
}

fn normalize(modifier: &str) -> &str {
    match modifier {
        "control" => "ctrl",
        "option" => "alt",
        "super" | "win" | "platform" | "meta" => "cmd",
        other => other,
    }
}

fn key_label(key: &str, platform: KeyPlatform) -> String {
    let named = match key {
        "left" => Some("←"),
        "right" => Some("→"),
        "up" => Some("↑"),
        "down" => Some("↓"),
        "enter" => Some(if platform == KeyPlatform::Mac {
            "↩"
        } else {
            "Enter"
        }),
        "escape" => Some("Esc"),
        "backspace" => Some(if platform == KeyPlatform::Mac {
            "⌫"
        } else {
            "Backspace"
        }),
        "tab" => Some("Tab"),
        "space" => Some("Space"),
        _ => None,
    };
    match named {
        Some(label) => label.to_string(),
        None => key.to_uppercase(),
    }
}

/// A shortcut drawn as a small key cap.
#[derive(IntoElement)]
pub struct KeyCap {
    keystroke: SharedString,
    platform: KeyPlatform,
}

impl KeyCap {
    /// `keystroke` uses GPUI syntax, such as `cmd-k`.
    pub fn new(keystroke: impl Into<SharedString>) -> Self {
        Self {
            keystroke: keystroke.into(),
            platform: KeyPlatform::current(),
        }
    }

    /// Spells the shortcut for a specific platform, for documentation.
    pub fn platform(mut self, platform: KeyPlatform) -> Self {
        self.platform = platform;
        self
    }
}

impl RenderOnce for KeyCap {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(m.control_small - m.space[2])
            .px(m.space_fine)
            .rounded(m.radius_control)
            .border(m.hairline)
            .border_color(c.border)
            .text_color(c.text_muted)
            .type_style(theme.typography.micro)
            .font_family(theme.typography.ui_family.clone())
            .whitespace_nowrap()
            .child(format_keystroke(&self.keystroke, self.platform))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_shortcuts_use_glyphs_in_canonical_order() {
        assert_eq!(format_keystroke("cmd-k", KeyPlatform::Mac), "⌘K");
        assert_eq!(format_keystroke("cmd-shift-b", KeyPlatform::Mac), "⇧⌘B");
        assert_eq!(format_keystroke("alt-cmd-right", KeyPlatform::Mac), "⌥⌘→");
    }

    #[test]
    fn other_platforms_spell_out_modifiers() {
        assert_eq!(format_keystroke("cmd-k", KeyPlatform::Other), "Ctrl+K");
        assert_eq!(
            format_keystroke("cmd-shift-b", KeyPlatform::Other),
            "Ctrl+Shift+B"
        );
        assert_eq!(format_keystroke("ctrl-cmd-d", KeyPlatform::Other), "Ctrl+D");
    }

    #[test]
    fn chords_are_joined_with_spaces() {
        assert_eq!(format_keystroke("cmd-k cmd-s", KeyPlatform::Mac), "⌘K ⌘S");
    }
}
