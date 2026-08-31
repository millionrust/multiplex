use std::fmt;

use hpke::{
    Deserializable, Kem as _, OpModeR, OpModeS, Serializable, aead::ChaCha20Poly1305,
    kdf::HkdfSha256, kem::X25519HkdfSha256, single_shot_open, single_shot_seal,
};
use rand_core_09::{CryptoRng, RngCore};
use termirust_domain::{ReplicationReplicaId, ReplicationWorkspaceId};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{ReplicationCryptoError, ReplicationEpochKey, ReplicationKeyEpoch};

pub const REPLICATION_KEY_PACKAGE_VERSION: u16 = 1;
pub const REPLICATION_HPKE_PUBLIC_KEY_BYTES: usize = 32;
pub const REPLICATION_HPKE_PRIVATE_KEY_BYTES: usize = 32;
pub const REPLICATION_HPKE_ENCAPSULATED_KEY_BYTES: usize = 32;
pub const REPLICATION_WRAPPED_EPOCH_CIPHERTEXT_BYTES: usize = 48;
pub const MAX_REPLICATION_KEY_PACKAGE_BYTES: usize = 256;

const PACKAGE_MAGIC: &[u8; 4] = b"TRKW";
const PACKAGE_HEADER_BYTES: usize = PACKAGE_MAGIC.len() + 2 + 1 + 8 + 32 + 2;
const HPKE_INFO_DOMAIN: &[u8] = b"termirust.replication.epoch-wrap.hpke-info.v1";
const HPKE_AAD_DOMAIN: &[u8] = b"termirust.replication.epoch-wrap.aad.v1";

type WrappingKem = X25519HkdfSha256;
type WrappingKdf = HkdfSha256;
type WrappingAead = ChaCha20Poly1305;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ReplicationKeyWrappingSuite {
    HpkeAuthX25519HkdfSha256ChaCha20Poly1305 = 1,
}

impl ReplicationKeyWrappingSuite {
    fn from_id(id: u8) -> Result<Self, ReplicationKeyWrappingError> {
        match id {
            1 => Ok(Self::HpkeAuthX25519HkdfSha256ChaCha20Poly1305),
            _ => Err(ReplicationKeyWrappingError::UnsupportedSuite),
        }
    }

    fn id(self) -> u8 {
        self as u8
    }
}

macro_rules! private_key_type {
    ($private:ident, $public:ident, $label:literal) => {
        #[derive(Zeroize, ZeroizeOnDrop)]
        pub struct $private([u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES]);

        impl $private {
            pub fn from_bytes(
                mut bytes: [u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES],
            ) -> Result<Self, ReplicationKeyWrappingError> {
                if validate_private_key_bytes(&bytes).is_err() {
                    bytes.zeroize();
                    return Err(ReplicationKeyWrappingError::InvalidPrivateKey);
                }
                if hpke_private_key(&bytes).is_err() {
                    bytes.zeroize();
                    return Err(ReplicationKeyWrappingError::InvalidPrivateKey);
                }
                Ok(Self(bytes))
            }

            pub fn public_key(&self) -> $public {
                let private = hpke_private_key(&self.0)
                    .expect("validated replication wrapping private key must deserialize");
                let public = WrappingKem::sk_to_pk(&private);
                let bytes = public.to_bytes();
                $public(
                    bytes
                        .as_slice()
                        .try_into()
                        .expect("X25519 public keys are exactly 32 bytes"),
                )
            }
        }

        impl fmt::Debug for $private {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!($label, "(<redacted>)"))
            }
        }

        #[derive(Clone, Eq, PartialEq)]
        pub struct $public([u8; REPLICATION_HPKE_PUBLIC_KEY_BYTES]);

        impl $public {
            pub fn from_bytes(
                bytes: [u8; REPLICATION_HPKE_PUBLIC_KEY_BYTES],
            ) -> Result<Self, ReplicationKeyWrappingError> {
                validate_public_key_bytes(&bytes)?;
                hpke_public_key(&bytes)?;
                Ok(Self(bytes))
            }

            pub fn as_bytes(&self) -> &[u8; REPLICATION_HPKE_PUBLIC_KEY_BYTES] {
                &self.0
            }
        }

        impl fmt::Debug for $public {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($public), "(<redacted>)"))
            }
        }
    };
}

private_key_type!(
    ReplicationAuthorityPrivateKey,
    ReplicationAuthorityPublicKey,
    "ReplicationAuthorityPrivateKey"
);
private_key_type!(
    ReplicationDevicePrivateKey,
    ReplicationDevicePublicKey,
    "ReplicationDevicePrivateKey"
);

