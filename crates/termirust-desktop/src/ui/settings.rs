use termirust_client::HostReconciliationErrorCode;
use termirust_diagnostics::{DiagnosticStatus, DiagnosticUsage, ExportErrorCode};
use termirust_domain::{HostedSessionState, SessionLaunchRoute};
use termirust_store::{
    HealthCheckKind, HealthErrorCode, HealthEvidenceCode, HealthFindingState, HealthReport,
    RecoveryErrorCode, RecoveryResult,
};

use crate::ui::localization;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsViewModel {
    pub status: String,
    pub usage: String,
    pub can_clear: bool,
    pub can_preview: bool,
    pub can_export: bool,
}

#[cfg(test)]
mod search {
    use termirust_ui_contract::{
        MAX_SETTINGS_QUERY_CHARS, SettingId, SettingsSearchDocument, SettingsSectionId,
        search_settings,
    };

    fn documents() -> Vec<SettingsSearchDocument> {
        vec![
            SettingsSearchDocument {
                id: SettingId::TerminalFontSize,
                section: SettingsSectionId::Terminal,
                label: "Terminal font size".to_string(),
                help: "Adjust terminal typography".to_string(),
            },
            SettingsSearchDocument {
                id: SettingId::BackupExportPassphrase,
                section: SettingsSectionId::StoragePrivacyDiagnostics,
                label: "Export passphrase".to_string(),
                help: "Protect an encrypted backup".to_string(),
            },
            SettingsSearchDocument {
                id: SettingId::SyncFolder,
                section: SettingsSectionId::StoragePrivacyDiagnostics,
                label: "Sync folder".to_string(),
                help: "Choose the shared bundle folder".to_string(),
            },
        ]
    }

