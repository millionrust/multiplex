#![allow(dead_code)]

use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use std::net::SocketAddr;
use std::sync::Arc;
use tempfile::TempDir;
use termirust_relay_client::{
    MemoryRelaySecretStore, RelayCredentialRef, RelayCredentialSecret, RelayDeviceId,
    RelayEndpointConfig, RelayEndpointId, RelayRouteBinding, RelaySecretStore, RelaySpkiPin,
    RelayTlsClientConfig, RelayWssUrl, spki_pin_from_certificate,
};
use termirust_relay_protocol::{
    RELAY_LOOPBACK_ORIGIN, RelayAdmissionCredential, RelayRevocationEpoch, RelayRouteId,
    RelayRouteRegistration,
};
use termirust_relay_server::{
    RelayServer, RelayServerConfig, RelayServerHandle, RelayServerLimits, RelayTlsServerConfig,
};

pub struct WssFixture {
    pub _temp: TempDir,
    pub server: RelayServerHandle,
    pub host_endpoint: RelayEndpointConfig,
    pub controller_endpoint: RelayEndpointConfig,
    pub host_store: Arc<MemoryRelaySecretStore>,
    pub controller_store: Arc<MemoryRelaySecretStore>,
    pub tls: RelayTlsClientConfig,
    pub registration: RelayRouteRegistration,
}

pub async fn start_wss_fixture(index: u8) -> WssFixture {
    let temp = tempfile::tempdir().unwrap();
    let host_bytes = fixture_bytes(index);
    let controller_bytes = fixture_bytes(index.wrapping_add(64));
    let host_credential = RelayAdmissionCredential::from_secret_bytes(host_bytes);
    let controller_credential = RelayAdmissionCredential::from_secret_bytes(controller_bytes);
    let route_id = RelayRouteId(fixture_bytes(index.wrapping_add(128)));
    let registration =
        RelayRouteRegistration::new(route_id, &host_credential, &controller_credential);

    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
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
    let config = RelayServerConfig {
        bind: SocketAddr::from(([127, 0, 0, 1], 0)),
        state_path: temp.path().join("relay-state-v1.json"),
        allowed_origin: RELAY_LOOPBACK_ORIGIN.to_owned(),
        limits: RelayServerLimits::default(),
    };
    let server = RelayServer::start_tls(config, RelayTlsServerConfig::new(server_config))
        .await
        .unwrap();
    server.register_route(registration.clone()).await.unwrap();

    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tls = RelayTlsClientConfig::from_rustls(client_config);
    let pin = spki_pin_from_certificate(cert.der().as_ref()).unwrap();
    let host_ref = RelayCredentialRef::new("relay:test:host").unwrap();
    let controller_ref = RelayCredentialRef::new("relay:test:controller").unwrap();
    let host_store = Arc::new(MemoryRelaySecretStore::default());
    host_store
        .put(&host_ref, &RelayCredentialSecret::from_bytes(host_bytes))
        .unwrap();
    let controller_store = Arc::new(MemoryRelaySecretStore::default());
    controller_store
        .put(
            &controller_ref,
            &RelayCredentialSecret::from_bytes(controller_bytes),
        )
        .unwrap();
    let binding = RelayRouteBinding {
        host_identity_generation: 1,
        device_id: RelayDeviceId([index.max(1); 16]),
        relay_epoch: RelayRevocationEpoch(0),
    };
    let websocket_url = server.websocket_url();
    let endpoint = |label: &str, credential_ref| {
        RelayEndpointConfig::new(
            RelayEndpointId::new(label).unwrap(),
            RelayWssUrl::parse(&websocket_url).unwrap(),
            route_id,
            credential_ref,
            pin,
            binding,
        )
        .unwrap()
    };
    let host_endpoint = endpoint("host", host_ref);
    let controller_endpoint = endpoint("controller", controller_ref);
    WssFixture {
        _temp: temp,
        server,
        host_endpoint,
        controller_endpoint,
        host_store,
        controller_store,
        tls,
        registration,
    }
}

pub fn endpoint_with_pin(endpoint: &RelayEndpointConfig, pin: RelaySpkiPin) -> RelayEndpointConfig {
    RelayEndpointConfig::new(
        endpoint.endpoint_id.clone(),
        endpoint.wss_url.clone(),
        endpoint.route_id,
        endpoint.credential_ref.clone(),
        pin,
        endpoint.binding,
    )
    .unwrap()
}

fn fixture_bytes(start: u8) -> [u8; 32] {
    core::array::from_fn(|index| start.wrapping_add(index as u8))
}
