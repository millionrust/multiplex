#![cfg(unix)]

use std::time::Duration;

use termirust_client::synthetic::{self, SyntheticHostConfig};
use termirust_client::{
    AsyncEnvelopeStream, ClientErrorCode, ConnectOptions, ConnectionState, HostClient,
    LocalEndpoint, UserOnlyUnixListener,
};
use termirust_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use termirust_host_protocol::wire::{self, envelope_payload};
use termirust_host_protocol::{
    CURRENT_PROTOCOL, CapabilitySet, ENVELOPE_HEADER_BYTES, FRAME_MAGIC, FrameKind,
    MAX_FRAME_BYTES, PreservedPayload, ProtocolRange, ProtocolVersion, WireEnvelope,
    encode_payload, encode_session_id, local_limits,
};
use tokio::io::AsyncWriteExt as _;
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn session(value: u128) -> HostedSessionId {
    HostedSessionId::from_uuid(Uuid::from_u128(value))
}

fn host(value: u128) -> HostInstanceId {
    HostInstanceId::from_uuid(Uuid::from_u128(value))
}

fn command(value: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(value))
}

#[tokio::test]
async fn handshake_replay_and_mutation_idempotency_conform() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(1);
    let host_id = host(2);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let mut config = SyntheticHostConfig::local(session_id, host_id, [2; 32]);
    config.output = vec![b"first".to_vec(), b"second".to_vec()];
    let server = synthetic::start(endpoint.clone(), config).await.unwrap();
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();

    assert_eq!(client.state(), ConnectionState::Ready);
    assert_eq!(client.host_instance_id(), Some(host_id));
    let state = client.get_state(&cancel).await.unwrap();
    assert_eq!(state.latest_sequence, 2);
    let replay = client
        .attach(OutputSequence::ZERO, 120, 40, &cancel)
        .await
        .unwrap();
    assert_eq!(
        replay
            .iter()
            .map(|output| (output.sequence, output.bytes.as_slice()))
            .collect::<Vec<_>>(),
        [
            (OutputSequence::new(1), b"first".as_slice()),
            (OutputSequence::new(2), b"second".as_slice())
        ]
    );

    let command_id = command(3);
    assert!(
        client
            .input(command_id, b"literal input".to_vec(), &cancel)
            .await
            .unwrap()
    );
    assert!(
        client
            .input(command_id, b"literal input".to_vec(), &cancel)
            .await
            .unwrap()
    );
    assert_eq!(server.stats().await.applied_mutations, 1);
    assert_eq!(server.stats().await.cached_outcomes, 1);

    assert_eq!(
        client
            .input(command_id, b"conflicting input".to_vec(), &cancel)
            .await
            .unwrap_err()
            .code,
        ClientErrorCode::ConflictingDuplicate
    );
    assert!(matches!(
        client.get_state(&cancel).await.unwrap_err().code,
        ClientErrorCode::EndOfStream | ClientErrorCode::Io
    ));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_duplicate_mutations_apply_exactly_once() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(5);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let server = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(6), [3; 32]),
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut first = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let mut second = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [2; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let command_id = command(7);

    let (first_outcome, second_outcome) = tokio::join!(
        first.input(command_id, b"same input".to_vec(), &cancel),
        second.input(command_id, b"same input".to_vec(), &cancel),
    );

    assert!(first_outcome.unwrap());
    assert!(second_outcome.unwrap());
    let stats = server.stats().await;
    assert_eq!(stats.applied_mutations, 1);
    assert_eq!(stats.cached_outcomes, 1);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn read_only_clients_acquire_and_release_writer_explicitly() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(0x51);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let server = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(0x52), [9; 32]),
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local_read_only(session_id, [8; 32]),
        &cancel,
    )
    .await
    .unwrap();

    assert!(!client.get_state(&cancel).await.unwrap().has_writer_lease);
    assert!(
        client
            .set_writer_lease(CommandId::new(), true, &cancel)
            .await
            .unwrap()
    );
    assert!(client.get_state(&cancel).await.unwrap().has_writer_lease);
    assert!(
        !client
            .set_writer_lease(CommandId::new(), false, &cancel)
            .await
            .unwrap()
    );
    assert!(!client.get_state(&cancel).await.unwrap().has_writer_lease);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn raw_peer_cannot_use_an_unnegotiated_capability() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(8);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let server = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(9), [3; 32]),
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut stream =
        AsyncEnvelopeStream::new(UnixStream::connect(endpoint.socket_path()).await.unwrap());
    let handshake_id = [8; 16];
    let handshake = wire::EnvelopePayload {
        message: Some(envelope_payload::Message::HandshakeRequest(
            wire::HandshakeRequest {
                session_id: encode_session_id(session_id),
                protocol: Some(CURRENT_PROTOCOL.into()),
                capabilities: CapabilitySet::from_wire(&[]).to_wire(),
                limits: Some(local_limits().into()),
                client_nonce: vec![4; 32],
                request_writer_lease: false,
            },
        )),
    };
    stream
        .write(
            &WireEnvelope {
                protocol_major: CURRENT_PROTOCOL.maximum.major,
                protocol_minor: CURRENT_PROTOCOL.maximum.minor,
                kind: FrameKind::HandshakeRequest,
                flags: 0,
                request_id: handshake_id,
                payload: encode_payload(&handshake),
            },
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        stream.read(&cancel).await.unwrap().kind,
        FrameKind::HandshakeResponse
    );

    let request_id = [9; 16];
    let state_request = wire::EnvelopePayload {
        message: Some(envelope_payload::Message::GetStateRequest(
            wire::GetStateRequest {
                session_id: encode_session_id(session_id),
            },
        )),
    };
    stream
        .write(
            &WireEnvelope {
                protocol_major: CURRENT_PROTOCOL.maximum.major,
                protocol_minor: CURRENT_PROTOCOL.maximum.minor,
                kind: FrameKind::GetStateRequest,
                flags: 0,
                request_id,
                payload: encode_payload(&state_request),
            },
            &cancel,
        )
        .await
        .unwrap();
    let response = stream.read(&cancel).await.unwrap();
    let error = match PreservedPayload::decode(&response.payload)
        .unwrap()
        .value
        .message
    {
        Some(envelope_payload::Message::ProtocolError(error)) => error,
        _ => panic!("expected protocol error"),
    };
    assert_eq!(response.kind, FrameKind::ProtocolError);
    assert_eq!(response.request_id, request_id);
    assert_eq!(
        wire::ErrorCode::try_from(error.code).unwrap(),
        wire::ErrorCode::InvalidState
    );
    assert_eq!(
        wire::RecoveryHint::try_from(error.recovery).unwrap(),
        wire::RecoveryHint::Reauthorize
    );
    assert_eq!(
        stream.read(&cancel).await.unwrap_err().code,
        ClientErrorCode::EndOfStream
    );
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_session_and_incompatible_major_fail_without_ready_state() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(10);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime-one"), session_id);
    let server = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(11), [2; 32]),
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let error = match HostClient::connect(
        endpoint,
        ConnectOptions::local(session(12), [1; 32]),
        &cancel,
    )
    .await
    {
        Ok(_) => panic!("wrong session unexpectedly connected"),
        Err(error) => error,
    };
    assert_eq!(error.code, ClientErrorCode::WrongSession);
    server.shutdown().await.unwrap();

    let endpoint = LocalEndpoint::new(fixture.path().join("runtime-two"), session_id);
    let server = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(13), [3; 32]),
    )
    .await
    .unwrap();
    let mut options = ConnectOptions::local(session_id, [4; 32]);
    options.protocol = ProtocolRange {
        minimum: ProtocolVersion { major: 2, minor: 0 },
        maximum: ProtocolVersion { major: 2, minor: 0 },
    };
    let error = match HostClient::connect(endpoint, options, &cancel).await {
        Ok(_) => panic!("incompatible protocol unexpectedly connected"),
        Err(error) => error,
    };
    assert_eq!(error.code, ClientErrorCode::ProtocolIncompatible);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn replayed_handshake_is_rejected_and_fresh_nonce_connects() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(15);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let server = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(16), [2; 32]),
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut first = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    first.disconnect();

    let replay = match HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    {
        Ok(_) => panic!("replayed nonce unexpectedly connected"),
        Err(error) => error,
    };
    assert_eq!(replay.code, ClientErrorCode::HandshakeReplay);
    let fresh = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [3; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(fresh.state(), ConnectionState::Ready);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_joins_connections_and_client_reconnects_to_restarted_host() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(20);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let first = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(21), [2; 32]),
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(client.host_instance_id(), Some(host(21)));
    first.shutdown().await.unwrap();

    let second = synthetic::start(
        endpoint,
        SyntheticHostConfig::local(session_id, host(22), [3; 32]),
    )
    .await
    .unwrap();
    client.reconnect([4; 32], &cancel).await.unwrap();
    assert_eq!(client.host_instance_id(), Some(host(22)));
    second.shutdown().await.unwrap();
}

