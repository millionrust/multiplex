use std::collections::BTreeMap;
use std::fmt;

use zeroize::{Zeroize, Zeroizing};

use crate::{
    OsReplicationEntropy, ReplicationAuthorityPrivateKey, ReplicationDevicePrivateKey,
    ReplicationEntropy, ReplicationEpochKey, ReplicationKeyEpoch,
};

pub const REPLICATION_STORED_SECRET_VERSION: u16 = 1;
pub const REPLICATION_STORED_SECRET_BYTES: usize = 47;
pub const REPLICATION_SECRET_REFERENCE_BYTES: usize = 47;
pub const MAX_REPLICATION_RETAINED_EPOCH_KEYS: usize = 64;

const SECRET_MAGIC: &[u8; 4] = b"TRSC";
const SECRET_REFERENCE_MAGIC: &[u8; 4] = b"TRRF";
const SECRET_REFERENCE_BYTES: usize = 32;
const SECRET_KEY_BYTES: usize = 32;
const SECRET_HEADER_BYTES: usize = SECRET_MAGIC.len() + 2 + 1 + 8;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum ReplicationSecretKind {
    AuthorityPrivateKey = 1,
    DevicePrivateKey = 2,
    EpochKey = 3,
}

impl ReplicationSecretKind {
    fn from_id(id: u8) -> Result<Self, ReplicationSecretCustodyError> {
        match id {
            1 => Ok(Self::AuthorityPrivateKey),
            2 => Ok(Self::DevicePrivateKey),
            3 => Ok(Self::EpochKey),
            _ => Err(ReplicationSecretCustodyError::InvalidEnvelope),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AuthorityPrivateKey => "authority",
            Self::DevicePrivateKey => "device",
            Self::EpochKey => "epoch",
        }
    }
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplicationSecretRef {
    version: u16,
    kind: ReplicationSecretKind,
    key_epoch: Option<ReplicationKeyEpoch>,
    identifier: [u8; SECRET_REFERENCE_BYTES],
}

impl ReplicationSecretRef {
    pub fn from_identifier(
        kind: ReplicationSecretKind,
        key_epoch: Option<ReplicationKeyEpoch>,
        mut identifier: [u8; SECRET_REFERENCE_BYTES],
    ) -> Result<Self, ReplicationSecretCustodyError> {
        if identifier.iter().all(|byte| *byte == 0)
            || matches!(kind, ReplicationSecretKind::EpochKey) != key_epoch.is_some()
        {
            identifier.zeroize();
            return Err(ReplicationSecretCustodyError::InvalidReference);
        }
        Ok(Self {
            version: REPLICATION_STORED_SECRET_VERSION,
            kind,
            key_epoch,
            identifier,
        })
    }

    pub fn kind(&self) -> ReplicationSecretKind {
        self.kind
    }

    pub fn key_epoch(&self) -> Option<ReplicationKeyEpoch> {
        self.key_epoch
    }

    /// Returns the non-secret random account used by a platform credential store.
    /// Exclude it from logs because it links public metadata to a secret item.
    pub fn expose_opaque_account(&self) -> String {
        let epoch = self.key_epoch.map_or(0, ReplicationKeyEpoch::get);
        format!(
            "v{}-{}-{}-{}",
            self.version,
            self.kind.label(),
            epoch,
            encode_hex(&self.identifier)
        )
    }

