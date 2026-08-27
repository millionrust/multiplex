//! Narrow generated boundary for Controller-v1 wire, crypto, and authorization.
//!
//! This crate deliberately owns no transport, retry loop, runtime, filesystem, terminal,
//! application lifecycle, or user interface.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard};

use termirust_controller_security::{
    AuthorizationDecision as CoreAuthorizationDecision, AuthorizationPolicy, CONTROLLER_V1,
    ControllerCapability as CoreCapability, ControllerFrameKind as CoreFrameKind,
    ControllerSecurityError, ControllerTransport, ErrorCode, MAX_CONTROL_PAYLOAD_BYTES,
    MAX_TERMINAL_FRAME_BYTES, PairingMachine, RevocationEpoch, StaticPrivateKey, decode_offer,
};
use zeroize::Zeroize;

const PRIVATE_KEY_BYTES: usize = 32;
const MAX_OPAQUE_KEY_ID_BYTES: usize = 128;
const MAX_SECURE_BLOB_BYTES: usize = 4 * 1024;
const MAX_HANDSHAKE_MESSAGE_BYTES: usize = 256;

uniffi::setup_scaffolding!();

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum ControllerCapability {
    ObserveSessions,
    AttachOutput,
    SendInput,
    Resize,
    RespondToApproval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum ControllerFrameKind {
    Control,
    Terminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum PairingRole {
    DeviceInitiator,
    HostResponder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum SasSymbolKind {
    Letter,
    Digit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum AuthorizationDecision {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum SecureBlobStatus {
    Present,
    Missing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum PairingConfirmation {
    Confirm,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct PublicOfferSummary {
    pub version: ProtocolVersion,
    pub expires_at_unix_seconds: u64,
    pub host_static_public_key: Vec<u8>,
    pub capability_bits: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct PairingStartRequest {
    pub role: PairingRole,
    pub offer_bytes: Vec<u8>,
    pub static_key_id: String,
    pub ephemeral_private_key: Vec<u8>,
    pub now_millis: u64,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct SasDisplay {
    pub value: String,
    pub symbol_kinds: Vec<SasSymbolKind>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct PairingPublicResult {
    pub host_static_public_key: Vec<u8>,
    pub device_static_public_key: Vec<u8>,
    pub capability_bits: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct OpenedControllerFrame {
    pub kind: ControllerFrameKind,
    pub capability: ControllerCapability,
    pub revocation_epoch: u64,
    pub sequence: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Error)]
pub enum SecureBlobError {
    Missing,
    Locked,
    PermissionDenied,
    Invalid,
    Unavailable,
}

impl fmt::Display for SecureBlobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "secure_blob_missing",
            Self::Locked => "secure_blob_locked",
            Self::PermissionDenied => "secure_blob_permission_denied",
            Self::Invalid => "secure_blob_invalid",
            Self::Unavailable => "secure_blob_unavailable",
        })
    }
}

impl std::error::Error for SecureBlobError {}

impl From<uniffi::UnexpectedUniFFICallbackError> for SecureBlobError {
    fn from(_: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Unavailable
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Error)]
pub enum ControllerBindingError {
    InvalidEncoding,
    IncompatibleVersion,
    UnknownCapability,
    CapabilityDenied,
    WrongState,
    WrongKey,
    Expired,
    TimedOut,
    Rejected,
    Cancelled,
    SasMismatch,
    DuplicateFrame,
    OutOfOrderFrame,
    FrameTooLarge,
    SequenceExhausted,
    AuthenticationFailed,
    CryptoFailure,
    SecureBlobMissing,
    SecureBlobLocked,
    SecureBlobPermissionDenied,
    SecureBlobInvalid,
    SecureBlobUnavailable,
    Disposed,
    Unexpected,
}

impl fmt::Display for ControllerBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ControllerBindingError {}

impl ControllerBindingError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidEncoding => "invalid_encoding",
            Self::IncompatibleVersion => "incompatible_version",
            Self::UnknownCapability => "unknown_capability",
            Self::CapabilityDenied => "capability_denied",
            Self::WrongState => "wrong_state",
            Self::WrongKey => "wrong_key",
            Self::Expired => "expired",
            Self::TimedOut => "timed_out",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::SasMismatch => "sas_mismatch",
            Self::DuplicateFrame => "duplicate_frame",
            Self::OutOfOrderFrame => "out_of_order_frame",
            Self::FrameTooLarge => "frame_too_large",
            Self::SequenceExhausted => "sequence_exhausted",
            Self::AuthenticationFailed => "authentication_failed",
            Self::CryptoFailure => "crypto_failure",
            Self::SecureBlobMissing => "secure_blob_missing",
            Self::SecureBlobLocked => "secure_blob_locked",
            Self::SecureBlobPermissionDenied => "secure_blob_permission_denied",
            Self::SecureBlobInvalid => "secure_blob_invalid",
            Self::SecureBlobUnavailable => "secure_blob_unavailable",
            Self::Disposed => "disposed",
            Self::Unexpected => "unexpected",
        }
    }
}

#[uniffi::export(foreign)]
pub trait SecureBlobStore: Send + Sync {
    fn load(&self, key_id: String) -> Result<Option<Vec<u8>>, SecureBlobError>;
    fn store(&self, key_id: String, value: Vec<u8>) -> Result<(), SecureBlobError>;
    fn delete(&self, key_id: String) -> Result<(), SecureBlobError>;
}

#[derive(uniffi::Object)]
pub struct ControllerSecurityEngine {
    blobs: Arc<dyn SecureBlobStore>,
}

#[uniffi::export]
impl ControllerSecurityEngine {
    #[uniffi::constructor]
    pub fn new(blobs: Arc<dyn SecureBlobStore>) -> Result<Arc<Self>, ControllerBindingError> {
        boundary(|| Ok(Arc::new(Self { blobs })))
    }

    pub fn protocol_version(&self) -> Result<ProtocolVersion, ControllerBindingError> {
        boundary(|| {
            Ok(ProtocolVersion {
                major: CONTROLLER_V1.major,
                minor: CONTROLLER_V1.minor,
            })
        })
    }

    pub fn decode_offer_summary(
        &self,
        offer_bytes: Vec<u8>,
    ) -> Result<PublicOfferSummary, ControllerBindingError> {
        boundary(|| {
            let offer = decode_offer(&offer_bytes).map_err(ControllerBindingError::from)?;
            Ok(PublicOfferSummary {
                version: ProtocolVersion {
                    major: offer.version.major,
                    minor: offer.version.minor,
                },
                expires_at_unix_seconds: offer.expires_at_unix_seconds,
                host_static_public_key: offer.host_static_public_key.0.to_vec(),
                capability_bits: offer.capabilities.bits(),
            })
        })
    }

    pub fn secure_blob_status(
        &self,
        key_id: String,
    ) -> Result<SecureBlobStatus, ControllerBindingError> {
        boundary(|| {
            validate_key_id(&key_id)?;
            let mut value = self
                .blobs
                .load(key_id)
                .map_err(ControllerBindingError::from)?;
            let status = if value.is_some() {
                SecureBlobStatus::Present
            } else {
                SecureBlobStatus::Missing
            };
            if let Some(value) = value.as_mut() {
                value.zeroize();
            }
            Ok(status)
        })
    }

    pub fn store_secure_blob(
        &self,
        key_id: String,
        mut value: Vec<u8>,
    ) -> Result<(), ControllerBindingError> {
        boundary(|| {
            validate_key_id(&key_id)?;
            if value.is_empty() || value.len() > MAX_SECURE_BLOB_BYTES {
                value.zeroize();
                return Err(ControllerBindingError::SecureBlobInvalid);
            }
            let result = self
                .blobs
                .store(key_id, value.clone())
                .map_err(ControllerBindingError::from);
            value.zeroize();
            result
        })
    }

    pub fn delete_secure_blob(&self, key_id: String) -> Result<(), ControllerBindingError> {
        boundary(|| {
            validate_key_id(&key_id)?;
            self.blobs
                .delete(key_id)
                .map_err(ControllerBindingError::from)
        })
    }

    pub fn pairing_start(
        &self,
        mut request: PairingStartRequest,
    ) -> Result<Arc<ControllerPairingSession>, ControllerBindingError> {
        boundary(|| {
            validate_key_id(&request.static_key_id)?;
            if request.offer_bytes.len() != termirust_controller_security::PAIRING_OFFER_BYTES {
                request.ephemeral_private_key.zeroize();
                return Err(ControllerBindingError::InvalidEncoding);
            }
            let offer = decode_offer(&request.offer_bytes).map_err(ControllerBindingError::from)?;
            let static_private = self.load_private_key(request.static_key_id)?;
            let ephemeral_private = take_private_key(&mut request.ephemeral_private_key)?;
            let machine = match request.role {
                PairingRole::DeviceInitiator => PairingMachine::new_device_initiator(
                    offer,
                    static_private,
                    ephemeral_private,
                    request.now_millis,
                    request.now_unix_seconds,
                ),
                PairingRole::HostResponder => PairingMachine::new_host_responder(
                    offer,
                    static_private,
                    ephemeral_private,
                    request.now_millis,
                    request.now_unix_seconds,
                ),
            }
            .map_err(ControllerBindingError::from)?;
            Ok(Arc::new(ControllerPairingSession {
                inner: Mutex::new(SessionInner {
                    machine: Some(machine),
                    transport: None,
                    policy: None,
                    closed: false,
                }),
            }))
        })
    }
}

impl ControllerSecurityEngine {
    fn load_private_key(&self, key_id: String) -> Result<StaticPrivateKey, ControllerBindingError> {
        let mut value = self
            .blobs
            .load(key_id)
            .map_err(ControllerBindingError::from)?
            .ok_or(ControllerBindingError::SecureBlobMissing)?;
        let result = take_private_key(&mut value);
        value.zeroize();
        result
    }
}

#[derive(uniffi::Object)]
pub struct ControllerPairingSession {
    inner: Mutex<SessionInner>,
}

struct SessionInner {
    machine: Option<PairingMachine>,
    transport: Option<ControllerTransport>,
    policy: Option<AuthorizationPolicy>,
    closed: bool,
}

#[uniffi::export]
impl ControllerPairingSession {
    pub fn pairing_outbound(&self, now_millis: u64) -> Result<Vec<u8>, ControllerBindingError> {
        boundary(|| {
            let mut inner = self.lock()?;
            ensure_open(&inner)?;
            inner
                .machine
                .as_mut()
                .ok_or(ControllerBindingError::WrongState)?
                .write_next(now_millis)
                .map(|message| message.as_bytes().to_vec())
                .map_err(ControllerBindingError::from)
        })
    }

    pub fn pairing_receive(
        &self,
        message: Vec<u8>,
        now_millis: u64,
    ) -> Result<(), ControllerBindingError> {
        boundary(|| {
            if message.len() > MAX_HANDSHAKE_MESSAGE_BYTES {
                return Err(ControllerBindingError::FrameTooLarge);
            }
            let mut inner = self.lock()?;
            ensure_open(&inner)?;
            inner
                .machine
                .as_mut()
                .ok_or(ControllerBindingError::WrongState)?
                .read_next(&message, now_millis)
                .map_err(ControllerBindingError::from)
        })
    }

    pub fn sas(&self) -> Result<SasDisplay, ControllerBindingError> {
        boundary(|| {
            let inner = self.lock()?;
            ensure_open(&inner)?;
            let sas = inner
                .machine
                .as_ref()
                .and_then(PairingMachine::sas)
                .ok_or(ControllerBindingError::WrongState)?;
            Ok(SasDisplay {
                value: sas.as_str().to_owned(),
                symbol_kinds: sas
                    .accessibility_symbols()
                    .into_iter()
                    .map(|kind| {
                        if kind == "digit" {
                            SasSymbolKind::Digit
                        } else {
                            SasSymbolKind::Letter
                        }
                    })
                    .collect(),
            })
        })
    }

    pub fn handshake_hash(&self) -> Result<Vec<u8>, ControllerBindingError> {
        boundary(|| {
            let inner = self.lock()?;
            ensure_open(&inner)?;
            inner
                .machine
                .as_ref()
                .and_then(PairingMachine::handshake_hash)
                .map(|hash| hash.0.to_vec())
                .ok_or(ControllerBindingError::WrongState)
        })
    }

    pub fn confirm_or_reject(
        &self,
        confirmation: PairingConfirmation,
        compared_sas: String,
        revocation_epoch: u64,
    ) -> Result<PairingPublicResult, ControllerBindingError> {
        boundary(|| {
            let mut inner = self.lock()?;
            ensure_open(&inner)?;
            let machine = inner
                .machine
                .take()
                .ok_or(ControllerBindingError::WrongState)?;
            if confirmation == PairingConfirmation::Reject {
                let _ = machine.reject();
                return Err(ControllerBindingError::Rejected);
            }
            let actual = machine
                .sas()
                .cloned()
                .ok_or(ControllerBindingError::WrongState)?;
            if compared_sas.as_bytes() != actual.as_str().as_bytes() {
                let _ = machine.reject();
                return Err(ControllerBindingError::SasMismatch);
            }
            let confirmed = machine
                .confirm(&actual, RevocationEpoch(revocation_epoch))
                .map_err(ControllerBindingError::from)?;
            let result = PairingPublicResult {
                host_static_public_key: confirmed.host_key.0.to_vec(),
                device_static_public_key: confirmed.device_key.0.to_vec(),
                capability_bits: confirmed.capabilities.bits(),
            };
            inner.policy = Some(AuthorizationPolicy::new(
                confirmed.capabilities,
                RevocationEpoch(revocation_epoch),
            ));
            inner.transport = Some(confirmed.transport);
            Ok(result)
        })
    }

    pub fn seal_frame(
        &self,
        kind: ControllerFrameKind,
        capability: ControllerCapability,
        revocation_epoch: u64,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, ControllerBindingError> {
        boundary(|| {
            validate_payload_size(kind, payload.len())?;
            let mut inner = self.lock()?;
            ensure_open(&inner)?;
            inner
                .transport
                .as_mut()
                .ok_or(ControllerBindingError::WrongState)?
                .seal(
                    kind.into(),
                    capability.into(),
                    RevocationEpoch(revocation_epoch),
                    &payload,
                )
                .map(|frame| frame.as_bytes().to_vec())
                .map_err(ControllerBindingError::from)
        })
    }

    pub fn open_frame(
        &self,
        frame: Vec<u8>,
    ) -> Result<OpenedControllerFrame, ControllerBindingError> {
        boundary(|| {
            if frame.len() > MAX_TERMINAL_FRAME_BYTES {
                return Err(ControllerBindingError::FrameTooLarge);
            }
            let mut inner = self.lock()?;
            ensure_open(&inner)?;
            let opened = inner
                .transport
                .as_mut()
                .ok_or(ControllerBindingError::WrongState)?
                .open(&frame)
                .map_err(ControllerBindingError::from)?;
            Ok(OpenedControllerFrame {
                kind: opened.kind.into(),
                capability: opened.capability.into(),
                revocation_epoch: opened.revocation_epoch.0,
                sequence: opened.sequence,
                payload: opened.payload.clone(),
            })
        })
    }

    pub fn authorize(
        &self,
        capability: ControllerCapability,
        presented_revocation_epoch: u64,
    ) -> Result<AuthorizationDecision, ControllerBindingError> {
        boundary(|| {
            let inner = self.lock()?;
            ensure_open(&inner)?;
            let policy = inner.policy.ok_or(ControllerBindingError::WrongState)?;
            Ok(
                match policy.evaluate(
                    capability.into(),
                    RevocationEpoch(presented_revocation_epoch),
                ) {
                    CoreAuthorizationDecision::Allow => AuthorizationDecision::Allow,
                    CoreAuthorizationDecision::Deny(_) => AuthorizationDecision::Deny,
                },
            )
        })
    }

    pub fn cancel(&self) -> Result<(), ControllerBindingError> {
        boundary(|| {
            let mut inner = self.lock()?;
            ensure_open(&inner)?;
            if let Some(machine) = inner.machine.as_mut() {
                let _ = machine.cancel();
            }
            inner.machine = None;
            inner.transport = None;
            inner.policy = None;
            inner.closed = true;
            Ok(())
        })
    }

    pub fn finish(&self) -> Result<(), ControllerBindingError> {
        boundary(|| {
            let mut inner = self.lock()?;
            if !inner.closed {
                if let Some(machine) = inner.machine.as_mut() {
                    let _ = machine.cancel();
                }
                inner.machine = None;
                inner.transport = None;
                inner.policy = None;
                inner.closed = true;
            }
            Ok(())
        })
    }
}

impl ControllerPairingSession {
    fn lock(&self) -> Result<MutexGuard<'_, SessionInner>, ControllerBindingError> {
        self.inner
            .lock()
            .map_err(|_| ControllerBindingError::Unexpected)
    }
}

fn boundary<T>(
    operation: impl FnOnce() -> Result<T, ControllerBindingError>,
) -> Result<T, ControllerBindingError> {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or(Err(ControllerBindingError::Unexpected))
}

fn ensure_open(inner: &SessionInner) -> Result<(), ControllerBindingError> {
    if inner.closed {
        Err(ControllerBindingError::Disposed)
    } else {
        Ok(())
    }
}

fn validate_key_id(key_id: &str) -> Result<(), ControllerBindingError> {
    if key_id.is_empty()
        || key_id.len() > MAX_OPAQUE_KEY_ID_BYTES
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'-'))
    {
        Err(ControllerBindingError::SecureBlobInvalid)
    } else {
        Ok(())
    }
}

fn take_private_key(bytes: &mut Vec<u8>) -> Result<StaticPrivateKey, ControllerBindingError> {
    if bytes.len() != PRIVATE_KEY_BYTES {
        bytes.zeroize();
        return Err(ControllerBindingError::SecureBlobInvalid);
    }
    let mut key = [0_u8; PRIVATE_KEY_BYTES];
    key.copy_from_slice(bytes);
    bytes.zeroize();
    Ok(StaticPrivateKey::from_bytes(key))
}

fn validate_payload_size(
    kind: ControllerFrameKind,
    length: usize,
) -> Result<(), ControllerBindingError> {
    let allowed = match kind {
        ControllerFrameKind::Control => length <= MAX_CONTROL_PAYLOAD_BYTES,
        ControllerFrameKind::Terminal => length
            .checked_add(48)
            .is_some_and(|frame_length| frame_length <= MAX_TERMINAL_FRAME_BYTES),
    };
    if allowed {
        Ok(())
    } else {
        Err(ControllerBindingError::FrameTooLarge)
    }
}

impl From<SecureBlobError> for ControllerBindingError {
    fn from(error: SecureBlobError) -> Self {
        match error {
            SecureBlobError::Missing => Self::SecureBlobMissing,
            SecureBlobError::Locked => Self::SecureBlobLocked,
            SecureBlobError::PermissionDenied => Self::SecureBlobPermissionDenied,
            SecureBlobError::Invalid => Self::SecureBlobInvalid,
            SecureBlobError::Unavailable => Self::SecureBlobUnavailable,
        }
    }
}

impl From<ControllerSecurityError> for ControllerBindingError {
    fn from(error: ControllerSecurityError) -> Self {
        match error.code() {
            ErrorCode::IncompatibleVersion => Self::IncompatibleVersion,
            ErrorCode::UnknownCapability => Self::UnknownCapability,
            ErrorCode::CapabilityDenied => Self::CapabilityDenied,
            ErrorCode::WrongState => Self::WrongState,
            ErrorCode::WrongKey => Self::WrongKey,
            ErrorCode::Expired => Self::Expired,
            ErrorCode::TimedOut => Self::TimedOut,
            ErrorCode::Rejected => Self::Rejected,
            ErrorCode::Cancelled => Self::Cancelled,
            ErrorCode::SasMismatch => Self::SasMismatch,
            ErrorCode::DuplicateFrame => Self::DuplicateFrame,
            ErrorCode::OutOfOrderFrame => Self::OutOfOrderFrame,
            ErrorCode::FrameTooLarge => Self::FrameTooLarge,
            ErrorCode::SequenceExhausted => Self::SequenceExhausted,
            ErrorCode::AuthenticationFailed => Self::AuthenticationFailed,
            ErrorCode::CryptoFailure => Self::CryptoFailure,
            ErrorCode::InvalidMagic
            | ErrorCode::InvalidEncoding
            | ErrorCode::UnsupportedSuite
            | ErrorCode::InvalidRole
            | ErrorCode::InvalidStep
            | ErrorCode::WrongNonce => Self::InvalidEncoding,
            _ => Self::Unexpected,
        }
    }
}

impl From<ControllerCapability> for CoreCapability {
    fn from(capability: ControllerCapability) -> Self {
        match capability {
            ControllerCapability::ObserveSessions => Self::ObserveSessions,
            ControllerCapability::AttachOutput => Self::AttachOutput,
            ControllerCapability::SendInput => Self::SendInput,
            ControllerCapability::Resize => Self::Resize,
            ControllerCapability::RespondToApproval => Self::RespondToApproval,
        }
    }
}

impl From<CoreCapability> for ControllerCapability {
    fn from(capability: CoreCapability) -> Self {
        match capability {
            CoreCapability::ObserveSessions => Self::ObserveSessions,
            CoreCapability::AttachOutput => Self::AttachOutput,
            CoreCapability::SendInput => Self::SendInput,
            CoreCapability::Resize => Self::Resize,
            CoreCapability::RespondToApproval => Self::RespondToApproval,
        }
    }
}

impl From<ControllerFrameKind> for CoreFrameKind {
    fn from(kind: ControllerFrameKind) -> Self {
        match kind {
            ControllerFrameKind::Control => Self::Control,
            ControllerFrameKind::Terminal => Self::Terminal,
        }
    }
}

impl From<CoreFrameKind> for ControllerFrameKind {
    fn from(kind: CoreFrameKind) -> Self {
        match kind {
            CoreFrameKind::Control => Self::Control,
            CoreFrameKind::Terminal => Self::Terminal,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct MemoryBlobs(Mutex<HashMap<String, Vec<u8>>>);

    impl SecureBlobStore for MemoryBlobs {
        fn load(&self, key_id: String) -> Result<Option<Vec<u8>>, SecureBlobError> {
            Ok(self
                .0
                .lock()
                .map_err(|_| SecureBlobError::Unavailable)?
                .get(&key_id)
                .cloned())
        }

        fn store(&self, key_id: String, value: Vec<u8>) -> Result<(), SecureBlobError> {
            self.0
                .lock()
                .map_err(|_| SecureBlobError::Unavailable)?
                .insert(key_id, value);
            Ok(())
        }

        fn delete(&self, key_id: String) -> Result<(), SecureBlobError> {
            self.0
                .lock()
                .map_err(|_| SecureBlobError::Unavailable)?
                .remove(&key_id);
            Ok(())
        }
    }

    #[test]
    fn boundary_contains_panics_as_closed_error() {
        assert_eq!(
            boundary::<()>(|| panic!("ffi canary")),
            Err(ControllerBindingError::Unexpected)
        );
    }

    #[test]
    fn secure_blob_bridge_is_bounded_and_redacted() {
        let engine = ControllerSecurityEngine::new(Arc::new(MemoryBlobs::default()))
            .unwrap_or_else(|error| panic!("engine: {error}"));
        assert_eq!(
            engine.secure_blob_status("device:key".into()),
            Ok(SecureBlobStatus::Missing)
        );
        assert!(
            engine
                .store_secure_blob("device:key".into(), vec![7; 32])
                .is_ok()
        );
        assert_eq!(
            engine.secure_blob_status("device:key".into()),
            Ok(SecureBlobStatus::Present)
        );
        assert_eq!(
            engine.store_secure_blob("device:key".into(), vec![0; MAX_SECURE_BLOB_BYTES + 1]),
            Err(ControllerBindingError::SecureBlobInvalid)
        );
        assert!(engine.delete_secure_blob("device:key".into()).is_ok());
    }

    #[test]
    fn independent_sessions_close_idempotently() {
        let inner = SessionInner {
            machine: None,
            transport: None,
            policy: None,
            closed: false,
        };
        let session = Arc::new(ControllerPairingSession {
            inner: Mutex::new(inner),
        });
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let session = Arc::clone(&session);
                std::thread::spawn(move || assert!(session.finish().is_ok()))
            })
            .collect();
        for thread in threads {
            thread
                .join()
                .unwrap_or_else(|_| panic!("close thread panicked"));
        }
        assert_eq!(session.sas(), Err(ControllerBindingError::Disposed));
    }
}
