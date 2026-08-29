use std::path::Path;

use termirust_domain::{
    OccupantOwnership, RecognitionConfidence, RuntimeCapability, RuntimeCapabilitySet,
    RuntimeDetectionStatus, RuntimeRecognition,
};

use crate::ui::localization;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RuntimeInspectorProjection {
    pub runtime: String,
    pub version: String,
    pub ownership: String,
    pub confidence: String,
    pub generation: String,
    pub capabilities: String,
    pub stale: bool,
    pub explanation: Option<String>,
}

pub(super) fn runtime_label(runtime: &str) -> String {
    match runtime {
        "codex" => localization::runtime_label_codex(),
        "claude" => localization::runtime_label_claude(),
        "gemini" => localization::runtime_label_gemini(),
        "generic" => localization::runtime_label_generic(),
        value => value.to_string(),
    }
}

pub(super) fn executable_basename(value: &str) -> String {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Executable")
        .to_string()
}

pub(super) fn detection_status_label(status: RuntimeDetectionStatus) -> String {
    match status {
        RuntimeDetectionStatus::Available => localization::preset_status_supported(),
        RuntimeDetectionStatus::UnsupportedVersion => localization::preset_status_unsupported(),
        RuntimeDetectionStatus::Missing => localization::preset_status_missing(),
        RuntimeDetectionStatus::PermissionDenied => localization::preset_status_permission(),
        RuntimeDetectionStatus::Partial => localization::preset_status_failed(),
    }
}

pub(super) fn capability_summary(capabilities: &RuntimeCapabilitySet) -> String {
    let labels = capabilities
        .iter()
        .map(|capability| match capability {
            RuntimeCapability::InteractivePty => localization::runtime_capability_interactive(),
            RuntimeCapability::StructuredEvents => localization::runtime_capability_structured(),
            RuntimeCapability::ApprovalRequests => localization::runtime_capability_approvals(),
            RuntimeCapability::Cancellation => localization::runtime_capability_cancellation(),
            RuntimeCapability::ContextHandoff => localization::runtime_capability_context(),
            RuntimeCapability::RemoteLaunch => localization::runtime_capability_remote(),
            RuntimeCapability::Resume => localization::runtime_capability_resume(),
            RuntimeCapability::TranscriptExport => localization::runtime_capability_transcript(),
        })
        .collect::<Vec<_>>();
    if labels.is_empty() {
        localization::runtime_capabilities_none()
    } else {
        labels.join(", ")
    }
}

pub(super) fn runtime_inspector_projection(
    recognition: Option<&RuntimeRecognition>,
) -> RuntimeInspectorProjection {
    let Some(recognition) = recognition else {
        return unverified_projection();
    };
    let Some(occupant) = recognition.occupant.as_ref() else {
        return unverified_projection();
    };
    let ownership = match occupant.ownership {
        OccupantOwnership::Managed { .. } => localization::runtime_ownership_managed(),
        OccupantOwnership::Observed { .. } => localization::runtime_ownership_observed(),
        OccupantOwnership::Ambiguous => localization::runtime_ownership_ambiguous(),
    };
    let confidence = match recognition.confidence {
        RecognitionConfidence::Verified => localization::runtime_confidence_verified(),
        RecognitionConfidence::Observed => localization::runtime_confidence_observed(),
        RecognitionConfidence::Uncertain => localization::runtime_confidence_uncertain(),
    };
    let effective = occupant.effective_capabilities();
    RuntimeInspectorProjection {
        runtime: runtime_label(occupant.runtime_id.as_str()),
        version: occupant
            .safe_version
            .clone()
            .unwrap_or_else(localization::runtime_version_unverified),
        ownership,
        confidence,
        generation: occupant.generation.get().to_string(),
        capabilities: capability_summary(&effective),
        stale: occupant.stale,
        explanation: effective
            .is_empty()
            .then(localization::runtime_unverified_explanation),
    }
}

fn unverified_projection() -> RuntimeInspectorProjection {
    RuntimeInspectorProjection {
        runtime: localization::runtime_label_generic(),
        version: localization::runtime_version_unverified(),
        ownership: localization::runtime_ownership_ambiguous(),
        confidence: localization::runtime_confidence_uncertain(),
        generation: "0".to_string(),
        capabilities: localization::runtime_capabilities_none(),
        stale: false,
        explanation: Some(localization::runtime_unverified_explanation()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::{
        HostInstanceId, OccupantGeneration, ProcessToken, RuntimeId, RuntimeOccupant,
    };

    #[test]
    fn runtime_registry_every_detection_state_has_text() {
        for status in [
            RuntimeDetectionStatus::Available,
            RuntimeDetectionStatus::UnsupportedVersion,
            RuntimeDetectionStatus::Missing,
            RuntimeDetectionStatus::PermissionDenied,
            RuntimeDetectionStatus::Partial,
        ] {
            assert!(!detection_status_label(status).is_empty());
        }
        assert!(!localization::runtime_status_not_checked().is_empty());
        assert!(!localization::presets_scanning().is_empty());
    }

    #[test]
    fn runtime_registry_executable_display_never_exposes_parent_path() {
        let display = executable_basename("/Users/private/customer/codex");
        assert_eq!(display, "codex");
        assert!(!display.contains("private"));
    }

    #[test]
    fn runtime_registry_unverified_projection_explains_empty_capabilities() {
        let projection = runtime_inspector_projection(None);
        assert_eq!(projection.generation, "0");
        assert!(projection.explanation.is_some());
        assert_eq!(
            projection.capabilities,
            localization::runtime_capabilities_none()
        );
    }

    #[test]
    fn runtime_registry_managed_projection_exposes_only_effective_capabilities() {
        let host = HostInstanceId::new();
        let capabilities = RuntimeCapabilitySet::new([
            RuntimeCapability::InteractivePty,
            RuntimeCapability::Cancellation,
        ]);
        let recognition = RuntimeRecognition {
            occupant: Some(RuntimeOccupant {
                runtime_id: RuntimeId::new("codex").unwrap(),
                descriptor_version: 1,
                safe_version: Some("1.0.7".to_string()),
                executable_fingerprint: None,
                generation: OccupantGeneration::new(4),
                ownership: OccupantOwnership::Managed {
                    host_instance: host,
                    child_token: ProcessToken::new(host, 42, 4),
                },
                capabilities,
                stale: false,
            }),
            confidence: RecognitionConfidence::Verified,
            observed_at_nanos: 7,
        };
        let projection = runtime_inspector_projection(Some(&recognition));
        assert_eq!(projection.runtime, localization::runtime_label_codex());
        assert_eq!(projection.version, "1.0.7");
        assert_eq!(projection.generation, "4");
        assert!(projection.explanation.is_none());
        assert!(
            projection
                .capabilities
                .contains(&localization::runtime_capability_interactive())
        );
    }
}
