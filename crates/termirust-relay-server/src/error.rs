use std::fmt;
use termirust_relay_protocol::{RelayDiagnosticCode, RelayProtocolError};

pub struct RelayServerError {
    code: RelayDiagnosticCode,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Debug for RelayServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayServerError")
            .field("code", &self.code)
            .field("source", &self.source.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl RelayServerError {
    pub fn new(code: RelayDiagnosticCode) -> Self {
        Self { code, source: None }
    }

    pub fn with_source(
        code: RelayDiagnosticCode,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            source: Some(Box::new(source)),
        }
    }

    pub fn code(&self) -> RelayDiagnosticCode {
        self.code
    }
}

impl fmt::Display for RelayServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code.as_str())
    }
}

impl std::error::Error for RelayServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

impl From<RelayProtocolError> for RelayServerError {
    fn from(error: RelayProtocolError) -> Self {
        let code = match error {
            RelayProtocolError::VersionMismatch => RelayDiagnosticCode::VersionMismatch,
            RelayProtocolError::FrameLimit => RelayDiagnosticCode::FrameLimit,
            RelayProtocolError::InvalidEnvelope | RelayProtocolError::NonCanonical => {
                RelayDiagnosticCode::MalformedEnvelope
            }
            RelayProtocolError::InvalidAdmissionMessage
            | RelayProtocolError::InvalidVerifier
            | RelayProtocolError::InvalidProof => RelayDiagnosticCode::InvalidProof,
            RelayProtocolError::InvalidQuota => RelayDiagnosticCode::InvalidConfig,
            RelayProtocolError::UnknownDiagnostic => RelayDiagnosticCode::Internal,
        };
        Self::with_source(code, error)
    }
}
