use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use termirust_domain::{
    ReplicaAuthorization, ReplicatedVersion, ReplicationDocument, ReplicationOperation,
    ReplicationPolicy, ReplicationReplicaId, SealedReplicationPayload, merge_replication_documents,
};
use termirust_replication_security::{
    ReplicationHistoricalKeyIndex, ReplicationHistoricalKeyLimit, ReplicationKeyEpoch,
    ReplicationSecretKind, ReplicationSecretRef,
};
use termirust_store::replication::MAX_REPLICATION_REPOSITORY_BYTES;
use termirust_store::{
    AtomicWriter, Durability, ReplicationConflictResolution, ReplicationCustodyMetadata,
    ReplicationRepository, ReplicationRepositorySource, ReplicationStoreError,
    ReplicationSyncCoordinator, ReplicationSyncDisposition, SharedFolderReplicationTransport,
    SharedFolderSlot, SharedFolderTransportState, SystemAtomicWriter,
};

const TEST_SLOT: &str = "89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567";

#[derive(Debug, Deserialize)]
struct MergeFixture {
    replicas: Vec<FixtureReplica>,
    documents: BTreeMap<String, ReplicationDocument>,
}

#[derive(Debug, Deserialize)]
struct FixtureReplica {
    id: String,
    state: String,
    accepted_through: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SyncFixture {
    schema_version: u16,
    slot: String,
    conflict_count: usize,
    provider_conflict_count: usize,
    conflict_review_token: String,
    resolved_entry_count: usize,
}

fn fixture() -> MergeFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/merge-contract-v1.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn sync_fixture() -> SyncFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/sync-acceptance-v1.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn policy(fixture: &MergeFixture) -> ReplicationPolicy {
    ReplicationPolicy::new(fixture.replicas.iter().map(|replica| {
        let authorization = if replica.state == "active" {
            ReplicaAuthorization::Active
        } else {
            ReplicaAuthorization::Revoked {
                accepted_through: replica.accepted_through.unwrap(),
            }
        };
        (
            ReplicationReplicaId::new(replica.id.clone()).unwrap(),
            authorization,
        )
    }))
    .unwrap()
}

fn document(fixture: &MergeFixture, name: &str) -> ReplicationDocument {
    fixture.documents.get(name).unwrap().clone()
}

fn secret(kind: ReplicationSecretKind, epoch: Option<u64>, marker: u8) -> ReplicationSecretRef {
    ReplicationSecretRef::from_identifier(
        kind,
        epoch.map(|value| ReplicationKeyEpoch::new(value).unwrap()),
        [marker; 32],
    )
    .unwrap()
}

fn custody(device_marker: u8) -> ReplicationCustodyMetadata {
    let historical = ReplicationHistoricalKeyIndex::from_retained(
        ReplicationHistoricalKeyLimit::new(2).unwrap(),
        [secret(ReplicationSecretKind::EpochKey, Some(1), 1)],
    )
    .unwrap();
    ReplicationCustodyMetadata::new(
        secret(ReplicationSecretKind::AuthorityPrivateKey, None, 101),
        secret(ReplicationSecretKind::DevicePrivateKey, None, device_marker),
        historical,
    )
    .unwrap()
}

fn slot() -> SharedFolderSlot {
    SharedFolderSlot::new(TEST_SLOT).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn canonical_bytes(document: &ReplicationDocument, policy: &ReplicationPolicy) -> Vec<u8> {
    let document = merge_replication_documents(document, document, policy)
        .unwrap()
        .document;
    serde_json::to_vec(&document).unwrap()
}

fn coordinator(
    repository_root: &Path,
    shared_root: &Path,
    workspace: termirust_domain::ReplicationWorkspaceId,
) -> ReplicationSyncCoordinator {
    ReplicationSyncCoordinator::new(
        ReplicationRepository::open(repository_root).unwrap(),
        SharedFolderReplicationTransport::open(shared_root, workspace, slot()).unwrap(),
    )
}

fn resolutions_for(
    plan: &termirust_store::ReplicationSyncPlan,
    author: &ReplicationReplicaId,
    policy: &ReplicationPolicy,
) -> Vec<ReplicationConflictResolution> {
    plan.conflicts()
        .iter()
        .enumerate()
        .map(|(index, conflict)| {
            let context = plan
                .resolution_context(conflict.key(), author, policy)
                .unwrap();
            let operation = ReplicationOperation::Put {
                sealed_payload: SealedReplicationPayload::new(vec![0xc7, index as u8, 1]).unwrap(),
            };
            let version =
                ReplicatedVersion::new(author.clone(), context.vector().clone(), operation, policy)
                    .unwrap();
            ReplicationConflictResolution::new(conflict.key().clone(), version)
        })
        .collect()
}

struct FailPrimaryWriter {
    remaining: Mutex<usize>,
}

struct ReplaceTransportThenFailWriter {
    replacement: Vec<u8>,
    remaining: Mutex<usize>,
}

impl ReplaceTransportThenFailWriter {
    fn once(replacement: Vec<u8>) -> Self {
        Self {
            replacement,
            remaining: Mutex::new(1),
        }
    }
}

impl AtomicWriter for ReplaceTransportThenFailWriter {
    fn write(&self, target: &Path, bytes: &[u8]) -> io::Result<Durability> {
        let mut remaining = self.remaining.lock().unwrap();
        if target
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".termirust-replica-") && name.ends_with(".json"))
            && *remaining > 0
        {
            *remaining -= 1;
            SystemAtomicWriter.write(target, &self.replacement)?;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "synthetic provider replacement after local commit",
            ));
        }
        SystemAtomicWriter.write(target, bytes)
    }
}

