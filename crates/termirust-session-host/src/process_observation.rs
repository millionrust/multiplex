use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash as _, Hasher as _};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime};

use termirust_domain::{
    OccupantGeneration, OccupantOwnership, ProcessIdentity, ProcessObservation,
    ProcessObservationStatus, RecognitionConfidence, RuntimeCapabilitySet, RuntimeDetectionResult,
    RuntimeDetectionStatus, RuntimeOccupant, RuntimeRecognition,
};

pub const ACTIVE_OBSERVATION_INTERVAL: Duration = Duration::from_millis(500);
pub const IDLE_OBSERVATION_INTERVAL: Duration = Duration::from_secs(5);
pub const PLATFORM_OBSERVATION_BUDGET: Duration = Duration::from_millis(250);
pub const OBSERVATION_STALE_WINDOW: Duration = Duration::from_secs(5);
pub const MAX_OBSERVED_PROCESSES: usize = 16;
const FINGERPRINT_SAMPLE_BYTES: usize = 64 * 1024;

pub fn fingerprint_executable(
    path: &Path,
) -> std::io::Result<termirust_domain::ExecutableFingerprint> {
    let metadata = path.metadata()?;
    let mut file = File::open(path)?;
    let mut hasher = DefaultHasher::new();
    let mut buffer = vec![0_u8; FINGERPRINT_SAMPLE_BYTES];
    let first = file.read(&mut buffer)?;
    buffer[..first].hash(&mut hasher);
    if metadata.len() > FINGERPRINT_SAMPLE_BYTES as u64 {
        file.seek(SeekFrom::End(-(FINGERPRINT_SAMPLE_BYTES as i64)))?;
        let last = file.read(&mut buffer)?;
        buffer[..last].hash(&mut hasher);
    }
    #[cfg(unix)]
    let file_identity = {
        use std::os::unix::fs::MetadataExt as _;
        (u128::from(metadata.dev()) << 64) | u128::from(metadata.ino())
    };
    #[cfg(not(unix))]
    let file_identity = 0;
    let modified_nanos = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64;
    Ok(termirust_domain::ExecutableFingerprint {
        file_identity,
        size: metadata.len(),
        modified_nanos,
        bounded_content_hash: hasher.finish(),
    })
}

pub trait PlatformProcessObserver {
    fn observe(&mut self, budget: Duration) -> ProcessObservation;
}

pub struct ProcessObservationEngine {
    expected_token: termirust_domain::ProcessToken,
    last_attempt_nanos: Option<u64>,
    last_success_nanos: Option<u64>,
    occupant_identity: Option<ProcessIdentity>,
    recognition: Option<RuntimeRecognition>,
}

impl ProcessObservationEngine {
    pub fn new(expected_token: termirust_domain::ProcessToken) -> Self {
        Self {
            expected_token,
            last_attempt_nanos: None,
            last_success_nanos: None,
            occupant_identity: None,
            recognition: None,
        }
    }

    pub fn recognition(&self) -> Option<&RuntimeRecognition> {
        self.recognition.as_ref()
    }

    pub fn sample(
        &mut self,
        platform: &mut impl PlatformProcessObserver,
        detection: Option<&RuntimeDetectionResult>,
        active: bool,
        now_nanos: u64,
    ) -> Option<&RuntimeRecognition> {
        let interval = if active {
            ACTIVE_OBSERVATION_INTERVAL
        } else {
            IDLE_OBSERVATION_INTERVAL
        };
        if self
            .last_attempt_nanos
            .is_some_and(|last| now_nanos.saturating_sub(last) < duration_nanos(interval))
        {
            return self.recognition.as_ref();
        }
        self.last_attempt_nanos = Some(now_nanos);
        let observation = platform.observe(PLATFORM_OBSERVATION_BUDGET);
        self.apply(observation, detection, now_nanos);
        self.recognition.as_ref()
    }

