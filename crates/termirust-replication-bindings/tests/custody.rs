use std::collections::{HashMap, hash_map::Entry};
use std::sync::{Arc, Mutex};

use termirust_replication_bindings::{
    MobileReplicationError, MobileReplicationProduct, NativeReplicationSecretBackend,
    ReplicationCustody, ReplicationSecureStore, ReplicationStorageError,
};
use termirust_replication_security::{
    ReplicationSecretBackend, ReplicationSecretKind, ReplicationSecretRef,
    ReplicationSecretStoreError,
};

#[derive(Default)]
struct MemoryStore {
    records: Mutex<HashMap<String, Vec<u8>>>,
    error: Mutex<Option<ReplicationStorageError>>,
}

#[test]
fn mobile_explicit_recovery_rolls_back_only_the_journaled_epoch() {
    use termirust_replication_security::{
        ReplicationEpochKey, ReplicationKeyEpoch, ReplicationSecretVault,
    };
    let dir = tempfile::tempdir().unwrap();
    let exchange = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryStore::default());
    let facade = MobileReplicationProduct::new(
        dir.path().canonicalize().unwrap().to_str().unwrap().into(),
        store.clone(),
    )
    .unwrap();
    assert!(facade.recover_pending_enrollment().is_err());
    assert!(store.records.lock().unwrap().is_empty());
    let request = facade
        .prepare_enrollment(
            exchange
                .path()
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
        )
        .unwrap();
    let before = store.records.lock().unwrap().clone();
    facade.recover_pending_enrollment().unwrap();
    assert_eq!(*store.records.lock().unwrap(), before);
    let vault = ReplicationSecretVault::new(NativeReplicationSecretBackend::new(store.clone()));
    let epoch = vault
        .store_epoch_key(
            &ReplicationEpochKey::from_bytes(ReplicationKeyEpoch::new(1).unwrap(), [7; 32])
                .unwrap(),
        )
        .unwrap();
    let root = dir.path().join("enrollment");
    // Journal field order is part of the service's canonical storage format.
    let reference_hex: String = epoch
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let journal = format!(
        "{{\"format_version\":1,\"epoch_reference_hex\":{}}}",
        serde_json::to_string(&reference_hex).unwrap()
    );
    std::fs::write(root.join("enrollment.transaction.json"), &journal).unwrap();
    assert!(matches!(
        facade.pending_enrollment(),
        Err(MobileReplicationError::RecoveryRequired)
    ));
    *store.error.lock().unwrap() = Some(ReplicationStorageError::Locked);
    assert_eq!(
        facade.recover_pending_enrollment(),
        Err(MobileReplicationError::Locked)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("enrollment.transaction.json")).unwrap(),
        journal
    );
    assert_eq!(store.records.lock().unwrap().len(), before.len() + 1);
    *store.error.lock().unwrap() = None;
    facade.recover_pending_enrollment().unwrap();
    facade.recover_pending_enrollment().unwrap();
    assert_eq!(*store.records.lock().unwrap(), before);
    assert_eq!(
        facade
            .pending_enrollment()
            .unwrap()
            .unwrap()
            .canonical_request,
        request.canonical_request
    );
    assert!(!root.join("enrollment.transaction.json").exists());
}