#[derive(Clone, Copy)]
pub struct ReplicationKeyWrapContext<'a> {
    pub workspace_id: &'a ReplicationWorkspaceId,
    pub recipient: &'a ReplicationReplicaId,
}

impl ReplicationKeyWrapContext<'_> {
    fn validate(&self) -> Result<(), ReplicationKeyWrappingError> {
        self.workspace_id
            .validate()
            .map_err(|_| ReplicationKeyWrappingError::InvalidContext)?;
        self.recipient
            .validate()
            .map_err(|_| ReplicationKeyWrappingError::InvalidContext)
    }
}

impl fmt::Debug for ReplicationKeyWrapContext<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationKeyWrapContext")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct WrappedReplicationEpochKey {
    version: u16,
    suite: ReplicationKeyWrappingSuite,
    key_epoch: ReplicationKeyEpoch,
    encapsulated_key: [u8; REPLICATION_HPKE_ENCAPSULATED_KEY_BYTES],
    ciphertext: [u8; REPLICATION_WRAPPED_EPOCH_CIPHERTEXT_BYTES],
}

impl WrappedReplicationEpochKey {
    pub fn version(&self) -> u16 {
        self.version
    }

    pub fn suite(&self) -> ReplicationKeyWrappingSuite {
        self.suite
    }

    pub fn key_epoch(&self) -> ReplicationKeyEpoch {
        self.key_epoch
    }

    pub fn encoded_len(&self) -> usize {
        PACKAGE_HEADER_BYTES + self.ciphertext.len()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(self.encoded_len());
        encoded.extend_from_slice(PACKAGE_MAGIC);
        encoded.extend_from_slice(&self.version.to_be_bytes());
        encoded.push(self.suite.id());
        encoded.extend_from_slice(&self.key_epoch.get().to_be_bytes());
        encoded.extend_from_slice(&self.encapsulated_key);
        encoded
            .extend_from_slice(&(REPLICATION_WRAPPED_EPOCH_CIPHERTEXT_BYTES as u16).to_be_bytes());
        encoded.extend_from_slice(&self.ciphertext);
        encoded
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ReplicationKeyWrappingError> {
        if bytes.len() > MAX_REPLICATION_KEY_PACKAGE_BYTES {
            return Err(ReplicationKeyWrappingError::PackageTooLarge);
        }
        if bytes.len() < PACKAGE_HEADER_BYTES {
            return Err(ReplicationKeyWrappingError::MalformedPackage);
        }
        if bytes.get(..PACKAGE_MAGIC.len()) != Some(PACKAGE_MAGIC) {
            return Err(ReplicationKeyWrappingError::MalformedPackage);
        }

        let version = u16::from_be_bytes([bytes[4], bytes[5]]);
        if version != REPLICATION_KEY_PACKAGE_VERSION {
            return Err(ReplicationKeyWrappingError::UnsupportedVersion);
        }
        let suite = ReplicationKeyWrappingSuite::from_id(bytes[6])?;
        let key_epoch = ReplicationKeyEpoch::new(u64::from_be_bytes(
            bytes[7..15]
                .try_into()
                .map_err(|_| ReplicationKeyWrappingError::MalformedPackage)?,
        ))
        .map_err(|_| ReplicationKeyWrappingError::InvalidKeyEpoch)?;
        let encapsulated_key = bytes[15..47]
            .try_into()
            .map_err(|_| ReplicationKeyWrappingError::MalformedPackage)?;
        let ciphertext_len = u16::from_be_bytes(
            bytes[47..49]
                .try_into()
                .map_err(|_| ReplicationKeyWrappingError::MalformedPackage)?,
        ) as usize;
        if ciphertext_len != REPLICATION_WRAPPED_EPOCH_CIPHERTEXT_BYTES
            || bytes.len() != PACKAGE_HEADER_BYTES + ciphertext_len
        {
            return Err(ReplicationKeyWrappingError::MalformedPackage);
        }
        let ciphertext = bytes[PACKAGE_HEADER_BYTES..]
            .try_into()
            .map_err(|_| ReplicationKeyWrappingError::MalformedPackage)?;

        Ok(Self {
            version,
            suite,
            key_epoch,
            encapsulated_key,
            ciphertext,
        })
    }
}

impl fmt::Debug for WrappedReplicationEpochKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WrappedReplicationEpochKey")
            .field("version", &self.version)
            .field("suite", &self.suite)
            .field("key_epoch", &self.key_epoch)
            .field("encoded_len", &self.encoded_len())
            .field("sealed_data", &"<redacted>")
            .finish()
    }
}

