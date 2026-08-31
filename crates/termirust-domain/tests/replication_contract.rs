use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use termirust_domain::{
    MAX_REPLICATION_CANDIDATES_PER_ENTRY, MAX_REPLICATION_DOCUMENT_BYTES,
    MAX_REPLICATION_SEALED_PAYLOAD_BYTES, REPLICATION_SCHEMA_VERSION, ReplicaAuthorization,
    ReplicatedVersion, ReplicationAuditOutcome, ReplicationCollectionId, ReplicationDocument,
    ReplicationEntry, ReplicationError, ReplicationOperation, ReplicationPolicy,
    ReplicationRecordId, ReplicationRecordKey, ReplicationReplicaId, ReplicationVersionVector,
    SealedReplicationPayload, VersionRelation, merge_replication_documents,
};

#[derive(Debug, Deserialize)]
struct ContractFixture {
    schema_version: u16,
    workspace_id: String,
    replicas: Vec<FixtureReplica>,
    documents: BTreeMap<String, ReplicationDocument>,
}

#[derive(Debug, Deserialize)]
struct FixtureReplica {
    id: String,
    state: String,
    accepted_through: Option<u64>,
}

fn fixture() -> ContractFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/merge-contract-v1.json");
    let bytes = std::fs::read(path).expect("replication fixture should be readable");
    assert!(bytes.len() < MAX_REPLICATION_DOCUMENT_BYTES);
    serde_json::from_slice(&bytes).expect("replication fixture should parse")
}

fn policy(fixture: &ContractFixture) -> ReplicationPolicy {
    ReplicationPolicy::new(fixture.replicas.iter().map(|replica| {
        let id = ReplicationReplicaId::new(replica.id.clone()).expect("fixture replica ID");
        let authorization = match replica.state.as_str() {
            "active" => ReplicaAuthorization::Active,
            "revoked" => ReplicaAuthorization::Revoked {
                accepted_through: replica
                    .accepted_through
                    .expect("revoked fixture replica needs a cutoff"),
            },
            state => panic!("unsupported fixture replica state {state}"),
        };
        (id, authorization)
    }))
    .expect("fixture policy should be valid")
}

fn document<'a>(fixture: &'a ContractFixture, name: &str) -> &'a ReplicationDocument {
    fixture
        .documents
        .get(name)
        .unwrap_or_else(|| panic!("fixture document {name} should exist"))
}

fn entry<'a>(
    document: &'a ReplicationDocument,
    collection: &str,
    id: &str,
) -> &'a ReplicationEntry {
    document
        .entries
        .iter()
        .find(|entry| {
            entry.key.collection.as_str() == collection && entry.key.record_id.as_str() == id
        })
        .unwrap_or_else(|| panic!("entry {collection}/{id} should exist"))
}

fn put(bytes: &[u8]) -> ReplicationOperation {
    ReplicationOperation::Put {
        sealed_payload: SealedReplicationPayload::new(bytes.to_vec())
            .expect("synthetic payload should fit"),
    }
}

#[test]
fn replication_fixture_enforces_bounded_schema_and_policy() {
    let fixture = fixture();
    let policy = policy(&fixture);
    assert_eq!(fixture.schema_version, REPLICATION_SCHEMA_VERSION);
    assert_eq!(fixture.workspace_id, "workspace-alpha");

    for name in ["left", "right", "resolved", "revoked_history"] {
        document(&fixture, name)
            .validate(&policy)
            .unwrap_or_else(|error| panic!("{name} should validate: {error}"));
    }
    assert_eq!(
        document(&fixture, "revoked_after").validate(&policy),
        Err(ReplicationError::PostRevocationCounter)
    );
    assert_eq!(
        document(&fixture, "unknown").validate(&policy),
        Err(ReplicationError::UnknownReplica)
    );
    ReplicationPolicy::new([(
        ReplicationReplicaId::new("never-authored").expect("replica ID"),
        ReplicaAuthorization::Revoked {
            accepted_through: 0,
        },
    )])
    .expect("revocation before the first accepted mutation should be valid");

    let encoded = serde_json::to_vec(document(&fixture, "left")).expect("document should encode");
    assert_eq!(
        ReplicationDocument::decode_json(&encoded, &policy).expect("document should decode"),
        *document(&fixture, "left")
    );
    assert_eq!(
        ReplicationDocument::decode_json(b"{", &policy),
        Err(ReplicationError::MalformedDocument)
    );
    assert_eq!(
        ReplicationDocument::decode_json(&vec![b' '; MAX_REPLICATION_DOCUMENT_BYTES + 1], &policy),
        Err(ReplicationError::DocumentTooLarge)
    );

    let mut duplicate = document(&fixture, "left").clone();
    duplicate.entries.push(duplicate.entries[0].clone());
    assert_eq!(
        duplicate.validate(&policy),
        Err(ReplicationError::DuplicateRecordKey)
    );
}

