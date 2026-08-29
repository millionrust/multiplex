use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{HostInstanceId, ProcessToken, RuntimeId};

pub const MAX_RUNTIME_DESCRIPTORS: usize = 16;
pub const MAX_RUNTIME_CANDIDATES: usize = 3;
pub const MAX_RUNTIME_VERSION_BYTES: usize = 128;
pub const RUNTIME_DESCRIPTOR_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLaunchMode {
    Interactive,
    Structured,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCapability {
    InteractivePty,
    StructuredEvents,
    ApprovalRequests,
    Cancellation,
    ContextHandoff,
    RemoteLaunch,
    Resume,
    TranscriptExport,
}

#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RuntimeCapabilitySet(BTreeSet<RuntimeCapability>);

impl RuntimeCapabilitySet {
    pub fn new(capabilities: impl IntoIterator<Item = RuntimeCapability>) -> Self {
        Self(capabilities.into_iter().collect())
    }

    pub fn contains(&self, capability: RuntimeCapability) -> bool {
        self.0.contains(&capability)
    }

    pub fn iter(&self) -> impl Iterator<Item = RuntimeCapability> + '_ {
        self.0.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn union_with(&mut self, other: &Self) {
        self.0.extend(other.iter());
    }

    pub fn effective_for(&self, ownership: &OccupantOwnership) -> Self {
        if matches!(ownership, OccupantOwnership::Managed { .. }) {
            self.clone()
        } else {
            Self::default()
        }
    }
}

impl fmt::Debug for RuntimeCapabilitySet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_set().entries(self.0.iter()).finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RuntimeVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl RuntimeVersion {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for RuntimeVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeVersionRule {
    pub minimum: RuntimeVersion,
    pub maximum_exclusive: RuntimeVersion,
    pub launch_mode: RuntimeLaunchMode,
    pub capabilities: RuntimeCapabilitySet,
}

impl RuntimeVersionRule {
    pub fn matches(&self, version: RuntimeVersion) -> bool {
        version >= self.minimum && version < self.maximum_exclusive
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDescriptorKind {
    Probed,
    GenericCommand,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeDescriptor {
    pub id: RuntimeId,
    pub descriptor_version: u16,
    pub display_name: String,
    pub kind: RuntimeDescriptorKind,
    pub executable_candidates: Vec<String>,
    pub version_arguments: Vec<String>,
    pub version_rules: Vec<RuntimeVersionRule>,
}

impl RuntimeDescriptor {
    pub fn capabilities_for(&self, version: RuntimeVersion) -> RuntimeCapabilitySet {
        let mut capabilities = RuntimeCapabilitySet::default();
        for rule in &self.version_rules {
            if rule.matches(version) {
                capabilities.union_with(&rule.capabilities);
            }
        }
        capabilities
    }

    pub fn validates(&self) -> bool {
        self.descriptor_version > 0
            && self.display_name.chars().count() <= 128
            && self.executable_candidates.len() <= MAX_RUNTIME_CANDIDATES
            && self.version_arguments.len() <= 8
            && self
                .executable_candidates
                .iter()
                .all(|candidate| !candidate.is_empty() && candidate.len() <= 128)
            && self
                .version_arguments
                .iter()
                .all(|argument| !argument.contains('\0') && argument.len() <= 128)
            && self.version_rules.iter().all(|rule| {
                rule.minimum < rule.maximum_exclusive
                    && (!rule.capabilities.contains(RuntimeCapability::Resume)
                        || (self.id.as_str() == "codex"
                            && rule.launch_mode == RuntimeLaunchMode::Interactive
                            && rule.minimum == crate::CODEX_RESUME_VERSION
                            && rule.maximum_exclusive == crate::CODEX_RESUME_MAXIMUM_EXCLUSIVE))
                    && !rule
                        .capabilities
                        .contains(RuntimeCapability::TranscriptExport)
            })
    }
}

pub fn compiled_runtime_descriptors() -> Vec<RuntimeDescriptor> {
    let interactive = RuntimeCapabilitySet::new([
        RuntimeCapability::InteractivePty,
        RuntimeCapability::Cancellation,
        RuntimeCapability::ContextHandoff,
        RuntimeCapability::RemoteLaunch,
    ]);
    let structured = RuntimeCapabilitySet::new([
        RuntimeCapability::StructuredEvents,
        RuntimeCapability::Cancellation,
        RuntimeCapability::ContextHandoff,
        RuntimeCapability::RemoteLaunch,
    ]);
    let codex_structured = RuntimeCapabilitySet::new([
        RuntimeCapability::StructuredEvents,
        RuntimeCapability::ApprovalRequests,
        RuntimeCapability::Cancellation,
        RuntimeCapability::ContextHandoff,
        RuntimeCapability::RemoteLaunch,
    ]);
    let descriptor = |id: &str,
                      display_name: &str,
                      executable: &str,
                      minimum: RuntimeVersion,
                      maximum_exclusive: RuntimeVersion,
                      structured_capabilities: RuntimeCapabilitySet| {
        RuntimeDescriptor {
            id: RuntimeId::new(id).expect("compiled runtime ID is valid"),
            descriptor_version: RUNTIME_DESCRIPTOR_VERSION,
            display_name: display_name.to_string(),
            kind: RuntimeDescriptorKind::Probed,
            executable_candidates: vec![executable.to_string()],
            version_arguments: vec!["--version".to_string()],
            version_rules: vec![
                RuntimeVersionRule {
                    minimum,
                    maximum_exclusive,
                    launch_mode: RuntimeLaunchMode::Interactive,
                    capabilities: interactive.clone(),
                },
                RuntimeVersionRule {
                    minimum,
                    maximum_exclusive,
                    launch_mode: RuntimeLaunchMode::Structured,
                    capabilities: structured_capabilities,
                },
            ],
        }
    };
    let mut codex = descriptor(
        "codex",
        "Codex",
        "codex",
        RuntimeVersion::new(1, 0, 0),
        RuntimeVersion::new(1, 1, 0),
        codex_structured.clone(),
    );
    codex.version_rules.push(RuntimeVersionRule {
        minimum: crate::CODEX_RESUME_VERSION,
        maximum_exclusive: crate::CODEX_RESUME_MAXIMUM_EXCLUSIVE,
        launch_mode: RuntimeLaunchMode::Interactive,
        capabilities: RuntimeCapabilitySet::new([
            RuntimeCapability::InteractivePty,
            RuntimeCapability::Cancellation,
            RuntimeCapability::ContextHandoff,
            RuntimeCapability::RemoteLaunch,
            RuntimeCapability::Resume,
        ]),
    });
    codex.version_rules.push(RuntimeVersionRule {
        minimum: crate::CODEX_RESUME_VERSION,
        maximum_exclusive: crate::CODEX_RESUME_MAXIMUM_EXCLUSIVE,
        launch_mode: RuntimeLaunchMode::Structured,
        capabilities: codex_structured,
    });
    let mut descriptors = vec![
        descriptor(
            "claude",
            "Claude Code",
            "claude",
            RuntimeVersion::new(2, 0, 0),
            RuntimeVersion::new(2, 1, 0),
            structured.clone(),
        ),
        codex,
        descriptor(
            "gemini",
            "Gemini CLI",
            "gemini",
            RuntimeVersion::new(1, 0, 0),
            RuntimeVersion::new(1, 1, 0),
            structured,
        ),
        RuntimeDescriptor {
            id: RuntimeId::new("generic").expect("compiled runtime ID is valid"),
            descriptor_version: RUNTIME_DESCRIPTOR_VERSION,
            display_name: "Generic command".to_string(),
            kind: RuntimeDescriptorKind::GenericCommand,
            executable_candidates: Vec::new(),
            version_arguments: Vec::new(),
            version_rules: Vec::new(),
        },
    ];
    descriptors.sort_by(|left, right| left.id.cmp(&right.id));
    debug_assert!(descriptors.len() <= MAX_RUNTIME_DESCRIPTORS);
    debug_assert!(descriptors.iter().all(RuntimeDescriptor::validates));
    descriptors
}

pub fn parse_runtime_version(output: &str) -> Option<RuntimeVersion> {
    if output.len() > MAX_RUNTIME_VERSION_BYTES {
        return None;
    }
    output
        .split(|character: char| {
            !(character.is_ascii_digit() || character == '.' || character == '-')
        })
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| {
            let numeric = candidate.split('-').next()?;
            let mut parts = numeric.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            let patch = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some(RuntimeVersion::new(major, minor, patch))
        })
}

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ExecutableFingerprint {
    pub file_identity: u128,
    pub size: u64,
    pub modified_nanos: u64,
    pub bounded_content_hash: u64,
}

impl fmt::Debug for ExecutableFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutableFingerprint")
            .field("file_identity", &"<redacted>")
            .field("size", &self.size)
            .field("modified_nanos", &self.modified_nanos)
            .field("bounded_content_hash", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDetectionStatus {
    Available,
    UnsupportedVersion,
    Missing,
    Partial,
    PermissionDenied,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeDetectionResult {
    pub runtime_id: RuntimeId,
    pub descriptor_version: u16,
    pub status: RuntimeDetectionStatus,
    pub fingerprint: Option<ExecutableFingerprint>,
    pub safe_version: Option<String>,
    pub capabilities: RuntimeCapabilitySet,
    pub diagnostic_code: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ProcessIdentity {
    pub platform_id: u64,
    pub start_identity: u64,
}

impl fmt::Debug for ProcessIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProcessIdentity(<redacted>)")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ObservedProcess {
    pub identity: ProcessIdentity,
    pub runtime_id: Option<RuntimeId>,
    pub executable: ExecutableFingerprint,
    pub host_token: Option<ProcessToken>,
    pub descends_from_host: bool,
}

impl fmt::Debug for ObservedProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedProcess")
            .field("identity", &self.identity)
            .field("runtime_id", &self.runtime_id)
            .field("executable", &self.executable)
            .field("host_token", &self.host_token.map(|_| "<redacted>"))
            .field("descends_from_host", &self.descends_from_host)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessObservationStatus {
    Available,
    PermissionDenied,
    TimedOut,
    Unavailable,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessObservation {
    pub observed_at_nanos: u64,
    pub status: ProcessObservationStatus,
    pub candidates: Vec<ObservedProcess>,
}

impl fmt::Debug for ProcessObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessObservation")
            .field("observed_at_nanos", &self.observed_at_nanos)
            .field("status", &self.status)
            .field("candidate_count", &self.candidates.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionConfidence {
    Verified,
    Observed,
    Uncertain,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeRecognition {
    pub occupant: Option<RuntimeOccupant>,
    pub confidence: RecognitionConfidence,
    pub observed_at_nanos: u64,
}

impl fmt::Debug for RuntimeDetectionResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeDetectionResult")
            .field("runtime_id", &self.runtime_id)
            .field("descriptor_version", &self.descriptor_version)
            .field("status", &self.status)
            .field("fingerprint", &self.fingerprint)
            .field("safe_version", &self.safe_version)
            .field("capabilities", &self.capabilities)
            .field("diagnostic_code", &self.diagnostic_code)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct OccupantGeneration(u64);

impl OccupantGeneration {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OccupantOwnership {
    Managed {
        host_instance: HostInstanceId,
        child_token: ProcessToken,
    },
    Observed {
        executable: ExecutableFingerprint,
    },
    Ambiguous,
}

impl fmt::Debug for OccupantOwnership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Managed { host_instance, .. } => formatter
                .debug_struct("Managed")
                .field("host_instance", host_instance)
                .field("child_token", &"<redacted>")
                .finish(),
            Self::Observed { .. } => formatter.write_str("Observed(<redacted>)"),
            Self::Ambiguous => formatter.write_str("Ambiguous"),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeOccupant {
    pub runtime_id: RuntimeId,
    pub descriptor_version: u16,
    pub safe_version: Option<String>,
    #[serde(default)]
    pub executable_fingerprint: Option<ExecutableFingerprint>,
    pub generation: OccupantGeneration,
    pub ownership: OccupantOwnership,
    pub capabilities: RuntimeCapabilitySet,
    pub stale: bool,
}

impl RuntimeOccupant {
    pub fn effective_capabilities(&self) -> RuntimeCapabilitySet {
        self.capabilities.effective_for(&self.ownership)
    }
}

impl fmt::Debug for RuntimeOccupant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeOccupant")
            .field("runtime_id", &self.runtime_id)
            .field("descriptor_version", &self.descriptor_version)
            .field("safe_version", &self.safe_version)
            .field("executable_fingerprint", &self.executable_fingerprint)
            .field("generation", &self.generation)
            .field("ownership", &self.ownership)
            .field("capabilities", &self.capabilities)
            .field("stale", &self.stale)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_registry_is_stable_bounded_and_has_only_exact_codex_resume_claim() {
        let descriptors = compiled_runtime_descriptors();
        assert_eq!(
            descriptors
                .iter()
                .map(|descriptor| descriptor.id.as_str())
                .collect::<Vec<_>>(),
            vec!["claude", "codex", "gemini", "generic"]
        );
        assert!(descriptors.iter().all(RuntimeDescriptor::validates));
        let resume_rules = descriptors
            .iter()
            .flat_map(|descriptor| {
                descriptor.version_rules.iter().filter_map(move |rule| {
                    rule.capabilities
                        .contains(RuntimeCapability::Resume)
                        .then_some((descriptor.id.as_str(), rule))
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(resume_rules.len(), 1);
        assert_eq!(resume_rules[0].0, "codex");
        assert_eq!(resume_rules[0].1.minimum, crate::CODEX_RESUME_VERSION);
        assert_eq!(
            resume_rules[0].1.maximum_exclusive,
            crate::CODEX_RESUME_MAXIMUM_EXCLUSIVE
        );
    }

    #[test]
    fn runtime_version_truth_table_is_exact_at_both_boundaries() {
        for descriptor in compiled_runtime_descriptors()
            .into_iter()
            .filter(|descriptor| descriptor.kind == RuntimeDescriptorKind::Probed)
        {
            let lower = descriptor.version_rules[0].minimum;
            let upper = descriptor.version_rules[0].maximum_exclusive;
            assert!(!descriptor.capabilities_for(lower).is_empty());
            assert!(descriptor.capabilities_for(upper).is_empty());
            assert!(
                descriptor
                    .capabilities_for(RuntimeVersion::new(0, 0, 0))
                    .is_empty()
            );
        }
    }

    #[test]
    fn runtime_parser_is_total_bounded_and_returns_only_safe_numeric_version() {
        assert_eq!(
            parse_runtime_version("codex-cli 1.0.7 (fixture)"),
            Some(RuntimeVersion::new(1, 0, 7))
        );
        assert_eq!(parse_runtime_version("not a version"), None);
        assert_eq!(
            parse_runtime_version(&"1".repeat(MAX_RUNTIME_VERSION_BYTES + 1)),
            None
        );
    }

    #[test]
    fn runtime_observed_and_ambiguous_occupants_receive_no_effective_capabilities() {
        let capabilities = RuntimeCapabilitySet::new([
            RuntimeCapability::InteractivePty,
            RuntimeCapability::Cancellation,
        ]);
        let fingerprint = ExecutableFingerprint {
            file_identity: 7,
            size: 8,
            modified_nanos: 9,
            bounded_content_hash: 10,
        };
        for ownership in [
            OccupantOwnership::Observed {
                executable: fingerprint,
            },
            OccupantOwnership::Ambiguous,
        ] {
            let occupant = RuntimeOccupant {
                runtime_id: RuntimeId::new("codex").unwrap(),
                descriptor_version: 1,
                safe_version: Some("1.0.0".to_string()),
                executable_fingerprint: Some(fingerprint),
                generation: OccupantGeneration::new(1),
                ownership,
                capabilities: capabilities.clone(),
                stale: false,
            };
            assert!(occupant.effective_capabilities().is_empty());
        }
    }

    #[test]
    fn runtime_debug_does_not_expose_executable_identity() {
        let fingerprint = ExecutableFingerprint {
            file_identity: 123_456,
            size: 8,
            modified_nanos: 9,
            bounded_content_hash: 654_321,
        };
        let debug = format!("{fingerprint:?}");
        assert!(!debug.contains("123456"));
        assert!(!debug.contains("654321"));
    }
}
