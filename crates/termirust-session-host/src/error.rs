use std::fmt;
use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostErrorCode {
    Cancelled,
    DescriptorInvalid,
    DescriptorTooLarge,
    PermissionDenied,
    LeaseBusy,
    PtyUnavailable,
    ExecFailed,
    ProcessIdentityUnavailable,
    Protocol,
    Journal,
    ResourceLimit,
    JoinFailed,
    Io,
}

#[derive(Debug)]
pub struct HostError {
    pub code: HostErrorCode,
    pub io_kind: Option<io::ErrorKind>,
    stage: Option<&'static str>,
}

impl HostError {
    pub const fn new(code: HostErrorCode) -> Self {
        Self {
            code,
            io_kind: None,
            stage: None,
        }
    }

    pub fn io(error: io::Error) -> Self {
        let code = if error.kind() == io::ErrorKind::PermissionDenied {
            HostErrorCode::PermissionDenied
        } else {
            HostErrorCode::Io
        };
        Self {
            code,
            io_kind: Some(error.kind()),
            stage: None,
        }
    }

    pub(crate) const fn at_stage(mut self, stage: &'static str) -> Self {
        self.stage = Some(stage);
        self
    }

    pub const fn stable_code(&self) -> &'static str {
        match self.code {
            HostErrorCode::Cancelled => "host_cancelled",
            HostErrorCode::DescriptorInvalid => "descriptor_invalid",
            HostErrorCode::DescriptorTooLarge => "descriptor_too_large",
            HostErrorCode::PermissionDenied => "permission_denied",
            HostErrorCode::LeaseBusy => "host_lease_busy",
            HostErrorCode::PtyUnavailable => "pty_unavailable",
            HostErrorCode::ExecFailed => "exec_failed",
            HostErrorCode::ProcessIdentityUnavailable => "process_identity_unavailable",
            HostErrorCode::Protocol => "protocol_error",
            HostErrorCode::Journal => "journal_error",
            HostErrorCode::ResourceLimit => "resource_limit",
            HostErrorCode::JoinFailed => "task_join_failed",
            HostErrorCode::Io => "io_error",
        }
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.stable_code())
    }
}

impl std::error::Error for HostError {}

impl From<io::Error> for HostError {
    fn from(error: io::Error) -> Self {
        Self::io(error)
    }
}

impl From<prost::DecodeError> for HostError {
    fn from(_: prost::DecodeError) -> Self {
        Self::new(HostErrorCode::Protocol)
    }
}

impl From<termirust_host_protocol::CodecError> for HostError {
    fn from(error: termirust_host_protocol::CodecError) -> Self {
        match error {
            termirust_host_protocol::CodecError::FrameTooLarge => {
                Self::new(HostErrorCode::ResourceLimit)
            }
            _ => Self::new(HostErrorCode::Protocol),
        }
    }
}

impl From<termirust_store::JournalError> for HostError {
    fn from(_: termirust_store::JournalError) -> Self {
        Self::new(HostErrorCode::Journal)
    }
}

impl From<termirust_store::LeaseError> for HostError {
    fn from(error: termirust_store::LeaseError) -> Self {
        match error.code {
            termirust_store::LeaseErrorCode::Busy => Self::new(HostErrorCode::LeaseBusy),
            termirust_store::LeaseErrorCode::PermissionDenied
            | termirust_store::LeaseErrorCode::UnsafeEntry => {
                Self::new(HostErrorCode::PermissionDenied)
            }
            _ => Self::new(HostErrorCode::Io),
        }
    }
}