#[test]
fn replication_merge_preserves_causal_maxima_conflicts_and_tombstones() {
    let fixture = fixture();
    let policy = policy(&fixture);
    let merged = merge_replication_documents(
        document(&fixture, "left"),
        document(&fixture, "right"),
        &policy,
    )
    .expect("fixture documents should merge");

    let divergent = entry(&merged.document, "connections", "server-1");
    assert_eq!(divergent.candidates.len(), 2);
    assert_eq!(
        divergent.candidates[0]
            .vector
            .relation(&divergent.candidates[1].vector),
        VersionRelation::Concurrent
    );

    let removed = entry(&merged.document, "connections", "server-removed");
    assert_eq!(removed.candidates.len(), 1);
    assert!(matches!(
        removed.candidates[0].operation,
        ReplicationOperation::Delete { .. }
    ));

    let delete_conflict = entry(&merged.document, "vaults", "vault-concurrent");
    assert_eq!(delete_conflict.candidates.len(), 2);
    assert!(
        delete_conflict
            .candidates
            .iter()
            .any(|candidate| matches!(candidate.operation, ReplicationOperation::Delete { .. }))
    );
    assert!(
        delete_conflict
            .candidates
            .iter()
            .any(|candidate| matches!(candidate.operation, ReplicationOperation::Put { .. }))
    );

    let resolved =
        merge_replication_documents(&merged.document, document(&fixture, "resolved"), &policy)
            .expect("reviewed resolution should merge");
    let resolved_entry = entry(&resolved.document, "connections", "server-1");
    assert_eq!(resolved_entry.candidates.len(), 1);
    assert_eq!(resolved_entry.candidates[0].author.as_str(), "device-c");

    let author = ReplicationReplicaId::new("device-c").expect("active author ID");
    let reviewed: Vec<_> = divergent
        .candidates
        .iter()
        .map(|candidate| candidate.vector.clone())
        .collect();
    let next = policy
        .next_version(&author, &reviewed, put(&[170, 1, 1]))
        .expect("active author should resolve reviewed candidates");
    assert!(
        reviewed
            .iter()
            .all(|vector| vector.relation(&next.vector) == VersionRelation::Before)
    );
}

#[test]
fn replication_merge_is_commutative_associative_and_idempotent() {
    let fixture = fixture();
    let policy = policy(&fixture);
    let left = document(&fixture, "left");
    let right = document(&fixture, "right");
    let resolution = document(&fixture, "resolved");

    let left_right = merge_replication_documents(left, right, &policy).expect("left/right merge");
    let right_left = merge_replication_documents(right, left, &policy).expect("right/left merge");
    assert_eq!(left_right, right_left, "merge must be commutative");

    let left_grouped = merge_replication_documents(&left_right.document, resolution, &policy)
        .expect("(left + right) + resolution");
    let right_resolution =
        merge_replication_documents(right, resolution, &policy).expect("right + resolution");
    let right_grouped = merge_replication_documents(left, &right_resolution.document, &policy)
        .expect("left + (right + resolution)");
    assert_eq!(
        left_grouped.document, right_grouped.document,
        "merge must be associative"
    );

    let canonical = merge_replication_documents(left, left, &policy)
        .expect("self merge should canonicalize")
        .document;
    let repeated = merge_replication_documents(&canonical, &canonical, &policy)
        .expect("canonical self merge")
        .document;
    assert_eq!(canonical, repeated, "merge must be idempotent");
}