pub trait ReplicationEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), ReplicationEntropyError>;
}

pub struct OsReplicationEntropy;

impl ReplicationEntropy for OsReplicationEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), ReplicationEntropyError> {
        getrandom::fill(destination).map_err(|_| ReplicationEntropyError)
    }
}

pub fn generate_replication_authority_private_key()
-> Result<ReplicationAuthorityPrivateKey, ReplicationKeyWrappingError> {
    generate_replication_authority_private_key_with_entropy(&mut OsReplicationEntropy)
}

pub fn generate_replication_authority_private_key_with_entropy(
    entropy: &mut impl ReplicationEntropy,
) -> Result<ReplicationAuthorityPrivateKey, ReplicationKeyWrappingError> {
    let bytes = generate_private_key_bytes(entropy)?;
    ReplicationAuthorityPrivateKey::from_bytes(bytes)
}

pub fn generate_replication_device_private_key()
-> Result<ReplicationDevicePrivateKey, ReplicationKeyWrappingError> {
    generate_replication_device_private_key_with_entropy(&mut OsReplicationEntropy)
}

pub fn generate_replication_device_private_key_with_entropy(
    entropy: &mut impl ReplicationEntropy,
) -> Result<ReplicationDevicePrivateKey, ReplicationKeyWrappingError> {
    let bytes = generate_private_key_bytes(entropy)?;
    ReplicationDevicePrivateKey::from_bytes(bytes)
}

pub fn wrap_replication_epoch_key(
    context: ReplicationKeyWrapContext<'_>,
    authority_private: &ReplicationAuthorityPrivateKey,
    recipient_public: &ReplicationDevicePublicKey,
    epoch_key: &ReplicationEpochKey,
) -> Result<WrappedReplicationEpochKey, ReplicationKeyWrappingError> {
    wrap_replication_epoch_key_with_entropy(
        context,
        authority_private,
        recipient_public,
        epoch_key,
        &mut OsReplicationEntropy,
    )
}

pub fn wrap_replication_epoch_key_with_entropy(
    context: ReplicationKeyWrapContext<'_>,
    authority_private: &ReplicationAuthorityPrivateKey,
    recipient_public: &ReplicationDevicePublicKey,
    epoch_key: &ReplicationEpochKey,
    entropy: &mut impl ReplicationEntropy,
) -> Result<WrappedReplicationEpochKey, ReplicationKeyWrappingError> {
    context.validate()?;
    let authority_private_hpke = hpke_private_key(&authority_private.0)?;
    let authority_public_hpke = WrappingKem::sk_to_pk(&authority_private_hpke);
    let authority_public = authority_private.public_key();
    let recipient_public_hpke = hpke_public_key(&recipient_public.0)?;
    let suite = ReplicationKeyWrappingSuite::HpkeAuthX25519HkdfSha256ChaCha20Poly1305;
    let info = encode_wrap_metadata(
        HPKE_INFO_DOMAIN,
        context,
        epoch_key.epoch(),
        suite,
        &authority_public.0,
        &recipient_public.0,
    )?;
    let aad = encode_wrap_metadata(
        HPKE_AAD_DOMAIN,
        context,
        epoch_key.epoch(),
        suite,
        &authority_public.0,
        &recipient_public.0,
    )?;
    let mut ephemeral_ikm = Zeroizing::new([0_u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES]);
    entropy
        .fill(ephemeral_ikm.as_mut())
        .map_err(|_| ReplicationKeyWrappingError::RandomUnavailable)?;
    if ephemeral_ikm.iter().all(|byte| *byte == 0) {
        return Err(ReplicationKeyWrappingError::RandomUnavailable);
    }
    let mut rng = FixedEntropyRng::new(*ephemeral_ikm);
    let mode = OpModeS::<WrappingKem>::Auth((authority_private_hpke, authority_public_hpke));
    let (encapsulated_key, ciphertext) =
        single_shot_seal::<WrappingAead, WrappingKdf, WrappingKem, _>(
            &mode,
            &recipient_public_hpke,
            info.as_ref(),
            &epoch_key.bytes,
            aad.as_ref(),
            &mut rng,
        )
        .map_err(|_| ReplicationKeyWrappingError::SealFailed)?;
    if ciphertext.len() != REPLICATION_WRAPPED_EPOCH_CIPHERTEXT_BYTES {
        return Err(ReplicationKeyWrappingError::SealFailed);
    }
    let encapsulated_key = encapsulated_key.to_bytes();

    Ok(WrappedReplicationEpochKey {
        version: REPLICATION_KEY_PACKAGE_VERSION,
        suite,
        key_epoch: epoch_key.epoch(),
        encapsulated_key: encapsulated_key
            .as_slice()
            .try_into()
            .map_err(|_| ReplicationKeyWrappingError::SealFailed)?,
        ciphertext: ciphertext
            .try_into()
            .map_err(|_| ReplicationKeyWrappingError::SealFailed)?,
    })
}

