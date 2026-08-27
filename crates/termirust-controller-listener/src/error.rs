use std::fmt;
use std::io;

use termirust_domain::{AuthorizationDenial, ControllerNetworkError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListenerErrorCode {
    Disabled,
    InvalidPolicy,
    NoEligibleInterface,
    InterfaceGone,
    PermissionDenied,
    PortConflict,
    BindFailed,
    RandomUnavailable,
    ConnectionLimit,
    RateLimited,
    HandshakeTimeout,
    AuthenticationFailed,
    MalformedFrame,
    FrameTooLarge,
    QueueFull,
    Unauthorized,
    StaleGeneration,
    WriterLeaseRequired,
    HostUnavailable,
    Cancelled,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListenerError {
    pub code: ListenerErrorCode,
    pub io_kind: Option<io::ErrorKind>,
    pub authorization: Option<AuthorizationDenial>,
}

impl ListenerError {
    pub const fn new(code: ListenerErrorCode) -> Self {
        Self {
            code,
            io_kind: None,
            authorization: None,
        }
    }

    pub const fn authorization(denial: AuthorizationDenial) -> Self {
        Self {
            code: ListenerErrorCode::Unauthorized,
            io_kind: None,
            authorization: Some(denial),
        }
    }
}

impl fmt::Display for ListenerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            ListenerErrorCode::Disabled => "controller listener is disabled",
            ListenerErrorCode::InvalidPolicy => "controller listener policy is invalid",
            ListenerErrorCode::NoEligibleInterface => "no eligible private LAN or VPN interface",
            ListenerErrorCode::InterfaceGone => "selected controller interface is unavailable",
            ListenerErrorCode::PermissionDenied => "controller listener bind was denied",
            ListenerErrorCode::PortConflict => "controller listener port is already in use",
            ListenerErrorCode::BindFailed => "controller listener could not bind",
            ListenerErrorCode::RandomUnavailable => {
                "secure generated-port selection is unavailable"
            }
            ListenerErrorCode::ConnectionLimit => "controller connection limit reached",
            ListenerErrorCode::RateLimited => "controller authentication is rate limited",
            ListenerErrorCode::HandshakeTimeout => "controller handshake timed out",
            ListenerErrorCode::AuthenticationFailed => "controller authentication failed",
            ListenerErrorCode::MalformedFrame => "controller frame is malformed",
            ListenerErrorCode::FrameTooLarge => "controller frame exceeds its limit",
            ListenerErrorCode::QueueFull => "controller endpoint queue is full",
            ListenerErrorCode::Unauthorized => "controller command is not authorized",
            ListenerErrorCode::StaleGeneration => "controller command generation is stale",
            ListenerErrorCode::WriterLeaseRequired => "controller writer lease is unavailable",
            ListenerErrorCode::HostUnavailable => "authoritative Host is unavailable",
            ListenerErrorCode::Cancelled => "controller listener operation was cancelled",
            ListenerErrorCode::Io => "controller listener I/O failed",
        })
    }
}

impl std::error::Error for ListenerError {}

impl From<ControllerNetworkError> for ListenerError {
    fn from(_: ControllerNetworkError) -> Self {
        Self::new(ListenerErrorCode::InvalidPolicy)
    }
}

impl From<io::Error> for ListenerError {
    fn from(error: io::Error) -> Self {
        Self {
            code: ListenerErrorCode::Io,
            io_kind: Some(error.kind()),
            authorization: None,
        }
    }
}

pub(crate) fn bind_error(error: io::Error) -> ListenerError {
    let code = match error.kind() {
        io::ErrorKind::AddrInUse => ListenerErrorCode::PortConflict,
        io::ErrorKind::PermissionDenied => ListenerErrorCode::PermissionDenied,
        io::ErrorKind::AddrNotAvailable => ListenerErrorCode::InterfaceGone,
        _ => ListenerErrorCode::BindFailed,
    };
    ListenerError {
        code,
        io_kind: Some(error.kind()),
        authorization: None,
    }
}
