use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorCode {
    InvalidMagic,
    InvalidEncoding,
    IncompatibleVersion,
    UnsupportedSuite,
    UnknownCapability,
    CapabilityDenied,
    InvalidRole,
    InvalidStep,
    WrongState,
    WrongKey,
    WrongNonce,
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
}

impl ErrorCode {
    #[must_use]
    pub const fn localization_id(self) -> &'static str {
        match self {
            Self::InvalidMagic => "controller.security.invalid_magic",
            Self::InvalidEncoding => "controller.security.invalid_encoding",
            Self::IncompatibleVersion => "controller.security.incompatible_version",
            Self::UnsupportedSuite => "controller.security.unsupported_suite",
            Self::UnknownCapability => "controller.security.unknown_capability",
            Self::CapabilityDenied => "controller.security.capability_denied",
            Self::InvalidRole => "controller.security.invalid_role",
            Self::InvalidStep => "controller.security.invalid_step",
            Self::WrongState => "controller.security.wrong_state",
            Self::WrongKey => "controller.security.wrong_key",
            Self::WrongNonce => "controller.security.wrong_nonce",
            Self::Expired => "controller.security.expired",
            Self::TimedOut => "controller.security.timed_out",
            Self::Rejected => "controller.security.rejected",
            Self::Cancelled => "controller.security.cancelled",
            Self::SasMismatch => "controller.security.sas_mismatch",
            Self::DuplicateFrame => "controller.security.duplicate_frame",
            Self::OutOfOrderFrame => "controller.security.out_of_order_frame",
            Self::FrameTooLarge => "controller.security.frame_too_large",
            Self::SequenceExhausted => "controller.security.sequence_exhausted",
            Self::AuthenticationFailed => "controller.security.authentication_failed",
            Self::CryptoFailure => "controller.security.crypto_failure",
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ControllerSecurityError {
    code: ErrorCode,
}

impl ControllerSecurityError {
    #[must_use]
    pub const fn new(code: ErrorCode) -> Self {
        Self { code }
    }

    #[must_use]
    pub const fn code(self) -> ErrorCode {
        self.code
    }
}

impl fmt::Debug for ControllerSecurityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControllerSecurityError")
            .field("code", &self.code)
            .finish()
    }
}

impl fmt::Display for ControllerSecurityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code.localization_id())
    }
}

impl std::error::Error for ControllerSecurityError {}

impl From<ErrorCode> for ControllerSecurityError {
    fn from(code: ErrorCode) -> Self {
        Self::new(code)
    }
}

pub(crate) type Result<T> = core::result::Result<T, ControllerSecurityError>;