#[tokio::test]
async fn remaining_minimum_commands_are_capability_checked_and_bounded() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(25);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let server = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(26), [2; 32]),
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(client.resize(command(27), 120, 40, &cancel).await.unwrap());
    assert!(client.interrupt(command(28), &cancel).await.unwrap());
    let activity = client.request_activity_snapshot(&cancel).await.unwrap();
    assert_eq!(activity.state, termirust_domain::ActivityState::Unknown);
    assert!(activity.stale);
    assert!(
        client
            .stop(command(29), wire::StopMode::Graceful, &cancel)
            .await
            .unwrap()
    );
    assert_eq!(server.stats().await.applied_mutations, 3);
    client.detach(&cancel).await.unwrap();
    assert_eq!(client.state(), ConnectionState::Disconnected);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn unavailable_replay_range_returns_typed_gap_recovery() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(35);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let mut config = SyntheticHostConfig::local(session_id, host(36), [2; 32]);
    config.first_output_sequence = OutputSequence::new(10);
    config.output = vec![b"retained".to_vec()];
    let server = synthetic::start(endpoint.clone(), config).await.unwrap();
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let error = client
        .attach(OutputSequence::ZERO, 120, 40, &cancel)
        .await
        .unwrap_err();
    assert_eq!(error.code, ClientErrorCode::SequenceGap);
    assert_eq!(error.expected_sequence, Some(OutputSequence::new(1)));
    assert_eq!(error.recovery, Some(wire::RecoveryHint::Replay));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn handshake_wait_cancels_and_returns_disconnected() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(30);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let listener = UserOnlyUnixListener::bind(endpoint.clone()).unwrap();
    let accepted = tokio::spawn(async move {
        let _stream = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        trigger.cancel();
    });
    let mut client = HostClient::disconnected(endpoint, ConnectOptions::local(session_id, [1; 32]));
    assert_eq!(
        client.reconnect([2; 32], &cancel).await.unwrap_err().code,
        ClientErrorCode::Cancelled
    );
    assert_eq!(client.state(), ConnectionState::Disconnected);
    accepted.abort();
    let _ = accepted.await;
}

