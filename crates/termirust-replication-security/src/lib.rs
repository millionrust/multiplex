use std::fmt;

use aes_gcm_siv::aead::{Aead, Payload};
use aes_gcm_siv::{Aes256GcmSiv, KeyInit, Nonce};
use hkdf::Hkdf;
use rand_core::{CryptoRng, OsRng, RngCore};
use sha2::Sha256;
use termirust_domain::{
    MAX_REPLICATION_SEALED_PAYLOAD_BYTES, ReplicationRecordKey, ReplicationReplicaId,
    ReplicationVersionVector, ReplicationWorkspaceId, SealedReplicationPayload,
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const REPLICATION_ENVELOPE_VERSION: u16 = 1;
pub const REPLICATION_NONCE_BYTES: usize = 12;
pub const REPLICATION_AUTH_TAG_BYTES: usize = 16;

const ENVELOPE_MAGIC: &[u8; 4] = b"TRRS";
const ENVELOPE_HEADER_BYTES: usize = ENVELOPE_MAGIC.len() + 2 + 1 + 8 + REPLICATION_NONCE_BYTES + 4;
pub const MAX_REPLICATION_PLAINTEXT_BYTES: usize =
    MAX_REPLICATION_SEALED_PAYLOAD_BYTES - ENVELOPE_HEADER_BYTES - REPLICATION_AUTH_TAG_BYTES;

const HKDF_SALT: &[u8] = b"termirust.replication.hkdf-sha256.v1";
const KEY_INFO_DOMAIN: &[u8] = b"termirust.replication.record-key.v1";
const AAD_DOMAIN: &[u8] = b"termirust.replication.record-envelope.v1";

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplicationKeyEpoch(u64);

impl ReplicationKeyEpoch {
    pub fn new(value: u64) -> Result<Self, ReplicationCryptoError> {
        if value == 0 {
            return Err(ReplicationCryptoError::InvalidKeyEpoch);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for ReplicationKeyEpoch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ReplicationKeyEpoch")
            .field(&self.0)
            .finish()
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ReplicationEpochKey {
    #[zeroize(skip)]
    epoch: ReplicationKeyEpoch,
    bytes: [u8; 32],
}

impl ReplicationEpochKey {
    pub fn from_bytes(
        epoch: ReplicationKeyEpoch,
        bytes: [u8; 32],
    ) -> Result<Self, ReplicationCryptoError> {
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(ReplicationCryptoError::InvalidKeyMaterial);
        }
        Ok(Self { epoch, bytes })
    }

    pub fn epoch(&self) -> ReplicationKeyEpoch {
        self.epoch
    }
}

impl fmt::Debug for ReplicationEpochKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationEpochKey")
            .field("epoch", &self.epoch)
            .field("key", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ReplicationCipherSuite {
    Aes256GcmSivHkdfSha256 = 1,
}

impl ReplicationCipherSuite {
    fn from_id(id: u8) -> Result<Self, ReplicationCryptoError> {
        match id {
            1 => Ok(Self::Aes256GcmSivHkdfSha256),
            _ => Err(ReplicationCryptoError::UnsupportedCipherSuite),
        }
    }

    fn id(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ReplicationOperationKind {
    Put = 1,
    Delete = 2,
}

impl ReplicationOperationKind {
    fn id(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy)]
pub struct ReplicationSealContext<'a> {
    pub workspace_id: &'a ReplicationWorkspaceId,
    pub record_key: &'a ReplicationRecordKey,
    pub author: &'a ReplicationReplicaId,
    pub vector: &'a ReplicationVersionVector,
    pub operation: ReplicationOperationKind,
}

impl fmt::Debug for ReplicationSealContext<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationSealContext")
            .field("operation", &self.operation)
            .field("vector_entries", &self.vector.len())
            .finish_non_exhaustive()
    }
}

impl ReplicationSealContext<'_> {
    fn validate(&self) -> Result<(), ReplicationCryptoError> {
        self.workspace_id
            .validate()
            .map_err(|_| ReplicationCryptoError::InvalidContext)?;
        self.record_key
            .validate()
            .map_err(|_| ReplicationCryptoError::InvalidContext)?;
        self.author
            .validate()
            .map_err(|_| ReplicationCryptoError::InvalidContext)?;
        self.vector
            .validate()
            .map_err(|_| ReplicationCryptoError::InvalidContext)?;
        if self.vector.counter(self.author) == 0 {
            return Err(ReplicationCryptoError::InvalidContext);
        }
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationEnvelope {
    version: u16,
    cipher_suite: ReplicationCipherSuite,
    key_epoch: ReplicationKeyEpoch,
    nonce: [u8; REPLICATION_NONCE_BYTES],
    ciphertext: Vec<u8>,
}

impl ReplicationEnvelope {
    pub fn version(&self) -> u16 {
        self.version
    }

    pub fn cipher_suite(&self) -> ReplicationCipherSuite {
        self.cipher_suite
    }

    pub fn key_epoch(&self) -> ReplicationKeyEpoch {
        self.key_epoch
    }

    pub fn ciphertext_len(&self) -> usize {
        self.ciphertext.len()
    }

    pub fn encoded_len(&self) -> usize {
        ENVELOPE_HEADER_BYTES + self.ciphertext.len()
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ReplicationCryptoError> {
        validate_ciphertext_len(self.ciphertext.len())?;
        let ciphertext_len = u32::try_from(self.ciphertext.len())
            .map_err(|_| ReplicationCryptoError::EnvelopeTooLarge)?;
        let mut encoded = Vec::with_capacity(self.encoded_len());
        encoded.extend_from_slice(ENVELOPE_MAGIC);
        encoded.extend_from_slice(&self.version.to_be_bytes());
        encoded.push(self.cipher_suite.id());
        encoded.extend_from_slice(&self.key_epoch.get().to_be_bytes());
        encoded.extend_from_slice(&self.nonce);
        encoded.extend_from_slice(&ciphertext_len.to_be_bytes());
        encoded.extend_from_slice(&self.ciphertext);
        if encoded.len() > MAX_REPLICATION_SEALED_PAYLOAD_BYTES {
            return Err(ReplicationCryptoError::EnvelopeTooLarge);
        }
        Ok(encoded)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ReplicationCryptoError> {
        if bytes.len() > MAX_REPLICATION_SEALED_PAYLOAD_BYTES {
            return Err(ReplicationCryptoError::EnvelopeTooLarge);
        }
        if bytes.len() < ENVELOPE_HEADER_BYTES + REPLICATION_AUTH_TAG_BYTES {
            return Err(ReplicationCryptoError::MalformedEnvelope);
        }
        if bytes.get(..ENVELOPE_MAGIC.len()) != Some(ENVELOPE_MAGIC) {
            return Err(ReplicationCryptoError::MalformedEnvelope);
        }

        let version = u16::from_be_bytes([bytes[4], bytes[5]]);
        if version != REPLICATION_ENVELOPE_VERSION {
            return Err(ReplicationCryptoError::UnsupportedEnvelopeVersion);
        }
        let cipher_suite = ReplicationCipherSuite::from_id(bytes[6])?;
        let key_epoch = ReplicationKeyEpoch::new(u64::from_be_bytes(
            bytes[7..15]
                .try_into()
                .map_err(|_| ReplicationCryptoError::MalformedEnvelope)?,
        ))?;
        let nonce = bytes[15..27]
            .try_into()
            .map_err(|_| ReplicationCryptoError::MalformedEnvelope)?;
        let ciphertext_len = u32::from_be_bytes(
            bytes[27..31]
                .try_into()
                .map_err(|_| ReplicationCryptoError::MalformedEnvelope)?,
        ) as usize;
        validate_ciphertext_len(ciphertext_len)?;
        let expected_len = ENVELOPE_HEADER_BYTES
            .checked_add(ciphertext_len)
            .ok_or(ReplicationCryptoError::EnvelopeTooLarge)?;
        if bytes.len() != expected_len {
            return Err(ReplicationCryptoError::MalformedEnvelope);
        }

        Ok(Self {
            version,
            cipher_suite,
            key_epoch,
            nonce,
            ciphertext: bytes[ENVELOPE_HEADER_BYTES..].to_vec(),
        })
    }

    pub fn to_sealed_payload(&self) -> Result<SealedReplicationPayload, ReplicationCryptoError> {
        SealedReplicationPayload::new(self.to_bytes()?)
            .map_err(|_| ReplicationCryptoError::DomainPayloadRejected)
    }

    pub fn from_sealed_payload(
        payload: &SealedReplicationPayload,
    ) -> Result<Self, ReplicationCryptoError> {
        Self::from_bytes(payload.as_bytes())
    }
}

impl fmt::Debug for ReplicationEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationEnvelope")
            .field("version", &self.version)
            .field("cipher_suite", &self.cipher_suite)
            .field("key_epoch", &self.key_epoch)
            .field("ciphertext_len", &self.ciphertext.len())
            .field("sealed_data", &"<redacted>")
            .finish()
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct OpenedReplicationPayload(Vec<u8>);

impl OpenedReplicationPayload {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for OpenedReplicationPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpenedReplicationPayload(<redacted>)")
    }
}

pub enum OpenedReplicationOperation {
    Put(OpenedReplicationPayload),
    Delete,
}

impl fmt::Debug for OpenedReplicationOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Put(_) => formatter.write_str("OpenedReplicationOperation::Put(<redacted>)"),
            Self::Delete => formatter.write_str("OpenedReplicationOperation::Delete"),
        }
    }
}

pub fn seal_put(
    context: ReplicationSealContext<'_>,
    key: &ReplicationEpochKey,
    plaintext: &[u8],
) -> Result<ReplicationEnvelope, ReplicationCryptoError> {
    seal_put_with_rng(context, key, plaintext, &mut OsRng)
}

pub fn seal_put_with_rng<R: CryptoRng + RngCore>(
    context: ReplicationSealContext<'_>,
    key: &ReplicationEpochKey,
    plaintext: &[u8],
    rng: &mut R,
) -> Result<ReplicationEnvelope, ReplicationCryptoError> {
    if context.operation != ReplicationOperationKind::Put {
        return Err(ReplicationCryptoError::OperationMismatch);
    }
    if plaintext.is_empty() {
        return Err(ReplicationCryptoError::EmptyPutPayload);
    }
    if plaintext.len() > MAX_REPLICATION_PLAINTEXT_BYTES {
        return Err(ReplicationCryptoError::PlaintextTooLarge);
    }
    seal(context, key, plaintext, rng)
}

pub fn seal_delete(
    context: ReplicationSealContext<'_>,
    key: &ReplicationEpochKey,
) -> Result<ReplicationEnvelope, ReplicationCryptoError> {
    seal_delete_with_rng(context, key, &mut OsRng)
}

pub fn seal_delete_with_rng<R: CryptoRng + RngCore>(
    context: ReplicationSealContext<'_>,
    key: &ReplicationEpochKey,
    rng: &mut R,
) -> Result<ReplicationEnvelope, ReplicationCryptoError> {
    if context.operation != ReplicationOperationKind::Delete {
        return Err(ReplicationCryptoError::OperationMismatch);
    }
    seal(context, key, &[], rng)
}

pub fn open(
    context: ReplicationSealContext<'_>,
    key: &ReplicationEpochKey,
    envelope: &ReplicationEnvelope,
) -> Result<OpenedReplicationOperation, ReplicationCryptoError> {
    context.validate()?;
    if envelope.version != REPLICATION_ENVELOPE_VERSION {
        return Err(ReplicationCryptoError::UnsupportedEnvelopeVersion);
    }
    if envelope.cipher_suite != ReplicationCipherSuite::Aes256GcmSivHkdfSha256 {
        return Err(ReplicationCryptoError::UnsupportedCipherSuite);
    }
    if envelope.key_epoch != key.epoch {
        return Err(ReplicationCryptoError::KeyEpochMismatch);
    }
    validate_ciphertext_len(envelope.ciphertext.len())?;
    match context.operation {
        ReplicationOperationKind::Put
            if envelope.ciphertext.len() == REPLICATION_AUTH_TAG_BYTES =>
        {
            return Err(ReplicationCryptoError::AuthenticationFailed);
        }
        ReplicationOperationKind::Delete
            if envelope.ciphertext.len() != REPLICATION_AUTH_TAG_BYTES =>
        {
            return Err(ReplicationCryptoError::AuthenticationFailed);
        }
        _ => {}
    }

    let derived_key = derive_record_key(context, key)?;
    let cipher = Aes256GcmSiv::new_from_slice(derived_key.as_ref())
        .map_err(|_| ReplicationCryptoError::CipherInitializationFailed)?;
    let aad = encode_aad(context, envelope.key_epoch, envelope.cipher_suite)?;
    let mut plaintext = cipher
        .decrypt(
            Nonce::from_slice(&envelope.nonce),
            Payload {
                msg: &envelope.ciphertext,
                aad: aad.as_ref(),
            },
        )
        .map_err(|_| ReplicationCryptoError::AuthenticationFailed)?;

    match context.operation {
        ReplicationOperationKind::Put => {
            if plaintext.is_empty() || plaintext.len() > MAX_REPLICATION_PLAINTEXT_BYTES {
                plaintext.zeroize();
                return Err(ReplicationCryptoError::AuthenticationFailed);
            }
            Ok(OpenedReplicationOperation::Put(OpenedReplicationPayload(
                plaintext,
            )))
        }
        ReplicationOperationKind::Delete => {
            if !plaintext.is_empty() {
                plaintext.zeroize();
                return Err(ReplicationCryptoError::AuthenticationFailed);
            }
            plaintext.zeroize();
            Ok(OpenedReplicationOperation::Delete)
        }
    }
}

fn seal<R: CryptoRng + RngCore>(
    context: ReplicationSealContext<'_>,
    key: &ReplicationEpochKey,
    plaintext: &[u8],
    rng: &mut R,
) -> Result<ReplicationEnvelope, ReplicationCryptoError> {
    context.validate()?;
    let mut nonce = [0_u8; REPLICATION_NONCE_BYTES];
    rng.try_fill_bytes(&mut nonce)
        .map_err(|_| ReplicationCryptoError::RandomUnavailable)?;

    let suite = ReplicationCipherSuite::Aes256GcmSivHkdfSha256;
    let derived_key = derive_record_key(context, key)?;
    let cipher = Aes256GcmSiv::new_from_slice(derived_key.as_ref())
        .map_err(|_| ReplicationCryptoError::CipherInitializationFailed)?;
    let aad = encode_aad(context, key.epoch, suite)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: aad.as_ref(),
            },
        )
        .map_err(|_| ReplicationCryptoError::EncryptionFailed)?;
    validate_ciphertext_len(ciphertext.len())?;
    if ENVELOPE_HEADER_BYTES + ciphertext.len() > MAX_REPLICATION_SEALED_PAYLOAD_BYTES {
        return Err(ReplicationCryptoError::EnvelopeTooLarge);
    }

    Ok(ReplicationEnvelope {
        version: REPLICATION_ENVELOPE_VERSION,
        cipher_suite: suite,
        key_epoch: key.epoch,
        nonce,
        ciphertext,
    })
}

