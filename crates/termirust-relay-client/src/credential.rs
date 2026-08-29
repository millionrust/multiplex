use rand_core::{OsRng, RngCore};
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use termirust_relay_protocol::RelayAdmissionCredential;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::RelayCredentialRef;

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct RelayCredentialSecret([u8; 32]);

impl RelayCredentialSecret {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn admission_credential(&self) -> RelayAdmissionCredential {
        RelayAdmissionCredential::from_secret_bytes(self.0)
    }

    pub fn expose_for_store(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for RelayCredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayCredentialSecret([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelaySecretStoreError {
    Missing,
    Locked,
    PermissionDenied,
    Invalid,
    Unavailable,
}

pub trait RelaySecretStore: Send + Sync {
    fn put(
        &self,
        reference: &RelayCredentialRef,
        secret: &RelayCredentialSecret,
    ) -> Result<(), RelaySecretStoreError>;
    fn get(
        &self,
        reference: &RelayCredentialRef,
    ) -> Result<RelayCredentialSecret, RelaySecretStoreError>;
    fn delete(&self, reference: &RelayCredentialRef) -> Result<bool, RelaySecretStoreError>;
}

#[derive(Default)]
pub struct MemoryRelaySecretStore {
    values: Mutex<HashMap<String, RelayCredentialSecret>>,
}

impl RelaySecretStore for MemoryRelaySecretStore {
    fn put(
        &self,
        reference: &RelayCredentialRef,
        secret: &RelayCredentialSecret,
    ) -> Result<(), RelaySecretStoreError> {
        self.values
            .lock()
            .map_err(|_| RelaySecretStoreError::Unavailable)?
            .insert(reference.expose_for_store().to_owned(), secret.clone());
        Ok(())
    }

    fn get(
        &self,
        reference: &RelayCredentialRef,
    ) -> Result<RelayCredentialSecret, RelaySecretStoreError> {
        self.values
            .lock()
            .map_err(|_| RelaySecretStoreError::Unavailable)?
            .get(reference.expose_for_store())
            .cloned()
            .ok_or(RelaySecretStoreError::Missing)
    }

    fn delete(&self, reference: &RelayCredentialRef) -> Result<bool, RelaySecretStoreError> {
        Ok(self
            .values
            .lock()
            .map_err(|_| RelaySecretStoreError::Unavailable)?
            .remove(reference.expose_for_store())
            .is_some())
    }
}
