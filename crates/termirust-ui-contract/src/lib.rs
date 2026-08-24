mod contract;
pub mod generated;
pub mod lint;
pub mod localization_lint;
pub mod messages;

pub use contract::{
    ContractError, GenerationArtifacts, TokenManifest, generate_artifacts, load_manifest,
    parse_manifest,
};
pub use generated::*;
pub use messages::*;

#[cfg(test)]
mod tokens_tests {
    use super::*;

    #[test]
    fn tokens_all_themes_resolve_focus_status_and_motion_contracts() {
        for theme in ThemeKind::ALL {
            let tokens = DesignTokens::new(theme);
            assert_eq!(tokens.theme(), theme);
            assert!(tokens.color_bg_canvas().alpha > 0);
            assert!(tokens.focus_ring_width().0 >= 2.0);
            assert_eq!(tokens.motion_progress(true), DurationValue(0));
            for kind in [
                StatusKind::Idle,
                StatusKind::Busy,
                StatusKind::Done,
                StatusKind::Attention,
                StatusKind::Error,
                StatusKind::Offline,
                StatusKind::Orphaned,
                StatusKind::PermissionDenied,
            ] {
                let status = tokens.status(kind);
                assert!(!status.icon.0.is_empty());
                assert!(!status.text.0.is_empty());
                assert!(!status.shape.is_empty());
                assert!(status.color.alpha > 0);
            }
        }
    }

    #[test]
    fn tokens_high_contrast_and_recording_friendly_have_fixed_safe_variants() {
        let high_contrast = DesignTokens::new(ThemeKind::HighContrast);
        assert_eq!(high_contrast.focus_ring_width(), BorderValue(3.0));
        assert!(!high_contrast.shadow_modal().visible);

        let dark = DesignTokens::new(ThemeKind::Dark);
        let recording = DesignTokens::new(ThemeKind::RecordingFriendly);
        assert_ne!(
            dark.color_action_primary(),
            recording.color_action_primary()
        );
        assert_eq!(recording.motion_progress(true), DurationValue(0));
    }
}
