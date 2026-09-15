use crate::error::{ErrorCode, Result};
use crate::types::{CapabilitySet, ControllerCapability, RevocationEpoch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationDecision {
    Allow,
    Deny(ErrorCode),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorizationPolicy {
    granted: CapabilitySet,
    revocation_epoch: RevocationEpoch,
}

impl AuthorizationPolicy {
    #[must_use]
    pub const fn new(granted: CapabilitySet, revocation_epoch: RevocationEpoch) -> Self {
        Self {
            granted,
            revocation_epoch,
        }
    }

    #[must_use]
    pub fn evaluate(
        self,
        requested: ControllerCapability,
        presented_epoch: RevocationEpoch,
    ) -> AuthorizationDecision {
        if presented_epoch != self.revocation_epoch || !self.granted.contains(requested) {
            AuthorizationDecision::Deny(ErrorCode::CapabilityDenied)
        } else {
            AuthorizationDecision::Allow
        }
    }

    pub fn require(
        self,
        requested: ControllerCapability,
        presented_epoch: RevocationEpoch,
    ) -> Result<()> {
        match self.evaluate(requested, presented_epoch) {
            AuthorizationDecision::Allow => Ok(()),
            AuthorizationDecision::Deny(code) => Err(code.into()),
        }
    }
}