pub fn open_wrapped_replication_epoch_key(
    context: ReplicationKeyWrapContext<'_>,
    trusted_authority: &ReplicationAuthorityPublicKey,
    recipient_private: &ReplicationDevicePrivateKey,
    expected_epoch: ReplicationKeyEpoch,
    package: &WrappedReplicationEpochKey,
) -> Result<ReplicationEpochKey, ReplicationKeyWrappingError> {
    context.validate()?;
    if package.version != REPLICATION_KEY_PACKAGE_VERSION {
        return Err(ReplicationKeyWrappingError::UnsupportedVersion);
    }
    if package.suite != ReplicationKeyWrappingSuite::HpkeAuthX25519HkdfSha256ChaCha20Poly1305 {
        return Err(ReplicationKeyWrappingError::UnsupportedSuite);
    }
    if package.key_epoch != expected_epoch {
        return Err(ReplicationKeyWrappingError::KeyEpochMismatch);
    }

    let trusted_authority_hpke = hpke_public_key(&trusted_authority.0)?;
    let recipient_private_hpke = hpke_private_key(&recipient_private.0)?;
    let recipient_public = recipient_private.public_key();
    let encapsulated_key =
        <WrappingKem as hpke::Kem>::EncappedKey::from_bytes(&package.encapsulated_key)
            .map_err(|_| ReplicationKeyWrappingError::MalformedPackage)?;
    let info = encode_wrap_metadata(
        HPKE_INFO_DOMAIN,
        context,
        package.key_epoch,
        package.suite,
        &trusted_authority.0,
        &recipient_public.0,
    )?;
    let aad = encode_wrap_metadata(
        HPKE_AAD_DOMAIN,
        context,
        package.key_epoch,
        package.suite,
        &trusted_authority.0,
        &recipient_public.0,
    )?;
    let mode = OpModeR::<WrappingKem>::Auth(trusted_authority_hpke);
    let mut plaintext = single_shot_open::<WrappingAead, WrappingKdf, WrappingKem>(
        &mode,
        &recipient_private_hpke,
        &encapsulated_key,
        info.as_ref(),
        &package.ciphertext,
        aad.as_ref(),
    )
    .map_err(|_| ReplicationKeyWrappingError::AuthenticationFailed)?;
    if plaintext.len() != REPLICATION_HPKE_PRIVATE_KEY_BYTES {
        plaintext.zeroize();
        return Err(ReplicationKeyWrappingError::AuthenticationFailed);
    }
    let mut key_bytes = [0_u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES];
    key_bytes.copy_from_slice(&plaintext);
    plaintext.zeroize();
    let result = ReplicationEpochKey::from_bytes(package.key_epoch, key_bytes);
    key_bytes.zeroize();
    result.map_err(|error| match error {
        ReplicationCryptoError::InvalidKeyMaterial => {
            ReplicationKeyWrappingError::AuthenticationFailed
        }
        _ => ReplicationKeyWrappingError::InvalidKeyEpoch,
    })
}

fn generate_private_key_bytes(
    entropy: &mut impl ReplicationEntropy,
) -> Result<[u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES], ReplicationKeyWrappingError> {
    let mut ikm = Zeroizing::new([0_u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES]);
    entropy
        .fill(ikm.as_mut())
        .map_err(|_| ReplicationKeyWrappingError::RandomUnavailable)?;
    if ikm.iter().all(|byte| *byte == 0) {
        return Err(ReplicationKeyWrappingError::InvalidPrivateKey);
    }
    let (private, _) = WrappingKem::derive_keypair(ikm.as_ref());
    let mut encoded = Zeroizing::new([0_u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES]);
    private.write_exact(encoded.as_mut());
    Ok(*encoded)
}

fn hpke_private_key(
    bytes: &[u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES],
) -> Result<<WrappingKem as hpke::Kem>::PrivateKey, ReplicationKeyWrappingError> {
    <WrappingKem as hpke::Kem>::PrivateKey::from_bytes(bytes)
        .map_err(|_| ReplicationKeyWrappingError::InvalidPrivateKey)
}

