use std::collections::VecDeque;
use std::fmt;

use termirust_domain::{
    DevUrlCancellation, DevUrlCandidate, DevUrlDetector, DevUrlDetectorCounters, DevUrlError,
    DevUrlPolicy, HostInstanceId, HostedSessionId, LocalDevUrl, OpenUrlError, OutputSequence,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DevUrlProjectionUpdate {
    Observed {
        accepted: usize,
    },
    Duplicate,
    Gap {
        expected: OutputSequence,
        received: OutputSequence,
        accepted: usize,
    },
}

#[derive(Clone)]
pub struct DevUrlProjection {
    session_id: HostedSessionId,
    host_instance: Option<HostInstanceId>,
    last_sequence: OutputSequence,
    candidates: VecDeque<DevUrlCandidate>,
    next_id: u64,
    partial: bool,
    host_available: bool,
    detector: DevUrlDetector,
    cancellation: DevUrlCancellation,
    policy: DevUrlPolicy,
}

impl fmt::Debug for DevUrlProjection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevUrlProjection")
            .field("session_id", &self.session_id)
            .field("host_instance", &self.host_instance)
            .field("last_sequence", &self.last_sequence)
            .field("candidate_count", &self.candidates.len())
            .field("next_id", &self.next_id)
            .field("partial", &self.partial)
            .field("host_available", &self.host_available)
            .field("detector", &self.detector)
            .finish()
    }
}

impl DevUrlProjection {
    pub fn new(session_id: HostedSessionId) -> Self {
        Self::with_policy(session_id, DevUrlPolicy::default())
            .expect("default URL projection policy must be valid")
    }

    pub fn with_policy(
        session_id: HostedSessionId,
        policy: DevUrlPolicy,
    ) -> Result<Self, DevUrlError> {
        policy.validate()?;
        Ok(Self {
            session_id,
            host_instance: None,
            last_sequence: OutputSequence::ZERO,
            candidates: VecDeque::with_capacity(policy.maximum_candidates),
            next_id: 1,
            partial: false,
            host_available: false,
            detector: DevUrlDetector::new(policy)?,
            cancellation: DevUrlCancellation::default(),
            policy,
        })
    }

    pub const fn session_id(&self) -> HostedSessionId {
        self.session_id
    }

    pub const fn host_instance(&self) -> Option<HostInstanceId> {
        self.host_instance
    }

    pub const fn last_sequence(&self) -> OutputSequence {
        self.last_sequence
    }

    pub const fn is_partial(&self) -> bool {
        self.partial
    }

    pub const fn host_available(&self) -> bool {
        self.host_available
    }

    pub fn candidates(&self) -> impl DoubleEndedIterator<Item = &DevUrlCandidate> {
        self.candidates.iter()
    }

    pub fn latest(&self) -> Option<&DevUrlCandidate> {
        self.candidates.back()
    }

    pub fn counters(&self) -> DevUrlDetectorCounters {
        self.detector.counters()
    }

    pub fn observe(
        &mut self,
        host_instance: HostInstanceId,
        sequence: OutputSequence,
        bytes: &[u8],
    ) -> Result<DevUrlProjectionUpdate, DevUrlError> {
        self.bind_host(host_instance);
        self.host_available = true;
        if sequence <= self.last_sequence {
            return Ok(DevUrlProjectionUpdate::Duplicate);
        }

        let gap = self
            .last_sequence
            .checked_next()
            .filter(|_| self.last_sequence != OutputSequence::ZERO)
            .filter(|expected| *expected != sequence);
        if gap.is_some() {
            self.partial = true;
            self.detector.reset_carry();
        }
        self.last_sequence = sequence;
        let urls = self.detector.observe(bytes, &self.cancellation)?;
        let accepted = urls.len();
        for url in urls {
            self.insert_candidate(host_instance, sequence, url);
        }
        Ok(match gap {
            Some(expected) => DevUrlProjectionUpdate::Gap {
                expected,
                received: sequence,
                accepted,
            },
            None => DevUrlProjectionUpdate::Observed { accepted },
        })
    }

    pub fn bind_available_host(&mut self, host_instance: HostInstanceId) {
        self.bind_host(host_instance);
        self.host_available = true;
    }

