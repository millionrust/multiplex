use std::fmt;

use base64::Engine as _;
use keyring::{Entry, Error as KeyringError};
use rand::RngCore as _;
use termirust_controller_security::{StaticPrivateKey, host_public_key_from_private};
use termirust_domain::{
    ControllerDeviceAuthority, HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef,
    HostIdentityState, HostPublicKey,
};
use termirust_store::{
    ControllerDeviceRepository, ControllerDeviceSnapshot, ControllerDeviceStoreError,
};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

const IDENTITY_SERVICE: &str = "com.termirust.controller.identity";

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct HostIdentitySecret([u8; 32]);

impl HostIdentitySecret {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    fn static_key(&self) -> StaticPrivateKey {
        StaticPrivateKey::from_bytes(self.0)
    }

    fn encode(&self) -> String {
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(self.0)
    }

    fn decode(value: &str) -> Result<Self, SecretStoreError> {
        let mut decoded = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(value)
            .map_err(|_| SecretStoreError::Invalid)?;
        if decoded.len() != 32 {
            decoded.zeroize();
            return Err(SecretStoreError::Invalid);
        }
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&decoded);
        decoded.zeroize();
        Ok(Self(bytes))
    }
}

impl fmt::Debug for HostIdentitySecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HostIdentitySecret([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretStoreError {
    Missing,
    Locked,
    PermissionDenied,
    Invalid,
    Unavailable,
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "Host identity secret is missing",
            Self::Locked => "the system credential store is locked",
            Self::PermissionDenied => "system credential access was denied",
            Self::Invalid => "Host identity secret is invalid",
            Self::Unavailable => "the system credential store is unavailable",
        })
    }
}

impl std::error::Error for SecretStoreError {}

pub trait SecretStore: Send + Sync {
    fn put(
        &self,
        reference: &HostIdentitySecretRef,
        secret: &HostIdentitySecret,
    ) -> Result<(), SecretStoreError>;
    fn get(
        &self,
        reference: &HostIdentitySecretRef,
    ) -> Result<HostIdentitySecret, SecretStoreError>;
    fn delete(&self, reference: &HostIdentitySecretRef) -> Result<bool, SecretStoreError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OsSecretStore;

impl SecretStore for OsSecretStore {
    fn put(
        &self,
        reference: &HostIdentitySecretRef,
        secret: &HostIdentitySecret,
    ) -> Result<(), SecretStoreError> {
        entry(reference)?
            .set_password(&secret.encode())
            .map_err(map_keyring_error)
    }

    fn get(
        &self,
        reference: &HostIdentitySecretRef,
    ) -> Result<HostIdentitySecret, SecretStoreError> {
        let encoded = entry(reference)?
            .get_password()
            .map_err(map_keyring_error)?;
        HostIdentitySecret::decode(&encoded)
    }

    fn delete(&self, reference: &HostIdentitySecretRef) -> Result<bool, SecretStoreError> {
        match entry(reference)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(error) => Err(map_keyring_error(error)),
        }
    }
}

fn entry(reference: &HostIdentitySecretRef) -> Result<Entry, SecretStoreError> {
    Entry::new(IDENTITY_SERVICE, reference.expose_reference())
        .map_err(|_| SecretStoreError::Unavailable)
}

fn map_keyring_error(error: KeyringError) -> SecretStoreError {
    match error {
        KeyringError::NoEntry => SecretStoreError::Missing,
        KeyringError::NoStorageAccess(_) => SecretStoreError::PermissionDenied,
        KeyringError::BadEncoding(_) => SecretStoreError::Invalid,
        _ => SecretStoreError::Unavailable,
    }
}

#[derive(Clone, Debug)]
pub struct LoadedHostIdentity {
    pub state: HostIdentityState,
    pub public: Option<HostIdentityPublic>,
    secret: Option<HostIdentitySecret>,
}

impl LoadedHostIdentity {
    pub fn static_private_key(&self) -> Option<StaticPrivateKey> {
        self.secret.as_ref().map(HostIdentitySecret::static_key)
    }

    pub const fn can_authorize(&self) -> bool {
        matches!(self.state, HostIdentityState::Ready)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OldSecretDeletion {
    Deleted,
    AlreadyMissing,
    Failed(SecretStoreError),
}

#[derive(Clone, Debug)]
pub struct IdentityResetOutcome {
    pub identity: LoadedHostIdentity,
    pub deletion: OldSecretDeletion,
}

#[derive(Debug)]
pub enum HostIdentityError {
    Store(ControllerDeviceStoreError),
    Secret(SecretStoreError),
    InvalidMetadata,
    Entropy,
}

impl fmt::Display for HostIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Secret(error) => error.fmt(formatter),
            Self::InvalidMetadata => formatter.write_str("Host identity metadata is inconsistent"),
            Self::Entropy => formatter.write_str("secure random generation failed"),
        }
    }
}

