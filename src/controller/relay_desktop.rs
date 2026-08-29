use std::sync::Arc;
use termirust_controller_listener::{
    ControllerClientChannel, ListenerError, SystemHandshakeEntropy,
};
use termirust_controller_security::{CapabilitySet, HostStaticPublicKey, StaticPrivateKey};
use termirust_relay_client::{
    MutationReconciliation, RelayByteStream, RelayClientRole, RelayConnectionHandle, RelayDeviceId,
    RelayEndpointConfig, RelayRouteError, RelayRouteErrorCode, RelaySecretStore,
};

pub struct DesktopControllerAuth {
    pub host_identity_generation: u64,
    pub revocation_epoch: u64,
    pub session_generation: u64,
    pub device_id: RelayDeviceId,
    pub host_key: HostStaticPublicKey,
    pub device_private: StaticPrivateKey,
    pub capabilities: CapabilitySet,
}

impl std::fmt::Debug for DesktopControllerAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DesktopControllerAuth")
            .field("host_identity_generation", &self.host_identity_generation)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("session_generation", &self.session_generation)
            .field("device_id", &self.device_id)
            .field("keys", &"[REDACTED]")
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

#[derive(Debug)]
pub enum RelayDesktopError {
    Route(RelayRouteError),
    Controller(ListenerError),
}

pub struct DesktopRelaySession {
    pub channel: ControllerClientChannel<RelayByteStream>,
    relay: RelayConnectionHandle,
    pub mutations: MutationReconciliation<termirust_domain::CommandId>,
}

impl std::fmt::Debug for DesktopRelaySession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DesktopRelaySession")
            .field("relay", &self.relay)
            .field("controller", &"[AUTHENTICATED]")
            .finish()
    }
}

impl DesktopRelaySession {
    pub async fn shutdown(self) -> Result<(), RelayRouteError> {
        let Self { channel, relay, .. } = self;
        drop(channel);
        relay.shutdown().await
    }
}

pub struct RelayDesktopRouteOwner {
    endpoint: RelayEndpointConfig,
    secrets: Arc<dyn RelaySecretStore>,
}

impl RelayDesktopRouteOwner {
    pub fn new(endpoint: RelayEndpointConfig, secrets: Arc<dyn RelaySecretStore>) -> Self {
        Self { endpoint, secrets }
    }

    pub async fn connect(
        &self,
        auth: DesktopControllerAuth,
    ) -> Result<DesktopRelaySession, RelayDesktopError> {
        if auth.host_identity_generation != self.endpoint.binding.host_identity_generation
            || auth.device_id != self.endpoint.binding.device_id
        {
            return Err(RelayDesktopError::Route(RelayRouteError::new(
                RelayRouteErrorCode::InvalidConfig,
            )));
        }
        let mut relay = RelayConnectionHandle::connect(
            self.endpoint.clone(),
            RelayClientRole::DesktopController,
            self.secrets.clone(),
        )
        .await
        .map_err(RelayDesktopError::Route)?;
        let stream = relay.take_stream().ok_or_else(|| {
            RelayDesktopError::Route(RelayRouteError::new(RelayRouteErrorCode::Internal))
        })?;
        let channel = ControllerClientChannel::connect(
            stream,
            auth.host_identity_generation,
            auth.revocation_epoch,
            auth.session_generation,
            auth.host_key,
            auth.device_private,
            auth.capabilities,
            &mut SystemHandshakeEntropy,
        )
        .await
        .map_err(RelayDesktopError::Controller)?;
        Ok(DesktopRelaySession {
            channel,
            relay,
            mutations: MutationReconciliation::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_auth_debug_redacts_static_keys() {
        let auth = DesktopControllerAuth {
            host_identity_generation: 1,
            revocation_epoch: 1,
            session_generation: 1,
            device_id: RelayDeviceId([1; 16]),
            host_key: HostStaticPublicKey([2; 32]),
            device_private: StaticPrivateKey::from_fixture_bytes([3; 32]),
            capabilities: CapabilitySet::default(),
        };
        let debug = format!("{auth:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("3, 3, 3"));
    }
}