fn derive_record_key(
    context: ReplicationSealContext<'_>,
    key: &ReplicationEpochKey,
) -> Result<Zeroizing<[u8; 32]>, ReplicationCryptoError> {
    let info = encode_key_info(context, key.epoch)?;
    let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), &key.bytes);
    let mut derived = Zeroizing::new([0_u8; 32]);
    hkdf.expand(info.as_ref(), derived.as_mut())
        .map_err(|_| ReplicationCryptoError::KeyDerivationFailed)?;
    Ok(derived)
}

fn encode_key_info(
    context: ReplicationSealContext<'_>,
    epoch: ReplicationKeyEpoch,
) -> Result<Zeroizing<Vec<u8>>, ReplicationCryptoError> {
    let mut info = Zeroizing::new(Vec::with_capacity(384));
    push_field(&mut info, KEY_INFO_DOMAIN)?;
    info.extend_from_slice(&REPLICATION_ENVELOPE_VERSION.to_be_bytes());
    info.extend_from_slice(&epoch.get().to_be_bytes());
    push_field(&mut info, context.workspace_id.as_str().as_bytes())?;
    push_field(&mut info, context.record_key.collection.as_str().as_bytes())?;
    push_field(&mut info, context.record_key.record_id.as_str().as_bytes())?;
    Ok(info)
}

