use std::fmt;
use std::io;

use termirust_domain::OutputSequence;
use termirust_host_protocol::{CodecError, IdDecodeError, wire};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientErrorCode {
    Io,
    EndOfStream,
    MalformedFrame,
    FrameTooLarge,
    ChecksumMismatch,
    ProtocolIncompatible,
    PermissionDenied,
    WrongSession,
    ConflictingDuplicate,
    SequenceGap,
    ResourceLimit,
    Cancelled,
    InvalidState,
    InvalidIdentity,
    HandshakeReplay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientError {
    pub code: ClientErrorCode,
    pub io_kind: Option<io::ErrorKind>,
    pub recovery: Option<wire::RecoveryHint>,
    pub expected_sequence: Option<OutputSequence>,
}

impl ClientError {
    pub const fn new(code: ClientErrorCode) -> Self {
        Self {
            code,
            io_kind: None,
            recovery: None,
            expected_sequence: None,
        }
    }

    pub fn io(error: &io::Error) -> Self {
        Self {
            code: ClientErrorCode::Io,
            io_kind: Some(error.kind()),
            recovery: Some(wire::RecoveryHint::Reconnect),
            expected_sequence: None,
        }
    }

    pub fn protocol(error: &wire::ProtocolError) -> Self {
        let code =
            match wire::ErrorCode::try_from(error.code).unwrap_or(wire::ErrorCode::Unspecified) {
                wire::ErrorCode::FrameTooLarge => ClientErrorCode::FrameTooLarge,
                wire::ErrorCode::ChecksumMismatch => ClientErrorCode::ChecksumMismatch,
                wire::ErrorCode::ProtocolIncompatible => ClientErrorCode::ProtocolIncompatible,
                wire::ErrorCode::PermissionDenied => ClientErrorCode::PermissionDenied,
                wire::ErrorCode::WrongSession => ClientErrorCode::WrongSession,
                wire::ErrorCode::ConflictingDuplicate => ClientErrorCode::ConflictingDuplicate,
                wire::ErrorCode::SequenceGap => ClientErrorCode::SequenceGap,
                wire::ErrorCode::ResourceLimit => ClientErrorCode::ResourceLimit,
                wire::ErrorCode::Cancelled => ClientErrorCode::Cancelled,
                wire::ErrorCode::InvalidState => ClientErrorCode::InvalidState,
                wire::ErrorCode::HandshakeReplay => ClientErrorCode::HandshakeReplay,
                wire::ErrorCode::Unspecified | wire::ErrorCode::MalformedFrame => {
                    ClientErrorCode::MalformedFrame
                }
            };
        Self {
            code,
            io_kind: None,
            recovery: wire::RecoveryHint::try_from(error.recovery).ok(),
            expected_sequence: (error.expected_sequence != 0)
                .then_some(OutputSequence::new(error.expected_sequence)),
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            ClientErrorCode::Io => "local Host transport is unavailable",
            ClientErrorCode::EndOfStream => "local Host closed the connection",
            ClientErrorCode::MalformedFrame => "local Host sent a malformed frame",
            ClientErrorCode::FrameTooLarge => "local Host frame exceeds the byte limit",
            ClientErrorCode::ChecksumMismatch => "local Host frame checksum does not match",
            ClientErrorCode::ProtocolIncompatible => "local Host protocol is incompatible",
            ClientErrorCode::PermissionDenied => "local Host peer identity is not authorized",
            ClientErrorCode::WrongSession => "local Host session identity does not match",
            ClientErrorCode::ConflictingDuplicate => "local Host command duplicate conflicts",
            ClientErrorCode::SequenceGap => "local Host output sequence has a gap",
            ClientErrorCode::ResourceLimit => "local Host resource limit was reached",
            ClientErrorCode::Cancelled => "local Host operation was cancelled",
            ClientErrorCode::InvalidState => {
                "local Host client state does not allow this operation"
            }
            ClientErrorCode::InvalidIdentity => "local Host opaque identity is invalid",
            ClientErrorCode::HandshakeReplay => "local Host handshake nonce was already used",
        })
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(error: io::Error) -> Self {
        Self::io(&error)
    }
}

impl From<CodecError> for ClientError {
    fn from(error: CodecError) -> Self {
        Self::new(match error {
            CodecError::FrameTooLarge | CodecError::BufferLimit => ClientErrorCode::FrameTooLarge,
            CodecError::ChecksumMismatch => ClientErrorCode::ChecksumMismatch,
            CodecError::InvalidMagic
            | CodecError::UnknownFrameKind
            | CodecError::Truncated
            | CodecError::TrailingBytes => ClientErrorCode::MalformedFrame,
        })
    }
}

impl From<prost::DecodeError> for ClientError {
    fn from(_: prost::DecodeError) -> Self {
        Self::new(ClientErrorCode::MalformedFrame)
    }
}

impl From<IdDecodeError> for ClientError {
    fn from(_: IdDecodeError) -> Self {
        Self::new(ClientErrorCode::InvalidIdentity)
    }
}