#[test]
fn mobile_enrollment_review_and_accept_preserve_exact_request_and_custody() {
    use termirust_store::replication::{ReplicationEnrollmentRequest, ReplicationProductService};
    let member = tempfile::tempdir().unwrap();
    let exchange = tempfile::tempdir().unwrap();
    let owner = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryStore::default());
    let path = member
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let facade = MobileReplicationProduct::new(path.clone(), store.clone()).unwrap();
    let request = facade
        .prepare_enrollment(
            exchange
                .path()
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
        )
        .unwrap()
        .canonical_request;
    let mut desktop = ReplicationProductService::bootstrap(
        owner.path().join("owner"),
        exchange.path(),
        NativeReplicationSecretBackend::new(Arc::new(MemoryStore::default())),
    )
    .unwrap();
    let bundle = desktop
        .enroll_request(&ReplicationEnrollmentRequest::from_canonical_bytes(&request).unwrap())
        .unwrap()
        .to_canonical_bytes()
        .unwrap();
    let before = store.records.lock().unwrap().clone();
    let other = tempfile::tempdir().unwrap();
    let other_store = Arc::new(MemoryStore::default());
    let other_facade = MobileReplicationProduct::new(
        other
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned(),
        other_store.clone(),
    )
    .unwrap();
    let other_request = other_facade
        .prepare_enrollment(
            exchange
                .path()
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
        )
        .unwrap()
        .canonical_request;
    assert!(matches!(
        other_facade.review_enrollment(other_request.clone(), bundle.clone()),
        Err(MobileReplicationError::Invalid)
    ));
    assert!(
        other_facade
            .cancel_pending_enrollment(other_request.clone())
            .unwrap()
    );
    assert!(
        other_facade
            .review_enrollment(other_request, bundle.clone())
            .is_err()
    );
    assert!(other_store.records.lock().unwrap().is_empty());
    let review = facade
        .review_enrollment(request.clone(), bundle.clone())
        .unwrap();
    assert_eq!(*store.records.lock().unwrap(), before);
    assert!(matches!(
        facade.review_enrollment(vec![], bundle.clone()),
        Err(MobileReplicationError::StaleRequest)
    ));
    assert!(
        facade
            .review_enrollment(request.clone(), vec![0; 192 * 1024 + 1])
            .is_err()
    );
    assert_eq!(
        facade.accept_enrollment(
            request.clone(),
            bundle.clone(),
            "wrong".into(),
            review.verification_code.clone()
        ),
        Err(MobileReplicationError::Invalid)
    );
    assert_eq!(
        facade.accept_enrollment(
            request.clone(),
            bundle.clone(),
            review.workspace_id.clone(),
            "wrong".into()
        ),
        Err(MobileReplicationError::Invalid)
    );
    assert_eq!(*store.records.lock().unwrap(), before);
    *store.error.lock().unwrap() = Some(ReplicationStorageError::Missing);
    assert_eq!(
        facade.accept_enrollment(
            request.clone(),
            bundle.clone(),
            review.workspace_id.clone(),
            review.verification_code.clone()
        ),
        Err(MobileReplicationError::MissingSecret)
    );
    *store.error.lock().unwrap() = None;
    assert_eq!(*store.records.lock().unwrap(), before);
    drop(facade);
    let reopened = MobileReplicationProduct::new(path, store.clone()).unwrap();
    reopened
        .accept_enrollment(
            request.clone(),
            bundle.clone(),
            review.workspace_id.clone(),
            review.verification_code.clone(),
        )
        .unwrap();
    assert_eq!(store.records.lock().unwrap().len(), before.len() + 1);
    assert_eq!(
        reopened.accept_enrollment(
            request,
            bundle,
            review.workspace_id,
            review.verification_code
        ),
        Err(MobileReplicationError::AlreadyConfigured)
    );
    let service = ReplicationProductService::open(
        member.path().join("enrollment"),
        NativeReplicationSecretBackend::new(store),
    )
    .unwrap();
    assert!(!service.status().unwrap().authority_owner);
    let key = termirust_domain::ReplicationRecordKey::new(
        termirust_domain::ReplicationCollectionId::new("desktop-profiles").unwrap(),
        termirust_domain::ReplicationRecordId::new("fixture-host").unwrap(),
    );
    desktop
        .put_record(key.clone(), b"inert fixture bytes")
        .unwrap();
    let published = desktop.apply_sync(&desktop.review_sync().unwrap()).unwrap();
    let document = serde_json::to_vec(&published.transport.document).unwrap();
    let review = reopened
        .review_record_transfer(
            document.clone(),
            "desktop-profiles".into(),
            "fixture-host".into(),
        )
        .unwrap();
    assert_eq!(review.plaintext, b"inert fixture bytes");
    assert!(matches!(
        reopened.review_host_transfer(document.clone(), "fixture-host".into()),
        Err(MobileReplicationError::Invalid)
    ));
    assert_eq!(
        reopened.apply_host_transfer(
            document.clone(),
            "fixture-host".into(),
            review.review_token.clone()
        ),
        Err(MobileReplicationError::Invalid)
    );
    assert!(review.changes_local);
    assert!(service.records().unwrap().is_empty());
    assert_eq!(
        reopened.apply_record_transfer(
            document.clone(),
            "desktop-profiles".into(),
            "fixture-host".into(),
            vec![0; 31]
        ),
        Err(MobileReplicationError::Invalid)
    );
    assert_eq!(
        reopened.apply_record_transfer(
            document.clone(),
            "desktop-profiles".into(),
            "fixture-host".into(),
            vec![0; 32]
        ),
        Err(MobileReplicationError::StaleRequest)
    );
    reopened
        .apply_record_transfer(
            document.clone(),
            "desktop-profiles".into(),
            "fixture-host".into(),
            review.review_token.clone(),
        )
        .unwrap();
    assert_eq!(
        service.read_record(&key).unwrap(),
        Some(b"inert fixture bytes".to_vec())
    );
    assert_eq!(
        reopened.apply_record_transfer(
            document.clone(),
            "desktop-profiles".into(),
            "fixture-host".into(),
            review.review_token
        ),
        Err(MobileReplicationError::StaleRequest)
    );
    let duplicate = reopened
        .review_record_transfer(
            document.clone(),
            "desktop-profiles".into(),
            "fixture-host".into(),
        )
        .unwrap();
    assert!(!duplicate.changes_local);
    reopened
        .apply_record_transfer(
            document,
            "desktop-profiles".into(),
            "fixture-host".into(),
            duplicate.review_token,
        )
        .unwrap();

    // Golden identity uses the existing desktop domain-separated record-ID format.
    let host_id = "79ac23a7e0e3d16216f125ae099fad0f3edc0dca33540c12c25c9d44766aaf16";
    let host_key = termirust_domain::ReplicationRecordKey::new(
        termirust_domain::ReplicationCollectionId::new("desktop-profiles").unwrap(),
        termirust_domain::ReplicationRecordId::new(host_id).unwrap(),
    );
    let host_bytes = br#"{"schema_version":1,"stable_id":"profile-1","value":{"id":"profile-1","label":"Reviewed host","host":"example.test","port":2222,"username":"demo","startup_command":"never execute this","password_credential_id":"never reuse this","persistent_session":true}}"#;
    desktop.put_record(host_key.clone(), host_bytes).unwrap();
    let published = desktop.apply_sync(&desktop.review_sync().unwrap()).unwrap();
    let document = serde_json::to_vec(&published.transport.document).unwrap();
    let review = reopened
        .review_host_transfer(document.clone(), host_id.into())
        .unwrap();
    assert_eq!(review.stable_id, "profile-1");
    assert_eq!(review.label, "Reviewed host");
    assert_eq!(review.host, "example.test");
    assert_eq!(review.port, 2222);
    assert_eq!(review.username, "demo");
    assert!(review.changes_local);
    assert_eq!(service.read_record(&host_key).unwrap(), None);
    assert_eq!(
        reopened.apply_host_transfer(document.clone(), host_id.into(), vec![0; 32]),
        Err(MobileReplicationError::StaleRequest)
    );
    reopened
        .apply_host_transfer(document.clone(), host_id.into(), review.review_token)
        .unwrap();
    assert_eq!(
        service.read_record(&host_key).unwrap(),
        Some(host_bytes.to_vec())
    );
    let duplicate = reopened
        .review_host_transfer(document.clone(), host_id.into())
        .unwrap();
    assert!(!duplicate.changes_local);
    reopened
        .apply_host_transfer(document, host_id.into(), duplicate.review_token)
        .unwrap();
}