    pub fn apply(
        &mut self,
        mut observation: ProcessObservation,
        detection: Option<&RuntimeDetectionResult>,
        now_nanos: u64,
    ) {
        observation.candidates.truncate(MAX_OBSERVED_PROCESSES);
        if observation.status != ProcessObservationStatus::Available {
            self.apply_failure(now_nanos);
            return;
        }
        self.last_success_nanos = Some(now_nanos);
        let Some(detection) = detection.filter(|detection| {
            detection.status == RuntimeDetectionStatus::Available
                && detection.fingerprint.is_some()
                && !detection.capabilities.is_empty()
        }) else {
            self.occupant_identity = None;
            self.recognition = Some(RuntimeRecognition {
                occupant: None,
                confidence: RecognitionConfidence::Uncertain,
                observed_at_nanos: now_nanos,
            });
            return;
        };
        let fingerprint = detection.fingerprint.expect("filtered above");
        let mut candidates = observation
            .candidates
            .into_iter()
            .filter(|candidate| {
                candidate.runtime_id.as_ref() == Some(&detection.runtime_id)
                    && candidate.executable == fingerprint
            })
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            let identity = candidates.first().map(|candidate| candidate.identity);
            self.set_occupant(
                detection,
                identity,
                OccupantOwnership::Ambiguous,
                RecognitionConfidence::Uncertain,
                now_nanos,
                true,
            );
            return;
        }
        let candidate = candidates.remove(0);
        let pid_reused = self.occupant_identity.is_some_and(|previous| {
            previous.platform_id == candidate.identity.platform_id
                && previous.start_identity != candidate.identity.start_identity
        });
        let ownership = if pid_reused {
            OccupantOwnership::Ambiguous
        } else if candidate.host_token == Some(self.expected_token) && candidate.descends_from_host
        {
            OccupantOwnership::Managed {
                host_instance: self.expected_token.host_instance(),
                child_token: self.expected_token,
            }
        } else {
            OccupantOwnership::Observed {
                executable: candidate.executable,
            }
        };
        let confidence = match ownership {
            OccupantOwnership::Managed { .. } => RecognitionConfidence::Verified,
            OccupantOwnership::Observed { .. } => RecognitionConfidence::Observed,
            OccupantOwnership::Ambiguous => RecognitionConfidence::Uncertain,
        };
        self.set_occupant(
            detection,
            Some(candidate.identity),
            ownership,
            confidence,
            now_nanos,
            pid_reused,
        );
    }

    fn set_occupant(
        &mut self,
        detection: &RuntimeDetectionResult,
        identity: Option<ProcessIdentity>,
        ownership: OccupantOwnership,
        confidence: RecognitionConfidence,
        now_nanos: u64,
        force_generation: bool,
    ) {
        let previous = self
            .recognition
            .as_ref()
            .and_then(|recognition| recognition.occupant.as_ref())
            .map(|occupant| occupant.generation)
            .unwrap_or(OccupantGeneration::ZERO);
        let generation = if previous == OccupantGeneration::ZERO
            || force_generation
            || self.occupant_identity != identity
        {
            previous.next()
        } else {
            previous
        };
        self.occupant_identity = identity;
        self.recognition = Some(RuntimeRecognition {
            occupant: Some(RuntimeOccupant {
                runtime_id: detection.runtime_id.clone(),
                descriptor_version: detection.descriptor_version,
                safe_version: detection.safe_version.clone(),
                executable_fingerprint: detection.fingerprint,
                generation,
                ownership,
                capabilities: detection.capabilities.clone(),
                stale: false,
            }),
            confidence,
            observed_at_nanos: now_nanos,
        });
    }

    fn apply_failure(&mut self, now_nanos: u64) {
        let stale = self.last_success_nanos.is_some_and(|last| {
            now_nanos.saturating_sub(last) <= duration_nanos(OBSERVATION_STALE_WINDOW)
        });
        if let Some(recognition) = self.recognition.as_mut() {
            recognition.confidence = RecognitionConfidence::Uncertain;
            recognition.observed_at_nanos = now_nanos;
            if let Some(occupant) = recognition.occupant.as_mut() {
                occupant.stale = stale;
                if !stale {
                    occupant.ownership = OccupantOwnership::Ambiguous;
                    occupant.capabilities = RuntimeCapabilitySet::default();
                }
            }
        }
    }
}

