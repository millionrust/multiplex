use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

pub const REPLICATION_SCHEMA_VERSION: u16 = 1;
pub const MAX_REPLICATION_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_REPLICATION_ENTRIES: usize = 4_096;
pub const MAX_REPLICATION_CANDIDATES_PER_ENTRY: usize = 8;
pub const MAX_REPLICATION_REPLICAS: usize = 16;
pub const MAX_REPLICATION_SEALED_PAYLOAD_BYTES: usize = 1024 * 1024;

const MAX_REPLICATION_WORKSPACE_ID_BYTES: usize = 128;
const MAX_REPLICATION_REPLICA_ID_BYTES: usize = 64;
const MAX_REPLICATION_COLLECTION_ID_BYTES: usize = 32;
const MAX_REPLICATION_RECORD_ID_BYTES: usize = 128;
const MAX_REPLICATION_TOTAL_PAYLOAD_BYTES: usize = MAX_REPLICATION_DOCUMENT_BYTES;

macro_rules! opaque_token {
    ($name:ident, $maximum:ident, $error:ident) => {
        #[derive(Clone, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ReplicationError> {
                let value = value.into();
                let token = Self(value);
                token.validate()?;
                Ok(token)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn validate(&self) -> Result<(), ReplicationError> {
                if self.0.is_empty()
                    || self.0.len() > $maximum
                    || !self.0.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-')
                    })
                {
                    return Err(ReplicationError::$error);
                }
                Ok(())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(<opaque>)"))
            }
        }
    };
}

opaque_token!(
    ReplicationWorkspaceId,
    MAX_REPLICATION_WORKSPACE_ID_BYTES,
    InvalidWorkspaceId
);
opaque_token!(
    ReplicationReplicaId,
    MAX_REPLICATION_REPLICA_ID_BYTES,
    InvalidReplicaId
);
opaque_token!(
    ReplicationCollectionId,
    MAX_REPLICATION_COLLECTION_ID_BYTES,
    InvalidCollectionId
);
opaque_token!(
    ReplicationRecordId,
    MAX_REPLICATION_RECORD_ID_BYTES,
    InvalidRecordId
);

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ReplicationRecordKey {
    pub collection: ReplicationCollectionId,
    pub record_id: ReplicationRecordId,
}

impl ReplicationRecordKey {
    pub fn new(collection: ReplicationCollectionId, record_id: ReplicationRecordId) -> Self {
        Self {
            collection,
            record_id,
        }
    }

    pub fn validate(&self) -> Result<(), ReplicationError> {
        self.collection.validate()?;
        self.record_id.validate()
    }
}

#[derive(Clone, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ReplicationVersionVector(BTreeMap<ReplicationReplicaId, u64>);

impl fmt::Debug for ReplicationVersionVector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationVersionVector")
            .field("replica_count", &self.0.len())
            .finish()
    }
}

impl ReplicationVersionVector {
    pub fn new(
        entries: impl IntoIterator<Item = (ReplicationReplicaId, u64)>,
    ) -> Result<Self, ReplicationError> {
        let mut vector = BTreeMap::new();
        for (replica_id, counter) in entries {
            if vector.insert(replica_id, counter).is_some() {
                return Err(ReplicationError::DuplicateReplicaCounter);
            }
        }
        let vector = Self(vector);
        vector.validate()?;
        Ok(vector)
    }

    pub fn counter(&self, replica_id: &ReplicationReplicaId) -> u64 {
        self.0.get(replica_id).copied().unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&ReplicationReplicaId, u64)> {
        self.0
            .iter()
            .map(|(replica_id, counter)| (replica_id, *counter))
    }

    pub fn relation(&self, other: &Self) -> VersionRelation {
        let mut less = false;
        let mut greater = false;

        for (replica_id, left) in &self.0 {
            match left.cmp(&other.counter(replica_id)) {
                Ordering::Less => less = true,
                Ordering::Greater => greater = true,
                Ordering::Equal => {}
            }
        }
        for (replica_id, right) in &other.0 {
            if !self.0.contains_key(replica_id) && *right > 0 {
                less = true;
            }
        }

        match (less, greater) {
            (false, false) => VersionRelation::Equal,
            (true, false) => VersionRelation::Before,
            (false, true) => VersionRelation::After,
            (true, true) => VersionRelation::Concurrent,
        }
    }

