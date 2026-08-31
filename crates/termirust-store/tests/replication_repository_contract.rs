use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use termirust_domain::{
    MAX_REPLICATION_DOCUMENT_BYTES, ReplicaAuthorization, ReplicationDocument, ReplicationPolicy,
    ReplicationReplicaId, merge_replication_documents,
};
use termirust_replication_security::{
    ReplicationHistoricalKeyIndex, ReplicationHistoricalKeyLimit, ReplicationKeyEpoch,
    ReplicationSecretBackend, ReplicationSecretKind, ReplicationSecretRef,
    ReplicationSecretStoreError,
};
use termirust_store::{
    AtomicWriter, Durability, MAX_REPLICATION_CONFLICT_ARTIFACTS, ReplicationCustodyMetadata,
    ReplicationRepository, ReplicationRepositoryRevision, ReplicationRepositorySource,
    ReplicationRetirementOutcome, ReplicationStoreError, SharedFolderReplicationTransport,
    SharedFolderSlot, SharedFolderTransportState, SystemAtomicWriter,
};
use zeroize::Zeroizing;

const TEST_SLOT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
struct RepositoryTransportFixture {
    schema_version: u16,
    repository_format_version: u16,
    slot: String,
    artifact_file_name: String,
    left_canonical_sha256: String,
    right_canonical_sha256: String,
    max_repository_bytes: u64,
    max_conflict_artifacts: usize,
}

fn fixture() -> MergeFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/merge-contract-v1.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn repository_transport_fixture() -> RepositoryTransportFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/repository-transport-contract-v1.json");
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

fn custody(epoch_markers: &[(u64, u8)], limit: usize) -> ReplicationCustodyMetadata {
    let historical = ReplicationHistoricalKeyIndex::from_retained(
        ReplicationHistoricalKeyLimit::new(limit).unwrap(),
        epoch_markers
            .iter()
            .map(|(epoch, marker)| secret(ReplicationSecretKind::EpochKey, Some(*epoch), *marker)),
    )
    .unwrap();
    ReplicationCustodyMetadata::new(
        secret(ReplicationSecretKind::AuthorityPrivateKey, None, 101),
        secret(ReplicationSecretKind::DevicePrivateKey, None, 102),
        historical,
    )
    .unwrap()
}

#[derive(Default)]
struct MemorySecretBackend {
    entries: Mutex<BTreeSet<Vec<u8>>>,
    failure: Mutex<Option<ReplicationSecretStoreError>>,
}

impl MemorySecretBackend {
    fn insert(&self, reference: &ReplicationSecretRef) {
        self.entries
            .lock()
            .unwrap()
            .insert(reference.to_bytes().to_vec());
    }

    fn contains(&self, reference: &ReplicationSecretRef) -> bool {
        self.entries
            .lock()
            .unwrap()
            .contains(reference.to_bytes().as_slice())
    }

    fn fail_with(&self, error: Option<ReplicationSecretStoreError>) {
        *self.failure.lock().unwrap() = error;
    }