impl FailPrimaryWriter {
    fn once() -> Self {
        Self {
            remaining: Mutex::new(1),
        }
    }
}

impl AtomicWriter for FailPrimaryWriter {
    fn write(&self, target: &Path, bytes: &[u8]) -> io::Result<Durability> {
        let mut remaining = self.remaining.lock().unwrap();
        if target.file_name().and_then(|name| name.to_str()) == Some("replica.json")
            && *remaining > 0
        {
            *remaining -= 1;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "synthetic recovery activation failure",
            ));
        }
        SystemAtomicWriter.write(target, bytes)
    }
}

#[test]
fn review_fixture_is_exact_bounded_and_conflict_preserving() {
    let contract = sync_fixture();
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    repository.create(right, custody(102), &policy).unwrap();
    let transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    transport
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    let coordinator = ReplicationSyncCoordinator::new(repository, transport);
    let plan = coordinator.review(&policy).unwrap();

    assert_eq!(contract.schema_version, 1);
    assert_eq!(contract.slot, TEST_SLOT);
    assert_eq!(
        plan.disposition(),
        ReplicationSyncDisposition::ConflictReviewRequired
    );
    assert_eq!(plan.conflicts().len(), contract.conflict_count);
    assert_eq!(
        plan.provider_conflict_count(),
        contract.provider_conflict_count
    );
    assert_eq!(hex(plan.token().as_bytes()), contract.conflict_review_token);

    let before_local = plan.local_revision();
    let before_transport = plan.transport_state();
    assert_eq!(
        coordinator.apply(&plan, &policy).unwrap_err(),
        ReplicationStoreError::ConflictResolutionRequired
    );
    let after = coordinator.review(&policy).unwrap();
    assert_eq!(after.local_revision(), before_local);
    assert_eq!(after.transport_state(), before_transport);
}

#[test]
fn provider_conflict_order_does_not_change_review_token_or_delete_evidence() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let resolved = document(&source, "resolved");
    let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let shared = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];

    let mut tokens = Vec::new();
    for index in 0..2 {
        let repository = ReplicationRepository::open(roots[index].path()).unwrap();
        repository
            .create(left.clone(), custody(110 + index as u8), &policy)
            .unwrap();
        let transport = SharedFolderReplicationTransport::open(
            shared[index].path(),
            left.workspace_id.clone(),
            slot(),
        )
        .unwrap();
        transport
            .publish(SharedFolderTransportState::Absent, &left, &policy)
            .unwrap();
        let stem = transport
            .artifact_path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let names = if index == 0 {
            ["a.sync-conflict-one", "z.sync-conflict-two"]
        } else {
            ["z.sync-conflict-two", "a.sync-conflict-one"]
        };
        fs::write(
            shared[index]
                .path()
                .join(format!("{stem}.{}.json", names[0])),
            canonical_bytes(&right, &policy),
        )
        .unwrap();
        fs::write(
            shared[index]
                .path()
                .join(format!("{stem}.{}.json", names[1])),
            canonical_bytes(&resolved, &policy),
        )
        .unwrap();
        let coordinator = ReplicationSyncCoordinator::new(repository, transport.clone());
        let plan = coordinator.review(&policy).unwrap();
        assert_eq!(plan.provider_conflict_count(), 2);
        assert_eq!(transport.conflict_artifacts(&policy).unwrap().len(), 2);
        tokens.push(plan.token());
    }
    assert_eq!(tokens[0], tokens[1]);
}

