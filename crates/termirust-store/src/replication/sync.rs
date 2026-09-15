use std::collections::BTreeMap;
use std::fmt;

use sha2::{Digest as _, Sha256};
use termirust_domain::{
    ReplicatedVersion, ReplicationDocument, ReplicationOperation, ReplicationPolicy,
    ReplicationRecordKey, ReplicationReplicaId, ReplicationVersionVector,
    merge_replication_documents,
};

use super::{
    ReplicationContentRevision, ReplicationRepository, ReplicationRepositoryRevision,
    ReplicationRepositorySnapshot, ReplicationRepositorySource, ReplicationStoreError,
    SharedFolderReplicationInputs, SharedFolderReplicationTransport, SharedFolderTransportSnapshot,
    SharedFolderTransportState, canonical_replication,
};

const SYNC_REVIEW_DOMAIN: &[u8] = b"termirust-replication-sync-review-v1\0";

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplicationSyncReviewToken([u8; 32]);

impl ReplicationSyncReviewToken {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ReplicationSyncReviewToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReplicationSyncReviewToken(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationSyncDisposition {
    InSync,
    PublishLocal,
    UpdateLocal,
    Converge,
    ConflictReviewRequired,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationConflictOperationMix {
    PutOnly,
    DeleteOnly,
    Mixed,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationConflictReview {
    key: ReplicationRecordKey,
    pub candidate_count: u8,
    pub operation_mix: ReplicationConflictOperationMix,
}

impl ReplicationConflictReview {
    pub fn key(&self) -> &ReplicationRecordKey {
        &self.key
    }
}

impl fmt::Debug for ReplicationConflictReview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationConflictReview")
            .field("key", &"<opaque>")
            .field("candidate_count", &self.candidate_count)
            .field("operation_mix", &self.operation_mix)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationResolutionContext {
    key: ReplicationRecordKey,
    author: ReplicationReplicaId,
    vector: ReplicationVersionVector,
    pub candidate_count: u8,
}

impl ReplicationResolutionContext {
    pub fn key(&self) -> &ReplicationRecordKey {
        &self.key
    }

    pub fn author(&self) -> &ReplicationReplicaId {
        &self.author
    }

    pub fn vector(&self) -> &ReplicationVersionVector {
        &self.vector
    }
}

impl fmt::Debug for ReplicationResolutionContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationResolutionContext")
            .field("key", &"<opaque>")
            .field("author", &"<opaque>")
            .field("vector_entries", &self.vector.len())
            .field("candidate_count", &self.candidate_count)
            .finish()
    }
}

#[derive(Clone)]
pub struct ReplicationConflictResolution {
    key: ReplicationRecordKey,
    version: ReplicatedVersion,
}

impl ReplicationConflictResolution {
    pub fn new(key: ReplicationRecordKey, version: ReplicatedVersion) -> Self {
        Self { key, version }
    }

    pub fn key(&self) -> &ReplicationRecordKey {
        &self.key
    }

    pub fn version(&self) -> &ReplicatedVersion {
        &self.version
    }
}

impl fmt::Debug for ReplicationConflictResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationConflictResolution")
            .field("key", &"<opaque>")
            .field("author", &"<opaque>")
            .field("vector_entries", &self.version.vector.len())
            .finish()
    }
}

#[derive(Clone)]
pub struct ReplicationSyncPlan {
    token: ReplicationSyncReviewToken,
    disposition: ReplicationSyncDisposition,
    local: ReplicationRepositorySnapshot,
    transport: SharedFolderReplicationInputs,
    merged: ReplicationDocument,
    conflicts: Vec<ReplicationConflictReview>,
}

impl ReplicationSyncPlan {
    pub fn token(&self) -> ReplicationSyncReviewToken {
        self.token
    }

    pub fn disposition(&self) -> ReplicationSyncDisposition {
        self.disposition
    }

    pub fn local_revision(&self) -> ReplicationRepositoryRevision {
        self.local.revision
    }

    pub fn transport_state(&self) -> SharedFolderTransportState {
        transport_state(&self.transport)
    }