    pub fn apply_snapshot(&mut self, host_instance: HostInstanceId, boundary: OutputSequence) {
        self.bind_host(host_instance);
        self.candidates.clear();
        self.detector.reset_carry();
        self.last_sequence = boundary;
        self.partial = true;
        self.host_available = true;
    }

    pub fn mark_gap(&mut self) {
        self.partial = true;
        self.detector.reset_carry();
    }

    pub fn mark_host_unavailable(&mut self) {
        self.host_available = false;
        self.detector.reset_carry();
    }

    pub fn dismiss(&mut self, candidate_id: u64) -> bool {
        let Some(index) = self
            .candidates
            .iter()
            .position(|candidate| candidate.id == candidate_id)
        else {
            return false;
        };
        self.candidates.remove(index);
        true
    }

    pub fn clear(&mut self) {
        self.candidates.clear();
    }

    pub fn resolve_for_open(
        &self,
        session_id: HostedSessionId,
        host_instance: HostInstanceId,
        candidate_id: u64,
    ) -> Result<LocalDevUrl, OpenUrlError> {
        if session_id != self.session_id {
            return Err(OpenUrlError::SessionUnavailable);
        }
        if self.host_instance != Some(host_instance) {
            return Err(OpenUrlError::StaleHost);
        }
        if !self.host_available {
            return Err(OpenUrlError::SessionUnavailable);
        }
        let candidate = self
            .candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
            .ok_or(OpenUrlError::Invalidated)?;
        if candidate.session_id != session_id || candidate.host_instance != host_instance {
            return Err(OpenUrlError::Invalidated);
        }
        let revalidated = candidate
            .normalized_url
            .revalidate(self.policy)
            .map_err(|_| OpenUrlError::Invalidated)?;
        if revalidated != candidate.normalized_url {
            return Err(OpenUrlError::Invalidated);
        }
        Ok(revalidated)
    }

    fn bind_host(&mut self, host_instance: HostInstanceId) {
        if self.host_instance == Some(host_instance) {
            return;
        }
        self.host_instance = Some(host_instance);
        self.last_sequence = OutputSequence::ZERO;
        self.candidates.clear();
        self.next_id = 1;
        self.partial = false;
        self.host_available = true;
        self.detector.reset_carry();
    }