#[test]
fn stale_plan_is_rejected_before_local_or_transport_mutation() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let resolved = document(&source, "resolved");
    let root = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    let initial = repository
        .create(left.clone(), custody(102), &policy)
        .unwrap();
    let transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    let remote = transport
        .publish(SharedFolderTransportState::Absent, &right, &policy)
        .unwrap();
    let coordinator = ReplicationSyncCoordinator::new(repository.clone(), transport.clone());
    let stale = coordinator.review(&policy).unwrap();
    transport
        .publish(
            SharedFolderTransportState::Present(remote.revision),
            &resolved,
            &policy,
        )
        .unwrap();

    assert_eq!(
        coordinator.apply(&stale, &policy).unwrap_err(),
        ReplicationStoreError::StaleSyncPlan
    );
    assert_eq!(
        repository
            .load(&left.workspace_id, &policy)
            .unwrap()
            .revision,
        initial.revision
    );
    assert_eq!(transport.pull(&policy).unwrap().unwrap().document, resolved);
}

#[test]
fn local_only_and_remote_ahead_states_converge_without_false_writes() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let resolved = document(&source, "resolved");
    let shared = tempfile::tempdir().unwrap();

    let first_root = tempfile::tempdir().unwrap();
    let first_repository = ReplicationRepository::open(first_root.path()).unwrap();
    first_repository
        .create(left.clone(), custody(102), &policy)
        .unwrap();
    let first = coordinator(first_root.path(), shared.path(), left.workspace_id.clone());
    let publish = first.review(&policy).unwrap();
    assert_eq!(
        publish.disposition(),
        ReplicationSyncDisposition::PublishLocal
    );
    let published = first.apply(&publish, &policy).unwrap();
    assert!(!published.local_changed);
    assert!(published.transport_published);

    let transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    transport
        .publish(
            SharedFolderTransportState::Present(published.transport.revision),
            &resolved,
            &policy,
        )
        .unwrap();
    let subset = ReplicationDocument::new(
        left.workspace_id.clone(),
        vec![left.entries[0].clone()],
        &policy,
    )
    .unwrap();
    let second_root = tempfile::tempdir().unwrap();
    let second_repository = ReplicationRepository::open(second_root.path()).unwrap();
    second_repository
        .create(subset, custody(103), &policy)
        .unwrap();
    let second = ReplicationSyncCoordinator::new(second_repository, transport);
    let update = second.review(&policy).unwrap();
    assert_eq!(
        update.disposition(),
        ReplicationSyncDisposition::UpdateLocal
    );
    let updated = second.apply(&update, &policy).unwrap();
    assert!(updated.local_changed);
    assert!(!updated.transport_published);
    assert_eq!(updated.local.document, resolved);
}

#[test]
fn policy_changes_invalidate_review_tokens_even_when_existing_history_remains_valid() {
    let source = fixture();
    let active_policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    repository
        .create(right, custody(102), &active_policy)
        .unwrap();
    let transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    transport
        .publish(SharedFolderTransportState::Absent, &left, &active_policy)
        .unwrap();
    let coordinator = ReplicationSyncCoordinator::new(repository, transport);
    let old = coordinator.review(&active_policy).unwrap();
    let changed_policy = ReplicationPolicy::new([
        (
            ReplicationReplicaId::new("device-a").unwrap(),
            ReplicaAuthorization::Active,
        ),
        (
            ReplicationReplicaId::new("device-b").unwrap(),
            ReplicaAuthorization::Active,
        ),
        (
            ReplicationReplicaId::new("device-c").unwrap(),
            ReplicaAuthorization::Revoked {
                accepted_through: 100,
            },
        ),
    ])
    .unwrap();
    let changed = coordinator.review(&changed_policy).unwrap();
    assert_ne!(old.token(), changed.token());
    assert_eq!(
        coordinator.apply(&old, &changed_policy).unwrap_err(),
        ReplicationStoreError::StaleSyncPlan
    );
}