fn duration_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::{
        ExecutableFingerprint, HostInstanceId, ObservedProcess, ProcessObservationStatus,
        RuntimeCapability, RuntimeCapabilitySet, RuntimeId,
    };

    struct FakePlatform {
        observations: std::collections::VecDeque<ProcessObservation>,
        calls: usize,
        budgets: Vec<Duration>,
    }

    impl PlatformProcessObserver for FakePlatform {
        fn observe(&mut self, budget: Duration) -> ProcessObservation {
            self.calls += 1;
            self.budgets.push(budget);
            self.observations.pop_front().unwrap()
        }
    }

    fn fingerprint(value: u64) -> ExecutableFingerprint {
        ExecutableFingerprint {
            file_identity: u128::from(value),
            size: value,
            modified_nanos: value,
            bounded_content_hash: value,
        }
    }

    fn detection() -> RuntimeDetectionResult {
        RuntimeDetectionResult {
            runtime_id: RuntimeId::new("codex").unwrap(),
            descriptor_version: 1,
            status: RuntimeDetectionStatus::Available,
            fingerprint: Some(fingerprint(7)),
            safe_version: Some("1.0.7".to_string()),
            capabilities: RuntimeCapabilitySet::new([
                RuntimeCapability::InteractivePty,
                RuntimeCapability::Cancellation,
            ]),
            diagnostic_code: None,
        }
    }

    fn observation(
        identity: ProcessIdentity,
        token: Option<termirust_domain::ProcessToken>,
        descends: bool,
    ) -> ProcessObservation {
        ProcessObservation {
            observed_at_nanos: 0,
            status: ProcessObservationStatus::Available,
            candidates: vec![ObservedProcess {
                identity,
                runtime_id: Some(RuntimeId::new("codex").unwrap()),
                executable: fingerprint(7),
                host_token: token,
                descends_from_host: descends,
            }],
        }
    }

    fn unavailable() -> ProcessObservation {
        ProcessObservation {
            observed_at_nanos: 0,
            status: ProcessObservationStatus::PermissionDenied,
            candidates: Vec::new(),
        }
    }

    #[test]
    fn process_observation_managed_requires_exact_host_token_and_descendant() {
        let host = HostInstanceId::new();
        let token = termirust_domain::ProcessToken::new(host, 42, 1);
        let identity = ProcessIdentity {
            platform_id: 42,
            start_identity: 100,
        };
        let mut engine = ProcessObservationEngine::new(token);
        engine.apply(
            observation(identity, Some(token), true),
            Some(&detection()),
            1,
        );
        let recognition = engine.recognition().unwrap();
        assert_eq!(recognition.confidence, RecognitionConfidence::Verified);
        assert!(matches!(
            recognition.occupant.as_ref().unwrap().ownership,
            OccupantOwnership::Managed { .. }
        ));
        assert!(
            !recognition
                .occupant
                .as_ref()
                .unwrap()
                .effective_capabilities()
                .is_empty()
        );

        engine.apply(observation(identity, None, true), Some(&detection()), 2);
        let occupant = engine.recognition().unwrap().occupant.as_ref().unwrap();
        assert!(matches!(
            occupant.ownership,
            OccupantOwnership::Observed { .. }
        ));
        assert!(occupant.effective_capabilities().is_empty());
    }

    #[test]
    fn process_observation_pid_reuse_reparent_and_multiple_candidates_fail_closed() {
        let host = HostInstanceId::new();
        let token = termirust_domain::ProcessToken::new(host, 42, 1);
        let mut engine = ProcessObservationEngine::new(token);
        let first = ProcessIdentity {
            platform_id: 42,
            start_identity: 100,
        };
        engine.apply(observation(first, Some(token), true), Some(&detection()), 1);
        let first_generation = engine
            .recognition()
            .unwrap()
            .occupant
            .as_ref()
            .unwrap()
            .generation;

        let reused = ProcessIdentity {
            platform_id: 42,
            start_identity: 101,
        };
        engine.apply(
            observation(reused, Some(token), true),
            Some(&detection()),
            2,
        );
        let occupant = engine.recognition().unwrap().occupant.as_ref().unwrap();
        assert!(matches!(occupant.ownership, OccupantOwnership::Ambiguous));
        assert!(occupant.generation > first_generation);

        engine.apply(
            observation(reused, Some(token), false),
            Some(&detection()),
            3,
        );
        assert!(matches!(
            engine
                .recognition()
                .unwrap()
                .occupant
                .as_ref()
                .unwrap()
                .ownership,
            OccupantOwnership::Observed { .. }
        ));

        let mut multiple = observation(reused, None, false);
        multiple.candidates.push(multiple.candidates[0].clone());
        engine.apply(multiple, Some(&detection()), 4);
        assert!(matches!(
            engine
                .recognition()
                .unwrap()
                .occupant
                .as_ref()
                .unwrap()
                .ownership,
            OccupantOwnership::Ambiguous
        ));
    }

    #[test]
    fn process_observation_failure_is_stale_for_five_seconds_then_ambiguous() {
        let host = HostInstanceId::new();
        let token = termirust_domain::ProcessToken::new(host, 42, 1);
        let identity = ProcessIdentity {
            platform_id: 42,
            start_identity: 100,
        };
        let mut engine = ProcessObservationEngine::new(token);
        engine.apply(
            observation(identity, Some(token), true),
            Some(&detection()),
            1,
        );
        engine.apply(unavailable(), Some(&detection()), 4_000_000_001);
        assert!(
            engine
                .recognition()
                .unwrap()
                .occupant
                .as_ref()
                .unwrap()
                .stale
        );
        engine.apply(unavailable(), Some(&detection()), 5_000_000_002);
        let occupant = engine.recognition().unwrap().occupant.as_ref().unwrap();
        assert!(!occupant.stale);
        assert!(matches!(occupant.ownership, OccupantOwnership::Ambiguous));
        assert!(occupant.capabilities.is_empty());
    }

    #[test]
    fn process_observation_sampling_honors_active_idle_rate_and_platform_budget() {
        let host = HostInstanceId::new();
        let token = termirust_domain::ProcessToken::new(host, 42, 1);
        let identity = ProcessIdentity {
            platform_id: 42,
            start_identity: 100,
        };
        let mut platform = FakePlatform {
            observations: [
                observation(identity, Some(token), true),
                observation(identity, Some(token), true),
                observation(identity, Some(token), true),
            ]
            .into_iter()
            .collect(),
            calls: 0,
            budgets: Vec::new(),
        };
        let mut engine = ProcessObservationEngine::new(token);
        engine.sample(&mut platform, Some(&detection()), true, 0);
        engine.sample(&mut platform, Some(&detection()), true, 499_000_000);
        engine.sample(&mut platform, Some(&detection()), true, 500_000_000);
        engine.sample(&mut platform, Some(&detection()), false, 5_499_000_000);
        engine.sample(&mut platform, Some(&detection()), false, 5_500_000_000);
        assert_eq!(platform.calls, 3);
        assert!(
            platform
                .budgets
                .iter()
                .all(|budget| *budget == PLATFORM_OBSERVATION_BUDGET)
        );
    }

    #[test]
    fn process_observation_unverified_detection_never_inherits_capabilities() {
        let host = HostInstanceId::new();
        let token = termirust_domain::ProcessToken::new(host, 42, 1);
        let identity = ProcessIdentity {
            platform_id: 42,
            start_identity: 100,
        };
        let mut engine = ProcessObservationEngine::new(token);
        let mut unsupported = detection();
        unsupported.status = RuntimeDetectionStatus::UnsupportedVersion;
        unsupported.capabilities = RuntimeCapabilitySet::default();
        engine.apply(
            observation(identity, Some(token), true),
            Some(&unsupported),
            1,
        );
        assert!(engine.recognition().unwrap().occupant.is_none());
    }
}
