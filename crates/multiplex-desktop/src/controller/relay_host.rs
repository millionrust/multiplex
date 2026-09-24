use base64::Engine as _;
use multiplex_controller_listener::{
    ControllerAuthorityProvider, ControllerBackendFactory, ListenerError,
    serve_authenticated_stdio_stream, serve_repository_stdio_bridge,
};
use multiplex_controller_security::StaticPrivateKey;
use multiplex_relay_client::{
    RelayClientRole, RelayConnectionHandle, RelayCredentialRef, RelayCredentialSecret,
    RelayEndpointConfig, RelayRouteError, RelaySecretStore, RelaySecretStoreError,
};
use multiplex_store::keychain::{
    self, CredentialError as KeyringError, CredentialStore as _, SystemCredentials,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize;

const RELAY_SECRET_SERVICE: keychain::ServiceNames = keychain::ServiceNames::new(
    "com.millionrust.multiplex.controller.relay",
    &["com.termirust.controller.relay"],
);

#[derive(Clone, Copy, Debug, Default)]
pub struct OsRelaySecretStore;

impl RelaySecretStore for OsRelaySecretStore {
    fn put(
        &self,
        reference: &RelayCredentialRef,
        secret: &RelayCredentialSecret,
    ) -> Result<(), RelaySecretStoreError> {
        let mut encoded =
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(secret.expose_for_store());
        let result = SystemCredentials.set_password(
            RELAY_SECRET_SERVICE.current,
            reference.expose_for_store(),
            &encoded,
        );
        encoded.zeroize();
        result.map_err(map_keyring_error)
    }

    fn get(
        &self,
        reference: &RelayCredentialRef,
    ) -> Result<RelayCredentialSecret, RelaySecretStoreError> {
        let mut encoded = keychain::password(
            &SystemCredentials,
            RELAY_SECRET_SERVICE,
            reference.expose_for_store(),
        )
        .map_err(map_keyring_error)?
        .ok_or(RelaySecretStoreError::Missing)?;
        let decoded = base64::engine::general_purpose::STANDARD_NO_PAD.decode(&encoded);
        encoded.zeroize();
        let mut decoded = decoded.map_err(|_| RelaySecretStoreError::Invalid)?;
        if decoded.len() != 32 {
            decoded.zeroize();
            return Err(RelaySecretStoreError::Invalid);
        }
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&decoded);
        decoded.zeroize();
        Ok(RelayCredentialSecret::from_bytes(bytes))
    }

    fn delete(&self, reference: &RelayCredentialRef) -> Result<bool, RelaySecretStoreError> {
        keychain::delete(
            &SystemCredentials,
            RELAY_SECRET_SERVICE,
            reference.expose_for_store(),
        )
        .map_err(map_keyring_error)
    }
}

fn map_keyring_error(error: KeyringError) -> RelaySecretStoreError {
    match error {
        KeyringError::NoEntry => RelaySecretStoreError::Missing,
        KeyringError::NoStorageAccess(_) => RelaySecretStoreError::PermissionDenied,
        KeyringError::BadEncoding(_) => RelaySecretStoreError::Invalid,
        _ => RelaySecretStoreError::Unavailable,
    }
}

#[derive(Debug)]
pub enum RelayHostError {
    Route(RelayRouteError),
    Controller(ListenerError),
}

pub struct RelayHostRouteOwner {
    endpoint: RelayEndpointConfig,
    secrets: Arc<dyn RelaySecretStore>,
}

impl std::fmt::Debug for RelayHostRouteOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayHostRouteOwner")
            .field("endpoint", &"[REDACTED]")
            .field("secrets", &"[PROTECTED]")
            .finish()
    }
}

impl RelayHostRouteOwner {
    pub fn new(endpoint: RelayEndpointConfig, secrets: Arc<dyn RelaySecretStore>) -> Self {
        Self { endpoint, secrets }
    }

    pub async fn serve(
        &self,
        authority: Arc<dyn ControllerAuthorityProvider>,
        backends: Arc<dyn ControllerBackendFactory>,
        cancel: CancellationToken,
    ) -> Result<(), RelayHostError> {
        let mut relay = RelayConnectionHandle::connect(
            self.endpoint.clone(),
            RelayClientRole::Host,
            self.secrets.clone(),
        )
        .await
        .map_err(RelayHostError::Route)?;
        let mut stream = relay.take_stream().ok_or_else(|| {
            RelayHostError::Route(RelayRouteError::new(
                multiplex_relay_client::RelayRouteErrorCode::Internal,
            ))
        })?;
        let result = serve_authenticated_stdio_stream(&mut stream, authority, backends, cancel)
            .await
            .map_err(RelayHostError::Controller);
        drop(stream);
        let shutdown = relay.shutdown().await.map_err(RelayHostError::Route);
        result.and(shutdown)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn serve_repository(
        &self,
        controller_root: PathBuf,
        project_root: PathBuf,
        session_data_root: PathBuf,
        runtime_parent: PathBuf,
        pairing_broker_path: PathBuf,
        host_private: StaticPrivateKey,
        sources: multiplex_controller_listener::RepositoryBridgeSources,
        cancel: CancellationToken,
    ) -> Result<(), RelayHostError> {
        let mut relay = RelayConnectionHandle::connect(
            self.endpoint.clone(),
            RelayClientRole::Host,
            self.secrets.clone(),
        )
        .await
        .map_err(RelayHostError::Route)?;
        let stream = relay.take_stream().ok_or_else(|| {
            RelayHostError::Route(RelayRouteError::new(
                multiplex_relay_client::RelayRouteErrorCode::Internal,
            ))
        })?;
        let (reader, writer) = tokio::io::split(stream);
        let result = serve_repository_stdio_bridge(
            reader,
            writer,
            controller_root,
            project_root,
            session_data_root,
            runtime_parent,
            pairing_broker_path,
            host_private,
            sources,
            cancel,
        )
        .await
        .map_err(RelayHostError::Controller);
        let shutdown = relay.shutdown().await.map_err(RelayHostError::Route);
        result.and(shutdown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_contains_secret_service_or_endpoint() {
        assert!(!format!("{:?}", OsRelaySecretStore).contains(RELAY_SECRET_SERVICE.current));
        assert_eq!(
            map_keyring_error(KeyringError::NoEntry),
            RelaySecretStoreError::Missing
        );
    }
}