    fn insert_candidate(
        &mut self,
        host_instance: HostInstanceId,
        sequence: OutputSequence,
        url: LocalDevUrl,
    ) {
        if let Some(index) = self
            .candidates
            .iter()
            .position(|candidate| candidate.normalized_url == url)
        {
            if let Some(mut existing) = self.candidates.remove(index) {
                existing.output_sequence = sequence;
                self.candidates.push_back(existing);
            }
            return;
        }
        if self.candidates.len() == self.policy.maximum_candidates {
            self.candidates.pop_front();
        }
        let Some(next_id) = self.next_id.checked_add(1) else {
            // Disable actions rather than reuse an ID that a pending click may still reference.
            self.candidates.clear();
            self.host_available = false;
            return;
        };
        let id = self.next_id;
        self.next_id = next_id;
        self.candidates.push_back(DevUrlCandidate {
            id,
            session_id: self.session_id,
            host_instance,
            output_sequence: sequence,
            display_origin: url.display_origin().to_string(),
            has_hidden_query: url.has_hidden_query(),
            normalized_url: url,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn session(value: u128) -> HostedSessionId {
        HostedSessionId::from_uuid(Uuid::from_u128(value))
    }

    fn host(value: u128) -> HostInstanceId {
        HostInstanceId::from_uuid(Uuid::from_u128(value))
    }

    #[test]
    fn dev_url_projection_binds_candidates_to_session_host_and_sequence() {
        let mut projection = DevUrlProjection::new(session(1));
        projection
            .observe(host(2), OutputSequence::new(1), b"http://localhost:3000\n")
            .unwrap();
        let candidate = projection.latest().unwrap().clone();
        assert_eq!(candidate.output_sequence, OutputSequence::new(1));
        assert_eq!(
            projection
                .resolve_for_open(session(1), host(2), candidate.id)
                .unwrap()
                .display_origin(),
            "localhost:3000"
        );
        assert_eq!(
            projection.resolve_for_open(session(1), host(3), candidate.id),
            Err(OpenUrlError::StaleHost)
        );
        assert_eq!(
            projection.resolve_for_open(session(9), host(2), candidate.id),
            Err(OpenUrlError::SessionUnavailable)
        );
    }

    #[test]
    fn dev_url_projection_gap_snapshot_and_host_replacement_fail_closed() {
        let mut projection = DevUrlProjection::new(session(1));
        projection
            .observe(host(2), OutputSequence::new(1), b"http://localhost:3000\n")
            .unwrap();
        let old = projection.latest().unwrap().clone();
        assert!(matches!(
            projection
                .observe(host(2), OutputSequence::new(3), b"http://localhost:4000\n")
                .unwrap(),
            DevUrlProjectionUpdate::Gap { .. }
        ));
        assert!(projection.is_partial());
        projection.apply_snapshot(host(2), OutputSequence::new(10));
        assert!(projection.candidates().next().is_none());
        projection
            .observe(host(3), OutputSequence::new(1), b"http://localhost:5000\n")
            .unwrap();
        assert_eq!(projection.host_instance(), Some(host(3)));
        assert_eq!(projection.candidates().count(), 1);
        assert_eq!(
            projection.resolve_for_open(session(1), host(2), old.id),
            Err(OpenUrlError::StaleHost)
        );
    }

    #[test]
    fn dev_url_projection_lru_dedupe_dismiss_and_unavailable_are_bounded() {
        let mut projection = DevUrlProjection::new(session(1));
        for sequence in 1..=65 {
            projection
                .observe(
                    host(2),
                    OutputSequence::new(sequence),
                    format!("http://localhost:{}/\n", 3000 + sequence).as_bytes(),
                )
                .unwrap();
        }
        assert_eq!(
            projection.candidates().count(),
            termirust_domain::MAX_DEV_URL_CANDIDATES
        );
        assert_eq!(
            projection.candidates().next().unwrap().display_origin,
            "localhost:3002"
        );

        let latest = projection.latest().unwrap().clone();
        projection
            .observe(
                host(2),
                OutputSequence::new(66),
                format!("{}\n", latest.normalized_url.as_str()).as_bytes(),
            )
            .unwrap();
        assert_eq!(
            projection.candidates().count(),
            termirust_domain::MAX_DEV_URL_CANDIDATES
        );
        assert_eq!(projection.latest().unwrap().id, latest.id);
        assert!(projection.dismiss(latest.id));
        assert!(!projection.dismiss(latest.id));

        let remaining = projection.latest().unwrap().clone();
        projection.mark_host_unavailable();
        assert_eq!(
            projection.resolve_for_open(session(1), host(2), remaining.id),
            Err(OpenUrlError::SessionUnavailable)
        );
        projection.clear();
        assert!(projection.candidates().next().is_none());
    }

    #[test]
    fn dev_url_projection_debug_never_contains_full_sensitive_urls() {
        let mut projection = DevUrlProjection::new(session(1));
        projection
            .observe(
                host(2),
                OutputSequence::new(1),
                b"http://localhost:3000/private?canary-secret\n",
            )
            .unwrap();
        let debug = format!("{projection:?}");
        assert!(!debug.contains("private"));
        assert!(!debug.contains("canary-secret"));
    }

    #[test]
    fn dev_url_projection_clears_old_bindings_before_id_space_reuse() {
        let mut projection = DevUrlProjection::new(session(1));
        projection
            .observe(host(2), OutputSequence::new(1), b"http://localhost:3000\n")
            .unwrap();
        let old_id = projection.latest().unwrap().id;
        projection.next_id = u64::MAX;
        projection
            .observe(host(2), OutputSequence::new(2), b"http://localhost:4000\n")
            .unwrap();
        assert_eq!(projection.candidates().count(), 0);
        assert!(!projection.host_available());
        assert_eq!(projection.next_id, u64::MAX);
        assert_eq!(
            projection.resolve_for_open(session(1), host(2), old_id),
            Err(OpenUrlError::SessionUnavailable)
        );
    }
}