    fn check_failure(&self) -> Result<(), ReplicationSecretStoreError> {
        match *self.failure.lock().unwrap() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl ReplicationSecretBackend for MemorySecretBackend {
    fn put(
        &self,
        reference: &ReplicationSecretRef,
        _secret: &[u8],
    ) -> Result<(), ReplicationSecretStoreError> {
        self.check_failure()?;
        if !self
            .entries
            .lock()
            .unwrap()
            .insert(reference.to_bytes().to_vec())
        {
            return Err(ReplicationSecretStoreError::Collision);
        }
        Ok(())
    }

    fn get(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<Zeroizing<Vec<u8>>, ReplicationSecretStoreError> {
        self.check_failure()?;
        self.contains(reference)
            .then(|| Zeroizing::new(vec![1]))
            .ok_or(ReplicationSecretStoreError::Missing)
    }

    fn delete(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<bool, ReplicationSecretStoreError> {
        self.check_failure()?;
        Ok(self
            .entries
            .lock()
            .unwrap()
            .remove(reference.to_bytes().as_slice()))
    }
}

struct FailTargetWriter {
    file_name: &'static str,
    remaining: Mutex<usize>,
}

impl FailTargetWriter {
    fn new(file_name: &'static str, failures: usize) -> Self {
        Self {
            file_name,
            remaining: Mutex::new(failures),
        }
    }
}

impl AtomicWriter for FailTargetWriter {
    fn write(&self, target: &Path, bytes: &[u8]) -> io::Result<Durability> {
        let mut remaining = self.remaining.lock().unwrap();
        if target.file_name().and_then(|name| name.to_str()) == Some(self.file_name)
            && *remaining > 0
        {
            *remaining -= 1;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "synthetic failure",
            ));
        }
        SystemAtomicWriter.write(target, bytes)
    }
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

#[test]
fn exact_repository_transport_fixture_pins_paths_hashes_and_limits() {
    let contract = repository_transport_fixture();
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let folder = tempfile::tempdir().unwrap();
    let transport = SharedFolderReplicationTransport::open(
        folder.path(),
        left.workspace_id.clone(),
        SharedFolderSlot::new(contract.slot).unwrap(),
    )
    .unwrap();
    let left = transport
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    let right = transport
        .publish(
            SharedFolderTransportState::Present(left.revision),
            &right,
            &policy,
        )
        .unwrap();

    assert_eq!(contract.schema_version, 1);
    assert_eq!(contract.repository_format_version, 1);
    assert_eq!(
        transport.artifact_path().file_name().unwrap(),
        contract.artifact_file_name.as_str()
    );
    assert_eq!(
        hex(left.revision.as_bytes()),
        contract.left_canonical_sha256
    );
    assert_eq!(
        hex(right.revision.as_bytes()),
        contract.right_canonical_sha256
    );
    assert_eq!(
        contract.max_repository_bytes,
        termirust_store::replication::MAX_REPLICATION_REPOSITORY_BYTES
    );
    assert_eq!(
        contract.max_conflict_artifacts,
        MAX_REPLICATION_CONFLICT_ARTIFACTS
    );
}

#[test]
fn private_repository_round_trips_rejects_stale_writers_and_uses_private_modes() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path().join("private")).unwrap();
    let initial = repository
        .create(left.clone(), custody(&[(1, 1)], 2), &policy)
        .unwrap();
    assert_eq!(initial.revision, ReplicationRepositoryRevision::INITIAL);
    assert_eq!(initial.source, ReplicationRepositorySource::Primary);
    assert_eq!(
        initial.document,
        merge_replication_documents(&left, &left, &policy)
            .unwrap()
            .document
    );

    let saved = repository
        .commit(
            initial.revision,
            right.clone(),
            custody(&[(1, 1)], 2),
            &[],
            &policy,
        )
        .unwrap();
    assert_eq!(saved.revision.get(), 2);
    assert_eq!(
        repository
            .load(&right.workspace_id, &policy)
            .unwrap()
            .document,
        right
    );
    assert_eq!(
        repository
            .commit(initial.revision, left, custody(&[(1, 1)], 2), &[], &policy)
            .unwrap_err(),
        ReplicationStoreError::StaleRepositoryRevision {
            expected: initial.revision,
            actual: saved.revision,
        }
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(root.path().join("private"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(repository.metadata_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn corrupt_primary_uses_last_good_read_only_but_newer_and_unsafe_never_fall_back() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    let initial = repository
        .create(left.clone(), custody(&[(1, 1)], 2), &policy)
        .unwrap();
    repository
        .commit(initial.revision, right, custody(&[(1, 1)], 2), &[], &policy)
        .unwrap();
    fs::write(repository.metadata_path(), b"{").unwrap();
    let recovered = repository.load(&left.workspace_id, &policy).unwrap();
    assert_eq!(recovered.source, ReplicationRepositorySource::LastGood);
    assert_eq!(recovered.document, left);

    fs::write(
        repository.metadata_path(),
        br#"{"format_version":65535,"future":true}"#,
    )
    .unwrap();
    assert_eq!(
        repository
            .load(&recovered.document.workspace_id, &policy)
            .unwrap_err(),
        ReplicationStoreError::Newer {
            found: 65535,
            supported: 1,
        }
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::remove_file(repository.metadata_path()).unwrap();
        symlink(
            root.path().join("replica.last-good.json"),
            repository.metadata_path(),
        )
        .unwrap();
        assert_eq!(
            repository
                .load(&recovered.document.workspace_id, &policy)
                .unwrap_err(),
            ReplicationStoreError::UnsafeEntry
        );
    }
}

#[test]
fn corrupt_initial_repository_and_oversized_reference_sequences_fail_bounded() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let root = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    repository
        .create(left.clone(), custody(&[(1, 1)], 2), &policy)
        .unwrap();
    let mut stored: serde_json::Value =
        serde_json::from_slice(&fs::read(repository.metadata_path()).unwrap()).unwrap();
    stored["custody"]["authority"] = serde_json::Value::Array(
        (0..=termirust_replication_security::REPLICATION_SECRET_REFERENCE_BYTES)
            .map(|_| serde_json::Value::from(1))
            .collect(),
    );
    fs::write(
        repository.metadata_path(),
        serde_json::to_vec(&stored).unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository.load(&left.workspace_id, &policy).unwrap_err(),
        ReplicationStoreError::Corrupt
    );

    stored["custody"]["authority"] = serde_json::to_value(
        secret(ReplicationSecretKind::AuthorityPrivateKey, None, 101)
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    stored["custody"]["epoch_references"] = serde_json::Value::Array(
        (0..=termirust_replication_security::MAX_REPLICATION_RETAINED_EPOCH_KEYS)
            .map(|_| {
                serde_json::to_value(
                    secret(ReplicationSecretKind::EpochKey, Some(1), 1)
                        .to_bytes()
                        .to_vec(),
                )
                .unwrap()
            })
            .collect(),
    );
    fs::write(
        repository.metadata_path(),
        serde_json::to_vec(&stored).unwrap(),
    )
    .unwrap();
    assert_eq!(
        repository.load(&left.workspace_id, &policy).unwrap_err(),
        ReplicationStoreError::Corrupt
    );
}

#[test]
fn interrupted_activation_keeps_old_primary_and_abandons_retirement_without_deleting() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let root = tempfile::tempdir().unwrap();
    let setup = ReplicationRepository::open(root.path()).unwrap();
    let initial = setup
        .create(left.clone(), custody(&[(1, 1), (2, 2)], 2), &policy)
        .unwrap();
    let retired = secret(ReplicationSecretKind::EpochKey, Some(1), 1);
    let failing = ReplicationRepository::open_with(
        root.path(),
        Arc::new(FailTargetWriter::new("replica.json", 1)),
    )
    .unwrap();
    assert!(matches!(
        failing.commit(
            initial.revision,
            right,
            custody(&[(2, 2), (3, 3)], 2),
            std::slice::from_ref(&retired),
            &policy,
        ),
        Err(ReplicationStoreError::Io {
            operation: "write repository",
            kind: io::ErrorKind::WriteZero,
        })
    ));
    let backend = MemorySecretBackend::default();
    backend.insert(&retired);
    let loaded = setup.load(&left.workspace_id, &policy).unwrap();
    assert_eq!(loaded.revision, initial.revision);
    assert!(loaded.retirement_pending);
    assert_eq!(
        setup
            .retire_pending(&backend, &left.workspace_id, &policy)
            .unwrap(),
        ReplicationRetirementOutcome::AbandonedUncommitted {
            reference_count: 1,
            durability: Durability::Full,
        }
    );
    assert!(backend.contains(&retired));
    assert!(!setup.transaction_path().exists());
}

#[test]
fn committed_retirement_survives_locked_backend_and_retries_idempotently() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let root = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    let initial = repository
        .create(left.clone(), custody(&[(1, 1), (2, 2)], 2), &policy)
        .unwrap();
    let retired = secret(ReplicationSecretKind::EpochKey, Some(1), 1);
    let committed = repository
        .commit(
            initial.revision,
            left.clone(),
            custody(&[(2, 2), (3, 3)], 2),
            std::slice::from_ref(&retired),
            &policy,
        )
        .unwrap();
    assert!(committed.retirement_pending);

    let backend = MemorySecretBackend::default();
    backend.insert(&retired);
    backend.fail_with(Some(ReplicationSecretStoreError::AccessDeniedOrLocked));
    assert_eq!(
        repository
            .retire_pending(&backend, &left.workspace_id, &policy)
            .unwrap_err(),
        ReplicationStoreError::Custody(
            termirust_replication_security::ReplicationSecretCustodyError::Store(
                ReplicationSecretStoreError::AccessDeniedOrLocked,
            ),
        )
    );
    assert!(repository.transaction_path().exists());
    assert!(backend.contains(&retired));

    backend.fail_with(None);
    assert_eq!(
        repository
            .retire_pending(&backend, &left.workspace_id, &policy)
            .unwrap(),
        ReplicationRetirementOutcome::Completed {
            deleted: 1,
            already_missing: 0,
            durability: Durability::Full,
        }
    );
    assert!(!backend.contains(&retired));
    assert_eq!(
        repository
            .retire_pending(&backend, &left.workspace_id, &policy)
            .unwrap(),
        ReplicationRetirementOutcome::NothingPending
    );
}

#[test]
fn retirement_requires_the_exact_removed_owned_epoch_set() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let root = tempfile::tempdir().unwrap();
    let repository = ReplicationRepository::open(root.path()).unwrap();
    let initial = repository
        .create(left.clone(), custody(&[(1, 1), (2, 2)], 2), &policy)
        .unwrap();
    let unrelated = secret(ReplicationSecretKind::EpochKey, Some(9), 9);
    assert_eq!(
        repository
            .commit(
                initial.revision,
                left.clone(),
                custody(&[(2, 2), (3, 3)], 2),
                std::slice::from_ref(&unrelated),
                &policy,
            )
            .unwrap_err(),
        ReplicationStoreError::InvalidCustodyTransition
    );
    assert_eq!(
        repository
            .load(&left.workspace_id, &policy)
            .unwrap()
            .revision,
        initial.revision
    );

    assert_eq!(
        repository
            .commit(
                initial.revision,
                left.clone(),
                custody(&[(2, 2), (3, 3)], 2),
                &[],
                &policy,
            )
            .unwrap_err(),
        ReplicationStoreError::InvalidCustodyTransition
    );
    let authority = secret(ReplicationSecretKind::AuthorityPrivateKey, None, 101);
    assert_eq!(
        repository
            .commit(
                initial.revision,
                left,
                custody(&[(1, 1), (2, 2)], 2),
                std::slice::from_ref(&authority),
                &policy,
            )
            .unwrap_err(),
        ReplicationStoreError::InvalidCustodyTransition
    );
}

#[test]
fn shared_folder_transport_uses_exact_cas_and_preserves_concurrent_candidates() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let folder = tempfile::tempdir().unwrap();
    let first =
        SharedFolderReplicationTransport::open(folder.path(), left.workspace_id.clone(), slot())
            .unwrap();
    let second = first.clone();
    assert_eq!(
        first.observe(&policy).unwrap(),
        SharedFolderTransportState::Absent
    );
    let left_snapshot = first
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    assert_eq!(
        second.pull(&policy).unwrap().unwrap().document,
        left_snapshot.document
    );

    let right_snapshot = second
        .publish(
            SharedFolderTransportState::Present(left_snapshot.revision),
            &right,
            &policy,
        )
        .unwrap();
    assert_eq!(
        first
            .publish(
                SharedFolderTransportState::Present(left_snapshot.revision),
                &left,
                &policy,
            )
            .unwrap_err(),
        ReplicationStoreError::StaleTransportRevision
    );

    let merged = merge_replication_documents(&left, &right, &policy)
        .unwrap()
        .document;
    let merged_snapshot = first
        .publish(
            SharedFolderTransportState::Present(right_snapshot.revision),
            &merged,
            &policy,
        )
        .unwrap();
    assert!(
        merged_snapshot
            .document
            .entries
            .iter()
            .any(|entry| entry.candidates.len() == 2)
    );
    assert_eq!(first.pull(&policy).unwrap().unwrap(), merged_snapshot);
}

#[test]
fn shared_folder_lock_and_cas_allow_exactly_one_concurrent_writer() {
    let source = fixture();
    let policy = Arc::new(policy(&source));
    let left = document(&source, "left");
    let right = document(&source, "right");
    let resolved = document(&source, "resolved");
    let folder = tempfile::tempdir().unwrap();
    let transport = Arc::new(
        SharedFolderReplicationTransport::open(folder.path(), left.workspace_id.clone(), slot())
            .unwrap(),
    );
    let initial = transport
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let handles = [right, resolved].map(|document| {
        let transport = Arc::clone(&transport);
        let policy = Arc::clone(&policy);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            transport.publish(
                SharedFolderTransportState::Present(initial.revision),
                &document,
                &policy,
            )
        })
    });
    barrier.wait();
    let results = handles.map(|handle| handle.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ReplicationStoreError::StaleTransportRevision)))
            .count(),
        1
    );
}

