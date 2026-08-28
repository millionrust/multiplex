use termirust_diagnostics::{DiagnosticStatus, DiagnosticUsage, ExportErrorCode};
use termirust_store::{HealthCheckKind, HealthErrorCode, HealthFindingState, HealthReport};

use crate::ui::localization;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsViewModel {
    pub status: String,
    pub usage: String,
    pub can_clear: bool,
    pub can_preview: bool,
    pub can_export: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthFindingView {
    pub kind: HealthCheckKind,
    pub label: String,
    pub state: String,
    pub can_rebuild: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthViewModel {
    pub status: String,
    pub findings: Vec<HealthFindingView>,
    pub can_scan: bool,
}

pub fn health_error_message(code: HealthErrorCode) -> String {
    match code {
        HealthErrorCode::StaleSource => localization::health_error_stale(),
        HealthErrorCode::NewerSource => localization::health_error_newer(),
        HealthErrorCode::PermissionDenied => localization::health_error_permission(),
        HealthErrorCode::CorruptSource
        | HealthErrorCode::VerificationFailed
        | HealthErrorCode::UnsafeEntry
        | HealthErrorCode::SizeLimit => localization::health_error_corrupt(),
        HealthErrorCode::Cancelled => localization::health_operation_cancelled(),
        HealthErrorCode::Unavailable | HealthErrorCode::InjectedCrash => {
            localization::health_error_storage()
        }
    }
}

pub fn health_view_model(report: Option<&HealthReport>, busy: bool) -> HealthViewModel {
    let Some(report) = report else {
        return HealthViewModel {
            status: if busy {
                localization::health_scanning()
            } else {
                localization::health_not_scanned()
            },
            findings: Vec::new(),
            can_scan: !busy,
        };
    };
    let source_healthy = [
        HealthCheckKind::StoreReadable,
        HealthCheckKind::StoreVersion,
        HealthCheckKind::RecordHashes,
    ]
    .into_iter()
    .all(|kind| {
        report
            .finding(kind)
            .is_some_and(|finding| finding.state == HealthFindingState::Healthy)
    });
    let findings = report
        .findings
        .iter()
        .map(|finding| HealthFindingView {
            kind: finding.kind,
            label: match finding.kind {
                HealthCheckKind::ProjectSessionIndex => {
                    localization::health_project_session_label()
                }
                HealthCheckKind::PaletteIndex => localization::health_palette_label(),
                HealthCheckKind::StoreReadable => localization::health_store_readable_label(),
                HealthCheckKind::StoreVersion => localization::health_store_version_label(),
                HealthCheckKind::RecordHashes => localization::health_record_hashes_label(),
            },
            state: health_state_label(finding.state),
            can_rebuild: source_healthy
                && !busy
                && matches!(
                    finding.kind,
                    HealthCheckKind::ProjectSessionIndex | HealthCheckKind::PaletteIndex
                )
                && matches!(
                    finding.state,
                    HealthFindingState::Partial | HealthFindingState::Corrupt
                ),
        })
        .collect();
    HealthViewModel {
        status: if busy {
            localization::health_repair_running()
        } else if report.is_healthy() {
            localization::health_state_healthy()
        } else {
            localization::health_review_findings()
        },
        findings,
        can_scan: !busy,
    }
}

fn health_state_label(state: HealthFindingState) -> String {
    match state {
        HealthFindingState::Healthy => localization::health_state_healthy(),
        HealthFindingState::Partial => localization::health_state_partial(),
        HealthFindingState::Corrupt => localization::health_state_corrupt(),
        HealthFindingState::Newer => localization::health_state_newer(),
        HealthFindingState::Permission => localization::health_state_permission(),
        HealthFindingState::Unavailable => localization::health_state_unavailable(),
    }
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

#[cfg(test)]
mod health {
    use super::*;
    use termirust_store::{HealthEvidenceCode, HealthFinding};

    fn report(index_state: HealthFindingState) -> HealthReport {
        let json = r#"{"id":"00000000-0000-0000-0000-000000000001","findings":[],"source_revisions":null,"authoritative_records":3}"#;
        let mut report: HealthReport = serde_json::from_str(json).unwrap();
        let finding = |kind, state| HealthFinding {
            kind,
            state,
            evidence: HealthEvidenceCode::Verified,
            actual_digest: None,
            expected_digest: None,
        };
        report.findings = vec![
            finding(HealthCheckKind::StoreReadable, HealthFindingState::Healthy),
            finding(HealthCheckKind::StoreVersion, HealthFindingState::Healthy),
            finding(HealthCheckKind::RecordHashes, HealthFindingState::Healthy),
            finding(HealthCheckKind::ProjectSessionIndex, index_state),
            finding(HealthCheckKind::PaletteIndex, HealthFindingState::Healthy),
        ];
        report
    }

    #[test]
    fn no_repair_is_offered_before_an_explicit_scan() {
        let model = health_view_model(None, false);
        assert!(model.can_scan);
        assert!(model.findings.is_empty());
        assert_eq!(model.status, localization::health_not_scanned());
    }

    #[test]
    fn only_a_named_unhealthy_index_can_be_rebuilt() {
        let report = report(HealthFindingState::Partial);
        let model = health_view_model(Some(&report), false);
        assert!(model.can_scan);
        assert_eq!(
            model
                .findings
                .iter()
                .filter(|finding| finding.can_rebuild)
                .count(),
            1
        );
        assert!(model.findings.iter().any(|finding| {
            finding.kind == HealthCheckKind::ProjectSessionIndex && finding.can_rebuild
        }));
    }

    #[test]
    fn source_failure_and_busy_state_disable_every_rebuild() {
        let mut report = report(HealthFindingState::Corrupt);
        report.findings[0].state = HealthFindingState::Corrupt;
        let failed = health_view_model(Some(&report), false);
        assert!(failed.findings.iter().all(|finding| !finding.can_rebuild));
        let busy = health_view_model(Some(&report), true);
        assert!(!busy.can_scan);
        assert!(busy.findings.iter().all(|finding| !finding.can_rebuild));
    }

    #[test]
    fn health_errors_are_closed_and_actionable() {
        assert_ne!(
            health_error_message(HealthErrorCode::StaleSource),
            health_error_message(HealthErrorCode::Unavailable)
        );
        assert!(!health_error_message(HealthErrorCode::NewerSource).is_empty());
        assert!(!health_error_message(HealthErrorCode::PermissionDenied).contains('/'));
    }
}