#[test]
fn exact_resolution_set_dominates_every_reviewed_candidate() {
    let contract = sync_fixture();
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    repository.create(right, custody(102), &policy).unwrap();
    let transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    transport
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    let coordinator = ReplicationSyncCoordinator::new(repository, transport);
    let plan = coordinator.review(&policy).unwrap();
    let author = ReplicationReplicaId::new("device-c").unwrap();
    let resolutions = resolutions_for(&plan, &author, &policy);

    assert_eq!(
        coordinator
            .resolve(&plan, &author, resolutions[..1].to_vec(), &policy)
            .unwrap_err(),
        ReplicationStoreError::InvalidConflictResolution
    );
    let duplicate = vec![resolutions[0].clone(), resolutions[0].clone()];
    assert_eq!(
        coordinator
            .resolve(&plan, &author, duplicate, &policy)
            .unwrap_err(),
        ReplicationStoreError::InvalidConflictResolution
    );

    let outcome = coordinator
        .resolve(&plan, &author, resolutions, &policy)
        .unwrap();
    assert!(outcome.local_changed);
    assert!(outcome.transport_published);
    assert_eq!(outcome.local.document, outcome.transport.document);
    assert_eq!(
        outcome.local.document.entries.len(),
        contract.resolved_entry_count
    );
    assert!(
        outcome
            .local
            .document
            .entries
            .iter()
            .all(|entry| entry.candidates.len() == 1)
    );
    assert_eq!(
        coordinator.review(&policy).unwrap().disposition(),
        ReplicationSyncDisposition::InSync
    );
}

#[test]
fn two_devices_diverge_resolve_restart_and_converge_without_reviving_revoked_authority() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let device_a = tempfile::tempdir().unwrap();
    let device_b = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let repository_a = ReplicationRepository::open(device_a.path()).unwrap();
    let repository_b = ReplicationRepository::open(device_b.path()).unwrap();
    repository_a
        .create(left.clone(), custody(111), &policy)
        .unwrap();
    repository_b.create(right, custody(112), &policy).unwrap();
    let transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    transport
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();

    let sync_b = ReplicationSyncCoordinator::new(repository_b, transport.clone());
    let review_b = sync_b.review(&policy).unwrap();
    assert_eq!(
        review_b.disposition(),
        ReplicationSyncDisposition::ConflictReviewRequired
    );
    let resolver = ReplicationReplicaId::new("device-c").unwrap();
    let resolved_b = sync_b
        .resolve(
            &review_b,
            &resolver,
            resolutions_for(&review_b, &resolver, &policy),
            &policy,
        )
        .unwrap();

    let sync_a = ReplicationSyncCoordinator::new(repository_a, transport);
    let review_a = sync_a.review(&policy).unwrap();
    assert_eq!(
        review_a.disposition(),
        ReplicationSyncDisposition::UpdateLocal
    );
    let resolved_a = sync_a.apply(&review_a, &policy).unwrap();
    assert_eq!(resolved_a.local.document, resolved_b.local.document);

    drop(sync_a);
    drop(sync_b);
    let reopened_a = coordinator(device_a.path(), shared.path(), left.workspace_id.clone());
    let reopened_b = coordinator(device_b.path(), shared.path(), left.workspace_id.clone());
    assert_eq!(
        reopened_a.review(&policy).unwrap().disposition(),
        ReplicationSyncDisposition::InSync
    );
    assert_eq!(
        reopened_b.review(&policy).unwrap().disposition(),
        ReplicationSyncDisposition::InSync
    );
    let reopened_document_a = ReplicationRepository::open(device_a.path())
        .unwrap()
        .load(&left.workspace_id, &policy)
        .unwrap()
        .document;
    let reopened_document_b = ReplicationRepository::open(device_b.path())
        .unwrap()
        .load(&left.workspace_id, &policy)
        .unwrap()
        .document;
    let reopened_transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap()
            .pull(&policy)
            .unwrap()
            .unwrap()
            .document;
    let canonical = canonical_bytes(&reopened_document_a, &policy);
    assert_eq!(canonical_bytes(&reopened_document_b, &policy), canonical);
    assert_eq!(canonical_bytes(&reopened_transport, &policy), canonical);

    let revoked_policy = ReplicationPolicy::new([
        (
            ReplicationReplicaId::new("device-a").unwrap(),
            ReplicaAuthorization::Active,
        ),
        (
            ReplicationReplicaId::new("device-b").unwrap(),
            ReplicaAuthorization::Active,
        ),
        (
            resolver.clone(),
            ReplicaAuthorization::Revoked {
                accepted_through: 1,
            },
        ),
    ])
    .unwrap();
    let revoked_root = tempfile::tempdir().unwrap();
    let revoked_shared = tempfile::tempdir().unwrap();
    let revoked_repository = ReplicationRepository::open(revoked_root.path()).unwrap();
    revoked_repository
        .create(document(&source, "right"), custody(113), &revoked_policy)
        .unwrap();
    let revoked_transport = SharedFolderReplicationTransport::open(
        revoked_shared.path(),
        left.workspace_id.clone(),
        slot(),
    )
    .unwrap();
    revoked_transport
        .publish(SharedFolderTransportState::Absent, &left, &revoked_policy)
        .unwrap();
    let revoked_sync = ReplicationSyncCoordinator::new(revoked_repository, revoked_transport);
    let revoked_plan = revoked_sync.review(&revoked_policy).unwrap();
    assert!(matches!(
        revoked_plan.resolution_context(
            revoked_plan.conflicts()[0].key(),
            &resolver,
            &revoked_policy,
        ),
        Err(ReplicationStoreError::Domain(
            termirust_domain::ReplicationError::ReplicaNotActive
        ))
    ));
}

