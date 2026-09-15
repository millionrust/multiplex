mod common;

use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use termirust_controller_listener::{
    ApprovalDecision, AuthoritySnapshot, ControllerAuthorityProvider, ControllerBackendFactory,
    ControllerClientChannel, ControllerCommand, ControllerCommandEnvelope,
    ControllerConnectionBackend, ControllerResponse, ControllerSessionOrigin,
    ControllerSessionSummary, HostCommandContext, ListenerError, SystemHandshakeEntropy,
    serve_authenticated_stdio_stream,
};
use termirust_controller_security::{
    CapabilitySet, ControllerCapability as SecurityCapability, HostStaticPublicKey,
    StaticPrivateKey, device_public_key_from_private, host_public_key_from_private,
};
use termirust_domain::{
    AuthenticatedPeer, ControllerCapabilities, ControllerCapability, ControllerDeviceAuthority,
    ControllerDeviceId, ControllerProtocolRange, DevicePublicKey, HostIdentityGeneration,
    HostIdentityPublic, HostIdentitySecretRef, HostIdentityState, HostPublicKey, HostedSessionId,
    OccupantGeneration, OutputSequence, PairedDeviceRecord, PairedDeviceStatus,
    PairingAttemptLedger, PairingOfferId,
};
use termirust_relay_client::{RelayClientRole, RelayConnectionHandle};
use tokio_util::sync::CancellationToken;

const SESSION_GENERATION: u64 = 9;
const REVOCATION_EPOCH: u64 = 2;

struct Authority {
    value: Mutex<ControllerDeviceAuthority>,
    host_private: StaticPrivateKey,
}

impl ControllerAuthorityProvider for Authority {
    fn snapshot(&self) -> Result<AuthoritySnapshot, ListenerError> {
        Ok(AuthoritySnapshot {
            authority: self.value.lock().unwrap().clone(),
            host_private: self.host_private.clone(),
        })
    }
}

struct Backends {
    session_id: HostedSessionId,
    occupant: OccupantGeneration,
}

struct Backend {
    session_id: HostedSessionId,
    occupant: OccupantGeneration,
}

impl ControllerBackendFactory for Backends {
    fn open(
        &self,
        _: &AuthenticatedPeer,
    ) -> Result<Box<dyn ControllerConnectionBackend>, ListenerError> {
        Ok(Box::new(Backend {
            session_id: self.session_id,
            occupant: self.occupant,
        }))
    }
}

#[async_trait]
impl ControllerConnectionBackend for Backend {
    async fn command_context(
        &mut self,
        command: &ControllerCommandEnvelope,
        _: &CancellationToken,
    ) -> Result<HostCommandContext, ListenerError> {
        Ok(HostCommandContext {
            occupant_generation: command.command.session_id().map(|_| self.occupant),
            has_writer_lease: true,
        })
    }

    async fn execute(
        &mut self,
        command: ControllerCommandEnvelope,
        _: &CancellationToken,
    ) -> Result<Vec<ControllerResponse>, ListenerError> {
        let response = match command.command {
            ControllerCommand::ListSessions { .. } => ControllerResponse::Sessions {
                command_id: command.command_id,
                revision: 1,
                update_sequence: 1,
                sessions: vec![ControllerSessionSummary {
                    session_id: self.session_id,
                    host_instance_id: None,
                    origin: ControllerSessionOrigin::Unknown,
                    runtime: None,
                    capabilities: Vec::new(),
                    title: "Authoritative fixture".to_owned(),
                    project: None,
                    group: None,
                    lifecycle: "running".to_owned(),
                    activity: "idle".to_owned(),
                    occupant_generation: Some(self.occupant),
                    last_output_sequence: OutputSequence::ZERO,
                    has_writer: true,
                    unread: false,
                }],
                next_offset: None,
            },
            ControllerCommand::Attach { session_id, .. } => ControllerResponse::Attached {
                command_id: command.command_id,
                session_id,
                occupant_generation: self.occupant,
                replay_through_sequence: OutputSequence::ZERO,
                has_writer_lease: true,
            },
            ControllerCommand::Detach { .. } => ControllerResponse::Detached {
                command_id: command.command_id,
            },
            _ => ControllerResponse::Completed {
                command_id: command.command_id,
                applied: true,
            },
        };
        Ok(vec![response])
    }
}

