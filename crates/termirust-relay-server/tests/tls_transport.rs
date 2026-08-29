mod common;

use futures_util::{SinkExt, StreamExt};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use std::sync::Arc;
use std::time::Duration;
use termirust_relay_protocol::{
    RELAY_LOOPBACK_ORIGIN, RELAY_SUBPROTOCOL, RelayAdmissionChallenge, RelayAdmissionResult,
    RelayClientHello, RelayDiagnosticCode, RelayEndpointRole,
};
use termirust_relay_server::{RelayServer, RelayTlsServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[tokio::test]
async fn injected_rustls_accepts_wss_and_rejects_cleartext_tls_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (registration, host_credential, _) = common::fixture_registration(30);
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der())),
    )
    .unwrap();
    let server = RelayServer::start_tls(
        common::config(&temp),
        RelayTlsServerConfig::new(server_config),
    )
    .await
    .unwrap();
    server.register_route(registration.clone()).await.unwrap();
    assert!(server.websocket_url().starts_with("wss://"));

    let mut cleartext = TcpStream::connect(server.address()).await.unwrap();
    cleartext
        .write_all(b"GET /relay/v1 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    let mut rejection = [0_u8; 32];
    let read = timeout(Duration::from_secs(1), cleartext.read(&mut rejection))
        .await
        .unwrap()
        .unwrap();
    assert!(!rejection[..read].starts_with(b"HTTP/"));

    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tcp = TcpStream::connect(server.address()).await.unwrap();
    let tls = TlsConnector::from(Arc::new(client_config))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    let mut request = server.websocket_url().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", RELAY_LOOPBACK_ORIGIN.parse().unwrap());
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", RELAY_SUBPROTOCOL.parse().unwrap());
    let (mut websocket, response) = client_async(request, tls).await.unwrap();
    assert_eq!(
        response.headers().get("Sec-WebSocket-Protocol").unwrap(),
        RELAY_SUBPROTOCOL
    );
    websocket
        .send(Message::Binary(
            RelayClientHello {
                route_id: registration.route_id,
                role: RelayEndpointRole::Host,
            }
            .encode()
            .to_vec()
            .into(),
        ))
        .await
        .unwrap();
    let Message::Binary(challenge) = websocket.next().await.unwrap().unwrap() else {
        panic!("expected binary admission challenge");
    };
    let challenge = RelayAdmissionChallenge::decode(&challenge).unwrap();
    websocket
        .send(Message::Binary(
            host_credential.prove(&challenge).encode().to_vec().into(),
        ))
        .await
        .unwrap();
    let Message::Binary(result) = websocket.next().await.unwrap().unwrap() else {
        panic!("expected binary admission result");
    };
    assert_eq!(
        RelayAdmissionResult::decode(&result).unwrap().diagnostic,
        RelayDiagnosticCode::Ready
    );
    websocket.close(None).await.unwrap();
    server.shutdown().await.unwrap();
}
