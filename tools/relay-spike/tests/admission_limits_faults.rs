use termirust_relay_spike::{
    AdmissionCredential, Direction, EndpointRole, MAX_CIPHERTEXT_BYTES, RelayEnvelopeV1,
    RelayError, RelayHarness, RouteRegistration, RouteState, connect_fixture_pair,
    fixture_credential, fixture_route,
};

#[test]
fn admission_replay_expiry_duplicate_and_revocation_fail_closed() {
    let route = fixture_route(0);
    let host = fixture_credential(0, EndpointRole::Host);
    let controller = fixture_credential(0, EndpointRole::Controller);
    let mut relay = RelayHarness::new();
    relay
        .register(RouteRegistration::new(route, &host, &controller))
        .unwrap();

    let challenge = relay
        .issue_challenge(route, EndpointRole::Host, 10)
        .unwrap();
    let replay = host.prove(challenge.clone());
    let host_handle = relay.connect(host.prove(challenge), 10).unwrap();
    assert_eq!(relay.connect(replay, 10), Err(RelayError::ReplayedProof));

    let challenge = relay
        .issue_challenge(route, EndpointRole::Controller, 10)
        .unwrap();
    let wrong = AdmissionCredential::from_fixture_bytes([0xEE; 32]);
    assert_eq!(
        relay.connect(wrong.prove(challenge), 10),
        Err(RelayError::InvalidProof)
    );
    let expired = relay
        .issue_challenge(route, EndpointRole::Controller, 10)
        .unwrap();
    assert_eq!(
        relay.connect(controller.prove(expired), 41),
        Err(RelayError::ExpiredProof)
    );

    let controller_challenge = relay
        .issue_challenge(route, EndpointRole::Controller, 50)
        .unwrap();
    let controller_handle = relay
        .connect(controller.prove(controller_challenge), 50)
        .unwrap();
    assert_eq!(relay.route_state(route), Ok(RouteState::PairedForwarding));

    let duplicate = relay
        .issue_challenge(route, EndpointRole::Host, 50)
        .unwrap();
    assert_eq!(
        relay.connect(host.prove(duplicate), 50),
        Err(RelayError::DuplicateEndpoint)
    );
    relay.revoke(route).unwrap();
    assert_eq!(relay.route_state(route), Ok(RouteState::Revoked));
    assert_eq!(
        relay.send(
            &host_handle,
            RelayEnvelopeV1::new(route, Direction::HostToController, 0, vec![1; 16],).unwrap(),
        ),
        Err(RelayError::Revoked)
    );
    assert_eq!(relay.receive(&controller_handle), Err(RelayError::Revoked));
}

#[test]
fn slow_reader_disconnect_and_restart_never_create_offline_ciphertext_storage() {
    let mut relay = RelayHarness::new();
    let (host, controller) = connect_fixture_pair(&mut relay, 1, 100).unwrap();
    let route = fixture_route(1);
    for sequence in 0..2 {
        relay
            .send(
                &host,
                RelayEnvelopeV1::new(
                    route,
                    Direction::HostToController,
                    sequence,
                    vec![0xA5; MAX_CIPHERTEXT_BYTES],
                )
                .unwrap(),
            )
            .unwrap();
    }
    assert_eq!(
        relay.stats().stored_ciphertext_bytes,
        2 * MAX_CIPHERTEXT_BYTES
    );
    assert_eq!(
        relay.send(
            &host,
            RelayEnvelopeV1::new(route, Direction::HostToController, 2, vec![0xA5; 1],).unwrap(),
        ),
        Err(RelayError::Backpressure)
    );
    assert_eq!(relay.stats().queue_drops, 1);

    let registrations = relay.registrations();
    relay.disconnect(&controller).unwrap();
    assert_eq!(relay.stats().stored_ciphertext_bytes, 0);
    assert_eq!(relay.route_state(route), Ok(RouteState::HostWaiting));
    assert_eq!(
        relay.send(
            &host,
            RelayEnvelopeV1::new(route, Direction::HostToController, 2, vec![1; 8],).unwrap(),
        ),
        Err(RelayError::PeerOffline)
    );

    let restarted = RelayHarness::restart_from(registrations).unwrap();
    assert_eq!(restarted.route_state(route), Ok(RouteState::Registered));
    assert_eq!(restarted.stats().stored_ciphertext_bytes, 0);
    assert_eq!(restarted.stats().persistent_ciphertext_bytes, 0);
}

#[test]
fn revocation_survives_restart_and_reconnect_resets_connection_sequences() {
    let mut relay = RelayHarness::new();
    let (host, controller) = connect_fixture_pair(&mut relay, 3, 10).unwrap();
    let route = fixture_route(3);
    relay
        .send(
            &host,
            RelayEnvelopeV1::new(route, Direction::HostToController, 0, vec![1; 32]).unwrap(),
        )
        .unwrap();
    let _ = relay.receive(&controller).unwrap();
    relay.disconnect(&controller).unwrap();
    let controller_credential = fixture_credential(3, EndpointRole::Controller);
    let challenge = relay
        .issue_challenge(route, EndpointRole::Controller, 11)
        .unwrap();
    let reconnected = relay
        .connect(controller_credential.prove(challenge), 11)
        .unwrap();
    relay
        .send(
            &host,
            RelayEnvelopeV1::new(route, Direction::HostToController, 0, vec![2; 32]).unwrap(),
        )
        .unwrap();
    assert!(relay.receive(&reconnected).unwrap().is_some());

    relay.revoke(route).unwrap();
    let registrations = relay.registrations();
    let mut restarted = RelayHarness::restart_from(registrations).unwrap();
    assert_eq!(restarted.route_state(route), Ok(RouteState::Revoked));
    assert_eq!(
        restarted.issue_challenge(route, EndpointRole::Host, 12),
        Err(RelayError::Revoked)
    );
}

#[test]
fn aggregate_ciphertext_queue_has_a_hard_global_cap() {
    let mut relay = RelayHarness::new();
    let mut hosts = Vec::new();
    for index in 10..43 {
        let (host, _controller) = connect_fixture_pair(&mut relay, index, 100).unwrap();
        hosts.push((index, host));
    }
    for (index, host) in hosts.iter().take(32) {
        for sequence in 0..2 {
            relay
                .send(
                    host,
                    RelayEnvelopeV1::new(
                        fixture_route(*index),
                        Direction::HostToController,
                        sequence,
                        vec![0x44; MAX_CIPHERTEXT_BYTES],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
    }
    assert_eq!(
        relay.send(
            &hosts[32].1,
            RelayEnvelopeV1::new(
                fixture_route(hosts[32].0),
                Direction::HostToController,
                0,
                vec![1],
            )
            .unwrap(),
        ),
        Err(RelayError::Backpressure)
    );
}

#[test]
fn envelope_version_length_direction_and_sequence_limits_are_exact() {
    let route = fixture_route(2);
    let mut bytes = RelayEnvelopeV1::new(route, Direction::HostToController, 0, vec![7; 32])
        .unwrap()
        .encode();
    bytes[5] = 2;
    assert_eq!(
        RelayEnvelopeV1::decode(&bytes),
        Err(RelayError::VersionMismatch)
    );

    let mut relay = RelayHarness::new();
    let (host, _controller) = connect_fixture_pair(&mut relay, 2, 1).unwrap();
    assert_eq!(
        relay.send(
            &host,
            RelayEnvelopeV1::new(route, Direction::HostToController, 1, vec![1; 32],).unwrap(),
        ),
        Err(RelayError::SequenceGap)
    );
}