    pub fn conflicts(&self) -> &[ReplicationConflictReview] {
        &self.conflicts
    }

    pub(super) fn candidates_for(
        &self,
        key: &ReplicationRecordKey,
    ) -> Option<&[ReplicatedVersion]> {
        self.merged
            .entries
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.candidates.as_slice())
    }

    pub fn provider_conflict_count(&self) -> usize {
        self.transport.conflicts.len()
    }

    pub fn resolution_context(
        &self,
        key: &ReplicationRecordKey,
        author: &ReplicationReplicaId,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationResolutionContext, ReplicationStoreError> {
        let entry = self
            .merged
            .entries
            .iter()
            .find(|entry| &entry.key == key && entry.candidates.len() > 1)
            .ok_or(ReplicationStoreError::InvalidConflictResolution)?;
        let operation = entry
            .candidates
            .first()
            .map(|candidate| candidate.operation.clone())
            .ok_or(ReplicationStoreError::InvalidConflictResolution)?;
        let observed = entry
            .candidates
            .iter()
            .map(|candidate| candidate.vector.clone())
            .collect::<Vec<_>>();
        let version = policy.next_version(author, &observed, operation)?;
        Ok(ReplicationResolutionContext {
            key: key.clone(),
            author: author.clone(),
            vector: version.vector,
            candidate_count: u8::try_from(entry.candidates.len())
                .map_err(|_| ReplicationStoreError::InvalidConflictResolution)?,
        })
    }
}

impl fmt::Debug for ReplicationSyncPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationSyncPlan")
            .field("token", &self.token)
            .field("disposition", &self.disposition)
            .field("local_revision", &self.local.revision)
            .field("transport_state", &self.transport_state())
            .field("entry_count", &self.merged.entries.len())
            .field("conflict_count", &self.conflicts.len())
            .field("provider_conflict_count", &self.transport.conflicts.len())
            .finish()
    }
}

#[derive(Clone)]
pub struct ReplicationSyncOutcome {
    pub local: ReplicationRepositorySnapshot,
    pub transport: SharedFolderTransportSnapshot,
    pub local_changed: bool,
    pub transport_published: bool,
    pub provider_conflict_count: usize,
}

impl fmt::Debug for ReplicationSyncOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationSyncOutcome")
            .field("local", &self.local)
            .field("transport", &self.transport)
            .field("local_changed", &self.local_changed)
            .field("transport_published", &self.transport_published)
            .field("provider_conflict_count", &self.provider_conflict_count)
            .finish()
    }
}

#[derive(Clone)]
pub struct ReplicationSyncCoordinator {
    repository: ReplicationRepository,
    transport: SharedFolderReplicationTransport,
}

impl ReplicationSyncCoordinator {
    pub fn new(
        repository: ReplicationRepository,
        transport: SharedFolderReplicationTransport,
    ) -> Self {
        Self {
            repository,
            transport,
        }
    }

