mod common;

use termirust_controller_security::{ControllerCapability, ControllerFrameKind, RevocationEpoch};
use termirust_relay_spike::{
    Direction, RelayEnvelopeV1, RelayHarness, connect_fixture_pair, fixture_route,
};

#[test]
fn malicious_relay_forwards_controller_ciphertext_without_reading_or_forging_it() {
    let (mut device, mut host) = common::confirmed_pair();
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

    let mut relay = RelayHarness::new();
    let (relay_host, relay_controller) = connect_fixture_pair(&mut relay, 0, 10).unwrap();
    let route = fixture_route(0);
    let envelope = RelayEnvelopeV1::new(
        route,
        Direction::ControllerToHost,
        0,
        sealed.as_bytes().to_vec(),
    )
    .unwrap();
    let decoded = RelayEnvelopeV1::decode(&envelope.encode()).unwrap();
    relay.send(&relay_controller, decoded).unwrap();
    let forwarded = relay.receive(&relay_host).unwrap().unwrap();
    assert_eq!(forwarded.ciphertext(), sealed.as_bytes());
    let opened = host.transport.open(forwarded.ciphertext()).unwrap();
    assert_eq!(opened.payload, plaintext);

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
    let last = forged.len() - 1;
    forged[last] ^= 1;
    relay
        .send(
            &relay_controller,
            RelayEnvelopeV1::new(route, Direction::ControllerToHost, 1, forged).unwrap(),
        )
        .unwrap();
    let forged = relay.receive(&relay_host).unwrap().unwrap();
    assert!(host.transport.open(forged.ciphertext()).is_err());

    let stats_json = serde_json::to_string(&relay.stats()).unwrap();
    assert!(!stats_json.contains("synthetic-controller-secret"));
    assert_eq!(relay.stats().persistent_ciphertext_bytes, 0);
    assert_eq!(relay.stats().per_route_log_bytes, 0);
}