#[test]
fn partial_local_commit_and_provider_replacement_converge_on_reviewed_retry() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let resolved = document(&source, "resolved");
    let root = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    let initial = repository
        .create(left.clone(), custody(102), &policy)
        .unwrap();
    let setup_transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    setup_transport
        .publish(SharedFolderTransportState::Absent, &resolved, &policy)
        .unwrap();
    let racing_transport = SharedFolderReplicationTransport::open_with(
        shared.path(),
        left.workspace_id.clone(),
        slot(),
        Arc::new(ReplaceTransportThenFailWriter::once(canonical_bytes(
            &right, &policy,
        ))),
    )
    .unwrap();
    let coordinator = ReplicationSyncCoordinator::new(repository.clone(), racing_transport);
    let plan = coordinator.review(&policy).unwrap();
    assert_eq!(plan.disposition(), ReplicationSyncDisposition::Converge);
    assert!(matches!(
        coordinator.apply(&plan, &policy),
        Err(ReplicationStoreError::Io {
            operation: "publish transport",
            kind: io::ErrorKind::WriteZero,
        })
    ));
    assert_eq!(
        repository
            .load(&left.workspace_id, &policy)
            .unwrap()
            .revision
            .get(),
        initial.revision.get() + 1
    );

    let retry = coordinator.review(&policy).unwrap();
    assert_eq!(
        retry.disposition(),
        ReplicationSyncDisposition::ConflictReviewRequired
    );
    let author = ReplicationReplicaId::new("device-c").unwrap();
    coordinator
        .resolve(
            &retry,
            &author,
            resolutions_for(&retry, &author, &policy),
            &policy,
        )
        .unwrap();
    assert_eq!(
        coordinator.review(&policy).unwrap().disposition(),
        ReplicationSyncDisposition::InSync
    );
}

#[test]
fn explicit_recovery_preserves_corrupt_primary_and_is_retryable_after_activation_failure() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let setup = ReplicationRepository::open(root.path()).unwrap();
    let initial = setup.create(left.clone(), custody(102), &policy).unwrap();
    setup
        .commit(initial.revision, right, custody(102), &[], &policy)
        .unwrap();
    let corrupt = b"{CORRUPT-PRIMARY-CANARY";
    fs::write(setup.metadata_path(), corrupt).unwrap();
    assert_eq!(
        setup.load(&left.workspace_id, &policy).unwrap().source,
        ReplicationRepositorySource::LastGood
    );

    let shared = tempfile::tempdir().unwrap();
    let transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    transport
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    let blocked = ReplicationSyncCoordinator::new(setup.clone(), transport);
    let recovery_plan = blocked.review(&policy).unwrap();
    assert_eq!(
        recovery_plan.disposition(),
        ReplicationSyncDisposition::RecoveryRequired
    );
    assert_eq!(
        blocked.apply(&recovery_plan, &policy).unwrap_err(),
        ReplicationStoreError::RecoveryRequired
    );

    let retryable =
        ReplicationRepository::open_with(root.path(), Arc::new(FailPrimaryWriter::once())).unwrap();
    assert!(matches!(
        retryable.recover_last_good(&left.workspace_id, &policy),
        Err(ReplicationStoreError::Io {
            operation: "write repository",
            kind: io::ErrorKind::WriteZero,
        })
    ));
    assert_eq!(fs::read(retryable.metadata_path()).unwrap(), corrupt);
    assert_eq!(
        fs::read(retryable.recovery_evidence_path()).unwrap(),
        corrupt
    );

    let recovered = retryable
        .recover_last_good(&left.workspace_id, &policy)
        .unwrap();
    assert_eq!(
        recovered.snapshot.source,
        ReplicationRepositorySource::Primary
    );
    assert_eq!(recovered.snapshot.document, left);
    assert_eq!(
        fs::read(retryable.recovery_evidence_path()).unwrap(),
        corrupt
    );
    assert_eq!(
        retryable
            .recover_last_good(&recovered.snapshot.document.workspace_id, &policy)
            .unwrap_err(),
        ReplicationStoreError::RecoveryNotRequired
    );
}

