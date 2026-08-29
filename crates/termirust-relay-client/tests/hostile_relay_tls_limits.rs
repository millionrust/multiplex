mod common;

use std::sync::Arc;
use termirust_relay_client::{
    MemoryRelaySecretStore, RelayClientRole, RelayClientState, RelayConnectionHandle,
    RelayCredentialSecret, RelayRouteErrorCode, RelaySecretStore, RelaySecretStoreError,
    RelaySpkiPin,
};
use termirust_relay_protocol::RelayServerState;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn rejects_wrong_spki_before_relay_admission() {
    let fixture = common::start_wss_fixture(18).await;
    let endpoint = common::endpoint_with_pin(&fixture.host_endpoint, RelaySpkiPin([0xFF; 32]));
    let error = RelayConnectionHandle::connect_with_tls(
        endpoint,
        RelayClientRole::Host,
        fixture.host_store.clone(),
        fixture.tls.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, RelayRouteErrorCode::SpkiPinMismatch);
    assert_eq!(fixture.server.snapshot().await.active_endpoints, 0);
    fixture.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_secret_disables_only_the_selected_route() {
    let fixture = common::start_wss_fixture(19).await;
    let error = RelayConnectionHandle::connect_with_tls(
        fixture.host_endpoint.clone(),
        RelayClientRole::Host,
        Arc::new(MemoryRelaySecretStore::default()),
        fixture.tls.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, RelayRouteErrorCode::CredentialLost);
    assert_eq!(fixture.server.snapshot().await.active_endpoints, 0);
    fixture.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_relay_secret_never_reaches_inner_controller_auth() {
    let fixture = common::start_wss_fixture(20).await;
    let wrong = Arc::new(MemoryRelaySecretStore::default());
    wrong
        .put(
            &fixture.host_endpoint.credential_ref,
            &RelayCredentialSecret::from_bytes([0xA5; 32]),
        )
        .unwrap();
    let error = RelayConnectionHandle::connect_with_tls(
        fixture.host_endpoint.clone(),
        RelayClientRole::Host,
        wrong,
        fixture.tls.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, RelayRouteErrorCode::AdmissionRejected);
    assert_eq!(fixture.server.snapshot().await.active_endpoints, 0);
    fixture.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn duplicate_role_is_rejected_without_displacing_the_owner() {
    let fixture = common::start_wss_fixture(21).await;
    let first = RelayConnectionHandle::connect_with_tls(
        fixture.host_endpoint.clone(),
        RelayClientRole::Host,
        fixture.host_store.clone(),
        fixture.tls.clone(),
    )
    .await
    .unwrap();
    let error = RelayConnectionHandle::connect_with_tls(
        fixture.host_endpoint.clone(),
        RelayClientRole::Host,
        fixture.host_store.clone(),
        fixture.tls.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, RelayRouteErrorCode::AdmissionRejected);
    assert_eq!(fixture.server.snapshot().await.active_endpoints, 1);
    first.shutdown().await.unwrap();
    fixture.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn live_revocation_becomes_a_typed_revoked_state() {
    let fixture = common::start_wss_fixture(22).await;
    let handle = RelayConnectionHandle::connect_with_tls(
        fixture.host_endpoint.clone(),
        RelayClientRole::Host,
        fixture.host_store.clone(),
        fixture.tls.clone(),
    )
    .await
    .unwrap();
    let mut state = handle.subscribe_state();
    fixture
        .server
        .revoke_route(fixture.registration.route_id)
        .await
        .unwrap();
    timeout(Duration::from_secs(2), async {
        while *state.borrow() != RelayClientState::Revoked {
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(handle.state(), RelayClientState::Revoked);
    let error = handle.shutdown().await.unwrap_err();
    assert_eq!(error.code, RelayRouteErrorCode::RelayEpochMismatch);
    fixture.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn explicit_disable_closes_only_the_owned_socket() {
    let fixture = common::start_wss_fixture(23).await;
    let handle = RelayConnectionHandle::connect_with_tls(
        fixture.host_endpoint.clone(),
        RelayClientRole::Host,
        fixture.host_store.clone(),
        fixture.tls.clone(),
    )
    .await
    .unwrap();
    handle.shutdown().await.unwrap();
    assert_eq!(fixture.server.state(), RelayServerState::ListeningLoopback);
    assert_eq!(fixture.server.snapshot().await.registered_routes, 1);
    timeout(Duration::from_secs(2), async {
        while fixture.server.snapshot().await.active_endpoints != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    fixture.server.shutdown().await.unwrap();
}

struct LockedStore;

impl RelaySecretStore for LockedStore {
    fn put(
        &self,
        _: &termirust_relay_client::RelayCredentialRef,
        _: &RelayCredentialSecret,
    ) -> Result<(), RelaySecretStoreError> {
        Err(RelaySecretStoreError::Locked)
    }

    fn get(
        &self,
        _: &termirust_relay_client::RelayCredentialRef,
    ) -> Result<RelayCredentialSecret, RelaySecretStoreError> {
        Err(RelaySecretStoreError::Locked)
    }

    fn delete(
        &self,
        _: &termirust_relay_client::RelayCredentialRef,
    ) -> Result<bool, RelaySecretStoreError> {
        Err(RelaySecretStoreError::Locked)
    }
}

#[tokio::test]
async fn locked_secret_store_is_distinct_from_missing_material() {
    let fixture = common::start_wss_fixture(24).await;
    let error = RelayConnectionHandle::connect_with_tls(
        fixture.host_endpoint.clone(),
        RelayClientRole::Host,
        Arc::new(LockedStore),
        fixture.tls.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, RelayRouteErrorCode::CredentialLocked);
    fixture.server.shutdown().await.unwrap();
}

#[test]
fn diagnostics_and_debug_output_do_not_expose_route_material() {
    let error = termirust_relay_client::RelayRouteError::new(RelayRouteErrorCode::SpkiPinMismatch);
    assert_eq!(error.to_string(), "relay.route.spki_pin_mismatch");
    assert!(!format!("{error:?}").contains("wss://"));
}