    pub fn joined(&self, other: &Self) -> Result<Self, ReplicationError> {
        let mut joined = self.0.clone();
        for (replica_id, counter) in &other.0 {
            joined
                .entry(replica_id.clone())
                .and_modify(|current| *current = (*current).max(*counter))
                .or_insert(*counter);
        }
        let joined = Self(joined);
        joined.validate()?;
        Ok(joined)
    }

    fn increment(&mut self, replica_id: &ReplicationReplicaId) -> Result<(), ReplicationError> {
        if !self.0.contains_key(replica_id) && self.0.len() == MAX_REPLICATION_REPLICAS {
            return Err(ReplicationError::TooManyReplicaCounters);
        }
        let next = self
            .counter(replica_id)
            .checked_add(1)
            .ok_or(ReplicationError::CounterOverflow)?;
        self.0.insert(replica_id.clone(), next);
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ReplicationError> {
        if self.0.len() > MAX_REPLICATION_REPLICAS {
            return Err(ReplicationError::TooManyReplicaCounters);
        }
        for (replica_id, counter) in &self.0 {
            replica_id.validate()?;
            if *counter == 0 {
                return Err(ReplicationError::InvalidCounter);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionRelation {
    Before,
    Equal,
    After,
    Concurrent,
}

#[derive(Clone, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SealedReplicationPayload(Vec<u8>);

impl fmt::Debug for SealedReplicationPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SealedReplicationPayload(<redacted>)")
    }
}

impl SealedReplicationPayload {
    pub fn new(bytes: Vec<u8>) -> Result<Self, ReplicationError> {
        let payload = Self(bytes);
        payload.validate()?;
        Ok(payload)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn validate(&self) -> Result<(), ReplicationError> {
        if self.0.is_empty() {
            return Err(ReplicationError::EmptySealedPayload);
        }
        if self.0.len() > MAX_REPLICATION_SEALED_PAYLOAD_BYTES {
            return Err(ReplicationError::SealedPayloadTooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ReplicationOperation {
    Put {
        sealed_payload: SealedReplicationPayload,
    },
    Delete {
        sealed_payload: SealedReplicationPayload,
    },
}

impl ReplicationOperation {
    fn validate(&self) -> Result<(), ReplicationError> {
        match self {
            Self::Put { sealed_payload } | Self::Delete { sealed_payload } => {
                sealed_payload.validate()
            }
        }
    }

    fn payload_len(&self) -> usize {
        match self {
            Self::Put { sealed_payload } | Self::Delete { sealed_payload } => {
                sealed_payload.0.len()
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ReplicatedVersion {
    pub author: ReplicationReplicaId,
    pub vector: ReplicationVersionVector,
    #[serde(flatten)]
    pub operation: ReplicationOperation,
}

impl ReplicatedVersion {
    pub fn new(
        author: ReplicationReplicaId,
        vector: ReplicationVersionVector,
        operation: ReplicationOperation,
        policy: &ReplicationPolicy,
    ) -> Result<Self, ReplicationError> {
        let version = Self {
            author,
            vector,
            operation,
        };
        policy.validate_version(&version)?;
        Ok(version)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplicationEntry {
    pub key: ReplicationRecordKey,
    pub candidates: Vec<ReplicatedVersion>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplicationDocument {
    pub schema_version: u16,
    pub workspace_id: ReplicationWorkspaceId,
    pub entries: Vec<ReplicationEntry>,
}

impl ReplicationDocument {
    pub fn new(
        workspace_id: ReplicationWorkspaceId,
        entries: Vec<ReplicationEntry>,
        policy: &ReplicationPolicy,
    ) -> Result<Self, ReplicationError> {
        let document = Self {
            schema_version: REPLICATION_SCHEMA_VERSION,
            workspace_id,
            entries,
        };
        document.validate(policy)?;
        Ok(document)
    }

    pub fn decode_json(bytes: &[u8], policy: &ReplicationPolicy) -> Result<Self, ReplicationError> {
        if bytes.len() > MAX_REPLICATION_DOCUMENT_BYTES {
            return Err(ReplicationError::DocumentTooLarge);
        }
        let document: Self =
            serde_json::from_slice(bytes).map_err(|_| ReplicationError::MalformedDocument)?;
        document.validate(policy)?;
        Ok(document)
    }

    pub fn validate(&self, policy: &ReplicationPolicy) -> Result<(), ReplicationError> {
        policy.validate()?;
        if self.schema_version != REPLICATION_SCHEMA_VERSION {
            return Err(ReplicationError::UnsupportedSchema);
        }
        self.workspace_id.validate()?;
        if self.entries.len() > MAX_REPLICATION_ENTRIES {
            return Err(ReplicationError::TooManyEntries);
        }

        let mut keys = BTreeSet::new();
        let mut total_payload_bytes = 0_usize;
        for entry in &self.entries {
            entry.key.validate()?;
            if !keys.insert(entry.key.clone()) {
                return Err(ReplicationError::DuplicateRecordKey);
            }
            if entry.candidates.is_empty() {
                return Err(ReplicationError::EmptyCandidateSet);
            }
            if entry.candidates.len() > MAX_REPLICATION_CANDIDATES_PER_ENTRY {
                return Err(ReplicationError::TooManyCandidates);
            }
            for candidate in &entry.candidates {
                policy.validate_version(candidate)?;
                total_payload_bytes = total_payload_bytes
                    .checked_add(candidate.operation.payload_len())
                    .ok_or(ReplicationError::TotalPayloadTooLarge)?;
                if total_payload_bytes > MAX_REPLICATION_TOTAL_PAYLOAD_BYTES {
                    return Err(ReplicationError::TotalPayloadTooLarge);
                }
            }
            validate_no_equivocation(&entry.candidates)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReplicaAuthorization {
    Active,
    Revoked { accepted_through: u64 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ReplicationPolicy(BTreeMap<ReplicationReplicaId, ReplicaAuthorization>);

impl ReplicationPolicy {
    pub fn new(
        replicas: impl IntoIterator<Item = (ReplicationReplicaId, ReplicaAuthorization)>,
    ) -> Result<Self, ReplicationError> {
        let mut policy = BTreeMap::new();
        for (replica_id, authorization) in replicas {
            if policy.insert(replica_id, authorization).is_some() {
                return Err(ReplicationError::DuplicateReplicaPolicy);
            }
        }
        let policy = Self(policy);
        policy.validate()?;
        Ok(policy)
    }

    pub fn next_version(
        &self,
        author: &ReplicationReplicaId,
        observed: &[ReplicationVersionVector],
        operation: ReplicationOperation,
    ) -> Result<ReplicatedVersion, ReplicationError> {
        if observed.len() > MAX_REPLICATION_CANDIDATES_PER_ENTRY {
            return Err(ReplicationError::TooManyCandidates);
        }
        match self.0.get(author) {
            Some(ReplicaAuthorization::Active) => {}
            Some(ReplicaAuthorization::Revoked { .. }) => {
                return Err(ReplicationError::ReplicaNotActive);
            }
            None => return Err(ReplicationError::UnknownReplica),
        }

        let mut vector = ReplicationVersionVector::default();
        for observed_vector in observed {
            self.validate_vector(observed_vector)?;
            vector = vector.joined(observed_vector)?;
        }
        vector.increment(author)?;
        ReplicatedVersion::new(author.clone(), vector, operation, self)
    }

    fn validate(&self) -> Result<(), ReplicationError> {
        if self.0.is_empty() {
            return Err(ReplicationError::EmptyReplicaPolicy);
        }
        if self.0.len() > MAX_REPLICATION_REPLICAS {
            return Err(ReplicationError::TooManyReplicas);
        }
        for replica_id in self.0.keys() {
            replica_id.validate()?;
        }
        Ok(())
    }

    fn validate_vector(&self, vector: &ReplicationVersionVector) -> Result<(), ReplicationError> {
        vector.validate()?;
        for (replica_id, counter) in &vector.0 {
            match self.0.get(replica_id) {
                Some(ReplicaAuthorization::Active) => {}
                Some(ReplicaAuthorization::Revoked { accepted_through })
                    if counter <= accepted_through => {}
                Some(ReplicaAuthorization::Revoked { .. }) => {
                    return Err(ReplicationError::PostRevocationCounter);
                }
                None => return Err(ReplicationError::UnknownReplica),
            }
        }
        Ok(())
    }

    fn validate_version(&self, version: &ReplicatedVersion) -> Result<(), ReplicationError> {
        version.author.validate()?;
        version.operation.validate()?;
        self.validate_vector(&version.vector)?;
        let author_counter = version.vector.counter(&version.author);
        if author_counter == 0 {
            return Err(ReplicationError::AuthorMissingFromVector);
        }
        match self.0.get(&version.author) {
            Some(ReplicaAuthorization::Active) => Ok(()),
            Some(ReplicaAuthorization::Revoked { accepted_through })
                if author_counter <= *accepted_through =>
            {
                Ok(())
            }
            Some(ReplicaAuthorization::Revoked { .. }) => {
                Err(ReplicationError::PostRevocationCounter)
            }
            None => Err(ReplicationError::UnknownReplica),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationAuditOutcome {
    Retained,
    Coalesced,
    Advanced,
    Deleted,
    ConflictPreserved,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplicationAuditEvent {
    pub sequence: u32,
    pub outcome: ReplicationAuditOutcome,
    pub candidate_count: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplicationMerge {
    pub document: ReplicationDocument,
    pub audit_events: Vec<ReplicationAuditEvent>,
}

pub fn merge_replication_documents(
    left: &ReplicationDocument,
    right: &ReplicationDocument,
    policy: &ReplicationPolicy,
) -> Result<ReplicationMerge, ReplicationError> {
    left.validate(policy)?;
    right.validate(policy)?;
    if left.workspace_id != right.workspace_id {
        return Err(ReplicationError::WorkspaceMismatch);
    }

    let mut candidates_by_key: BTreeMap<ReplicationRecordKey, Vec<ReplicatedVersion>> =
        BTreeMap::new();
    for entry in left.entries.iter().chain(&right.entries) {
        candidates_by_key
            .entry(entry.key.clone())
            .or_default()
            .extend(entry.candidates.iter().cloned());
    }

    let mut entries = Vec::with_capacity(candidates_by_key.len());
    let mut audit_events = Vec::with_capacity(candidates_by_key.len());
    for (index, (key, candidates)) in candidates_by_key.into_iter().enumerate() {
        let (candidates, outcome) = reduce_candidates(candidates)?;
        audit_events.push(ReplicationAuditEvent {
            sequence: u32::try_from(index + 1).map_err(|_| ReplicationError::TooManyEntries)?,
            outcome,
            candidate_count: u8::try_from(candidates.len())
                .map_err(|_| ReplicationError::TooManyCandidates)?,
        });
        entries.push(ReplicationEntry { key, candidates });
    }

    let document = ReplicationDocument::new(left.workspace_id.clone(), entries, policy)?;
    Ok(ReplicationMerge {
        document,
        audit_events,
    })
}

fn reduce_candidates(
    mut candidates: Vec<ReplicatedVersion>,
) -> Result<(Vec<ReplicatedVersion>, ReplicationAuditOutcome), ReplicationError> {
    validate_no_equivocation(&candidates)?;
    let original_len = candidates.len();
    candidates.sort();
    candidates.dedup();
    let coalesced = candidates.len() < original_len;

    let mut maximal = Vec::with_capacity(candidates.len());
    for (candidate_index, candidate) in candidates.iter().enumerate() {
        let dominated = candidates.iter().enumerate().any(|(other_index, other)| {
            candidate_index != other_index
                && candidate.vector.relation(&other.vector) == VersionRelation::Before
        });
        if !dominated {
            maximal.push(candidate.clone());
        }
    }
    maximal.sort();
    if maximal.len() > MAX_REPLICATION_CANDIDATES_PER_ENTRY {
        return Err(ReplicationError::TooManyCandidates);
    }

    let outcome = if maximal.len() > 1 {
        ReplicationAuditOutcome::ConflictPreserved
    } else if matches!(
        maximal.first().map(|candidate| &candidate.operation),
        Some(ReplicationOperation::Delete { .. })
    ) {
        ReplicationAuditOutcome::Deleted
    } else if maximal.len() < candidates.len() {
        ReplicationAuditOutcome::Advanced
    } else if coalesced {
        ReplicationAuditOutcome::Coalesced
    } else {
        ReplicationAuditOutcome::Retained
    };
    Ok((maximal, outcome))
}

fn validate_no_equivocation(candidates: &[ReplicatedVersion]) -> Result<(), ReplicationError> {
    for (index, candidate) in candidates.iter().enumerate() {
        for other in candidates.iter().skip(index + 1) {
            if candidate.vector == other.vector && candidate != other {
                return Err(ReplicationError::ClockEquivocation);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationError {
    DocumentTooLarge,
    MalformedDocument,
    UnsupportedSchema,
    InvalidWorkspaceId,
    InvalidReplicaId,
    InvalidCollectionId,
    InvalidRecordId,
    EmptyReplicaPolicy,
    TooManyReplicas,
    DuplicateReplicaPolicy,
    UnknownReplica,
    ReplicaNotActive,
    PostRevocationCounter,
    TooManyReplicaCounters,
    DuplicateReplicaCounter,
    InvalidCounter,
    CounterOverflow,
    AuthorMissingFromVector,
    TooManyEntries,
    DuplicateRecordKey,
    EmptyCandidateSet,
    TooManyCandidates,
    EmptySealedPayload,
    SealedPayloadTooLarge,
    TotalPayloadTooLarge,
    ClockEquivocation,
    WorkspaceMismatch,
}

impl fmt::Display for ReplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::DocumentTooLarge => "replication document exceeds its byte limit",
            Self::MalformedDocument => "replication document is malformed",
            Self::UnsupportedSchema => "replication schema is unsupported",
            Self::InvalidWorkspaceId => "replication workspace ID is invalid",
            Self::InvalidReplicaId => "replication replica ID is invalid",
            Self::InvalidCollectionId => "replication collection ID is invalid",
            Self::InvalidRecordId => "replication record ID is invalid",
            Self::EmptyReplicaPolicy => "replication policy has no replicas",
            Self::TooManyReplicas => "replication policy has too many replicas",
            Self::DuplicateReplicaPolicy => "replication policy repeats a replica",
            Self::UnknownReplica => "replication history contains an unknown replica",
            Self::ReplicaNotActive => "replication author is not active",
            Self::PostRevocationCounter => "replication history exceeds a revocation cutoff",
            Self::TooManyReplicaCounters => "replication vector has too many counters",
            Self::DuplicateReplicaCounter => "replication vector repeats a replica",
            Self::InvalidCounter => "replication vector counter is invalid",
            Self::CounterOverflow => "replication vector counter overflowed",
            Self::AuthorMissingFromVector => "replication author is absent from its vector",
            Self::TooManyEntries => "replication document has too many entries",
            Self::DuplicateRecordKey => "replication document repeats a record key",
            Self::EmptyCandidateSet => "replication entry has no candidates",
            Self::TooManyCandidates => "replication entry has too many candidates",
            Self::EmptySealedPayload => "replication sealed payload is empty",
            Self::SealedPayloadTooLarge => "replication sealed payload exceeds its byte limit",
            Self::TotalPayloadTooLarge => "replication payload total exceeds its byte limit",
            Self::ClockEquivocation => "one replication vector identifies divergent candidates",
            Self::WorkspaceMismatch => "replication documents belong to different workspaces",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ReplicationError {}