#[tokio::test]
async fn outbound_wss_host_and_controller_run_independent_g20_auth_and_exact_commands() {
    let fixture = common::start_wss_fixture(8).await;
    let host_private = StaticPrivateKey::from_fixture_bytes([21; 32]);
    let device_private = StaticPrivateKey::from_fixture_bytes([22; 32]);
    let authority: Arc<dyn ControllerAuthorityProvider> = Arc::new(Authority {
        value: Mutex::new(authority(&host_private, &device_private)),
        host_private: host_private.clone(),
    });
    let session_id = HostedSessionId::new();
    let occupant = OccupantGeneration::new(7);

    let mut host = RelayConnectionHandle::connect_with_tls(
        fixture.host_endpoint.clone(),
        RelayClientRole::Host,
        fixture.host_store.clone(),
        fixture.tls.clone(),
    )
    .await
    .unwrap();
    let mut host_stream = host.take_stream().unwrap();
    let host_cancel = CancellationToken::new();
    let host_task_cancel = host_cancel.clone();
    let host_task = tokio::spawn(async move {
        serve_authenticated_stdio_stream(
            &mut host_stream,
            authority,
            Arc::new(Backends {
                session_id,
                occupant,
            }),
            host_task_cancel,
        )
        .await
    });

    let mut controller = RelayConnectionHandle::connect_with_tls(
        fixture.controller_endpoint.clone(),
        RelayClientRole::DesktopController,
        fixture.controller_store.clone(),
        fixture.tls.clone(),
    )
    .await
    .unwrap();
    let controller_stream = controller.take_stream().unwrap();
    let capabilities = all_security_capabilities();
    let mut channel = ControllerClientChannel::connect(
        controller_stream,
        1,
        REVOCATION_EPOCH,
        SESSION_GENERATION,
        HostStaticPublicKey(host_public_key_from_private(&host_private).0),
        device_private,
        capabilities,
        &mut SystemHandshakeEntropy,
    )
    .await
    .unwrap();

    let listed = send_and_read(
        &mut channel,
        ControllerCommand::ListSessions {
            offset: 0,
            limit: 100,
            expected_revision: None,
        },
    )
    .await;
    let ControllerResponse::Sessions { sessions, .. } = listed else {
        panic!("expected sessions response");
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, session_id);

    let commands = [
        ControllerCommand::Attach {
            session_id,
            occupant_generation: occupant,
            from_sequence: OutputSequence::ZERO,
            columns: 120,
            rows: 40,
        },
        ControllerCommand::AcquireWriter {
            session_id,
            occupant_generation: occupant,
        },
        ControllerCommand::Input {
            session_id,
            occupant_generation: occupant,
            bytes: b"printf relay-e2e".to_vec(),
        },
        ControllerCommand::Resize {
            session_id,
            occupant_generation: occupant,
            columns: 100,
            rows: 32,
        },
        ControllerCommand::Approval {
            session_id,
            occupant_generation: occupant,
            approval_id: uuid::Uuid::new_v4(),
            decision: ApprovalDecision::Approve,
        },
        ControllerCommand::Detach {
            session_id,
            occupant_generation: occupant,
        },
    ];
    for command in commands {
        let _ = send_and_read(&mut channel, command).await;
    }

    drop(channel);
    controller.shutdown().await.unwrap();
    host_cancel.cancel();
    let _ = host_task.await.unwrap();
    host.shutdown().await.unwrap();
    fixture.server.shutdown().await.unwrap();
}

async fn send_and_read<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    channel: &mut ControllerClientChannel<S>,
    command: ControllerCommand,
) -> ControllerResponse {
    channel
        .send(command, unix_millis().saturating_add(10_000))
        .await
        .unwrap();
    channel.read_response().await.unwrap()
}

fn authority(
    host_private: &StaticPrivateKey,
    device_private: &StaticPrivateKey,
) -> ControllerDeviceAuthority {
    ControllerDeviceAuthority {
        identity: Some(HostIdentityPublic::new(
            HostIdentityGeneration::INITIAL,
            HostPublicKey(host_public_key_from_private(host_private).0),
        )),
        secret_ref: Some(HostIdentitySecretRef::new("identity:relay-e2e").unwrap()),
        state: HostIdentityState::Ready,
        revocation_epoch: REVOCATION_EPOCH,
        session_generation: SESSION_GENERATION,
        devices: vec![PairedDeviceRecord {
            device_id: ControllerDeviceId::new(),
            public_key: DevicePublicKey(device_public_key_from_private(device_private).0),
            display_name: "Desktop relay fixture".to_owned(),
            capabilities: ControllerCapabilities::default()
                .with(ControllerCapability::ObserveSessions)
                .with(ControllerCapability::AttachOutput)
                .with(ControllerCapability::SendInput)
                .with(ControllerCapability::Resize)
                .with(ControllerCapability::RespondToApproval),
            protocol_range: ControllerProtocolRange::V1,
            created_at: 1,
            last_seen_at: None,
            revocation_epoch: REVOCATION_EPOCH,
            identity_generation: HostIdentityGeneration::INITIAL,
            status: PairedDeviceStatus::Online,
            source_offer_id: PairingOfferId::new(),
        }],
        offers: Vec::new(),
        attempts: PairingAttemptLedger::default(),
    }
}

fn all_security_capabilities() -> CapabilitySet {
    CapabilitySet::default()
        .with(SecurityCapability::ObserveSessions)
        .with(SecurityCapability::AttachOutput)
        .with(SecurityCapability::SendInput)
        .with(SecurityCapability::Resize)
        .with(SecurityCapability::RespondToApproval)
}

fn unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}
