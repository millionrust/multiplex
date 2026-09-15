pub mod accessibility;
mod app;
pub mod autocomplete;
pub mod keys;
pub mod localization;
pub mod path;
pub mod render_terminal;
pub mod settings;
pub mod sftp_local;
pub mod shell;
pub mod snippet;
pub mod theme;
pub mod util;

pub use app::TermiRustApp;

#[cfg(test)]
mod recovery {
    use termirust_domain::{HostedSessionState, SessionLaunchRoute};
    use termirust_store::{
        HealthCheckKind, HealthEvidenceCode, HealthFinding, HealthFindingState, HealthReport,
        RecoveryErrorCode,
    };

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
    fn exact_finding_is_required_before_restore_is_visible() {
        let malformed = report(
            HealthFindingState::Corrupt,
            HealthEvidenceCode::StoreMalformed,
        );
        assert!(super::settings::metadata_restore_allowed(
            Some(&malformed),
            false,
        ));
        assert!(!super::settings::metadata_restore_allowed(
            Some(&malformed),
            true,
        ));
        assert!(!super::settings::metadata_restore_allowed(
            Some(&report(
                HealthFindingState::Newer,
                HealthEvidenceCode::StoreNewer,
            )),
            false,
        ));
        assert!(!super::settings::metadata_restore_allowed(
            Some(&report(
                HealthFindingState::Corrupt,
                HealthEvidenceCode::StoreUnsafe,
            )),
            false,
        ));
    }

    #[test]
    fn recovery_errors_are_localized_and_do_not_expose_paths() {
        for code in [
            RecoveryErrorCode::NoLastGood,
            RecoveryErrorCode::CorruptLastGood,
            RecoveryErrorCode::StaleRevision,
            RecoveryErrorCode::RecoveryRequired,
        ] {
            let message = super::settings::recovery_error_message(code);
            assert!(!message.is_empty());
            assert!(!message.contains('/'));
        }
    }

    #[test]
    fn host_recovery_requires_an_offline_or_orphaned_durable_session() {
        assert!(super::settings::host_recovery_allowed(
            SessionLaunchRoute::DurableHost,
            HostedSessionState::Offline,
            false,
        ));
        assert!(!super::settings::host_recovery_allowed(
            SessionLaunchRoute::DurableHost,
            HostedSessionState::Live,
            false,
        ));
        assert!(!super::settings::host_recovery_allowed(
            SessionLaunchRoute::LegacyAppAttached,
            HostedSessionState::Offline,
            false,
        ));
    }
}