#[test]
fn host_discovery_is_authenticated_inert_and_reopens_after_import() {
    use termirust_domain::{ReplicationCollectionId, ReplicationRecordId, ReplicationRecordKey};
    use termirust_store::replication::{ReplicationEnrollmentRequest, ReplicationProductService};
    let member = tempfile::tempdir().unwrap();
    let owner = tempfile::tempdir().unwrap();
    let exchange = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryStore::default());
    let path = member
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let facade = MobileReplicationProduct::new(path.clone(), store.clone()).unwrap();
    let request = facade
        .prepare_enrollment(
            exchange
                .path()
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
        )
        .unwrap();
    let mut desktop = ReplicationProductService::bootstrap(
        owner.path().join("owner"),
        exchange.path(),
        NativeReplicationSecretBackend::new(Arc::new(MemoryStore::default())),
    )
    .unwrap();
    let bundle = desktop
        .enroll_request(
            &ReplicationEnrollmentRequest::from_canonical_bytes(&request.canonical_request)
                .unwrap(),
        )
        .unwrap();
    let bytes = bundle.to_canonical_bytes().unwrap();
    let review = facade
        .review_enrollment(request.canonical_request.clone(), bytes.clone())
        .unwrap();
    facade
        .accept_enrollment(
            request.canonical_request,
            bytes,
            review.workspace_id,
            review.verification_code,
        )
        .unwrap();
    assert!(facade.imported_hosts().unwrap().is_empty());
    let id = "79ac23a7e0e3d16216f125ae099fad0f3edc0dca33540c12c25c9d44766aaf16";
    let key = ReplicationRecordKey::new(
        ReplicationCollectionId::new("desktop-profiles").unwrap(),
        ReplicationRecordId::new(id).unwrap(),
    );
    let host = br#"{"schema_version":1,"stable_id":"profile-1","value":{"id":"profile-1","label":"Reviewed host","host":"example.test","port":2222,"username":"demo","startup_command":"never execute","password_credential_id":"never reuse"}}"#;
    desktop.put_record(key.clone(), host).unwrap();
    // Unrelated collections are not interpreted as host schemas or imported.
    desktop
        .put_record(
            ReplicationRecordKey::new(
                ReplicationCollectionId::new("desktop-settings").unwrap(),
                ReplicationRecordId::new("other").unwrap(),
            ),
            b"not a host",
        )
        .unwrap();
    let published = desktop.apply_sync(&desktop.review_sync().unwrap()).unwrap();
    let document = serde_json::to_vec(&published.transport.document).unwrap();
    let secrets = store.records.lock().unwrap().clone();
    let candidates = facade.preview_host_transfers(document.clone()).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].record_id, id);
    assert_eq!(candidates[0].stable_id, "profile-1");
    assert_eq!(candidates[0].label, "Reviewed host");
    assert_eq!(candidates[0].host, "example.test");
    assert_eq!(candidates[0].port, 2222);
    assert_eq!(candidates[0].username, "demo");
    assert!(facade.imported_hosts().unwrap().is_empty());
    assert_eq!(*store.records.lock().unwrap(), secrets);
    let mut tampered = published.transport.document.clone();
    let review = facade
        .review_host_transfer(document.clone(), id.into())
        .unwrap();
    assert_eq!(
        facade.apply_host_transfer(vec![0], id.into(), review.review_token),
        Err(MobileReplicationError::Invalid)
    );
    assert!(facade.imported_hosts().unwrap().is_empty());
    tampered
        .entries
        .iter_mut()
        .find(|entry| entry.key == key)
        .unwrap()
        .candidates[0]
        .author = termirust_domain::ReplicationReplicaId::new("wrong-author").unwrap();
    assert!(
        facade
            .preview_host_transfers(serde_json::to_vec(&tampered).unwrap())
            .is_err()
    );
    let mut noncanonical = document.clone();
    noncanonical.push(b'\n');
    assert!(facade.preview_host_transfers(noncanonical).is_err());
    let mut wrong_workspace = published.transport.document.clone();
    wrong_workspace.workspace_id =
        termirust_domain::ReplicationWorkspaceId::new("other-workspace").unwrap();
    assert!(
        facade
            .preview_host_transfers(serde_json::to_vec(&wrong_workspace).unwrap())
            .is_err()
    );
    assert!(facade.preview_host_transfers(Vec::new()).is_err());
    *store.error.lock().unwrap() = Some(ReplicationStorageError::Missing);
    assert!(matches!(
        facade.preview_host_transfers(document.clone()),
        Err(MobileReplicationError::MissingSecret)
    ));
    assert!(matches!(
        facade.imported_hosts(),
        Err(MobileReplicationError::MissingSecret)
    ));
    *store.error.lock().unwrap() = None;
    assert_eq!(*store.records.lock().unwrap(), secrets);
    let review = facade
        .review_host_transfer(document.clone(), id.into())
        .unwrap();
    facade
        .apply_host_transfer(document, id.into(), review.review_token)
        .unwrap();
    drop(facade);
    let reopened = MobileReplicationProduct::new(path, store.clone()).unwrap();
    assert_eq!(reopened.imported_hosts().unwrap()[0].record_id, id);
    // Malformed host data fails the whole preview rather than hiding an unsupported host.
    desktop
        .put_record(key.clone(), b"unsupported host schema")
        .unwrap();
    let invalid = desktop.apply_sync(&desktop.review_sync().unwrap()).unwrap();
    assert!(matches!(
        reopened.preview_host_transfers(serde_json::to_vec(&invalid.transport.document).unwrap()),
        Err(MobileReplicationError::Invalid)
    ));
    assert_eq!(reopened.imported_hosts().unwrap().len(), 1);
    desktop.delete_record(key).unwrap();
    let deleted = desktop.apply_sync(&desktop.review_sync().unwrap()).unwrap();
    assert!(
        reopened
            .preview_host_transfers(serde_json::to_vec(&deleted.transport.document).unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(reopened.imported_hosts().unwrap().len(), 1);
    assert_eq!(*store.records.lock().unwrap(), secrets);
    let local = ReplicationProductService::open(
        member.path().join("enrollment"),
        NativeReplicationSecretBackend::new(store.clone()),
    )
    .unwrap();
    let local_key = ReplicationRecordKey::new(
        ReplicationCollectionId::new("desktop-profiles").unwrap(),
        ReplicationRecordId::new(id).unwrap(),
    );
    local
        .put_record(local_key.clone(), b"unsupported local host")
        .unwrap();
    assert!(matches!(
        reopened.imported_hosts(),
        Err(MobileReplicationError::Invalid)
    ));
    assert_eq!(
        local.read_record(&local_key).unwrap(),
        Some(b"unsupported local host".to_vec())
    );
    local.delete_record(local_key).unwrap();
    assert!(reopened.imported_hosts().unwrap().is_empty());
}

#[test]
fn mobile_enrollment_reopens_and_cancels_only_its_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let exchange = tempfile::tempdir().unwrap();
    let exchange_path = exchange
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let store = Arc::new(MemoryStore::default());
    let custody = ReplicationCustody::new(store.clone());
    let sentinel = custody.create_device_identity().unwrap();
    let first = MobileReplicationProduct::new(path.clone(), store.clone()).unwrap();
    assert!(first.pending_enrollment().unwrap().is_none());
    let request = first.prepare_enrollment(exchange_path.clone()).unwrap();
    assert!(request.canonical_request.len() < 4096);
    termirust_store::replication::ReplicationEnrollmentRequest::from_canonical_bytes(
        &request.canonical_request,
    )
    .unwrap();
    assert!(matches!(
        first.prepare_enrollment(exchange_path),
        Err(MobileReplicationError::AlreadyConfigured)
    ));
    drop(first);
    let reopened = MobileReplicationProduct::new(path, store.clone()).unwrap();
    assert_eq!(
        reopened
            .pending_enrollment()
            .unwrap()
            .unwrap()
            .canonical_request,
        request.canonical_request
    );
    assert!(
        reopened
            .cancel_pending_enrollment(request.canonical_request.clone())
            .unwrap()
    );
    assert!(
        !reopened
            .cancel_pending_enrollment(request.canonical_request.clone())
            .unwrap()
    );
    assert!(reopened.pending_enrollment().unwrap().is_none());
    assert_eq!(
        custody
            .device_public_key(sentinel.secret_reference)
            .unwrap(),
        sentinel.public_key
    );
    assert_eq!(store.records.lock().unwrap().len(), 1);
    assert!(dir.path().join("enrollment.lock").exists());
    let next = reopened
        .prepare_enrollment(
            dir.path()
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
        )
        .unwrap();
    assert_eq!(
        reopened.cancel_pending_enrollment(request.canonical_request),
        Err(MobileReplicationError::StaleRequest)
    );
    assert_eq!(
        reopened
            .pending_enrollment()
            .unwrap()
            .unwrap()
            .canonical_request,
        next.canonical_request
    );
    assert!(
        reopened
            .cancel_pending_enrollment(next.canonical_request)
            .unwrap()
    );
}