#[test]
fn shared_folder_publish_failure_preserves_the_observed_artifact() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");
    let right = document(&source, "right");
    let folder = tempfile::tempdir().unwrap();
    let setup =
        SharedFolderReplicationTransport::open(folder.path(), left.workspace_id.clone(), slot())
            .unwrap();
    let initial = setup
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    let failing = SharedFolderReplicationTransport::open_with(
        folder.path(),
        left.workspace_id.clone(),
        slot(),
        Arc::new(FailTargetWriter::new(
            ".termirust-replica-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.json",
            1,
        )),
    )
    .unwrap();
    assert!(matches!(
        failing.publish(
            SharedFolderTransportState::Present(initial.revision),
            &right,
            &policy,
        ),
        Err(ReplicationStoreError::Io {
            operation: "publish transport",
            kind: io::ErrorKind::WriteZero,
        })
    ));
    assert_eq!(
        setup.pull(&policy).unwrap().unwrap().revision,
        initial.revision
    );
}

#[test]
fn transport_rejects_noncanonical_oversized_and_unsafe_artifacts_and_bounds_conflicts() {
    let source = fixture();
    let policy = policy(&source);
    let left = document(&source, "left");

    let noncanonical_folder = tempfile::tempdir().unwrap();
    let noncanonical = SharedFolderReplicationTransport::open(
        noncanonical_folder.path(),
        left.workspace_id.clone(),
        slot(),
    )
    .unwrap();
    fs::write(
        noncanonical.artifact_path(),
        serde_json::to_vec_pretty(&left).unwrap(),
    )
    .unwrap();
    assert_eq!(
        noncanonical.pull(&policy).unwrap_err(),
        ReplicationStoreError::Corrupt
    );

    fs::write(
        noncanonical.artifact_path(),
        vec![b' '; MAX_REPLICATION_DOCUMENT_BYTES + 1],
    )
    .unwrap();
    assert_eq!(
        noncanonical.pull(&policy).unwrap_err(),
        ReplicationStoreError::TooLarge
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::remove_file(noncanonical.artifact_path()).unwrap();
        let outside = noncanonical_folder.path().join("outside");
        fs::write(&outside, b"sentinel").unwrap();
        symlink(&outside, noncanonical.artifact_path()).unwrap();
        assert_eq!(
            noncanonical.pull(&policy).unwrap_err(),
            ReplicationStoreError::UnsafeEntry
        );
        assert_eq!(fs::read(outside).unwrap(), b"sentinel");
    }

    let conflict_folder = tempfile::tempdir().unwrap();
    let transport = SharedFolderReplicationTransport::open(
        conflict_folder.path(),
        left.workspace_id.clone(),
        slot(),
    )
    .unwrap();
    let snapshot = transport
        .publish(SharedFolderTransportState::Absent, &left, &policy)
        .unwrap();
    let target = transport.artifact_path();
    let stem = target.file_stem().unwrap().to_str().unwrap();
    let first_conflict = conflict_folder
        .path()
        .join(format!("{stem}.sync-conflict-20260831-device.json"));
    fs::copy(&target, &first_conflict).unwrap();
    let conflicts = transport.conflict_artifacts(&policy).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].revision, snapshot.revision);

    for index in 0..16 {
        fs::copy(
            &target,
            conflict_folder
                .path()
                .join(format!("{stem}.conflict-extra-{index}.json")),
        )
        .unwrap();
    }
    assert_eq!(
        transport.conflict_artifacts(&policy).unwrap_err(),
        ReplicationStoreError::TooManyConflictArtifacts
    );
}

#[test]
fn public_errors_and_debug_output_do_not_expose_paths_slots_or_secret_references() {
    let slot = slot();
    let reference = secret(ReplicationSecretKind::EpochKey, Some(42), 77);
    let path_canary = "PRIVATE-PATH-CANARY";
    let rendered = format!(
        "{slot:?} {reference:?} {}",
        ReplicationStoreError::Io {
            operation: "read transport",
            kind: io::ErrorKind::PermissionDenied,
        }
    );
    assert!(!rendered.contains(TEST_SLOT));
    assert!(!rendered.contains(&reference.expose_opaque_account()));
    assert!(!rendered.contains(path_canary));
}
