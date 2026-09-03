use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use termirust_domain::{ReplicationCollectionId, ReplicationRecordId, ReplicationRecordKey};
use termirust_replication_security::{
    ReplicationSecretBackend, ReplicationSecretRef, ReplicationSecretStoreError,
    generate_replication_device_private_key,
};
use termirust_store::{
    ReplicationConflictChoice, ReplicationEnrollmentBundle, ReplicationEnrollmentRequest,
    ReplicationProductError, ReplicationProductService, ReplicationSyncDisposition,
};
use zeroize::Zeroizing;

#[derive(Clone, Default)]
struct MemorySecrets {
    inner: Arc<Mutex<MemorySecretsState>>,
}

#[derive(Default)]
struct MemorySecretsState {
    values: BTreeMap<Vec<u8>, Vec<u8>>,
    puts: usize,
    fail_at_put: Option<usize>,
    fail_delete: bool,
}

impl MemorySecrets {
    fn failing_at(put: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MemorySecretsState {
                fail_at_put: Some(put),
                ..MemorySecretsState::default()
            })),
        }
    }

    fn count(&self) -> usize {
        self.inner.lock().unwrap().values.len()
    }

    fn reference_at_epoch(&self, epoch: u64) -> ReplicationSecretRef {
        self.inner
            .lock()
            .unwrap()
            .values
            .keys()
            .filter_map(|bytes| ReplicationSecretRef::from_bytes(bytes).ok())
            .find(|reference| {
                reference
                    .key_epoch()
                    .is_some_and(|value| value.get() == epoch)
            })
            .expect("epoch reference should exist")
    }

    fn set_delete_failure(&self, fail: bool) {
        self.inner.lock().unwrap().fail_delete = fail;
    }
}

impl ReplicationSecretBackend for MemorySecrets {
    fn put(
        &self,
        reference: &ReplicationSecretRef,
        secret: &[u8],
    ) -> Result<(), ReplicationSecretStoreError> {
        let mut state = self.inner.lock().unwrap();
        state.puts += 1;
        if state.fail_at_put == Some(state.puts) {
            return Err(ReplicationSecretStoreError::Unavailable);
        }
        if state
            .values
            .insert(reference.to_bytes().to_vec(), secret.to_vec())
            .is_some()
        {
            return Err(ReplicationSecretStoreError::Collision);
        }
        Ok(())
    }