#[test]
fn replication_hostile_inputs_fail_closed_without_content_leaks() {
    let fixture = fixture();
    let policy = policy(&fixture);
    let left = document(&fixture, "left");

    let mut too_many = left.clone();
    too_many.entries[0].candidates =
        vec![too_many.entries[0].candidates[0].clone(); MAX_REPLICATION_CANDIDATES_PER_ENTRY + 1];
    assert_eq!(
        too_many.validate(&policy),
        Err(ReplicationError::TooManyCandidates)
    );
    assert_eq!(
        SealedReplicationPayload::new(vec![0; MAX_REPLICATION_SEALED_PAYLOAD_BYTES + 1]),
        Err(ReplicationError::SealedPayloadTooLarge)
    );

    let mut equivocation = left.clone();
    let mut divergent = equivocation.entries[0].candidates[0].clone();
    divergent.operation = ReplicationOperation::Delete {
        sealed_payload: SealedReplicationPayload::new(vec![222, 1, 1])
            .expect("synthetic tombstone should fit"),
    };
    equivocation.entries[0].candidates.push(divergent);
    assert_eq!(
        equivocation.validate(&policy),
        Err(ReplicationError::ClockEquivocation)
    );

    let old = ReplicationReplicaId::new("device-old").expect("revoked fixture ID");
    assert_eq!(
        policy.next_version(&old, &[], put(&[171, 1, 1])),
        Err(ReplicationError::ReplicaNotActive)
    );
    let active = ReplicationReplicaId::new("device-a").expect("active fixture ID");
    let maximum = ReplicationVersionVector::new([(active.clone(), u64::MAX)])
        .expect("maximum counter is valid history");
    assert_eq!(
        policy.next_version(&active, &[maximum], put(&[172, 1, 1])),
        Err(ReplicationError::CounterOverflow)
    );

    let payload = SealedReplicationPayload::new(b"PRIVATE-SYNTHETIC-PAYLOAD".to_vec())
        .expect("synthetic payload should fit");
    let version = ReplicatedVersion::new(
        active.clone(),
        ReplicationVersionVector::new([(active, 1)]).expect("vector should be valid"),
        ReplicationOperation::Put {
            sealed_payload: payload,
        },
        &policy,
    )
    .expect("version should be valid");
    let debug = format!("{version:?}");
    assert!(!debug.contains("PRIVATE-SYNTHETIC-PAYLOAD"));
    assert!(!debug.contains("device-a"));

    let merged = merge_replication_documents(left, document(&fixture, "right"), &policy)
        .expect("fixture should merge");
    let audit = serde_json::to_string(&merged.audit_events).expect("audit should encode");
    for forbidden in [
        "workspace-alpha",
        "device-a",
        "server-1",
        "connections",
        "sealed_payload",
    ] {
        assert!(!audit.contains(forbidden), "audit leaked {forbidden}");
    }
    assert!(merged.audit_events.iter().any(|event| {
        event.outcome == ReplicationAuditOutcome::ConflictPreserved && event.candidate_count == 2
    }));

    let key = ReplicationRecordKey::new(
        ReplicationCollectionId::new("oversize").expect("collection ID"),
        ReplicationRecordId::new("aggregate").expect("record ID"),
    );
    let full_payload = SealedReplicationPayload::new(vec![7; MAX_REPLICATION_SEALED_PAYLOAD_BYTES])
        .expect("one maximum payload should fit");
    let full_version = policy
        .next_version(
            &ReplicationReplicaId::new("device-b").expect("active fixture ID"),
            &[],
            ReplicationOperation::Put {
                sealed_payload: full_payload,
            },
        )
        .expect("maximum version should fit");
    let oversized_entries = (0..9)
        .map(|index| ReplicationEntry {
            key: ReplicationRecordKey::new(
                key.collection.clone(),
                ReplicationRecordId::new(format!("aggregate-{index}")).expect("record ID"),
            ),
            candidates: vec![full_version.clone()],
        })
        .collect();
    assert_eq!(
        ReplicationDocument::new(left.workspace_id.clone(), oversized_entries, &policy),
        Err(ReplicationError::TotalPayloadTooLarge)
    );
}
