use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayRouteErrorCode {
    InvalidConfig,
    CredentialLost,
    CredentialLocked,
    DnsFailed,
    ConnectFailed,
    TlsFailed,
    SpkiPinMismatch,
    UpgradeRejected,
    AdmissionRejected,
    RelayEpochMismatch,
    MalformedFrame,
    FrameLimit,
    SequenceMismatch,
    PeerDisconnected,
    QueuePressure,
    Cancelled,
    UnknownCompletion,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayRouteError {
    pub code: RelayRouteErrorCode,
}

impl RelayRouteError {
    pub const fn new(code: RelayRouteErrorCode) -> Self {
        Self { code }
    }

    pub const fn diagnostic_id(&self) -> &'static str {
        match self.code {
            RelayRouteErrorCode::InvalidConfig => "relay.route.invalid_config",
            RelayRouteErrorCode::CredentialLost => "relay.route.credential_lost",
            RelayRouteErrorCode::CredentialLocked => "relay.route.credential_locked",
            RelayRouteErrorCode::DnsFailed => "relay.route.dns_failed",
            RelayRouteErrorCode::ConnectFailed => "relay.route.connect_failed",
            RelayRouteErrorCode::TlsFailed => "relay.route.tls_failed",
            RelayRouteErrorCode::SpkiPinMismatch => "relay.route.spki_pin_mismatch",
            RelayRouteErrorCode::UpgradeRejected => "relay.route.upgrade_rejected",
            RelayRouteErrorCode::AdmissionRejected => "relay.route.admission_rejected",
            RelayRouteErrorCode::RelayEpochMismatch => "relay.route.epoch_mismatch",
            RelayRouteErrorCode::MalformedFrame => "relay.route.malformed_frame",
            RelayRouteErrorCode::FrameLimit => "relay.route.frame_limit",
            RelayRouteErrorCode::SequenceMismatch => "relay.route.sequence_mismatch",
            RelayRouteErrorCode::PeerDisconnected => "relay.route.peer_disconnected",
            RelayRouteErrorCode::QueuePressure => "relay.route.queue_pressure",
            RelayRouteErrorCode::Cancelled => "relay.route.cancelled",
            RelayRouteErrorCode::UnknownCompletion => "relay.route.unknown_completion",
            RelayRouteErrorCode::Internal => "relay.route.internal",
        }
    }
}

impl fmt::Display for RelayRouteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.diagnostic_id())
    }
}

impl std::error::Error for RelayRouteError {}