#[test]
fn recovery_refuses_newer_wrong_workspace_and_existing_or_unsafe_evidence() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");

    let newer_root = tempfile::tempdir().unwrap();
    let newer = ReplicationRepository::open(newer_root.path()).unwrap();
    let initial = newer.create(left.clone(), custody(102), &policy).unwrap();
    newer
        .commit(initial.revision, right.clone(), custody(102), &[], &policy)
        .unwrap();
    fs::write(
        newer.metadata_path(),
        br#"{"format_version":65535,"future":true}"#,
    )
    .unwrap();
    assert_eq!(
        newer
            .recover_last_good(&left.workspace_id, &policy)
            .unwrap_err(),
        ReplicationStoreError::Newer {
            found: 65535,
            supported: 1,
        }
    );
    assert!(!newer.recovery_evidence_path().exists());

    let wrong_workspace = termirust_domain::ReplicationWorkspaceId::new("workspace-other").unwrap();
    let wrong_workspace_root = tempfile::tempdir().unwrap();
    let wrong_workspace_repository =
        ReplicationRepository::open(wrong_workspace_root.path()).unwrap();
    wrong_workspace_repository
        .create(left.clone(), custody(103), &policy)
        .unwrap();
    assert_eq!(
        wrong_workspace_repository
            .recover_last_good(&wrong_workspace, &policy)
            .unwrap_err(),
        ReplicationStoreError::WorkspaceMismatch
    );
    assert!(!wrong_workspace_repository.recovery_evidence_path().exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let unsafe_journal_root = tempfile::tempdir().unwrap();
        let unsafe_journal = ReplicationRepository::open(unsafe_journal_root.path()).unwrap();
        let initial = unsafe_journal
            .create(left.clone(), custody(104), &policy)
            .unwrap();
        unsafe_journal
            .commit(initial.revision, right.clone(), custody(104), &[], &policy)
            .unwrap();
        let corrupt = b"{";
        fs::write(unsafe_journal.metadata_path(), corrupt).unwrap();
        let outside = unsafe_journal_root.path().join("outside-journal");
        fs::write(&outside, b"sentinel").unwrap();
        symlink(&outside, unsafe_journal.transaction_path()).unwrap();
        assert_eq!(
            unsafe_journal
                .recover_last_good(&left.workspace_id, &policy)
                .unwrap_err(),
            ReplicationStoreError::UnsafeEntry
        );
        assert_eq!(fs::read(unsafe_journal.metadata_path()).unwrap(), corrupt);
        assert!(!unsafe_journal.recovery_evidence_path().exists());
        assert_eq!(fs::read(outside).unwrap(), b"sentinel");
    }

    let occupied_root = tempfile::tempdir().unwrap();
    let occupied = ReplicationRepository::open(occupied_root.path()).unwrap();
    let initial = occupied
        .create(left.clone(), custody(105), &policy)
        .unwrap();
    occupied
        .commit(initial.revision, right, custody(105), &[], &policy)
        .unwrap();
    fs::write(occupied.metadata_path(), b"{").unwrap();
    fs::write(occupied.recovery_evidence_path(), b"existing-evidence").unwrap();
    assert_eq!(
        occupied
            .recover_last_good(&left.workspace_id, &policy)
            .unwrap_err(),
        ReplicationStoreError::RecoveryEvidenceExists
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::remove_file(occupied.recovery_evidence_path()).unwrap();
        let outside = occupied_root.path().join("outside");
        fs::write(&outside, b"sentinel").unwrap();
        symlink(&outside, occupied.recovery_evidence_path()).unwrap();
        assert_eq!(
            occupied
                .recover_last_good(&left.workspace_id, &policy)
                .unwrap_err(),
            ReplicationStoreError::UnsafeEntry
        );
        assert_eq!(fs::read(outside).unwrap(), b"sentinel");
    }
}

