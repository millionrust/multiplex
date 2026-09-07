//! Replication secret custody only. Native clients own secure storage and UI;
//! this boundary owns no filesystem, transport, or application lifecycle.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use termirust_replication_security::{
    REPLICATION_STORED_SECRET_BYTES, ReplicationSecretBackend, ReplicationSecretCustodyError,
    ReplicationSecretKind, ReplicationSecretRef, ReplicationSecretStoreError,
    ReplicationSecretVault, generate_replication_device_private_key,
};
use zeroize::Zeroizing;

uniffi::setup_scaffolding!();

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Error)]
pub enum ReplicationStorageError {
    Missing,
    Locked,
    Invalid,
    Collision,
    Unavailable,
}

impl fmt::Display for ReplicationStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "replication secret is missing",
            Self::Locked => "replication secure storage is locked or access was denied",
            Self::Invalid => "replication secret data is invalid",
            Self::Collision => "replication secret already exists",
            Self::Unavailable => "replication secure storage is unavailable",
        })
    }
}

impl std::error::Error for ReplicationStorageError {}

/// Use a replication-only namespace, excluded from backup and cloud key sync.
/// Callbacks may run on worker threads and must be thread-safe. Never log account
/// identifiers or secret buffers. Native code must clear owned mutable buffers.
#[uniffi::export(foreign)]
pub trait ReplicationSecureStore: Send + Sync {
    /// Atomically create; return Collision without changing an existing value.
    /// Success means the value is durably stored, not queued for a later write.
    fn create(&self, account: String, value: Vec<u8>) -> Result<(), ReplicationStorageError>;
    /// Missing is distinct from locked, denied, corrupt, or unavailable storage.
    fn load(&self, account: String) -> Result<Vec<u8>, ReplicationStorageError>;
    /// Delete exactly this account; return false only when it was already absent.
    fn delete(&self, account: String) -> Result<bool, ReplicationStorageError>;
}

/// Adapts native custody to the same backend used by the replication product service.
pub struct NativeReplicationSecretBackend {
    store: Arc<dyn ReplicationSecureStore>,
}

impl NativeReplicationSecretBackend {
    pub fn new(store: Arc<dyn ReplicationSecureStore>) -> Self {
        Self { store }
    }
}

impl ReplicationSecretBackend for NativeReplicationSecretBackend {
    fn put(
        &self,
        reference: &ReplicationSecretRef,
        secret: &[u8],
    ) -> Result<(), ReplicationSecretStoreError> {
        if secret.len() != REPLICATION_STORED_SECRET_BYTES {
            return Err(ReplicationSecretStoreError::Invalid);
        }
        native_call(|| {
            self.store
                .create(reference.expose_opaque_account(), secret.to_vec())
        })
    }

    fn get(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<Zeroizing<Vec<u8>>, ReplicationSecretStoreError> {
        let bytes = Zeroizing::new(native_call(|| {
            self.store.load(reference.expose_opaque_account())
        })?);
        if bytes.len() != REPLICATION_STORED_SECRET_BYTES {
            return Err(ReplicationSecretStoreError::Invalid);
        }
        Ok(bytes)
    }

    fn delete(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<bool, ReplicationSecretStoreError> {
        native_call(|| self.store.delete(reference.expose_opaque_account()))
    }
}

fn native_call<T>(
    operation: impl FnOnce() -> Result<T, ReplicationStorageError>,
) -> Result<T, ReplicationSecretStoreError> {
    catch_unwind(AssertUnwindSafe(operation))
        .map_err(|_| ReplicationSecretStoreError::Unavailable)?
        .map_err(|error| match error {
            ReplicationStorageError::Missing => ReplicationSecretStoreError::Missing,
            ReplicationStorageError::Locked => ReplicationSecretStoreError::AccessDeniedOrLocked,
            ReplicationStorageError::Invalid => ReplicationSecretStoreError::Invalid,
            ReplicationStorageError::Collision => ReplicationSecretStoreError::Collision,
            ReplicationStorageError::Unavailable => ReplicationSecretStoreError::Unavailable,
        })
}

fn binding_error(error: ReplicationSecretCustodyError) -> ReplicationStorageError {
    match error {
        ReplicationSecretCustodyError::Store(error) => match error {
            ReplicationSecretStoreError::Missing => ReplicationStorageError::Missing,
            ReplicationSecretStoreError::AccessDeniedOrLocked => ReplicationStorageError::Locked,
            ReplicationSecretStoreError::Invalid => ReplicationStorageError::Invalid,
            ReplicationSecretStoreError::Collision => ReplicationStorageError::Collision,
            ReplicationSecretStoreError::Unavailable => ReplicationStorageError::Unavailable,
        },
        ReplicationSecretCustodyError::EntropyUnavailable => ReplicationStorageError::Unavailable,
        _ => ReplicationStorageError::Invalid,
    }
}

#[derive(uniffi::Record)]
pub struct ReplicationDeviceIdentity {
    pub secret_reference: Vec<u8>,
    pub public_key: Vec<u8>,
}

#[derive(uniffi::Object)]
pub struct ReplicationCustody {
    vault: ReplicationSecretVault<NativeReplicationSecretBackend>,
}

#[uniffi::export]
impl ReplicationCustody {
    #[uniffi::constructor]
    pub fn new(store: Arc<dyn ReplicationSecureStore>) -> Arc<Self> {
        Arc::new(Self {
            vault: ReplicationSecretVault::new(NativeReplicationSecretBackend::new(store)),
        })
    }

    pub fn create_device_identity(
        &self,
    ) -> Result<ReplicationDeviceIdentity, ReplicationStorageError> {
        let key = generate_replication_device_private_key()
            .map_err(|_| ReplicationStorageError::Unavailable)?;
        let reference = self.vault.store_device_key(&key).map_err(binding_error)?;
        Ok(ReplicationDeviceIdentity {
            secret_reference: reference.to_bytes().to_vec(),
            public_key: key.public_key().as_bytes().to_vec(),
        })
    }

    pub fn device_public_key(
        &self,
        secret_reference: Vec<u8>,
    ) -> Result<Vec<u8>, ReplicationStorageError> {
        let reference = device_reference(&secret_reference)?;
        Ok(self
            .vault
            .load_device_key(&reference)
            .map_err(binding_error)?
            .public_key()
            .as_bytes()
            .to_vec())
    }

    pub fn delete_device_identity(
        &self,
        secret_reference: Vec<u8>,
    ) -> Result<bool, ReplicationStorageError> {
        self.vault
            .delete(&device_reference(&secret_reference)?)
            .map_err(binding_error)
    }
}

fn device_reference(bytes: &[u8]) -> Result<ReplicationSecretRef, ReplicationStorageError> {
    let reference = ReplicationSecretRef::from_bytes(bytes).map_err(binding_error)?;
    if reference.kind() != ReplicationSecretKind::DevicePrivateKey {
        return Err(ReplicationStorageError::Invalid);
    }
    Ok(reference)
}