    pub fn review(
        &self,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationSyncPlan, ReplicationStoreError> {
        let local = self
            .repository
            .load(self.transport.workspace_id(), policy)?;
        if local.retirement_pending {
            return Err(ReplicationStoreError::PendingRetirement);
        }
        let transport = self.transport.replication_inputs(policy)?;
        let merged = merge_inputs(&local.document, &transport, policy)?;
        let conflicts = conflict_reviews(&merged)?;
        let disposition = if local.source == ReplicationRepositorySource::LastGood {
            ReplicationSyncDisposition::RecoveryRequired
        } else if !conflicts.is_empty() {
            ReplicationSyncDisposition::ConflictReviewRequired
        } else {
            classify(&local.document, &transport, &merged)
        };
        let token = review_token(&local, &transport, &merged, policy)?;
        Ok(ReplicationSyncPlan {
            token,
            disposition,
            local,
            transport,
            merged,
            conflicts,
        })
    }

    pub fn apply(
        &self,
        plan: &ReplicationSyncPlan,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationSyncOutcome, ReplicationStoreError> {
        let current = self.fresh_plan(plan, policy)?;
        match current.disposition {
            ReplicationSyncDisposition::RecoveryRequired => {
                return Err(ReplicationStoreError::RecoveryRequired);
            }
            ReplicationSyncDisposition::ConflictReviewRequired => {
                return Err(ReplicationStoreError::ConflictResolutionRequired);
            }
            _ => {}
        }
        self.persist(&current, current.merged.clone(), policy)
    }

    pub fn resolve(
        &self,
        plan: &ReplicationSyncPlan,
        author: &ReplicationReplicaId,
        resolutions: Vec<ReplicationConflictResolution>,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationSyncOutcome, ReplicationStoreError> {
        let current = self.fresh_plan(plan, policy)?;
        if current.disposition == ReplicationSyncDisposition::RecoveryRequired {
            return Err(ReplicationStoreError::RecoveryRequired);
        }
        if current.disposition != ReplicationSyncDisposition::ConflictReviewRequired
            || resolutions.len() != current.conflicts.len()
        {
            return Err(ReplicationStoreError::InvalidConflictResolution);
        }
        let mut by_key = BTreeMap::new();
        for resolution in resolutions {
            if by_key.insert(resolution.key.clone(), resolution).is_some() {
                return Err(ReplicationStoreError::InvalidConflictResolution);
            }
        }

        let mut entries = current.merged.entries.clone();
        for entry in &mut entries {
            if entry.candidates.len() <= 1 {
                continue;
            }
            let resolution = by_key
                .remove(&entry.key)
                .ok_or(ReplicationStoreError::InvalidConflictResolution)?;
            let context = current.resolution_context(&entry.key, author, policy)?;
            if resolution.version.author != *context.author()
                || resolution.version.vector != *context.vector()
            {
                return Err(ReplicationStoreError::InvalidConflictResolution);
            }
            let validated = ReplicatedVersion::new(
                resolution.version.author.clone(),
                resolution.version.vector.clone(),
                resolution.version.operation.clone(),
                policy,
            )?;
            if validated != resolution.version {
                return Err(ReplicationStoreError::InvalidConflictResolution);
            }
            entry.candidates = vec![validated];
        }
        if !by_key.is_empty() {
            return Err(ReplicationStoreError::InvalidConflictResolution);
        }
        let resolved =
            ReplicationDocument::new(current.merged.workspace_id.clone(), entries, policy)?;
        self.persist(&current, resolved, policy)
    }

    fn fresh_plan(
        &self,
        expected: &ReplicationSyncPlan,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationSyncPlan, ReplicationStoreError> {
        let current = self.review(policy)?;
        if current.token != expected.token {
            return Err(ReplicationStoreError::StaleSyncPlan);
        }
        Ok(current)
    }

    fn persist(
        &self,
        plan: &ReplicationSyncPlan,
        document: ReplicationDocument,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationSyncOutcome, ReplicationStoreError> {
        let local_changed = plan.local.document != document;
        let local = if local_changed {
            self.repository.commit(
                plan.local.revision,
                document.clone(),
                plan.local.custody.clone(),
                &[],
                policy,
            )?
        } else {
            plan.local.clone()
        };
        let transport_published = plan
            .transport
            .current
            .as_ref()
            .is_none_or(|snapshot| snapshot.document != document);
        let transport = if transport_published {
            self.transport
                .publish(transport_state(&plan.transport), &document, policy)?
        } else {
            plan.transport
                .current
                .clone()
                .ok_or(ReplicationStoreError::Missing)?
        };
        Ok(ReplicationSyncOutcome {
            local,
            transport,
            local_changed,
            transport_published,
            provider_conflict_count: plan.transport.conflicts.len(),
        })
    }
}

fn merge_inputs(
    local: &ReplicationDocument,
    inputs: &SharedFolderReplicationInputs,
    policy: &ReplicationPolicy,
) -> Result<ReplicationDocument, ReplicationStoreError> {
    let mut documents = BTreeMap::<ReplicationContentRevision, ReplicationDocument>::new();
    if let Some(current) = &inputs.current {
        documents.insert(current.revision, current.document.clone());
    }
    for conflict in &inputs.conflicts {
        documents.insert(conflict.revision, conflict.document.clone());
    }
    let mut merged = local.clone();
    for document in documents.into_values() {
        merged = merge_replication_documents(&merged, &document, policy)?.document;
    }
    Ok(merged)
}

fn conflict_reviews(
    document: &ReplicationDocument,
) -> Result<Vec<ReplicationConflictReview>, ReplicationStoreError> {
    document
        .entries
        .iter()
        .filter(|entry| entry.candidates.len() > 1)
        .map(|entry| {
            let mut puts = false;
            let mut deletes = false;
            for candidate in &entry.candidates {
                match candidate.operation {
                    ReplicationOperation::Put { .. } => puts = true,
                    ReplicationOperation::Delete { .. } => deletes = true,
                }
            }
            let operation_mix = match (puts, deletes) {
                (true, false) => ReplicationConflictOperationMix::PutOnly,
                (false, true) => ReplicationConflictOperationMix::DeleteOnly,
                (true, true) => ReplicationConflictOperationMix::Mixed,
                (false, false) => return Err(ReplicationStoreError::Corrupt),
            };
            Ok(ReplicationConflictReview {
                key: entry.key.clone(),
                candidate_count: u8::try_from(entry.candidates.len())
                    .map_err(|_| ReplicationStoreError::Corrupt)?,
                operation_mix,
            })
        })
        .collect()
}

fn classify(
    local: &ReplicationDocument,
    inputs: &SharedFolderReplicationInputs,
    merged: &ReplicationDocument,
) -> ReplicationSyncDisposition {
    match &inputs.current {
        None => ReplicationSyncDisposition::PublishLocal,
        Some(remote) if &remote.document == local && local == merged => {
            ReplicationSyncDisposition::InSync
        }
        Some(remote) if &remote.document == merged => ReplicationSyncDisposition::UpdateLocal,
        Some(_) if local == merged => ReplicationSyncDisposition::PublishLocal,
        Some(_) => ReplicationSyncDisposition::Converge,
    }
}

fn transport_state(inputs: &SharedFolderReplicationInputs) -> SharedFolderTransportState {
    inputs
        .current
        .as_ref()
        .map_or(SharedFolderTransportState::Absent, |snapshot| {
            SharedFolderTransportState::Present(snapshot.revision)
        })
}

fn review_token(
    local: &ReplicationRepositorySnapshot,
    inputs: &SharedFolderReplicationInputs,
    merged: &ReplicationDocument,
    policy: &ReplicationPolicy,
) -> Result<ReplicationSyncReviewToken, ReplicationStoreError> {
    let (_, local_bytes) = canonical_replication(&local.document, policy)?;
    let (_, merged_bytes) = canonical_replication(merged, policy)?;
    let policy_bytes = serde_json::to_vec(policy).map_err(|_| ReplicationStoreError::Corrupt)?;
    let mut conflict_revisions = inputs
        .conflicts
        .iter()
        .map(|artifact| artifact.revision)
        .collect::<Vec<_>>();
    conflict_revisions.sort();

    let mut digest = Sha256::new();
    digest.update(SYNC_REVIEW_DOMAIN);
    digest.update((policy_bytes.len() as u64).to_be_bytes());
    digest.update(policy_bytes);
    digest.update(local.revision.get().to_be_bytes());
    digest.update([match local.source {
        ReplicationRepositorySource::Primary => 1,
        ReplicationRepositorySource::LastGood => 2,
    }]);
    digest.update((local_bytes.len() as u64).to_be_bytes());
    digest.update(local_bytes);
    match &inputs.current {
        Some(current) => {
            digest.update([1]);
            digest.update(current.revision.as_bytes());
        }
        None => digest.update([0]),
    }
    digest.update((conflict_revisions.len() as u64).to_be_bytes());
    for revision in conflict_revisions {
        digest.update(revision.as_bytes());
    }
    digest.update((merged_bytes.len() as u64).to_be_bytes());
    digest.update(merged_bytes);
    Ok(ReplicationSyncReviewToken(digest.finalize().into()))
}