fn encode_aad(
    context: ReplicationSealContext<'_>,
    epoch: ReplicationKeyEpoch,
    cipher_suite: ReplicationCipherSuite,
) -> Result<Zeroizing<Vec<u8>>, ReplicationCryptoError> {
    let mut aad = Zeroizing::new(Vec::with_capacity(768));
    push_field(&mut aad, AAD_DOMAIN)?;
    aad.extend_from_slice(&REPLICATION_ENVELOPE_VERSION.to_be_bytes());
    aad.push(cipher_suite.id());
    aad.extend_from_slice(&epoch.get().to_be_bytes());
    aad.push(context.operation.id());
    push_field(&mut aad, context.workspace_id.as_str().as_bytes())?;
    push_field(&mut aad, context.record_key.collection.as_str().as_bytes())?;
    push_field(&mut aad, context.record_key.record_id.as_str().as_bytes())?;
    push_field(&mut aad, context.author.as_str().as_bytes())?;
    let vector_len =
        u16::try_from(context.vector.len()).map_err(|_| ReplicationCryptoError::InvalidContext)?;
    aad.extend_from_slice(&vector_len.to_be_bytes());
    for (replica_id, counter) in context.vector.iter() {
        push_field(&mut aad, replica_id.as_str().as_bytes())?;
        aad.extend_from_slice(&counter.to_be_bytes());
    }
    Ok(aad)
}

