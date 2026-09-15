mod common;

use termirust_controller_security::{ControllerCapability, ControllerFrameKind, RevocationEpoch};
use termirust_relay_protocol::{
    RELAY_SUBPROTOCOL, RelayConnectionSequence, RelayDiagnosticCode, RelayDirection,
    RelayEnvelopeV1,
};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[tokio::test]
async fn real_controller_ciphertext_remains_opaque_and_forgery_fails_at_endpoint() {
    let server = common::start_registered(3).await;
    let (mut relay_host, mut relay_controller) = common::connect_pair(&server).await;
    let (mut device, mut host) = common::confirmed_controller_pair();
    let plaintext = b"synthetic-controller-secret-never-visible-to-relay";
    let sealed = device
        .transport
        .seal(
            ControllerFrameKind::Control,
            ControllerCapability::SendInput,
            RevocationEpoch(4),
            plaintext,
        )
        .unwrap();
    assert!(
        !sealed
            .as_bytes()
            .windows(plaintext.len())
            .any(|window| window == plaintext)
    );
    relay_controller
        .send_ciphertext(sealed.as_bytes().to_vec())
        .await
        .unwrap();
    let forwarded = relay_host.receive_envelope().await.unwrap();
    assert_eq!(
        host.transport.open(forwarded.ciphertext()).unwrap().payload,
        plaintext
    );

    let second = device
        .transport
        .seal(
            ControllerFrameKind::Control,
            ControllerCapability::SendInput,
            RevocationEpoch(4),
            b"second synthetic payload",
        )
        .unwrap();
    let mut forged = second.as_bytes().to_vec();
    *forged.last_mut().unwrap() ^= 1;
    relay_controller.send_ciphertext(forged).await.unwrap();
    let forged = relay_host.receive_envelope().await.unwrap();
    assert!(host.transport.open(forged.ciphertext()).is_err());

    let diagnostics = server.handle.diagnostics().await;
    let diagnostics_json = serde_json::to_string(&format!("{diagnostics:?}")).unwrap();
    assert!(!diagnostics_json.contains("synthetic-controller-secret"));
    assert_eq!(diagnostics.persistent_ciphertext_bytes, 0);
    server.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn route_direction_and_sequence_mutations_fail_closed() {
    for (index, envelope, expected) in [
        (
            10,
            RelayEnvelopeV1::new(
                termirust_relay_protocol::RelayRouteId([0xFF; 32]),
                RelayDirection::HostToController,
                RelayConnectionSequence(0),
                vec![1],
            )
            .unwrap(),
            RelayDiagnosticCode::RouteMismatch,
        ),
        (
            11,
            RelayEnvelopeV1::new(
                common::fixture_registration(11).0.route_id,
                RelayDirection::ControllerToHost,
                RelayConnectionSequence(0),
                vec![1],
            )
            .unwrap(),
            RelayDiagnosticCode::DirectionMismatch,
        ),
        (
            12,
            RelayEnvelopeV1::new(
                common::fixture_registration(12).0.route_id,
                RelayDirection::HostToController,
                RelayConnectionSequence(1),
                vec![1],
            )
            .unwrap(),
            RelayDiagnosticCode::SequenceGap,
        ),
    ] {
        let server = common::start_registered(index).await;
        let (mut host, mut controller) = common::connect_pair(&server).await;
        host.send_envelope(envelope).await.unwrap();
        let message = controller.next_message().await.unwrap().unwrap();
        let tokio_tungstenite::tungstenite::Message::Close(Some(frame)) = message else {
            panic!("expected hostile frame to close the route");
        };
        assert_eq!(frame.reason, expected.as_str());
        assert_eq!(server.handle.snapshot().await.active_endpoints, 0);
        server.handle.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn origin_upgrade_and_non_loopback_cleartext_fail_before_admission() {
    let server = common::start_registered(19).await;
    let mut request = server.handle.websocket_url().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", RELAY_SUBPROTOCOL.parse().unwrap());
    let error = connect_async(request).await.unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected rejected HTTP upgrade");
    };
    assert_eq!(response.status(), 403);
    server.handle.shutdown().await.unwrap();

    let temp = tempfile::tempdir().unwrap();
    let mut config = common::config(&temp);
    config.bind = std::net::SocketAddr::from(([0, 0, 0, 0], 0));
    let error = termirust_relay_server::RelayServer::start(config)
        .await
        .unwrap_err();
    assert_eq!(error.code(), RelayDiagnosticCode::LoopbackRequired);
}
