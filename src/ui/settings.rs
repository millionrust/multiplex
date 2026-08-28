use termirust_diagnostics::{DiagnosticStatus, DiagnosticUsage, ExportErrorCode};

use crate::ui::localization;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsViewModel {
    pub status: String,
    pub usage: String,
    pub can_clear: bool,
    pub can_preview: bool,
    pub can_export: bool,
}

pub fn diagnostics_error_message(code: ExportErrorCode) -> String {
    match code {
        ExportErrorCode::PermissionDenied | ExportErrorCode::InvalidDestination => {
            localization::diagnostics_error_permission()
        }
        ExportErrorCode::SizeLimit => localization::diagnostics_error_size(),
        ExportErrorCode::MalformedEntry | ExportErrorCode::RedactionUncertain => {
            localization::diagnostics_error_redaction()
        }
        ExportErrorCode::SourceChanged => localization::diagnostics_error_source_changed(),
        ExportErrorCode::DestinationExists => localization::diagnostics_error_destination_exists(),
        ExportErrorCode::RuntimeUnavailable | ExportErrorCode::StorageUnavailable => {
            localization::diagnostics_error_storage()
        }
        ExportErrorCode::Cancelled => localization::diagnostics_operation_cancelled(),
    }
}

pub fn diagnostics_view_model(
    enabled: bool,
    status: DiagnosticStatus,
    usage: DiagnosticUsage,
    retention_days: u8,
    has_preview: bool,
    busy: bool,
) -> DiagnosticsViewModel {
    let can_preview = enabled
        && matches!(
            status,
            DiagnosticStatus::Healthy | DiagnosticStatus::Dropping
        );
    let status = match status {
        DiagnosticStatus::Disabled => localization::diagnostics_status_disabled(),
        DiagnosticStatus::Healthy => localization::diagnostics_status_healthy(),
        DiagnosticStatus::Dropping => localization::diagnostics_status_dropping(),
        DiagnosticStatus::DiskError => localization::diagnostics_status_disk_error(),
    };
    DiagnosticsViewModel {
        status,
        usage: localization::diagnostics_usage_summary(usage.bytes, usage.files, retention_days),
        can_clear: usage.files > 0 && !busy,
        can_preview: can_preview && !busy,
        can_export: has_preview && !busy,
    }
}

#[cfg(test)]
mod diagnostics {
    use super::*;

    #[test]
    fn status_is_textual_and_actions_follow_runtime_state() {
        let healthy = diagnostics_view_model(
            true,
            DiagnosticStatus::Healthy,
            DiagnosticUsage {
                files: 2,
                bytes: 4096,
                ..DiagnosticUsage::default()
            },
            14,
            false,
            false,
        );
        assert!(healthy.status.contains("Healthy"));
        assert!(healthy.usage.contains("2 files"));
        assert!(healthy.usage.contains("14 days"));
        assert!(healthy.can_clear);
        assert!(healthy.can_preview);
        assert!(!healthy.can_export);

        let failed = diagnostics_view_model(
            true,
            DiagnosticStatus::DiskError,
            DiagnosticUsage::default(),
            7,
            true,
            false,
        );
        assert!(failed.status.contains("error"));
        assert!(!failed.can_preview);
        assert!(failed.can_export);
    }

    #[test]
    fn disabled_state_has_no_preview_route() {
        let model = diagnostics_view_model(
            false,
            DiagnosticStatus::Disabled,
            DiagnosticUsage::default(),
            1,
            false,
            false,
        );
        assert_eq!(model.status, localization::diagnostics_status_disabled());
        assert!(!model.can_preview);
        assert!(!model.can_export);
    }

    #[test]
    fn running_operation_disables_competing_actions() {
        let model = diagnostics_view_model(
            true,
            DiagnosticStatus::Healthy,
            DiagnosticUsage {
                files: 1,
                bytes: 64,
                ..DiagnosticUsage::default()
            },
            14,
            true,
            true,
        );
        assert!(!model.can_clear);
        assert!(!model.can_preview);
        assert!(!model.can_export);
    }

    #[test]
    fn export_failures_have_closed_actionable_messages() {
        let permission = diagnostics_error_message(ExportErrorCode::PermissionDenied);
        let redaction = diagnostics_error_message(ExportErrorCode::RedactionUncertain);
        let existing = diagnostics_error_message(ExportErrorCode::DestinationExists);
        assert!(permission.contains("permissions"));
        assert!(redaction.contains("privacy"));
        assert!(existing.contains("new filename"));
        assert!(!permission.contains('/'));
    }
}