#[test]
fn mobile_failed_cancellation_is_visible_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let store = Arc::new(MemoryStore::default());
    let facade = MobileReplicationProduct::new(path.clone(), store.clone()).unwrap();
    let request = facade.prepare_enrollment(path).unwrap();
    *store.error.lock().unwrap() = Some(ReplicationStorageError::Locked);
    assert_eq!(
        facade.cancel_pending_enrollment(request.canonical_request.clone()),
        Err(MobileReplicationError::Locked)
    );
    assert!(matches!(
        facade.pending_enrollment(),
        Err(MobileReplicationError::RecoveryRequired)
    ));
    assert_eq!(store.records.lock().unwrap().len(), 1);
    *store.error.lock().unwrap() = None;
    assert!(
        facade
            .cancel_pending_enrollment(request.canonical_request)
            .unwrap()
    );
    assert!(store.records.lock().unwrap().is_empty());
}

#[test]
fn mobile_busy_lock_and_invalid_provider_paths_do_not_create_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let store = Arc::new(MemoryStore::default());
    for invalid in [
        "content://provider/document/1",
        "file:///tmp/example",
        "relative",
    ] {
        assert!(matches!(
            MobileReplicationProduct::new(invalid.into(), store.clone()),
            Err(MobileReplicationError::Invalid)
        ));
    }
    let facade = MobileReplicationProduct::new(path.clone(), store.clone()).unwrap();
    assert!(facade.pending_enrollment().unwrap().is_none());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join("enrollment.lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    assert!(matches!(
        facade.prepare_enrollment(path),
        Err(MobileReplicationError::Busy)
    ));
    assert!(matches!(
        facade.pending_enrollment(),
        Err(MobileReplicationError::Busy)
    ));
    assert_eq!(
        facade.cancel_pending_enrollment(vec![]),
        Err(MobileReplicationError::Busy)
    );
    assert!(store.records.lock().unwrap().is_empty());
}

