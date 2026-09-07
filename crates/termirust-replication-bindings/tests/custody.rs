use std::collections::{HashMap, hash_map::Entry};
use std::sync::{Arc, Mutex};

use termirust_replication_bindings::{
    NativeReplicationSecretBackend, ReplicationCustody, ReplicationSecureStore,
    ReplicationStorageError,
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
