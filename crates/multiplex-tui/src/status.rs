//! Slate status semantics for the terminal UI.
//!
//! The TUI draws inside the user's terminal, whose palette the user chose, so it keeps the
//! named ANSI colors in the roles Slate gives them (green done, blue busy and accent, yellow
//! attention, red error) instead of forcing RGB onto an unknown background. Every status also
//! carries its Slate glyph shape, so state survives `NO_COLOR` and monochrome terminals.

use multiplex_ui_contract::{DesignTokens, StatusKind, ThemeKind};
use ratatui::style::Color;

use crate::model::LoadState;

/// The Slate accent in the terminal palette.
pub const ACCENT: Color = Color::Blue;

/// The glyph for a status, from the token contract's shape.
pub fn glyph(kind: StatusKind) -> char {
    match DesignTokens::new(ThemeKind::System).status(kind).shape {
        "filled_circle" => '\u{25CF}',
        "ring_spinner" => '\u{25D4}',
        "hollow_circle" => '\u{25CB}',
        "diamond" => '\u{25C6}',
        "octagon" => '\u{25A0}',
        "dashed_circle" => '\u{25CC}',
        "hollow_diamond" => '\u{25C7}',
        _ => '\u{25A1}',
    }
}

/// The terminal palette color in the role Slate gives the status.
pub fn color(kind: StatusKind) -> Color {
    match kind {
        StatusKind::Done => Color::Green,
        StatusKind::Busy => ACCENT,
        StatusKind::Attention | StatusKind::Orphaned => Color::Yellow,
        StatusKind::Error | StatusKind::PermissionDenied => Color::Red,
        StatusKind::Idle | StatusKind::Offline => Color::DarkGray,
    }
}

pub fn load_state(state: LoadState) -> StatusKind {
    match state {
        LoadState::Ready | LoadState::Empty => StatusKind::Done,
        LoadState::Starting | LoadState::Loading => StatusKind::Busy,
        LoadState::Partial | LoadState::RecoveryRequired => StatusKind::Attention,
        LoadState::Unavailable => StatusKind::Error,
    }
}

/// A session lifecycle code as the fleet source reports it.
pub fn session_state(code: &str) -> StatusKind {
    match code {
        "live" | "running_app_attached" => StatusKind::Done,
        "validating" | "starting" | "provisioning" | "attaching" | "replaying" | "stopping" => {
            StatusKind::Busy
        }
        "recording_paused" => StatusKind::Attention,
        "offline" => StatusKind::Offline,
        "orphaned" => StatusKind::Orphaned,
        "permission_denied" => StatusKind::PermissionDenied,
        "gap" | "incompatible" | "failed" => StatusKind::Error,
        _ => StatusKind::Idle,
    }
}

/// An activity code as the fleet source reports it.
pub fn activity(code: &str) -> StatusKind {
    match code {
        "busy" => StatusKind::Busy,
        "needs_input" => StatusKind::Attention,
        "done" => StatusKind::Done,
        "failed" => StatusKind::Error,
        _ => StatusKind::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [StatusKind; 8] = [
        StatusKind::Attention,
        StatusKind::Busy,
        StatusKind::Done,
        StatusKind::Error,
        StatusKind::Idle,
        StatusKind::Offline,
        StatusKind::Orphaned,
        StatusKind::PermissionDenied,
    ];

    #[test]
    fn every_status_has_its_own_single_width_glyph() {
        let glyphs = KINDS.map(glyph);
        for (index, value) in glyphs.iter().enumerate() {
            assert!(
                !glyphs[index + 1..].contains(value),
                "{value} is shared by two statuses"
            );
            assert_eq!(
                ratatui::text::Span::raw(value.to_string()).width(),
                1,
                "{value} must take one cell"
            );
        }
    }

    #[test]
    fn fleet_codes_map_to_slate_statuses() {
        assert_eq!(session_state("live"), StatusKind::Done);
        assert_eq!(
            session_state("permission_denied"),
            StatusKind::PermissionDenied
        );
        assert_eq!(session_state("offline"), StatusKind::Offline);
        assert_eq!(session_state("draft"), StatusKind::Idle);
        assert_eq!(activity("needs_input"), StatusKind::Attention);
        assert_eq!(activity("unknown"), StatusKind::Idle);
        assert_eq!(load_state(LoadState::Unavailable), StatusKind::Error);
        assert_eq!(color(StatusKind::Busy), ACCENT);
    }
}