#[test]
fn mobile_corrupt_pending_is_not_absent_and_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let store = Arc::new(MemoryStore::default());
    let facade = MobileReplicationProduct::new(path.clone(), store.clone()).unwrap();
    let request = facade.prepare_enrollment(path).unwrap();
    let pending = dir.path().join("enrollment/pending-enrollment.json");
    std::fs::write(&pending, b"broken").unwrap();
    assert!(matches!(
        facade.pending_enrollment(),
        Err(MobileReplicationError::Invalid)
    ));
    assert_eq!(
        facade.cancel_pending_enrollment(request.canonical_request),
        Err(MobileReplicationError::Invalid)
    );
    assert_eq!(std::fs::read(pending).unwrap(), b"broken");
    assert_eq!(store.records.lock().unwrap().len(), 1);
}

#[test]
fn mobile_concurrent_prepare_allocates_one_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let store = Arc::new(MemoryStore::default());
    let gate = Arc::new(std::sync::Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let facade = MobileReplicationProduct::new(path.clone(), store.clone()).unwrap();
        let gate = gate.clone();
        let path = path.clone();
        threads.push(std::thread::spawn(move || {
            gate.wait();
            facade.prepare_enrollment(path).map(|r| r.canonical_request)
        }));
    }
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(
                r,
                Err(MobileReplicationError::Busy | MobileReplicationError::AlreadyConfigured)
            ))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(store.records.lock().unwrap().len(), 1);
    let facade = MobileReplicationProduct::new(path, store).unwrap();
    assert_eq!(
        facade
            .pending_enrollment()
            .unwrap()
            .unwrap()
            .canonical_request,
        winner
    );
    assert!(facade.cancel_pending_enrollment(winner).unwrap());
}