    fn get(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<Zeroizing<Vec<u8>>, ReplicationSecretStoreError> {
        self.inner
            .lock()
            .unwrap()
            .values
            .get(reference.to_bytes().as_slice())
            .cloned()
            .map(Zeroizing::new)
            .ok_or(ReplicationSecretStoreError::Missing)
    }

    fn delete(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<bool, ReplicationSecretStoreError> {
        let mut state = self.inner.lock().unwrap();
        if state.fail_delete {
            return Err(ReplicationSecretStoreError::Unavailable);
        }
        Ok(state
            .values
            .remove(reference.to_bytes().as_slice())
            .is_some())
    }
}

fn record_key(name: &str) -> ReplicationRecordKey {
    ReplicationRecordKey::new(
        ReplicationCollectionId::new("connections").unwrap(),
        ReplicationRecordId::new(name).unwrap(),
    )
}

#[test]
fn bootstrap_mutate_review_publish_restart_and_delete_are_explicit() {
    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    let backend = MemorySecrets::default();
    let key = record_key("primary");

    let service = ReplicationProductService::bootstrap(&root, shared.path(), backend.clone())
        .expect("bootstrap should succeed");
    assert_eq!(service.root(), root);
    assert_eq!(service.shared_folder(), shared.path());
    assert_eq!(backend.count(), 3);
    assert_eq!(service.status().unwrap().record_count, 0);

    let first_revision = service
        .put_record(key.clone(), b"secret connection")
        .unwrap();
    assert_eq!(first_revision.get(), 2);
    assert_eq!(
        service.read_record(&key).unwrap(),
        Some(b"secret connection".to_vec())
    );
    let review = service.review_sync().unwrap();
    assert_eq!(
        review.disposition(),
        ReplicationSyncDisposition::PublishLocal
    );
    let outcome = service.apply_sync(&review).unwrap();
    assert!(outcome.transport_published);
    assert!(!outcome.local_changed);

    drop(service);
    let reopened = ReplicationProductService::open(&root, backend.clone()).unwrap();
    assert_eq!(
        reopened.read_record(&key).unwrap(),
        Some(b"secret connection".to_vec())
    );
    assert_eq!(
        reopened.review_sync().unwrap().disposition(),
        ReplicationSyncDisposition::InSync
    );
    let deleted_revision = reopened.delete_record(key.clone()).unwrap();
    assert_eq!(deleted_revision.get(), 3);
    assert_eq!(reopened.read_record(&key).unwrap(), None);
    assert_eq!(reopened.status().unwrap().record_count, 1);
}

#[test]
fn failed_bootstrap_removes_staging_and_every_created_secret() {
    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    let backend = MemorySecrets::failing_at(2);

    let error = ReplicationProductService::bootstrap(&root, shared.path(), backend.clone())
        .expect_err("second secret write should fail bootstrap");
    assert!(matches!(error, ReplicationProductError::Custody(_)));
    assert!(!root.exists());
    assert_eq!(backend.count(), 0);
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
}

#[test]
fn profile_is_canonical_bounded_and_rejects_unknown_fields() {
    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    let backend = MemorySecrets::default();
    let service = ReplicationProductService::bootstrap(&root, shared.path(), backend.clone())
        .expect("bootstrap should succeed");
    drop(service);

    let profile_path = root.join("profile.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), serde_json::Value::Bool(true));
    std::fs::write(&profile_path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(
        ReplicationProductService::open(&root, backend),
        Err(ReplicationProductError::InvalidProfile)
    ));
}

#[test]
fn existing_roots_are_never_repurposed() {
    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    std::fs::create_dir(&root).unwrap();
    let backend = MemorySecrets::default();
    assert!(matches!(
        ReplicationProductService::bootstrap(&root, shared.path(), backend),
        Err(ReplicationProductError::AlreadyConfigured)
    ));
}

#[cfg(unix)]
#[test]
fn bootstrap_uses_user_only_local_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    ReplicationProductService::bootstrap(&root, shared.path(), MemorySecrets::default()).unwrap();

    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(root.join("profile.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn enrollment_rotation_revocation_and_outbox_ack_survive_restart() {
    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    let backend = MemorySecrets::default();
    let mut service =
        ReplicationProductService::bootstrap(&root, shared.path(), backend.clone()).unwrap();
    let second_replica = termirust_domain::ReplicationReplicaId::new("device-phone").unwrap();
    let second_private = generate_replication_device_private_key().unwrap();

    let enrolled = service
        .enroll_device(second_replica.clone(), second_private.public_key())
        .unwrap();
    assert_eq!(enrolled.authority_revision, 2);
    assert_eq!(enrolled.key_epoch, 2);
    assert_eq!(enrolled.packages().len(), 2);
    assert!(enrolled.package_for(&second_replica).is_some());
    assert_eq!(service.status().unwrap().active_devices, 2);
    assert_eq!(
        service
            .pending_authority_update()
            .unwrap()
            .unwrap()
            .authority_revision,
        2
    );
    assert!(matches!(
        service.rotate_keys(),
        Err(ReplicationProductError::PendingAuthorityUpdate)
    ));
    assert!(matches!(
        service.acknowledge_authority_update(1),
        Err(ReplicationProductError::StaleAuthorityUpdate)
    ));
    assert!(service.acknowledge_authority_update(2).unwrap());
    assert!(!service.acknowledge_authority_update(2).unwrap());

    let rotated = service.rotate_keys().unwrap();
    assert_eq!(rotated.authority_revision, 3);
    assert_eq!(rotated.key_epoch, 3);
    assert_eq!(rotated.packages().len(), 2);
    drop(service);

    let mut reopened = ReplicationProductService::open(&root, backend.clone()).unwrap();
    assert_eq!(
        reopened
            .pending_authority_update()
            .unwrap()
            .unwrap()
            .authority_revision,
        3
    );
    assert!(reopened.acknowledge_authority_update(3).unwrap());
    let revoked = reopened.revoke_device(&second_replica).unwrap();
    assert_eq!(revoked.authority_revision, 4);
    assert_eq!(revoked.key_epoch, 4);
    assert_eq!(revoked.packages().len(), 1);
    assert!(revoked.package_for(&second_replica).is_none());
    let status = reopened.status().unwrap();
    assert_eq!(status.active_devices, 1);
    assert_eq!(status.total_devices, 2);
    let local_replica = reopened.local_replica_id().clone();
    assert!(matches!(
        reopened.revoke_device(&local_replica),
        Err(ReplicationProductError::LocalDeviceRevocation)
    ));
}

#[derive(serde::Deserialize, serde::Serialize)]
struct TestStoredPackage {
    recipient: String,
    package_hex: String,
}

#[derive(serde::Deserialize)]
struct TestStoredUpdate {
    format_version: u16,
    authority_state_hex: String,
    packages: Vec<TestStoredPackage>,
}

#[derive(serde::Serialize)]
struct TestStoredTransaction {
    format_version: u16,
    base_authority_revision: u64,
    base_repository_revision: u64,
    next_authority_state_hex: String,
    new_epoch_reference_hex: String,
    packages: Vec<TestStoredPackage>,
    publish_update: bool,
}

#[test]
fn restart_finishes_repository_committed_authority_transition() {
    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    let backend = MemorySecrets::default();
    let mut service =
        ReplicationProductService::bootstrap(&root, shared.path(), backend.clone()).unwrap();
    let old_profile = std::fs::read(root.join("profile.json")).unwrap();
    service.rotate_keys().unwrap();
    drop(service);

    let stored_update: TestStoredUpdate =
        serde_json::from_slice(&std::fs::read(root.join("authority-update.json")).unwrap())
            .unwrap();
    let epoch_reference = backend.reference_at_epoch(2);
    let transaction = TestStoredTransaction {
        format_version: stored_update.format_version,
        base_authority_revision: 1,
        base_repository_revision: 1,
        next_authority_state_hex: stored_update.authority_state_hex,
        new_epoch_reference_hex: hex_bytes(&epoch_reference.to_bytes()),
        packages: stored_update.packages,
        publish_update: true,
    };
    std::fs::write(root.join("profile.json"), old_profile).unwrap();
    std::fs::remove_file(root.join("authority-update.json")).unwrap();
    std::fs::write(
        root.join("authority.transaction.json"),
        serde_json::to_vec(&transaction).unwrap(),
    )
    .unwrap();

    let recovered = ReplicationProductService::open(&root, backend).unwrap();
    assert_eq!(recovered.status().unwrap().authority_revision, 2);
    assert_eq!(recovered.status().unwrap().repository_revision.get(), 2);
    assert_eq!(
        recovered
            .pending_authority_update()
            .unwrap()
            .unwrap()
            .authority_revision,
        2
    );
    assert!(!root.join("authority.transaction.json").exists());
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[test]
fn real_two_device_enrollment_converges_with_member_only_custody() {
    let owner_parent = tempfile::tempdir().unwrap();
    let member_parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let owner_root = owner_parent.path().join("replication");
    let member_root = member_parent.path().join("replication");
    let owner_backend = MemorySecrets::default();
    let member_backend = MemorySecrets::default();
    let existing = record_key("existing");
    let from_phone = record_key("from-phone");

    let mut owner =
        ReplicationProductService::bootstrap(&owner_root, shared.path(), owner_backend.clone())
            .unwrap();
    owner
        .put_record(existing.clone(), b"already saved")
        .unwrap();
    let publish = owner.review_sync().unwrap();
    owner.apply_sync(&publish).unwrap();

    let request = ReplicationProductService::<MemorySecrets>::prepare_enrollment(
        &member_root,
        shared.path(),
        member_backend.clone(),
    )
    .unwrap();
    let request =
        ReplicationEnrollmentRequest::from_canonical_bytes(&request.to_canonical_bytes().unwrap())
            .unwrap();
    let bundle = owner.enroll_request(&request).unwrap();
    let bundle =
        ReplicationEnrollmentBundle::from_canonical_bytes(&bundle.to_canonical_bytes().unwrap())
            .unwrap();
    let verification_code = bundle.verification_code(&request).unwrap();
    assert_eq!(verification_code.len(), 13);

    let rekey_publish = owner.review_sync().unwrap();
    assert_eq!(
        rekey_publish.disposition(),
        ReplicationSyncDisposition::PublishLocal
    );
    owner.apply_sync(&rekey_publish).unwrap();
    let member = ReplicationProductService::accept_enrollment(
        &member_root,
        member_backend.clone(),
        &bundle,
        &verification_code,
    )
    .unwrap();
    assert!(!member.status().unwrap().authority_owner);
    assert_eq!(member_backend.count(), 2);
    assert!(matches!(
        ReplicationProductService::accept_enrollment(
            &member_root,
            member_backend.clone(),
            &bundle,
            &verification_code,
        ),
        Err(ReplicationProductError::NotConfigured | ReplicationProductError::AlreadyConfigured)
    ));

    let pull = member.review_sync().unwrap();
    assert_eq!(pull.disposition(), ReplicationSyncDisposition::UpdateLocal);
    member.apply_sync(&pull).unwrap();
    assert_eq!(
        member.read_record(&existing).unwrap(),
        Some(b"already saved".to_vec())
    );
    assert!(owner.acknowledge_authority_update(2).unwrap());
    let rotation = owner.rotate_keys().unwrap();
    let rotation = termirust_store::ReplicationAuthorityUpdate::from_canonical_bytes(
        &rotation.to_canonical_bytes().unwrap(),
    )
    .unwrap();
    let mut member = member;
    member.apply_authority_update(&rotation).unwrap();
    assert_eq!(member.status().unwrap().key_epoch, 3);
    assert!(!member.status().unwrap().authority_owner);
    assert_eq!(
        member.read_record(&existing).unwrap(),
        Some(b"already saved".to_vec())
    );
    member
        .put_record(from_phone.clone(), b"created on phone")
        .unwrap();
    let member_publish = member.review_sync().unwrap();
    member.apply_sync(&member_publish).unwrap();
    let owner_pull = owner.review_sync().unwrap();
    owner.apply_sync(&owner_pull).unwrap();
    assert_eq!(
        owner.read_record(&from_phone).unwrap(),
        Some(b"created on phone".to_vec())
    );

    owner.put_record(existing.clone(), b"owner edit").unwrap();
    member.put_record(existing.clone(), b"phone edit").unwrap();
    let owner_publish = owner.review_sync().unwrap();
    owner.apply_sync(&owner_publish).unwrap();
    let conflict = member.review_sync().unwrap();
    assert_eq!(
        conflict.disposition(),
        ReplicationSyncDisposition::ConflictReviewRequired
    );
    let candidates = member.conflict_candidates(&conflict, &existing).unwrap();
    let mut values = candidates
        .iter()
        .filter_map(|candidate| candidate.value().map(<[u8]>::to_vec))
        .collect::<Vec<_>>();
    values.sort();
    assert_eq!(values, vec![b"owner edit".to_vec(), b"phone edit".to_vec()]);
    member
        .resolve_sync(
            &conflict,
            vec![
                ReplicationConflictChoice::put(existing.clone(), b"reviewed result".to_vec())
                    .unwrap(),
            ],
        )
        .unwrap();
    let owner_resolution = owner.review_sync().unwrap();
    owner.apply_sync(&owner_resolution).unwrap();
    assert_eq!(
        owner.read_record(&existing).unwrap(),
        Some(b"reviewed result".to_vec())
    );

    assert!(matches!(
        member.rotate_keys(),
        Err(ReplicationProductError::AuthorityOwnerRequired)
    ));
}

#[test]
fn local_replica_deletion_requires_confirmation_and_rejects_stale_plan() {
    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    let backend = MemorySecrets::default();
    let service =
        ReplicationProductService::bootstrap(&root, shared.path(), backend.clone()).unwrap();
    let plan = service.deletion_plan().unwrap();
    assert_eq!(plan.secret_count, 3);
    assert!(plan.authority_owner);
    assert!(matches!(
        service.delete_local_replica(&plan, "delete"),
        Err(ReplicationProductError::DeletionConfirmationRequired)
    ));
    assert!(root.exists());
    assert_eq!(backend.count(), 3);

    let service = ReplicationProductService::open(&root, backend.clone()).unwrap();
    let stale = service.deletion_plan().unwrap();
    service
        .put_record(record_key("changed"), b"changed")
        .unwrap();
    assert!(matches!(
        service.delete_local_replica(
            &stale,
            termirust_store::ReplicationDeletionPlan::confirmation_phrase(),
        ),
        Err(ReplicationProductError::StaleDeletionPlan)
    ));
    assert!(root.exists());

    let service = ReplicationProductService::open(&root, backend.clone()).unwrap();
    let current = service.deletion_plan().unwrap();
    service
        .delete_local_replica(
            &current,
            termirust_store::ReplicationDeletionPlan::confirmation_phrase(),
        )
        .unwrap();
    assert!(!root.exists());
    assert_eq!(backend.count(), 0);
}

#[test]
fn interrupted_local_deletion_resumes_before_profile_open() {
    let parent = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let root = parent.path().join("replication");
    let backend = MemorySecrets::default();
    let service =
        ReplicationProductService::bootstrap(&root, shared.path(), backend.clone()).unwrap();
    let plan = service.deletion_plan().unwrap();
    backend.set_delete_failure(true);
    assert!(matches!(
        service.delete_local_replica(
            &plan,
            termirust_store::ReplicationDeletionPlan::confirmation_phrase(),
        ),
        Err(ReplicationProductError::Custody(_))
    ));
    assert!(root.join("deletion.transaction.json").exists());
    backend.set_delete_failure(false);
    assert!(matches!(
        ReplicationProductService::open(&root, backend.clone()),
        Err(ReplicationProductError::NotConfigured)
    ));
    assert!(!root.exists());
    assert_eq!(backend.count(), 0);
}