impl std::error::Error for HostIdentityError {}

impl From<ControllerDeviceStoreError> for HostIdentityError {
    fn from(error: ControllerDeviceStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<SecretStoreError> for HostIdentityError {
    fn from(error: SecretStoreError) -> Self {
        Self::Secret(error)
    }
}

pub trait IdentityEntropy: Send + Sync {
    fn secret(&self) -> Result<HostIdentitySecret, HostIdentityError>;
    fn reference(&self) -> Result<HostIdentitySecretRef, HostIdentityError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OsIdentityEntropy;

impl IdentityEntropy for OsIdentityEntropy {
    fn secret(&self) -> Result<HostIdentitySecret, HostIdentityError> {
        let mut bytes = [0_u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| HostIdentityError::Entropy)?;
        Ok(HostIdentitySecret::from_bytes(bytes))
    }

    fn reference(&self) -> Result<HostIdentitySecretRef, HostIdentityError> {
        HostIdentitySecretRef::new(format!("host-identity:{}", Uuid::new_v4()))
            .map_err(|_| HostIdentityError::Entropy)
    }
}

pub struct HostIdentityService<S = OsSecretStore, E = OsIdentityEntropy> {
    repository: ControllerDeviceRepository,
    secrets: S,
    entropy: E,
}

impl<S: SecretStore, E: IdentityEntropy> HostIdentityService<S, E> {
    pub fn new(repository: ControllerDeviceRepository, secrets: S, entropy: E) -> Self {
        Self {
            repository,
            secrets,
            entropy,
        }
    }

    pub fn load_or_create(&self) -> Result<LoadedHostIdentity, HostIdentityError> {
        let snapshot = self.repository.load()?;
        if snapshot.authority.identity.is_none() {
            if !snapshot.authority.devices.is_empty()
                || !snapshot.authority.offers.is_empty()
                || snapshot.authority.secret_ref.is_some()
            {
                return Ok(unavailable(HostIdentityState::Lost, None));
            }
            return self.create(snapshot);
        }
        self.load_existing(&snapshot.authority)
    }

    pub fn reset(&self) -> Result<IdentityResetOutcome, HostIdentityError> {
        let current = self.repository.load()?;
        let old_reference = current.authority.secret_ref.clone();
        let mut disabled = current.authority;
        let next_generation = disabled
            .begin_identity_reset()
            .map_err(ControllerDeviceStoreError::Domain)?;
        let disabled = self.repository.save(current.revision, disabled)?;

        let deletion = match old_reference.as_ref() {
            Some(reference) => match self.secrets.delete(reference) {
                Ok(true) => OldSecretDeletion::Deleted,
                Ok(false) => OldSecretDeletion::AlreadyMissing,
                Err(error) => OldSecretDeletion::Failed(error),
            },
            None => OldSecretDeletion::AlreadyMissing,
        };

        let secret = self.entropy.secret()?;
        let reference = self.entropy.reference()?;
        self.secrets.put(&reference, &secret)?;
        let public = public_identity(next_generation, &secret);
        let mut ready = disabled.authority;
        if ready
            .finish_identity_reset(public.clone(), reference.clone())
            .is_err()
        {
            let _ = self.secrets.delete(&reference);
            return Err(HostIdentityError::InvalidMetadata);
        }
        if let Err(error) = self.repository.save(disabled.revision, ready) {
            let _ = self.secrets.delete(&reference);
            return Err(error.into());
        }
        Ok(IdentityResetOutcome {
            identity: LoadedHostIdentity {
                state: HostIdentityState::Ready,
                public: Some(public),
                secret: Some(secret),
            },
            deletion,
        })
    }

    fn create(
        &self,
        snapshot: ControllerDeviceSnapshot,
    ) -> Result<LoadedHostIdentity, HostIdentityError> {
        let secret = self.entropy.secret()?;
        let reference = self.entropy.reference()?;
        self.secrets.put(&reference, &secret)?;
        let public = public_identity(HostIdentityGeneration::INITIAL, &secret);
        let authority = ControllerDeviceAuthority {
            identity: Some(public.clone()),
            secret_ref: Some(reference.clone()),
            state: HostIdentityState::Ready,
            ..ControllerDeviceAuthority::default()
        };
        if let Err(error) = self.repository.save(snapshot.revision, authority) {
            let _ = self.secrets.delete(&reference);
            return Err(error.into());
        }
        Ok(LoadedHostIdentity {
            state: HostIdentityState::Ready,
            public: Some(public),
            secret: Some(secret),
        })
    }

    fn load_existing(
        &self,
        authority: &ControllerDeviceAuthority,
    ) -> Result<LoadedHostIdentity, HostIdentityError> {
        let public = authority
            .identity
            .clone()
            .ok_or(HostIdentityError::InvalidMetadata)?;
        if authority.state == HostIdentityState::ResetRequired {
            return Ok(unavailable(HostIdentityState::ResetRequired, Some(public)));
        }
        let reference = authority
            .secret_ref
            .as_ref()
            .ok_or(HostIdentityError::InvalidMetadata)?;
        let secret = match self.secrets.get(reference) {
            Ok(secret) => secret,
            Err(SecretStoreError::Locked) => {
                return Ok(unavailable(HostIdentityState::Locked, Some(public)));
            }
            Err(SecretStoreError::PermissionDenied) => {
                return Ok(unavailable(
                    HostIdentityState::PermissionDenied,
                    Some(public),
                ));
            }
            Err(SecretStoreError::Missing | SecretStoreError::Invalid) => {
                return Ok(unavailable(HostIdentityState::Lost, Some(public)));
            }
            Err(error) => return Err(error.into()),
        };
        if public_identity(public.generation, &secret) != public {
            return Ok(unavailable(HostIdentityState::Lost, Some(public)));
        }
        Ok(LoadedHostIdentity {
            state: HostIdentityState::Ready,
            public: Some(public),
            secret: Some(secret),
        })
    }
}

fn public_identity(
    generation: HostIdentityGeneration,
    secret: &HostIdentitySecret,
) -> HostIdentityPublic {
    let public = host_public_key_from_private(&secret.static_key());
    HostIdentityPublic::new(generation, HostPublicKey(public.0))
}

fn unavailable(state: HostIdentityState, public: Option<HostIdentityPublic>) -> LoadedHostIdentity {
    LoadedHostIdentity {
        state,
        public,
        secret: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct MemorySecretStore {
        values: Arc<Mutex<HashMap<String, HostIdentitySecret>>>,
        failure: Arc<Mutex<Option<SecretStoreError>>>,
    }

    impl MemorySecretStore {
        fn fail_with(&self, failure: Option<SecretStoreError>) {
            *self.failure.lock().unwrap() = failure;
        }

        fn failure(&self) -> Result<(), SecretStoreError> {
            self.failure.lock().unwrap().map_or(Ok(()), Err)
        }
    }

    impl SecretStore for MemorySecretStore {
        fn put(
            &self,
            reference: &HostIdentitySecretRef,
            secret: &HostIdentitySecret,
        ) -> Result<(), SecretStoreError> {
            self.failure()?;
            self.values
                .lock()
                .unwrap()
                .insert(reference.expose_reference().to_owned(), secret.clone());
            Ok(())
        }

        fn get(
            &self,
            reference: &HostIdentitySecretRef,
        ) -> Result<HostIdentitySecret, SecretStoreError> {
            self.failure()?;
            self.values
                .lock()
                .unwrap()
                .get(reference.expose_reference())
                .cloned()
                .ok_or(SecretStoreError::Missing)
        }

        fn delete(&self, reference: &HostIdentitySecretRef) -> Result<bool, SecretStoreError> {
            self.failure()?;
            Ok(self
                .values
                .lock()
                .unwrap()
                .remove(reference.expose_reference())
                .is_some())
        }
    }

    #[derive(Clone)]
    struct FixedEntropy {
        next: Arc<AtomicU8>,
    }

    #[derive(Clone, Copy)]
    struct FailingEntropy;

    impl IdentityEntropy for FailingEntropy {
        fn secret(&self) -> Result<HostIdentitySecret, HostIdentityError> {
            Err(HostIdentityError::Entropy)
        }

        fn reference(&self) -> Result<HostIdentitySecretRef, HostIdentityError> {
            Err(HostIdentityError::Entropy)
        }
    }

    impl FixedEntropy {
        fn new(start: u8) -> Self {
            Self {
                next: Arc::new(AtomicU8::new(start)),
            }
        }
    }

    impl IdentityEntropy for FixedEntropy {
        fn secret(&self) -> Result<HostIdentitySecret, HostIdentityError> {
            Ok(HostIdentitySecret::from_bytes(
                [self.next.fetch_add(1, Ordering::SeqCst); 32],
            ))
        }

        fn reference(&self) -> Result<HostIdentitySecretRef, HostIdentityError> {
            HostIdentitySecretRef::new(format!(
                "identity:test-{}",
                self.next.fetch_add(1, Ordering::SeqCst)
            ))
            .map_err(|_| HostIdentityError::Entropy)
        }
    }

    fn service(
        path: &std::path::Path,
        secrets: MemorySecretStore,
    ) -> HostIdentityService<MemorySecretStore, FixedEntropy> {
        HostIdentityService::new(
            ControllerDeviceRepository::open(path).unwrap(),
            secrets,
            FixedEntropy::new(3),
        )
    }

    #[test]
    fn controller_host_identity_is_stable_and_private_material_never_enters_json() {
        let fixture = tempfile::tempdir().unwrap();
        let secrets = MemorySecretStore::default();
        let first = service(fixture.path(), secrets.clone())
            .load_or_create()
            .unwrap();
        let second = service(fixture.path(), secrets).load_or_create().unwrap();
        assert_eq!(first.public, second.public);
        assert!(first.can_authorize());
        let json = fs::read_to_string(fixture.path().join("controller-devices.json")).unwrap();
        let secret_encoding = base64::engine::general_purpose::STANDARD_NO_PAD.encode([3; 32]);
        assert!(!json.contains(&secret_encoding));
        assert!(!format!("{first:?}").contains(&secret_encoding));
    }

    #[test]
    fn controller_host_identity_locked_denied_missing_and_mismatch_fail_closed() {
        for (failure, expected) in [
            (SecretStoreError::Locked, HostIdentityState::Locked),
            (
                SecretStoreError::PermissionDenied,
                HostIdentityState::PermissionDenied,
            ),
            (SecretStoreError::Missing, HostIdentityState::Lost),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let secrets = MemorySecretStore::default();
            service(fixture.path(), secrets.clone())
                .load_or_create()
                .unwrap();
            secrets.fail_with(Some(failure));
            let loaded = service(fixture.path(), secrets).load_or_create().unwrap();
            assert_eq!(loaded.state, expected);
            assert!(!loaded.can_authorize());
            assert!(loaded.static_private_key().is_none());
        }

        let fixture = tempfile::tempdir().unwrap();
        let secrets = MemorySecretStore::default();
        service(fixture.path(), secrets.clone())
            .load_or_create()
            .unwrap();
        let snapshot = ControllerDeviceRepository::open(fixture.path())
            .unwrap()
            .load()
            .unwrap();
        secrets
            .put(
                snapshot.authority.secret_ref.as_ref().unwrap(),
                &HostIdentitySecret::from_bytes([99; 32]),
            )
            .unwrap();
        assert_eq!(
            service(fixture.path(), secrets)
                .load_or_create()
                .unwrap()
                .state,
            HostIdentityState::Lost
        );
    }

    #[test]
    fn controller_host_identity_reset_commits_disabled_state_before_new_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let secrets = MemorySecretStore::default();
        let service = service(fixture.path(), secrets);
        let first = service.load_or_create().unwrap();
        let reset = service.reset().unwrap();
        assert_eq!(reset.deletion, OldSecretDeletion::Deleted);
        assert_eq!(reset.identity.state, HostIdentityState::Ready);
        assert_ne!(first.public, reset.identity.public);
        assert_eq!(
            reset.identity.public.unwrap().generation,
            HostIdentityGeneration::new(2)
        );
    }

    #[test]
    fn controller_host_identity_never_regenerates_around_existing_records() {
        let fixture = tempfile::tempdir().unwrap();
        let secrets = MemorySecretStore::default();
        let service = service(fixture.path(), secrets.clone());
        let original = service.load_or_create().unwrap();
        let snapshot = ControllerDeviceRepository::open(fixture.path())
            .unwrap()
            .load()
            .unwrap();
        secrets
            .delete(snapshot.authority.secret_ref.as_ref().unwrap())
            .unwrap();
        let lost = service.load_or_create().unwrap();
        assert_eq!(lost.state, HostIdentityState::Lost);
        assert_eq!(lost.public, original.public);
    }

    #[test]
    fn controller_host_identity_interrupted_reset_stays_disabled() {
        let fixture = tempfile::tempdir().unwrap();
        let secrets = MemorySecretStore::default();
        service(fixture.path(), secrets.clone())
            .load_or_create()
            .unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let reset = HostIdentityService::new(repository.clone(), secrets.clone(), FailingEntropy);

        assert!(matches!(reset.reset(), Err(HostIdentityError::Entropy)));
        let snapshot = repository.load().unwrap();
        assert_eq!(snapshot.authority.state, HostIdentityState::ResetRequired);
        assert!(snapshot.authority.offers.is_empty());
        assert!(
            snapshot
                .authority
                .devices
                .iter()
                .all(|device| device.status == termirust_domain::PairedDeviceStatus::Revoked)
        );
        let recovered = service(fixture.path(), secrets).load_or_create().unwrap();
        assert_eq!(recovered.state, HostIdentityState::ResetRequired);
        assert!(!recovered.can_authorize());
    }
}
