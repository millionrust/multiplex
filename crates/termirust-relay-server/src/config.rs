use crate::RelayServerError;
use std::net::SocketAddr;
use std::path::PathBuf;
use termirust_relay_protocol::{
    MAX_FORWARDING_PAIRS, MAX_REGISTERED_ROUTES, MAX_UNAUTHENTICATED_HANDSHAKES,
    RelayDiagnosticCode,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayServerLimits {
    pub registered_routes: usize,
    pub forwarding_pairs: usize,
    pub unauthenticated_handshakes: usize,
}

impl Default for RelayServerLimits {
    fn default() -> Self {
        Self {
            registered_routes: MAX_REGISTERED_ROUTES,
            forwarding_pairs: MAX_FORWARDING_PAIRS,
            unauthenticated_handshakes: MAX_UNAUTHENTICATED_HANDSHAKES,
        }
    }
}

impl RelayServerLimits {
    pub fn validate(self) -> Result<Self, RelayServerError> {
        if self.registered_routes == 0
            || self.registered_routes > MAX_REGISTERED_ROUTES
            || self.forwarding_pairs == 0
            || self.forwarding_pairs > MAX_FORWARDING_PAIRS
            || self.unauthenticated_handshakes == 0
            || self.unauthenticated_handshakes > MAX_UNAUTHENTICATED_HANDSHAKES
        {
            return Err(RelayServerError::new(RelayDiagnosticCode::InvalidConfig));
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub struct RelayServerConfig {
    pub bind: SocketAddr,
    pub state_path: PathBuf,
    pub allowed_origin: String,
    pub limits: RelayServerLimits,
}

impl std::fmt::Debug for RelayServerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayServerConfig")
            .field("bind", &"[LOOPBACK]")
            .field("state_path", &"[PROTECTED_PATH]")
            .field("allowed_origin", &self.allowed_origin)
            .field("limits", &self.limits)
            .finish()
    }
}

impl RelayServerConfig {
    pub fn validate(self) -> Result<Self, RelayServerError> {
        if !self.bind.ip().is_loopback() {
            return Err(RelayServerError::new(RelayDiagnosticCode::LoopbackRequired));
        }
        self.limits.validate()?;
        if self.state_path.as_os_str().is_empty() || self.allowed_origin.is_empty() {
            return Err(RelayServerError::new(RelayDiagnosticCode::InvalidConfig));
        }
        Ok(self)
    }
}