fn push_field(target: &mut Vec<u8>, field: &[u8]) -> Result<(), ReplicationCryptoError> {
    let field_len =
        u16::try_from(field.len()).map_err(|_| ReplicationCryptoError::InvalidContext)?;
    target.extend_from_slice(&field_len.to_be_bytes());
    target.extend_from_slice(field);
    Ok(())
}

fn validate_ciphertext_len(ciphertext_len: usize) -> Result<(), ReplicationCryptoError> {
    if ciphertext_len < REPLICATION_AUTH_TAG_BYTES {
        return Err(ReplicationCryptoError::MalformedEnvelope);
    }
    if ciphertext_len > MAX_REPLICATION_SEALED_PAYLOAD_BYTES - ENVELOPE_HEADER_BYTES {
        return Err(ReplicationCryptoError::EnvelopeTooLarge);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationCryptoError {
    InvalidKeyEpoch,
    InvalidKeyMaterial,
    InvalidContext,
    EmptyPutPayload,
    PlaintextTooLarge,
    OperationMismatch,
    RandomUnavailable,
    KeyDerivationFailed,
    CipherInitializationFailed,
    EncryptionFailed,
    AuthenticationFailed,
    KeyEpochMismatch,
    UnsupportedEnvelopeVersion,
    UnsupportedCipherSuite,
    MalformedEnvelope,
    EnvelopeTooLarge,
    DomainPayloadRejected,
}

impl fmt::Display for ReplicationCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidKeyEpoch => "replication key epoch is invalid",
            Self::InvalidKeyMaterial => "replication key material is invalid",
            Self::InvalidContext => "replication sealing context is invalid",
            Self::EmptyPutPayload => "replication put payload is empty",
            Self::PlaintextTooLarge => "replication plaintext exceeds its byte limit",
            Self::OperationMismatch => {
                "replication operation does not match its sealing entry point"
            }
            Self::RandomUnavailable => "secure randomness is unavailable",
            Self::KeyDerivationFailed => "replication record-key derivation failed",
            Self::CipherInitializationFailed => "replication cipher initialization failed",
            Self::EncryptionFailed => "replication record encryption failed",
            Self::AuthenticationFailed => "replication record authentication failed",
            Self::KeyEpochMismatch => "replication key epoch does not match the envelope",
            Self::UnsupportedEnvelopeVersion => "replication envelope version is unsupported",
            Self::UnsupportedCipherSuite => "replication cipher suite is unsupported",
            Self::MalformedEnvelope => "replication envelope is malformed",
            Self::EnvelopeTooLarge => "replication envelope exceeds its byte limit",
            Self::DomainPayloadRejected => "replication envelope cannot enter the domain payload",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ReplicationCryptoError {}