impl MemoryStore {
    fn check(&self) -> Result<(), ReplicationStorageError> {
        match *self.error.lock().unwrap() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl ReplicationSecureStore for MemoryStore {
    fn create(&self, account: String, value: Vec<u8>) -> Result<(), ReplicationStorageError> {
        self.check()?;
        match self.records.lock().unwrap().entry(account) {
            Entry::Occupied(_) => Err(ReplicationStorageError::Collision),
            Entry::Vacant(entry) => {
                entry.insert(value);
                Ok(())
            }
        }
    }

    fn load(&self, account: String) -> Result<Vec<u8>, ReplicationStorageError> {
        self.check()?;
        self.records
            .lock()
            .unwrap()
            .get(&account)
            .cloned()
            .ok_or(ReplicationStorageError::Missing)
    }

    fn delete(&self, account: String) -> Result<bool, ReplicationStorageError> {
        self.check()?;
        Ok(self.records.lock().unwrap().remove(&account).is_some())
    }
}

#[test]
fn identity_survives_engine_recreation_and_deletes_exactly() {
    let store = Arc::new(MemoryStore::default());
    let engine = ReplicationCustody::new(store.clone());
    let first = engine.create_device_identity().unwrap();
    let second = engine.create_device_identity().unwrap();
    assert_ne!(first.secret_reference, second.secret_reference);
    assert_eq!(first.public_key.len(), 32);
    drop(engine);
    let engine = ReplicationCustody::new(store.clone());
    assert_eq!(
        engine
            .device_public_key(first.secret_reference.clone())
            .unwrap(),
        first.public_key
    );
    assert!(
        engine
            .delete_device_identity(first.secret_reference.clone())
            .unwrap()
    );
    assert!(
        !engine
            .delete_device_identity(first.secret_reference.clone())
            .unwrap()
    );
    assert_eq!(
        engine
            .device_public_key(first.secret_reference)
            .unwrap_err(),
        ReplicationStorageError::Missing
    );
    assert_eq!(
        engine.device_public_key(second.secret_reference).unwrap(),
        second.public_key
    );
    assert_eq!(store.records.lock().unwrap().len(), 1);
}

#[test]
fn duplicate_create_preserves_the_existing_secret() {
    let store = Arc::new(MemoryStore::default());
    let engine = ReplicationCustody::new(store.clone());
    let identity = engine.create_device_identity().unwrap();
    let reference = ReplicationSecretRef::from_bytes(&identity.secret_reference).unwrap();
    let backend = NativeReplicationSecretBackend::new(store);
    assert_eq!(
        backend.put(&reference, &[9; 47]).unwrap_err(),
        ReplicationSecretStoreError::Collision
    );
    assert_eq!(
        engine.device_public_key(identity.secret_reference).unwrap(),
        identity.public_key
    );
}

#[test]
fn native_storage_errors_remain_distinct() {
    let store = Arc::new(MemoryStore::default());
    let engine = ReplicationCustody::new(store.clone());
    let identity = engine.create_device_identity().unwrap();
    for error in [
        ReplicationStorageError::Missing,
        ReplicationStorageError::Locked,
        ReplicationStorageError::Invalid,
        ReplicationStorageError::Collision,
        ReplicationStorageError::Unavailable,
    ] {
        *store.error.lock().unwrap() = Some(error);
        assert_eq!(
            engine
                .device_public_key(identity.secret_reference.clone())
                .unwrap_err(),
            error
        );
        assert_eq!(
            engine
                .delete_device_identity(identity.secret_reference.clone())
                .unwrap_err(),
            error
        );
        assert!(matches!(engine.create_device_identity(), Err(actual) if actual == error));
        assert_eq!(store.records.lock().unwrap().len(), 1);
    }
}

#[test]
fn malformed_and_wrong_role_references_never_reach_storage() {
    let store = Arc::new(MemoryStore::default());
    *store.error.lock().unwrap() = Some(ReplicationStorageError::Unavailable);
    let engine = ReplicationCustody::new(store);
    let authority = ReplicationSecretRef::from_identifier(
        ReplicationSecretKind::AuthorityPrivateKey,
        None,
        [1; 32],
    )
    .unwrap();
    for reference in [
        vec![],
        vec![0; 47],
        vec![0; 48],
        authority.to_bytes().to_vec(),
    ] {
        assert_eq!(
            engine.device_public_key(reference.clone()).unwrap_err(),
            ReplicationStorageError::Invalid
        );
        assert_eq!(
            engine.delete_device_identity(reference).unwrap_err(),
            ReplicationStorageError::Invalid
        );
    }
}

#[test]
fn corrupt_native_data_is_rejected_without_replacement() {
    let store = Arc::new(MemoryStore::default());
    let engine = ReplicationCustody::new(store.clone());
    let identity = engine.create_device_identity().unwrap();
    let reference = ReplicationSecretRef::from_bytes(&identity.secret_reference).unwrap();
    for value in [vec![], vec![1; 46], vec![0; 47], vec![1; 48]] {
        store
            .records
            .lock()
            .unwrap()
            .insert(reference.expose_opaque_account(), value.clone());
        assert_eq!(
            engine
                .device_public_key(identity.secret_reference.clone())
                .unwrap_err(),
            ReplicationStorageError::Invalid
        );
        assert_eq!(
            store.load(reference.expose_opaque_account()).unwrap(),
            value
        );
    }
}