#[test]
fn oversized_primary_recovers_without_copying_or_discarding_evidence() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    let initial = repository
        .create(left.clone(), custody(105), &policy)
        .unwrap();
    repository
        .commit(initial.revision, right, custody(105), &[], &policy)
        .unwrap();

    fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(repository.metadata_path())
        .unwrap()
        .set_len(MAX_REPLICATION_REPOSITORY_BYTES + 1)
        .unwrap();

    let recovered = repository
        .recover_last_good(&left.workspace_id, &policy)
        .unwrap();
    assert_eq!(
        recovered.snapshot.source,
        ReplicationRepositorySource::Primary
    );
    assert_eq!(recovered.snapshot.document, left);
    assert_eq!(
        fs::metadata(repository.recovery_evidence_path())
            .unwrap()
            .len(),
        MAX_REPLICATION_REPOSITORY_BYTES + 1
    );
}

#[test]
fn recovery_refuses_last_good_state_rejected_by_current_policy() {
    let source = fixture();
    let original_policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    let initial = repository
        .create(left.clone(), custody(106), &original_policy)
        .unwrap();
    repository
        .commit(initial.revision, right, custody(106), &[], &original_policy)
        .unwrap();
    let corrupt = b"{";
    fs::write(repository.metadata_path(), corrupt).unwrap();
    let rejecting_policy = ReplicationPolicy::new(source.replicas.iter().map(|replica| {
        (
            ReplicationReplicaId::new(replica.id.clone()).unwrap(),
            ReplicaAuthorization::Revoked {
                accepted_through: 0,
            },
        )
    }))
    .unwrap();

    assert!(matches!(
        repository.recover_last_good(&left.workspace_id, &rejecting_policy),
        Err(ReplicationStoreError::Domain(_))
    ));
    assert_eq!(fs::read(repository.metadata_path()).unwrap(), corrupt);
    assert!(!repository.recovery_evidence_path().exists());
}

#[cfg(unix)]
#[test]
fn recovery_permission_failure_leaves_corrupt_primary_untouched() {
    use std::os::unix::fs::PermissionsExt as _;

    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    let initial = repository
        .create(left.clone(), custody(107), &policy)
        .unwrap();
    repository
        .commit(initial.revision, right, custody(107), &[], &policy)
        .unwrap();
    let corrupt = b"{";
    fs::write(repository.metadata_path(), corrupt).unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o500)).unwrap();

    let result = repository.recover_last_good(&left.workspace_id, &policy);
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();

    assert!(matches!(
        result,
        Err(ReplicationStoreError::Io {
            kind: io::ErrorKind::PermissionDenied,
            ..
        })
    ));
    assert_eq!(fs::read(repository.metadata_path()).unwrap(), corrupt);
    assert!(!repository.recovery_evidence_path().exists());
}

#[test]
fn review_and_resolution_debug_output_redacts_tokens_keys_authors_and_paths() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    repository.create(right, custody(102), &policy).unwrap();
    let transport =
        SharedFolderReplicationTransport::open(shared.path(), left.workspace_id.clone(), slot())
            .unwrap();
    transport
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    let plan = ReplicationSyncCoordinator::new(repository, transport)
        .review(&policy)
        .unwrap();
    let author = ReplicationReplicaId::new("device-c").unwrap();
    let resolutions = resolutions_for(&plan, &author, &policy);
    let rendered = format!("{plan:?} {:?} {:?}", plan.conflicts(), resolutions);
    assert!(!rendered.contains(TEST_SLOT));
    assert!(!rendered.contains("workspace-alpha"));
    assert!(!rendered.contains("device-c"));
    assert!(!rendered.contains("server-1"));
    assert!(!rendered.contains(root.path().to_string_lossy().as_ref()));
    assert!(!rendered.contains(&hex(plan.token().as_bytes())));
}
