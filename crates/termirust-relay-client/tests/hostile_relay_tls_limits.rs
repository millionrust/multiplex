mod common;

use std::sync::Arc;
use termirust_relay_client::{
    MemoryRelaySecretStore, RelayClientRole, RelayConnectionHandle, RelayRouteErrorCode,
    RelaySpkiPin,
};

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

#[test]
fn diagnostics_and_debug_output_do_not_expose_route_material() {
    let error = termirust_relay_client::RelayRouteError::new(RelayRouteErrorCode::SpkiPinMismatch);
    assert_eq!(error.to_string(), "relay.route.spki_pin_mismatch");
    assert!(!format!("{error:?}").contains("wss://"));
}