    /// Encodes a non-secret but sensitive opaque reference for private metadata storage.
    pub fn to_bytes(&self) -> [u8; REPLICATION_SECRET_REFERENCE_BYTES] {
        let mut encoded = [0_u8; REPLICATION_SECRET_REFERENCE_BYTES];
        encoded[..4].copy_from_slice(SECRET_REFERENCE_MAGIC);
        encoded[4..6].copy_from_slice(&self.version.to_be_bytes());
        encoded[6] = self.kind as u8;
        encoded[7..15].copy_from_slice(
            &self
                .key_epoch
                .map_or(0, ReplicationKeyEpoch::get)
                .to_be_bytes(),
        );
        encoded[15..].copy_from_slice(&self.identifier);
        encoded
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ReplicationSecretCustodyError> {
        if bytes.len() != REPLICATION_SECRET_REFERENCE_BYTES
            || bytes.get(..4) != Some(SECRET_REFERENCE_MAGIC)
        {
            return Err(ReplicationSecretCustodyError::InvalidReference);
        }
        let version = u16::from_be_bytes(
            bytes[4..6]
                .try_into()
                .map_err(|_| ReplicationSecretCustodyError::InvalidReference)?,
        );
        if version != REPLICATION_STORED_SECRET_VERSION {
            return Err(ReplicationSecretCustodyError::InvalidReference);
        }
        let kind = ReplicationSecretKind::from_id(bytes[6])
            .map_err(|_| ReplicationSecretCustodyError::InvalidReference)?;
        let epoch = u64::from_be_bytes(
            bytes[7..15]
                .try_into()
                .map_err(|_| ReplicationSecretCustodyError::InvalidReference)?,
        );
        let key_epoch = match kind {
            ReplicationSecretKind::EpochKey => Some(
                ReplicationKeyEpoch::new(epoch)
                    .map_err(|_| ReplicationSecretCustodyError::InvalidReference)?,
            ),
            _ if epoch == 0 => None,
            _ => return Err(ReplicationSecretCustodyError::InvalidReference),
        };
        let identifier = bytes[15..]
            .try_into()
            .map_err(|_| ReplicationSecretCustodyError::InvalidReference)?;
        Self::from_identifier(kind, key_epoch, identifier)
    }

    fn generate(
        kind: ReplicationSecretKind,
        key_epoch: Option<ReplicationKeyEpoch>,
        entropy: &mut impl ReplicationEntropy,
    ) -> Result<Self, ReplicationSecretCustodyError> {
        let mut identifier = [0_u8; SECRET_REFERENCE_BYTES];
        entropy
            .fill(&mut identifier)
            .map_err(|_| ReplicationSecretCustodyError::EntropyUnavailable)?;
        Self::from_identifier(kind, key_epoch, identifier)
    }
}

impl fmt::Debug for ReplicationSecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationSecretRef")
            .field("kind", &self.kind)
            .field("key_epoch", &self.key_epoch)
            .field("identifier", &"<opaque>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationSecretStoreError {
    Missing,
    AccessDeniedOrLocked,
    Invalid,
    Collision,
    Unavailable,
}

impl fmt::Display for ReplicationSecretStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "replication secret is missing",
            Self::AccessDeniedOrLocked => "secure storage is locked or access was denied",
            Self::Invalid => "secure storage returned invalid secret data",
            Self::Collision => "replication secret reference already exists",
            Self::Unavailable => "secure storage is unavailable",
        })
    }
}

impl std::error::Error for ReplicationSecretStoreError {}

pub trait ReplicationSecretBackend: Send + Sync {
    /// Creates a secret at a fresh opaque reference and rejects existing references.
    fn put(
        &self,
        reference: &ReplicationSecretRef,
        secret: &[u8],
    ) -> Result<(), ReplicationSecretStoreError>;

    /// Loaded buffers remain zeroizing across the backend/vault boundary.
    fn get(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<Zeroizing<Vec<u8>>, ReplicationSecretStoreError>;

    fn delete(&self, reference: &ReplicationSecretRef)
    -> Result<bool, ReplicationSecretStoreError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationSecretCustodyError {
    InvalidReference,
    EntropyUnavailable,
    Store(ReplicationSecretStoreError),
    InvalidEnvelope,
    SecretKindMismatch,
    KeyEpochMismatch,
    InvalidHistoricalLimit,
    EmptyHistory,
    NonContiguousHistory,
    KeyEpochNotRetained,
    KeyEpochOverflow,
}

impl fmt::Display for ReplicationSecretCustodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReference => formatter.write_str("invalid replication secret reference"),
            Self::EntropyUnavailable => {
                formatter.write_str("secure random generation is unavailable")
            }
            Self::Store(error) => error.fmt(formatter),
            Self::InvalidEnvelope => formatter.write_str("invalid stored replication secret"),
            Self::SecretKindMismatch => {
                formatter.write_str("stored replication secret has the wrong role")
            }
            Self::KeyEpochMismatch => {
                formatter.write_str("stored replication secret has the wrong key epoch")
            }
            Self::InvalidHistoricalLimit => {
                formatter.write_str("invalid historical replication key limit")
            }
            Self::EmptyHistory => formatter.write_str("historical replication key index is empty"),
            Self::NonContiguousHistory => {
                formatter.write_str("historical replication key index is not contiguous")
            }
            Self::KeyEpochNotRetained => {
                formatter.write_str("requested replication key epoch is not retained")
            }
            Self::KeyEpochOverflow => formatter.write_str("replication key epoch overflow"),
        }
    }
}