    #[test]
    fn labels_and_help_filter_deterministically_without_values() {
        let docs = documents();
        let first = search_settings("terminal", &docs).unwrap();
        let second = search_settings("terminal", &docs).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, SettingId::TerminalFontSize);
        assert!(
            search_settings("super-secret-value", &docs)
                .unwrap()
                .is_empty()
        );
        assert!(
            search_settings("/Users/private/project", &docs)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn oversized_queries_fail_closed() {
        let query = "x".repeat(MAX_SETTINGS_QUERY_CHARS + 1);
        assert!(search_settings(&query, &documents()).is_err());
    }
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
    pub can_prepare_restore: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryViewModel {
    pub visible: bool,
    pub can_confirm: bool,
    pub can_cancel: bool,
    pub changed_files: usize,
    pub unchanged_files: usize,
    pub backup_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostRecoveryViewModel {
    pub visible: bool,
    pub can_confirm: bool,
    pub can_cancel: bool,
    pub evidence: String,
}

pub fn metadata_restore_allowed(report: Option<&HealthReport>, busy: bool) -> bool {
    !busy
        && report
            .and_then(|report| report.finding(HealthCheckKind::StoreReadable))
            .is_some_and(|finding| {
                finding.state == HealthFindingState::Corrupt
                    && finding.evidence == HealthEvidenceCode::StoreMalformed
            })
}

pub fn recovery_view_model(
    report: Option<&HealthReport>,
    plan: Option<&termirust_store::RecoveryPlan>,
    busy: bool,
) -> RecoveryViewModel {
    RecoveryViewModel {
        visible: metadata_restore_allowed(report, busy) || plan.is_some(),
        can_confirm: plan.is_some() && !busy,
        can_cancel: plan.is_some() || busy,
        changed_files: plan.map_or(0, |plan| plan.files.len()),
        unchanged_files: plan.map_or(0, |plan| plan.unchanged_files.len()),
        backup_bytes: plan.map_or(0, |plan| plan.estimated_backup_bytes),
    }
}

pub fn recovery_error_message(code: RecoveryErrorCode) -> String {
    match code {
        RecoveryErrorCode::Cancelled => localization::recovery_cancelled(),
        RecoveryErrorCode::NoLastGood => localization::recovery_error_no_backup(),
        RecoveryErrorCode::CorruptLastGood
        | RecoveryErrorCode::VerificationFailed
        | RecoveryErrorCode::RecoveryRequired => localization::recovery_error_verification(),
        RecoveryErrorCode::NewerFormat => localization::health_error_newer(),
        RecoveryErrorCode::StaleRevision => localization::health_error_stale(),
        RecoveryErrorCode::PermissionDenied => localization::health_error_permission(),
        RecoveryErrorCode::UnsafeEntry
        | RecoveryErrorCode::SizeLimit
        | RecoveryErrorCode::StorageUnavailable
        | RecoveryErrorCode::InjectedCrash => localization::health_error_storage(),
    }
}

pub fn host_recovery_allowed(
    route: SessionLaunchRoute,
    state: HostedSessionState,
    busy: bool,
) -> bool {
    !busy
        && route == SessionLaunchRoute::DurableHost
        && matches!(
            state,
            HostedSessionState::Offline | HostedSessionState::Orphaned
        )
}

pub fn host_recovery_view_model(
    route: SessionLaunchRoute,
    state: HostedSessionState,
    plan: Option<&termirust_client::HostReconciliationPlan>,
    busy: bool,
) -> HostRecoveryViewModel {
    let result = plan.map(|plan| plan.preview_result);
    HostRecoveryViewModel {
        visible: host_recovery_allowed(route, state, busy) || plan.is_some() || busy,
        can_confirm: result == Some(RecoveryResult::Reconciled) && !busy,
        can_cancel: plan.is_some() || busy,
        evidence: plan.map_or_else(String::new, |plan| {
            localization::host_recovery_impact(
                host_recovery_result_label(plan.preview_result),
                plan.authenticated_peers.len(),
                plan.current_bytes,
            )
        }),
    }
}

pub fn host_recovery_error_message(code: HostReconciliationErrorCode) -> String {
    match code {
        HostReconciliationErrorCode::Cancelled => localization::recovery_cancelled(),
        HostReconciliationErrorCode::PeerUnavailable => localization::new_session_phase_offline(),
        HostReconciliationErrorCode::StaleEvidence => localization::health_error_stale(),
        HostReconciliationErrorCode::PermissionDenied => localization::health_error_permission(),
        HostReconciliationErrorCode::UnsafeEntry
        | HostReconciliationErrorCode::StorageUnavailable
        | HostReconciliationErrorCode::VerificationFailed
        | HostReconciliationErrorCode::InjectedCrash
        | HostReconciliationErrorCode::RecoveryRequired => {
            localization::recovery_error_verification()
        }
    }
}

fn host_recovery_result_label(result: RecoveryResult) -> String {
    match result {
        RecoveryResult::Reconciled => localization::recovery_confirm_action(),
        RecoveryResult::NoChange => localization::health_state_healthy(),
        RecoveryResult::Ambiguous => localization::runtime_ownership_ambiguous(),
        RecoveryResult::Restored => localization::recovery_complete(),
        RecoveryResult::RolledBack => localization::recovery_cancelled(),
        RecoveryResult::RecoveryRequired => localization::recovery_error_verification(),
    }
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
            can_prepare_restore: false,
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
        can_prepare_restore: metadata_restore_allowed(Some(report), busy),
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

#[cfg(test)]
mod recovery {
    use super::*;
    use termirust_store::{HealthEvidenceCode, HealthFinding};

    fn report(state: HealthFindingState, evidence: HealthEvidenceCode) -> HealthReport {
        let json = r#"{"id":"00000000-0000-0000-0000-000000000001","findings":[],"source_revisions":null,"authoritative_records":0}"#;
        let mut report: HealthReport = serde_json::from_str(json).unwrap();
        report.findings.push(HealthFinding {
            kind: HealthCheckKind::StoreReadable,
            state,
            evidence,
            actual_digest: None,
            expected_digest: None,
        });
        report
    }

    #[test]
    fn restore_is_offered_only_for_exact_malformed_authoritative_metadata() {
        assert!(metadata_restore_allowed(
            Some(&report(
                HealthFindingState::Corrupt,
                HealthEvidenceCode::StoreMalformed,
            )),
            false,
        ));
        for (state, evidence) in [
            (HealthFindingState::Newer, HealthEvidenceCode::StoreNewer),
            (HealthFindingState::Corrupt, HealthEvidenceCode::StoreUnsafe),
            (
                HealthFindingState::Corrupt,
                HealthEvidenceCode::StoreTooLarge,
            ),
            (
                HealthFindingState::Unavailable,
                HealthEvidenceCode::IoUnavailable,
            ),
        ] {
            assert!(!metadata_restore_allowed(
                Some(&report(state, evidence)),
                false
            ));
        }
        assert!(!metadata_restore_allowed(
            Some(&report(
                HealthFindingState::Corrupt,
                HealthEvidenceCode::StoreMalformed,
            )),
            true,
        ));
    }

    #[test]
    fn recovery_failures_have_content_free_closed_copy() {
        for code in [
            RecoveryErrorCode::NoLastGood,
            RecoveryErrorCode::CorruptLastGood,
            RecoveryErrorCode::StaleRevision,
            RecoveryErrorCode::RecoveryRequired,
        ] {
            let message = recovery_error_message(code);
            assert!(!message.is_empty());
            assert!(!message.contains('/'));
        }
    }

    #[test]
    fn host_recovery_is_guarded_to_offline_durable_sessions() {
        assert!(host_recovery_allowed(
            SessionLaunchRoute::DurableHost,
            HostedSessionState::Offline,
            false,
        ));
        assert!(host_recovery_allowed(
            SessionLaunchRoute::DurableHost,
            HostedSessionState::Orphaned,
            false,
        ));
        assert!(!host_recovery_allowed(
            SessionLaunchRoute::DurableHost,
            HostedSessionState::Live,
            false,
        ));
        assert!(!host_recovery_allowed(
            SessionLaunchRoute::LegacyAppAttached,
            HostedSessionState::Offline,
            false,
        ));
        assert!(!host_recovery_allowed(
            SessionLaunchRoute::DurableHost,
            HostedSessionState::Offline,
            true,
        ));
    }

    #[test]
    fn host_recovery_failures_have_content_free_closed_copy() {
        for code in [
            HostReconciliationErrorCode::PeerUnavailable,
            HostReconciliationErrorCode::StaleEvidence,
            HostReconciliationErrorCode::UnsafeEntry,
            HostReconciliationErrorCode::RecoveryRequired,
        ] {
            let message = host_recovery_error_message(code);
            assert!(!message.is_empty());
            assert!(!message.contains('/'));
        }
    }
}
