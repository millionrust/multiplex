//! Slate status indicators: every state is a glyph shape and a color, so it survives color
//! blindness, grayscale recordings, and high contrast.

use gpui::{Hsla, IntoElement, Styled as _, px, svg};
use multiplex_domain::{ActivityState, ArtifactState, HostedSessionState, SearchStatus};
use multiplex_ui_contract::StatusKind;

use super::theme;

/// The asset for a status shape named in the token contract.
pub fn glyph_path(shape: &str) -> &'static str {
    match shape {
        "filled_circle" => "icons/status/filled-circle.svg",
        "ring_spinner" => "icons/status/ring-spinner.svg",
        "hollow_circle" => "icons/status/hollow-circle.svg",
        "diamond" => "icons/status/diamond.svg",
        "octagon" => "icons/status/octagon.svg",
        "dashed_circle" => "icons/status/dashed-circle.svg",
        "hollow_diamond" => "icons/status/hollow-diamond.svg",
        _ => "icons/status/hollow-square.svg",
    }
}

pub fn status_color(kind: StatusKind) -> Hsla {
    theme::token_status_color(kind)
}

/// The glyph for a status, drawn in its status color at the token status size.
pub fn status_glyph(kind: StatusKind) -> impl IntoElement {
    let visual = theme::semantic_status(kind);
    svg()
        .path(glyph_path(visual.shape))
        .flex_none()
        .size(px(theme::ICON_SIZE_STATUS))
        .text_color(status_color(kind))
}

pub fn session_state_status(state: HostedSessionState) -> StatusKind {
    match state {
        HostedSessionState::Live | HostedSessionState::RunningAppAttached => StatusKind::Done,
        HostedSessionState::Validating
        | HostedSessionState::Starting
        | HostedSessionState::Provisioning
        | HostedSessionState::Attaching
        | HostedSessionState::Replaying
        | HostedSessionState::Stopping => StatusKind::Busy,
        HostedSessionState::RecordingPaused => StatusKind::Attention,
        HostedSessionState::Offline => StatusKind::Offline,
        HostedSessionState::Orphaned => StatusKind::Orphaned,
        HostedSessionState::PermissionDenied => StatusKind::PermissionDenied,
        HostedSessionState::Gap | HostedSessionState::Incompatible | HostedSessionState::Failed => {
            StatusKind::Error
        }
        HostedSessionState::Draft | HostedSessionState::Cancelled | HostedSessionState::Exited => {
            StatusKind::Idle
        }
    }
}

pub fn activity_status(state: ActivityState) -> StatusKind {
    match state {
        ActivityState::Unknown | ActivityState::Idle => StatusKind::Idle,
        ActivityState::Busy => StatusKind::Busy,
        ActivityState::NeedsInput => StatusKind::Attention,
        ActivityState::Done => StatusKind::Done,
        ActivityState::Failed => StatusKind::Error,
    }
}

pub fn search_status(status: SearchStatus) -> StatusKind {
    match status {
        SearchStatus::Attention => StatusKind::Attention,
        SearchStatus::Busy | SearchStatus::Running => StatusKind::Busy,
        SearchStatus::Done => StatusKind::Done,
        SearchStatus::Idle | SearchStatus::Unknown => StatusKind::Idle,
        SearchStatus::Unavailable => StatusKind::Offline,
    }
}

pub fn artifact_status(state: ArtifactState) -> StatusKind {
    match state {
        ArtifactState::Ready => StatusKind::Done,
        ArtifactState::Staging => StatusKind::Busy,
        ArtifactState::Quarantined => StatusKind::Attention,
        ArtifactState::Corrupt => StatusKind::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_contract_shape_has_a_bundled_glyph() {
        let tokens = theme::current_design_tokens();
        for kind in [
            StatusKind::Attention,
            StatusKind::Busy,
            StatusKind::Done,
            StatusKind::Error,
            StatusKind::Idle,
            StatusKind::Offline,
            StatusKind::Orphaned,
            StatusKind::PermissionDenied,
        ] {
            let path = glyph_path(tokens.status(kind).shape);
            let name = path.trim_start_matches("icons/").trim_end_matches(".svg");
            assert!(
                path.ends_with(&format!(
                    "{}.svg",
                    tokens.status(kind).shape.replace('_', "-")
                )),
                "{kind:?} maps to {path}"
            );
            assert!(
                gpui::AssetSource::load(&crate::assets::Assets, path)
                    .unwrap()
                    .is_some(),
                "{name} is bundled"
            );
        }
    }

    #[test]
    fn distinct_states_never_share_a_shape_and_color() {
        let tokens = theme::current_design_tokens();
        let visual = |kind| {
            let status = tokens.status(kind);
            (status.shape, status.color)
        };
        assert_ne!(
            visual(session_state_status(HostedSessionState::Live)),
            visual(session_state_status(HostedSessionState::Offline))
        );
        assert_eq!(
            session_state_status(HostedSessionState::PermissionDenied),
            StatusKind::PermissionDenied
        );
        assert_eq!(
            activity_status(ActivityState::NeedsInput),
            StatusKind::Attention
        );
        assert_eq!(
            search_status(SearchStatus::Unavailable),
            StatusKind::Offline
        );
        assert_eq!(artifact_status(ArtifactState::Corrupt), StatusKind::Error);
    }
}