impl std::error::Error for ReplicationSecretCustodyError {}

impl From<ReplicationSecretStoreError> for ReplicationSecretCustodyError {
    fn from(error: ReplicationSecretStoreError) -> Self {
        Self::Store(error)
    }
}

pub struct ReplicationSecretVault<B> {
    backend: B,
}

impl<B: ReplicationSecretBackend> ReplicationSecretVault<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    pub fn store_authority_key(
        &self,
        key: &ReplicationAuthorityPrivateKey,
    ) -> Result<ReplicationSecretRef, ReplicationSecretCustodyError> {
        self.store_authority_key_with_entropy(key, &mut OsReplicationEntropy)
    }

    pub fn store_authority_key_with_entropy(
        &self,
        key: &ReplicationAuthorityPrivateKey,
        entropy: &mut impl ReplicationEntropy,
    ) -> Result<ReplicationSecretRef, ReplicationSecretCustodyError> {
        self.store(
            ReplicationSecretKind::AuthorityPrivateKey,
            None,
            key.copy_for_secret_storage(),
            entropy,
        )
    }

    pub fn store_device_key(
        &self,
        key: &ReplicationDevicePrivateKey,
    ) -> Result<ReplicationSecretRef, ReplicationSecretCustodyError> {
        self.store_device_key_with_entropy(key, &mut OsReplicationEntropy)
    }

    pub fn store_device_key_with_entropy(
        &self,
        key: &ReplicationDevicePrivateKey,
        entropy: &mut impl ReplicationEntropy,
    ) -> Result<ReplicationSecretRef, ReplicationSecretCustodyError> {
        self.store(
            ReplicationSecretKind::DevicePrivateKey,
            None,
            key.copy_for_secret_storage(),
            entropy,
        )
    }

    pub fn store_epoch_key(
        &self,
        key: &ReplicationEpochKey,
    ) -> Result<ReplicationSecretRef, ReplicationSecretCustodyError> {
        self.store_epoch_key_with_entropy(key, &mut OsReplicationEntropy)
    }

    pub fn store_epoch_key_with_entropy(
        &self,
        key: &ReplicationEpochKey,
        entropy: &mut impl ReplicationEntropy,
    ) -> Result<ReplicationSecretRef, ReplicationSecretCustodyError> {
        self.store(
            ReplicationSecretKind::EpochKey,
            Some(key.epoch()),
            key.copy_for_secret_storage(),
            entropy,
        )
    }

    pub fn load_authority_key(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<ReplicationAuthorityPrivateKey, ReplicationSecretCustodyError> {
        require_reference(reference, ReplicationSecretKind::AuthorityPrivateKey, None)?;
        let bytes = self.load(reference, ReplicationSecretKind::AuthorityPrivateKey, None)?;
        ReplicationAuthorityPrivateKey::from_bytes(bytes)
            .map_err(|_| ReplicationSecretCustodyError::InvalidEnvelope)
    }

    pub fn load_device_key(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<ReplicationDevicePrivateKey, ReplicationSecretCustodyError> {
        require_reference(reference, ReplicationSecretKind::DevicePrivateKey, None)?;
        let bytes = self.load(reference, ReplicationSecretKind::DevicePrivateKey, None)?;
        ReplicationDevicePrivateKey::from_bytes(bytes)
            .map_err(|_| ReplicationSecretCustodyError::InvalidEnvelope)
    }

    pub fn load_epoch_key(
        &self,
        reference: &ReplicationSecretRef,
        expected_epoch: ReplicationKeyEpoch,
    ) -> Result<ReplicationEpochKey, ReplicationSecretCustodyError> {
        require_reference(
            reference,
            ReplicationSecretKind::EpochKey,
            Some(expected_epoch),
        )?;
        let bytes = self.load(
            reference,
            ReplicationSecretKind::EpochKey,
            Some(expected_epoch),
        )?;
        ReplicationEpochKey::from_bytes(expected_epoch, bytes)
            .map_err(|_| ReplicationSecretCustodyError::InvalidEnvelope)
    }

    pub fn delete(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<bool, ReplicationSecretCustodyError> {
        self.backend.delete(reference).map_err(Into::into)
    }

    fn store(
        &self,
        kind: ReplicationSecretKind,
        key_epoch: Option<ReplicationKeyEpoch>,
        mut key_bytes: [u8; SECRET_KEY_BYTES],
        entropy: &mut impl ReplicationEntropy,
    ) -> Result<ReplicationSecretRef, ReplicationSecretCustodyError> {
        let result = (|| {
            let reference = ReplicationSecretRef::generate(kind, key_epoch, entropy)?;
            let encoded = encode_secret(kind, key_epoch, &key_bytes);
            self.backend.put(&reference, &encoded)?;
            Ok(reference)
        })();
        key_bytes.zeroize();
        result
    }

    fn load(
        &self,
        reference: &ReplicationSecretRef,
        expected_kind: ReplicationSecretKind,
        expected_epoch: Option<ReplicationKeyEpoch>,
    ) -> Result<[u8; SECRET_KEY_BYTES], ReplicationSecretCustodyError> {
        let encoded = self.backend.get(reference)?;
        decode_secret(&encoded, expected_kind, expected_epoch)
    }
}

impl<B> fmt::Debug for ReplicationSecretVault<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationSecretVault")
            .field("backend", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplicationHistoricalKeyLimit(usize);

impl ReplicationHistoricalKeyLimit {
    pub fn new(value: usize) -> Result<Self, ReplicationSecretCustodyError> {
        if value == 0 || value > MAX_REPLICATION_RETAINED_EPOCH_KEYS {
            return Err(ReplicationSecretCustodyError::InvalidHistoricalLimit);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> usize {
        self.0
    }
}

#[derive(Clone)]
pub struct ReplicationHistoricalKeyIndex {
    limit: ReplicationHistoricalKeyLimit,
    references: BTreeMap<ReplicationKeyEpoch, ReplicationSecretRef>,
}

impl ReplicationHistoricalKeyIndex {
    pub fn from_retained(
        limit: ReplicationHistoricalKeyLimit,
        references: impl IntoIterator<Item = ReplicationSecretRef>,
    ) -> Result<Self, ReplicationSecretCustodyError> {
        let mut by_epoch = BTreeMap::new();
        for reference in references {
            if reference.kind() != ReplicationSecretKind::EpochKey {
                return Err(ReplicationSecretCustodyError::SecretKindMismatch);
            }
            let epoch = reference
                .key_epoch()
                .ok_or(ReplicationSecretCustodyError::KeyEpochMismatch)?;
            if by_epoch.insert(epoch, reference).is_some() || by_epoch.len() > limit.get() {
                return Err(ReplicationSecretCustodyError::NonContiguousHistory);
            }
        }
        if by_epoch.is_empty() {
            return Err(ReplicationSecretCustodyError::EmptyHistory);
        }
        let mut epochs = by_epoch.keys().copied();
        let mut previous = epochs
            .next()
            .ok_or(ReplicationSecretCustodyError::EmptyHistory)?;
        for epoch in epochs {
            let expected = previous
                .get()
                .checked_add(1)
                .ok_or(ReplicationSecretCustodyError::KeyEpochOverflow)?;
            if epoch.get() != expected {
                return Err(ReplicationSecretCustodyError::NonContiguousHistory);
            }
            previous = epoch;
        }
        Ok(Self {
            limit,
            references: by_epoch,
        })
    }

    pub fn current_epoch(&self) -> ReplicationKeyEpoch {
        *self
            .references
            .last_key_value()
            .expect("validated historical index is non-empty")
            .0
    }

    pub fn len(&self) -> usize {
        self.references.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn limit(&self) -> ReplicationHistoricalKeyLimit {
        self.limit
    }

    pub fn reference_for(
        &self,
        epoch: ReplicationKeyEpoch,
    ) -> Result<&ReplicationSecretRef, ReplicationSecretCustodyError> {
        self.references
            .get(&epoch)
            .ok_or(ReplicationSecretCustodyError::KeyEpochNotRetained)
    }

    pub fn references(
        &self,
    ) -> impl ExactSizeIterator<Item = &ReplicationSecretRef> + DoubleEndedIterator {
        self.references.values()
    }

    pub fn append(
        &self,
        reference: ReplicationSecretRef,
    ) -> Result<ReplicationHistoricalKeyUpdate, ReplicationSecretCustodyError> {
        if reference.kind() != ReplicationSecretKind::EpochKey {
            return Err(ReplicationSecretCustodyError::SecretKindMismatch);
        }
        let epoch = reference
            .key_epoch()
            .ok_or(ReplicationSecretCustodyError::KeyEpochMismatch)?;
        let expected = self
            .current_epoch()
            .get()
            .checked_add(1)
            .ok_or(ReplicationSecretCustodyError::KeyEpochOverflow)?;
        if epoch.get() != expected {
            return Err(ReplicationSecretCustodyError::NonContiguousHistory);
        }

        let mut next = self.clone();
        next.references.insert(epoch, reference);
        let mut retired = Vec::new();
        while next.references.len() > next.limit.get() {
            let oldest = *next
                .references
                .first_key_value()
                .expect("append makes the historical index non-empty")
                .0;
            retired.push(
                next.references
                    .remove(&oldest)
                    .expect("oldest historical reference must exist"),
            );
        }
        Ok(ReplicationHistoricalKeyUpdate {
            index: next,
            retired,
        })
    }
}

impl fmt::Debug for ReplicationHistoricalKeyIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationHistoricalKeyIndex")
            .field("limit", &self.limit)
            .field("retained", &self.len())
            .field("current_epoch", &self.current_epoch())
            .field("references", &"<opaque>")
            .finish()
    }
}

pub struct ReplicationHistoricalKeyUpdate {
    index: ReplicationHistoricalKeyIndex,
    retired: Vec<ReplicationSecretRef>,
}

impl ReplicationHistoricalKeyUpdate {
    pub fn index(&self) -> &ReplicationHistoricalKeyIndex {
        &self.index
    }

    pub fn retired(&self) -> &[ReplicationSecretRef] {
        &self.retired
    }

    pub fn into_parts(self) -> (ReplicationHistoricalKeyIndex, Vec<ReplicationSecretRef>) {
        (self.index, self.retired)
    }
}

impl fmt::Debug for ReplicationHistoricalKeyUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationHistoricalKeyUpdate")
            .field("index", &self.index)
            .field("retired_count", &self.retired.len())
            .field("retired", &"<opaque>")
            .finish()
    }
}

fn require_reference(
    reference: &ReplicationSecretRef,
    expected_kind: ReplicationSecretKind,
    expected_epoch: Option<ReplicationKeyEpoch>,
) -> Result<(), ReplicationSecretCustodyError> {
    if reference.kind() != expected_kind {
        return Err(ReplicationSecretCustodyError::SecretKindMismatch);
    }
    if reference.key_epoch() != expected_epoch {
        return Err(ReplicationSecretCustodyError::KeyEpochMismatch);
    }
    Ok(())
}

fn encode_secret(
    kind: ReplicationSecretKind,
    key_epoch: Option<ReplicationKeyEpoch>,
    key_bytes: &[u8; SECRET_KEY_BYTES],
) -> Zeroizing<Vec<u8>> {
    let mut encoded = Zeroizing::new(Vec::with_capacity(REPLICATION_STORED_SECRET_BYTES));
    encoded.extend_from_slice(SECRET_MAGIC);
    encoded.extend_from_slice(&REPLICATION_STORED_SECRET_VERSION.to_be_bytes());
    encoded.push(kind as u8);
    encoded.extend_from_slice(&key_epoch.map_or(0, ReplicationKeyEpoch::get).to_be_bytes());
    encoded.extend_from_slice(key_bytes);
    debug_assert_eq!(encoded.len(), SECRET_HEADER_BYTES + SECRET_KEY_BYTES);
    encoded
}

fn decode_secret(
    encoded: &[u8],
    expected_kind: ReplicationSecretKind,
    expected_epoch: Option<ReplicationKeyEpoch>,
) -> Result<[u8; SECRET_KEY_BYTES], ReplicationSecretCustodyError> {
    if encoded.len() != REPLICATION_STORED_SECRET_BYTES
        || encoded.get(..SECRET_MAGIC.len()) != Some(SECRET_MAGIC)
    {
        return Err(ReplicationSecretCustodyError::InvalidEnvelope);
    }
    let version = u16::from_be_bytes([encoded[4], encoded[5]]);
    if version != REPLICATION_STORED_SECRET_VERSION {
        return Err(ReplicationSecretCustodyError::InvalidEnvelope);
    }
    let kind = ReplicationSecretKind::from_id(encoded[6])?;
    if kind != expected_kind {
        return Err(ReplicationSecretCustodyError::SecretKindMismatch);
    }
    let epoch_value = u64::from_be_bytes(
        encoded[7..15]
            .try_into()
            .map_err(|_| ReplicationSecretCustodyError::InvalidEnvelope)?,
    );
    let epoch = match kind {
        ReplicationSecretKind::EpochKey => Some(
            ReplicationKeyEpoch::new(epoch_value)
                .map_err(|_| ReplicationSecretCustodyError::InvalidEnvelope)?,
        ),
        _ if epoch_value == 0 => None,
        _ => return Err(ReplicationSecretCustodyError::InvalidEnvelope),
    };
    if epoch != expected_epoch {
        return Err(ReplicationSecretCustodyError::KeyEpochMismatch);
    }
    let mut key_bytes = [0_u8; SECRET_KEY_BYTES];
    key_bytes.copy_from_slice(&encoded[SECRET_HEADER_BYTES..]);
    if key_bytes.iter().all(|byte| *byte == 0) {
        key_bytes.zeroize();
        return Err(ReplicationSecretCustodyError::InvalidEnvelope);
    }
    Ok(key_bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(feature = "os-keyring")]
mod os_keyring {
    use keyring::{Entry, Error as KeyringError};
    use zeroize::{Zeroize, Zeroizing};

    use super::{ReplicationSecretBackend, ReplicationSecretRef, ReplicationSecretStoreError};

    const SERVICE_NAME: &str = "com.termirust.replication.secrets.v1";

    #[derive(Clone, Copy, Debug, Default)]
    pub struct OsReplicationSecretBackend;

    impl OsReplicationSecretBackend {
        pub const fn is_supported_target() -> bool {
            cfg!(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "windows",
                target_os = "linux"
            ))
        }
    }

    impl ReplicationSecretBackend for OsReplicationSecretBackend {
        fn put(
            &self,
            reference: &ReplicationSecretRef,
            secret: &[u8],
        ) -> Result<(), ReplicationSecretStoreError> {
            with_entry(reference, |entry| match entry.get_secret() {
                Ok(mut existing) => {
                    existing.zeroize();
                    Err(ReplicationSecretStoreError::Collision)
                }
                Err(KeyringError::NoEntry) => entry.set_secret(secret).map_err(map_error),
                Err(error) => Err(map_error(error)),
            })
        }

        fn get(
            &self,
            reference: &ReplicationSecretRef,
        ) -> Result<Zeroizing<Vec<u8>>, ReplicationSecretStoreError> {
            with_entry(reference, |entry| {
                entry.get_secret().map(Zeroizing::new).map_err(map_error)
            })
        }

        fn delete(
            &self,
            reference: &ReplicationSecretRef,
        ) -> Result<bool, ReplicationSecretStoreError> {
            with_entry(reference, |entry| match entry.delete_credential() {
                Ok(()) => Ok(true),
                Err(KeyringError::NoEntry) => Ok(false),
                Err(error) => Err(map_error(error)),
            })
        }
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "linux"
    ))]
    fn with_entry<T>(
        reference: &ReplicationSecretRef,
        operation: impl FnOnce(Entry) -> Result<T, ReplicationSecretStoreError>,
    ) -> Result<T, ReplicationSecretStoreError> {
        let entry =
            Entry::new(SERVICE_NAME, &reference.expose_opaque_account()).map_err(map_error)?;
        operation(entry)
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "linux"
    )))]
    fn with_entry<T>(
        _reference: &ReplicationSecretRef,
        _operation: impl FnOnce(Entry) -> Result<T, ReplicationSecretStoreError>,
    ) -> Result<T, ReplicationSecretStoreError> {
        Err(ReplicationSecretStoreError::Unavailable)
    }

    fn map_error(error: KeyringError) -> ReplicationSecretStoreError {
        match error {
            KeyringError::NoEntry => ReplicationSecretStoreError::Missing,
            KeyringError::NoStorageAccess(_) => ReplicationSecretStoreError::AccessDeniedOrLocked,
            KeyringError::BadEncoding(mut bytes) => {
                bytes.zeroize();
                ReplicationSecretStoreError::Invalid
            }
            KeyringError::TooLong(_, _)
            | KeyringError::Invalid(_, _)
            | KeyringError::Ambiguous(_) => ReplicationSecretStoreError::Invalid,
            _ => ReplicationSecretStoreError::Unavailable,
        }
    }
}

#[cfg(feature = "os-keyring")]
pub use os_keyring::OsReplicationSecretBackend;
