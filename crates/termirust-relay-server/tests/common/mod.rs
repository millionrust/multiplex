#![allow(dead_code)]

use std::net::SocketAddr;
use tempfile::TempDir;
use termirust_controller_security::{
    CONTROLLER_V1, CapabilitySet, ControllerCapability, PairingMachine, PairingNonce,
    PairingOfferCore, RevocationEpoch, StaticPrivateKey, host_public_key_from_private,
};
use termirust_relay_protocol::{
    RELAY_LOOPBACK_ORIGIN, RelayAdmissionCredential, RelayEndpointRole, RelayRouteId,
    RelayRouteRegistration,
};
use termirust_relay_server::harness::SyntheticRelayClient;
use termirust_relay_server::{
    RelayServer, RelayServerConfig, RelayServerHandle, RelayServerLimits,
};

pub struct TestServer {
    pub temp: TempDir,
    pub handle: RelayServerHandle,
    pub registration: RelayRouteRegistration,
    pub host_credential: RelayAdmissionCredential,
    pub controller_credential: RelayAdmissionCredential,
}

pub fn fixture_registration(
    index: u8,
) -> (
    RelayRouteRegistration,
    RelayAdmissionCredential,
    RelayAdmissionCredential,
) {
    let host = RelayAdmissionCredential::from_fixture_bytes(fixture_bytes(index));
    let controller =
        RelayAdmissionCredential::from_fixture_bytes(fixture_bytes(index.wrapping_add(64)));
    let registration = RelayRouteRegistration::new(
        RelayRouteId(fixture_bytes(index.wrapping_add(128))),
        &host,
        &controller,
    );
    (registration, host, controller)
}

pub fn config(temp: &TempDir) -> RelayServerConfig {
    RelayServerConfig {
        bind: SocketAddr::from(([127, 0, 0, 1], 0)),
        state_path: temp.path().join("relay-state-v1.json"),
        allowed_origin: RELAY_LOOPBACK_ORIGIN.to_owned(),
        limits: RelayServerLimits::default(),
    }
}

pub async fn start_registered(index: u8) -> TestServer {
    let temp = tempfile::tempdir().unwrap();
    let handle = RelayServer::start(config(&temp)).await.unwrap();
    let (registration, host_credential, controller_credential) = fixture_registration(index);
    handle.register_route(registration.clone()).await.unwrap();
    TestServer {
        temp,
        handle,
        registration,
        host_credential,
        controller_credential,
    }
}

pub async fn connect_pair(server: &TestServer) -> (SyntheticRelayClient, SyntheticRelayClient) {
    let host = SyntheticRelayClient::connect(
        &server.handle.websocket_url(),
        server.registration.route_id,
        RelayEndpointRole::Host,
        &server.host_credential,
    )
    .await
    .unwrap();
    let controller = SyntheticRelayClient::connect(
        &server.handle.websocket_url(),
        server.registration.route_id,
        RelayEndpointRole::Controller,
        &server.controller_credential,
    )
    .await
    .unwrap();
    (host, controller)
}

pub fn confirmed_controller_pair() -> (
    termirust_controller_security::ConfirmedPairing,
    termirust_controller_security::ConfirmedPairing,
) {
    let host_static = StaticPrivateKey::from_fixture_bytes(fixture_bytes(0));
    let offer = PairingOfferCore {
        version: CONTROLLER_V1,
        expires_at_unix_seconds: 1_300,
        nonce: PairingNonce(fixture_bytes(0x80)),
        host_static_public_key: host_public_key_from_private(&host_static),
        capabilities: CapabilitySet::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput),
    };
    let mut device = PairingMachine::new_device_initiator(
        offer.clone(),
        StaticPrivateKey::from_fixture_bytes(fixture_bytes(0x20)),
        StaticPrivateKey::from_fixture_bytes(fixture_bytes(0x60)),
        10_000,
        1_000,
    )
    .unwrap();
    let mut host = PairingMachine::new_host_responder(
        offer,
        host_static,
        StaticPrivateKey::from_fixture_bytes(fixture_bytes(0x40)),
        10_000,
        1_000,
    )
    .unwrap();
    let message_1 = device.write_next(10_001).unwrap();
    host.read_next(message_1.as_bytes(), 10_002).unwrap();
    let message_2 = host.write_next(10_003).unwrap();
    device.read_next(message_2.as_bytes(), 10_004).unwrap();
    let message_3 = device.write_next(10_005).unwrap();
    host.read_next(message_3.as_bytes(), 10_006).unwrap();
    let sas = device.sas().cloned().unwrap();
    (
        device.confirm(&sas, RevocationEpoch(4)).unwrap(),
        host.confirm(&sas, RevocationEpoch(4)).unwrap(),
    )
}

fn fixture_bytes(start: u8) -> [u8; 32] {
    core::array::from_fn(|index| start.wrapping_add(index as u8))
}
