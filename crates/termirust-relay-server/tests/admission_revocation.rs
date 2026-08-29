mod common;

use std::time::Duration;
use termirust_relay_protocol::{
    RelayDiagnosticCode, RelayEndpointRole, RelayRouteState, RelayServerState,
};
use termirust_relay_server::harness::SyntheticRelayClient;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn exact_roles_forward_and_live_revocation_closes_both_peers() {
    let server = common::start_registered(1).await;
    assert_eq!(server.handle.state(), RelayServerState::ListeningLoopback);
    let handle_debug = format!("{:?}", server.handle);
    assert!(!handle_debug.contains("127.0.0.1"));
    assert!(!handle_debug.contains(&server.handle.address().port().to_string()));

    let mut host = SyntheticRelayClient::connect(
        &server.handle.websocket_url(),
        server.registration.route_id,
        RelayEndpointRole::Host,
        &server.host_credential,
    )
    .await
    .unwrap();
    let duplicate = SyntheticRelayClient::connect(
        &server.handle.websocket_url(),
        server.registration.route_id,
        RelayEndpointRole::Host,
        &server.host_credential,
    )
    .await
    .unwrap_err();
    assert_eq!(duplicate.code(), RelayDiagnosticCode::DuplicateRole);

    let mut controller = SyntheticRelayClient::connect(
        &server.handle.websocket_url(),
        server.registration.route_id,
        RelayEndpointRole::Controller,
        &server.controller_credential,
    )
    .await
    .unwrap();
    assert_eq!(
        server
            .handle
            .route_state(server.registration.route_id)
            .await
            .unwrap(),
        RelayRouteState::Forwarding
    );
    host.send_ciphertext(b"opaque-host-frame".to_vec())
        .await
        .unwrap();
    assert_eq!(
        controller.receive_envelope().await.unwrap().ciphertext(),
        b"opaque-host-frame"
    );

    server
        .handle
        .revoke_route(server.registration.route_id)
        .await
        .unwrap();
    assert_eq!(
        server
            .handle
            .route_state(server.registration.route_id)
            .await
            .unwrap(),
        RelayRouteState::Revoked
    );
    for client in [&mut host, &mut controller] {
        let message = timeout(Duration::from_secs(1), client.next_message())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Close(Some(frame)) = message else {
            panic!("expected a close frame after revocation");
        };
        assert_eq!(frame.reason, RelayDiagnosticCode::RevokedLive.as_str());
    }
    let snapshot = server.handle.snapshot().await;
    assert_eq!(snapshot.active_endpoints, 0);
    assert_eq!(snapshot.queued_encoded_bytes, 0);
    let diagnostics = server.handle.diagnostics().await;
    assert_eq!(diagnostics.persistent_ciphertext_bytes, 0);
    assert_eq!(diagnostics.per_route_log_bytes, 0);
    server.handle.shutdown().await.unwrap();
    assert_eq!(server.handle.state(), RelayServerState::Stopped);
}

#[tokio::test]
async fn revoked_admission_fails_after_clean_restart() {
    let server = common::start_registered(2).await;
    let config = common::config(&server.temp);
    server
        .handle
        .revoke_route(server.registration.route_id)
        .await
        .unwrap();
    server.handle.shutdown().await.unwrap();

    let restarted = termirust_relay_server::RelayServer::start(config)
        .await
        .unwrap();
    assert_eq!(
        restarted
            .route_state(server.registration.route_id)
            .await
            .unwrap(),
        RelayRouteState::Revoked
    );
    let error = SyntheticRelayClient::connect(
        &restarted.websocket_url(),
        server.registration.route_id,
        RelayEndpointRole::Controller,
        &server.controller_credential,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), RelayDiagnosticCode::Revoked);
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn graceful_shutdown_cancels_active_peers_within_two_seconds() {
    let server = common::start_registered(4).await;
    let (mut host, mut controller) = common::connect_pair(&server).await;
    let started = std::time::Instant::now();

    server.handle.shutdown().await.unwrap();

    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(server.handle.state(), RelayServerState::Stopped);
    for client in [&mut host, &mut controller] {
        let message = timeout(Duration::from_secs(1), client.next_message())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Close(Some(frame)) = message else {
            panic!("expected a close frame during graceful shutdown");
        };
        assert_eq!(frame.reason, RelayDiagnosticCode::Shutdown.as_str());
    }
}