#[tokio::test]
async fn oversized_peer_gets_safe_code_then_connection_closes() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = session(40);
    let endpoint = LocalEndpoint::new(fixture.path().join("runtime"), session_id);
    let server = synthetic::start(
        endpoint.clone(),
        SyntheticHostConfig::local(session_id, host(41), [2; 32]),
    )
    .await
    .unwrap();
    let mut raw = UnixStream::connect(endpoint.socket_path()).await.unwrap();
    let mut prefix = [0_u8; ENVELOPE_HEADER_BYTES];
    prefix[..4].copy_from_slice(&FRAME_MAGIC);
    prefix[4..6].copy_from_slice(&CURRENT_PROTOCOL.maximum.major.to_be_bytes());
    prefix[6..8].copy_from_slice(&CURRENT_PROTOCOL.maximum.minor.to_be_bytes());
    prefix[8..10].copy_from_slice(&(FrameKind::HandshakeRequest as u16).to_be_bytes());
    prefix[28..32].copy_from_slice(&(MAX_FRAME_BYTES as u32).to_be_bytes());
    raw.write_all(&prefix).await.unwrap();

    let cancel = CancellationToken::new();
    let mut framed = AsyncEnvelopeStream::new(raw);
    let response = framed.read(&cancel).await.unwrap();
    assert_eq!(response.kind, FrameKind::ProtocolError);
    let payload = PreservedPayload::decode(&response.payload).unwrap();
    let error = match payload.value.message {
        Some(envelope_payload::Message::ProtocolError(error)) => error,
        _ => panic!("expected protocol error"),
    };
    assert_eq!(
        wire::ErrorCode::try_from(error.code).unwrap(),
        wire::ErrorCode::FrameTooLarge
    );
    assert_eq!(
        wire::RecoveryHint::try_from(error.recovery).unwrap(),
        wire::RecoveryHint::Reconnect
    );
    assert_eq!(
        framed.read(&cancel).await.unwrap_err().code,
        ClientErrorCode::EndOfStream
    );
    server.shutdown().await.unwrap();
}

#[test]
fn client_and_synthetic_host_sources_do_not_open_tcp() {
    let client = include_str!("../src/client.rs");
    let synthetic = include_str!("../src/synthetic.rs");
    assert!(!client.contains("TcpListener"));
    assert!(!client.contains("TcpStream"));
    assert!(!synthetic.contains("TcpListener"));
    assert!(!synthetic.contains("TcpStream"));
}