fn hpke_public_key(
    bytes: &[u8; REPLICATION_HPKE_PUBLIC_KEY_BYTES],
) -> Result<<WrappingKem as hpke::Kem>::PublicKey, ReplicationKeyWrappingError> {
    <WrappingKem as hpke::Kem>::PublicKey::from_bytes(bytes)
        .map_err(|_| ReplicationKeyWrappingError::InvalidPublicKey)
}

fn validate_private_key_bytes(
    bytes: &[u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES],
) -> Result<(), ReplicationKeyWrappingError> {
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(ReplicationKeyWrappingError::InvalidPrivateKey);
    }
    Ok(())
}

fn validate_public_key_bytes(
    bytes: &[u8; REPLICATION_HPKE_PUBLIC_KEY_BYTES],
) -> Result<(), ReplicationKeyWrappingError> {
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(ReplicationKeyWrappingError::InvalidPublicKey);
    }
    Ok(())
}

fn encode_wrap_metadata(
    domain: &[u8],
    context: ReplicationKeyWrapContext<'_>,
    epoch: ReplicationKeyEpoch,
    suite: ReplicationKeyWrappingSuite,
    authority_public: &[u8; REPLICATION_HPKE_PUBLIC_KEY_BYTES],
    recipient_public: &[u8; REPLICATION_HPKE_PUBLIC_KEY_BYTES],
) -> Result<Zeroizing<Vec<u8>>, ReplicationKeyWrappingError> {
    let mut encoded = Zeroizing::new(Vec::with_capacity(384));
    push_field(&mut encoded, domain)?;
    encoded.extend_from_slice(&REPLICATION_KEY_PACKAGE_VERSION.to_be_bytes());
    encoded.push(suite.id());
    encoded.extend_from_slice(&epoch.get().to_be_bytes());
    push_field(&mut encoded, context.workspace_id.as_str().as_bytes())?;
    push_field(&mut encoded, context.recipient.as_str().as_bytes())?;
    push_field(&mut encoded, authority_public)?;
    push_field(&mut encoded, recipient_public)?;
    Ok(encoded)
}

fn push_field(target: &mut Vec<u8>, field: &[u8]) -> Result<(), ReplicationKeyWrappingError> {
    let length =
        u16::try_from(field.len()).map_err(|_| ReplicationKeyWrappingError::InvalidContext)?;
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(field);
    Ok(())
}

struct FixedEntropyRng {
    bytes: Zeroizing<[u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES]>,
    offset: usize,
}

impl FixedEntropyRng {
    fn new(bytes: [u8; REPLICATION_HPKE_PRIVATE_KEY_BYTES]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes),
            offset: 0,
        }
    }

    fn take(&mut self, destination: &mut [u8]) {
        let end = self
            .offset
            .checked_add(destination.len())
            .expect("HPKE entropy request length must not overflow");
        assert!(
            end <= self.bytes.len(),
            "pinned X25519 HPKE must consume at most 32 bytes of ephemeral entropy"
        );
        destination.copy_from_slice(&self.bytes[self.offset..end]);
        self.bytes[self.offset..end].zeroize();
        self.offset = end;
    }
}

impl RngCore for FixedEntropyRng {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0_u8; 4];
        self.take(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0_u8; 8];
        self.take(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    fn fill_bytes(&mut self, destination: &mut [u8]) {
        self.take(destination);
    }
}

impl CryptoRng for FixedEntropyRng {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplicationEntropyError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationKeyWrappingError {
    InvalidPrivateKey,
    InvalidPublicKey,
    InvalidContext,
    InvalidKeyEpoch,
    RandomUnavailable,
    SealFailed,
    AuthenticationFailed,
    KeyEpochMismatch,
    UnsupportedVersion,
    UnsupportedSuite,
    MalformedPackage,
    PackageTooLarge,
}

impl fmt::Display for ReplicationKeyWrappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidPrivateKey => "replication wrapping private key is invalid",
            Self::InvalidPublicKey => "replication wrapping public key is invalid",
            Self::InvalidContext => "replication key wrapping context is invalid",
            Self::InvalidKeyEpoch => "replication key epoch is invalid",
            Self::RandomUnavailable => "secure randomness is unavailable",
            Self::SealFailed => "replication epoch-key wrapping failed",
            Self::AuthenticationFailed => "replication epoch-key authentication failed",
            Self::KeyEpochMismatch => "replication key epoch does not match the package",
            Self::UnsupportedVersion => "replication key package version is unsupported",
            Self::UnsupportedSuite => "replication key wrapping suite is unsupported",
            Self::MalformedPackage => "replication key package is malformed",
            Self::PackageTooLarge => "replication key package exceeds its byte limit",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ReplicationKeyWrappingError {}
