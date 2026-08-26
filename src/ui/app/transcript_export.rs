#![allow(dead_code)]

use std::collections::BTreeMap;

use termirust_domain::{
    ExportManifest, RuntimeCapability, RuntimeRecognition, TranscriptCategorySet, TranscriptKind,
    TranscriptRequest,
};

use crate::agents::sanitized_candidate_transcript_contract;
use crate::ui::localization;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranscriptUnavailableReason {
    UnverifiedRuntime,
    UnsupportedProviderVersion,
    PendingApproval,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TranscriptExportProjection {
    pub available: bool,
    pub reason: Option<TranscriptUnavailableReason>,
}

impl TranscriptExportProjection {
    pub fn reason_text(&self) -> Option<String> {
        self.reason.map(|reason| match reason {
            TranscriptUnavailableReason::UnverifiedRuntime => {
                localization::transcript_export_unavailable_unverified()
            }
            TranscriptUnavailableReason::UnsupportedProviderVersion => {
                localization::transcript_export_unavailable_contract()
            }
            TranscriptUnavailableReason::PendingApproval => {
                localization::transcript_export_unavailable_pending()
            }
        })
    }
}

pub(super) fn transcript_export_projection(
    recognition: Option<&RuntimeRecognition>,
) -> TranscriptExportProjection {
    let Some(occupant) = recognition.and_then(|recognition| recognition.occupant.as_ref()) else {
        return TranscriptExportProjection {
            available: false,
            reason: Some(TranscriptUnavailableReason::UnverifiedRuntime),
        };
    };
    let effective = occupant.effective_capabilities();
    let contract = sanitized_candidate_transcript_contract();
    let candidate_version = contract.version.to_string();
    let exact_candidate = occupant.runtime_id == contract.runtime_id
        && occupant.safe_version.as_deref() == Some(candidate_version.as_str())
        && effective.contains(RuntimeCapability::TranscriptExport);
    if exact_candidate && !contract.release_enabled {
        return TranscriptExportProjection {
            available: false,
            reason: Some(TranscriptUnavailableReason::PendingApproval),
        };
    }
    TranscriptExportProjection {
        available: false,
        reason: Some(TranscriptUnavailableReason::UnsupportedProviderVersion),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranscriptExportFailure {
    ChangedSource,
    PermissionDenied,
    Quota,
    DiskFull,
    DestinationConflict,
    InvalidSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum TranscriptExportPhase {
    Unavailable(TranscriptUnavailableReason),
    Preview,
    Exporting { entries: u64, bytes: u64 },
    Complete(ExportManifest),
    Cancelled,
    Failed(TranscriptExportFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TranscriptCategoryPreview {
    pub kind: TranscriptKind,
    pub count: u64,
    pub selected: bool,
    pub sensitive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TranscriptExportModel {
    pub request: TranscriptRequest,
    pub categories: Vec<TranscriptCategoryPreview>,
    pub skipped_count: u64,
    pub redaction_count: u64,
    pub phase: TranscriptExportPhase,
}

impl TranscriptExportModel {
    pub fn new(projection: &TranscriptExportProjection) -> Self {
        let reason = projection
            .reason
            .unwrap_or(TranscriptUnavailableReason::UnsupportedProviderVersion);
        let default_categories = TranscriptCategorySet::default();
        Self {
            request: TranscriptRequest::default(),
            categories: TranscriptKind::ALL
                .into_iter()
                .map(|kind| TranscriptCategoryPreview {
                    kind,
                    count: 0,
                    selected: default_categories.contains(kind),
                    sensitive: kind.sensitive(),
                })
                .collect(),
            skipped_count: 0,
            redaction_count: 0,
            phase: if projection.available {
                TranscriptExportPhase::Preview
            } else {
                TranscriptExportPhase::Unavailable(reason)
            },
        }
    }

    pub fn apply_preview(
        &mut self,
        counts: &BTreeMap<TranscriptKind, u64>,
        skipped_count: u64,
        redaction_count: u64,
    ) {
        for category in &mut self.categories {
            category.count = counts.get(&category.kind).copied().unwrap_or(0);
        }
        self.skipped_count = skipped_count;
        self.redaction_count = redaction_count;
    }

    pub fn set_category(&mut self, kind: TranscriptKind, selected: bool) {
        if let Some(category) = self
            .categories
            .iter_mut()
            .find(|category| category.kind == kind)
        {
            category.selected = selected;
        }
        self.request.categories = TranscriptCategorySet::new(
            self.categories
                .iter()
                .filter(|category| category.selected)
                .map(|category| category.kind),
        );
    }

    pub fn sensitive_categories_selected(&self) -> bool {
        self.categories
            .iter()
            .any(|category| category.selected && category.sensitive)
    }

    pub fn begin_export(&mut self) -> bool {
        if !matches!(self.phase, TranscriptExportPhase::Preview) || self.request.validate().is_err()
        {
            return false;
        }
        self.phase = TranscriptExportPhase::Exporting {
            entries: 0,
            bytes: 0,
        };
        true
    }

    pub fn update_progress(&mut self, entries: u64, bytes: u64) {
        if !matches!(self.phase, TranscriptExportPhase::Exporting { .. }) {
            return;
        }
        self.phase = TranscriptExportPhase::Exporting {
            entries: entries.min(self.request.limits.exported_entries as u64),
            bytes: bytes.min(self.request.limits.output_bytes as u64),
        };
    }

    pub fn cancel(&mut self) {
        if matches!(self.phase, TranscriptExportPhase::Exporting { .. }) {
            self.phase = TranscriptExportPhase::Cancelled;
        }
    }

    pub fn fail(&mut self, failure: TranscriptExportFailure) {
        self.phase = TranscriptExportPhase::Failed(failure);
    }

    pub fn complete(&mut self, manifest: ExportManifest) {
        if matches!(self.phase, TranscriptExportPhase::Exporting { .. }) {
            self.phase = TranscriptExportPhase::Complete(manifest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::{
        HostInstanceId, OccupantGeneration, OccupantOwnership, ProcessToken, RecognitionConfidence,
        RuntimeCapabilitySet, RuntimeId, RuntimeOccupant, TranscriptLimits, TranscriptRange,
        deterministic_content_hash,
    };

    fn available_projection() -> TranscriptExportProjection {
        TranscriptExportProjection {
            available: true,
            reason: None,
        }
    }

    fn managed_recognition(runtime: &str, version: &str) -> RuntimeRecognition {
        let host = HostInstanceId::new();
        RuntimeRecognition {
            occupant: Some(RuntimeOccupant {
                runtime_id: RuntimeId::new(runtime).unwrap(),
                descriptor_version: 1,
                safe_version: Some(version.to_string()),
                generation: OccupantGeneration::new(1),
                ownership: OccupantOwnership::Managed {
                    host_instance: host,
                    child_token: ProcessToken::new(host, 42, 1),
                },
                capabilities: RuntimeCapabilitySet::new([RuntimeCapability::TranscriptExport]),
                stale: false,
            }),
            confidence: RecognitionConfidence::Verified,
            observed_at_nanos: 1,
        }
    }

    #[test]
    fn transcript_export_defaults_to_user_and_assistant_with_explicit_sensitive_consent() {
        let mut model = TranscriptExportModel::new(&available_projection());
        assert_eq!(
            model.request.categories.iter().collect::<Vec<_>>(),
            vec![TranscriptKind::User, TranscriptKind::Assistant]
        );
        assert!(!model.sensitive_categories_selected());
        model.set_category(TranscriptKind::ToolResult, true);
        assert!(model.sensitive_categories_selected());
        model.set_category(TranscriptKind::ToolResult, false);
        assert!(!model.sensitive_categories_selected());
    }

    #[test]
    fn transcript_export_preview_counts_and_progress_are_bounded_and_content_free() {
        let mut model = TranscriptExportModel::new(&available_projection());
        model.apply_preview(
            &BTreeMap::from([
                (TranscriptKind::User, 3),
                (TranscriptKind::Assistant, 2),
                (TranscriptKind::Reasoning, 1),
            ]),
            4,
            5,
        );
        assert_eq!(model.categories[0].count, 3);
        assert_eq!(model.skipped_count, 4);
        assert_eq!(model.redaction_count, 5);
        assert!(model.begin_export());
        model.update_progress(u64::MAX, u64::MAX);
        assert_eq!(
            model.phase,
            TranscriptExportPhase::Exporting {
                entries: TranscriptLimits::default().exported_entries as u64,
                bytes: TranscriptLimits::default().output_bytes as u64,
            }
        );
        assert!(!format!("{model:?}").contains("transcript body"));
    }

    #[test]
    fn transcript_export_cancel_failure_and_completion_are_terminal_states() {
        let mut cancelled = TranscriptExportModel::new(&available_projection());
        assert!(cancelled.begin_export());
        cancelled.cancel();
        assert_eq!(cancelled.phase, TranscriptExportPhase::Cancelled);

        let mut failed = TranscriptExportModel::new(&available_projection());
        assert!(failed.begin_export());
        failed.fail(TranscriptExportFailure::DiskFull);
        assert_eq!(
            failed.phase,
            TranscriptExportPhase::Failed(TranscriptExportFailure::DiskFull)
        );

        let mut completed = TranscriptExportModel::new(&available_projection());
        assert!(completed.begin_export());
        let manifest = ExportManifest {
            provider_contract: "fixture-v1".to_string(),
            categories: vec![TranscriptKind::User, TranscriptKind::Assistant],
            entry_count: 2,
            skipped_count: 0,
            redaction_count: 1,
            deterministic_content_hash: deterministic_content_hash(b"export"),
        };
        completed.complete(manifest.clone());
        assert_eq!(completed.phase, TranscriptExportPhase::Complete(manifest));
    }

    #[test]
    fn transcript_export_unavailable_reason_is_visible_and_localized() {
        let projection = transcript_export_projection(None);
        assert!(!projection.available);
        let reason = projection.reason_text().unwrap();
        assert!(!reason.is_empty());
        assert!(!reason.contains('/'));
        assert_eq!(
            TranscriptExportModel::new(&projection).phase,
            TranscriptExportPhase::Unavailable(TranscriptUnavailableReason::UnverifiedRuntime)
        );

        let unsupported = managed_recognition("codex", "1.0.0");
        assert_eq!(
            transcript_export_projection(Some(&unsupported)).reason,
            Some(TranscriptUnavailableReason::UnsupportedProviderVersion)
        );
        let pending = managed_recognition("fixture", "1.0.0");
        assert_eq!(
            transcript_export_projection(Some(&pending)).reason,
            Some(TranscriptUnavailableReason::PendingApproval)
        );
    }

    #[test]
    fn transcript_export_request_keeps_bounded_range_and_limits() {
        let model = TranscriptExportModel::new(&available_projection());
        assert_eq!(model.request.range, TranscriptRange::default());
        assert_eq!(model.request.limits, TranscriptLimits::default());
    }
}
