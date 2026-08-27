use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use termirust_controller_security::{
    ControllerCapability as SecurityCapability, ControllerFrameKind, MAX_TERMINAL_FRAME_BYTES,
    RevocationEpoch, StaticPrivateKey,
};
use termirust_domain::{
    AuthenticatedPeer, ConnectionBudget, ControllerAuthorizationRequest,
    ControllerCapability as DomainCapability, ControllerDeviceAuthority, ControllerListenPolicy,
    ListenerInstanceId, OccupantGeneration,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{
    AuthRateLimiter, BoundedFrameQueue, BridgeAuthorization, ControllerCommandEnvelope,
    ControllerResponse, InterfaceProvider, ListenerError, ListenerErrorCode, QueueClass,
    SourceBucket, SourceBucketKey, SystemHandshakeEntropy, authenticate_controller, decode_command,
    encode_response, read_bounded_frame, resolve_selected_interface, write_bounded_frame,
};

const AUTHORITY_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const INTERFACE_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const SEALED_FRAME_OVERHEAD_BUDGET: usize = 128;

pub struct AuthoritySnapshot {
    pub authority: ControllerDeviceAuthority,
    pub host_private: StaticPrivateKey,
}

impl std::fmt::Debug for AuthoritySnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthoritySnapshot")
            .field("authority", &"[REDACTED]")
            .field("host_private", &"[REDACTED]")
            .finish()
    }
}

pub trait ControllerAuthorityProvider: Send + Sync {
    fn snapshot(&self) -> Result<AuthoritySnapshot, ListenerError>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostCommandContext {
    pub occupant_generation: Option<OccupantGeneration>,
    pub has_writer_lease: bool,
}

#[async_trait]
pub trait ControllerConnectionBackend: Send {
    async fn command_context(
        &mut self,
        command: &ControllerCommandEnvelope,
        cancel: &CancellationToken,
    ) -> Result<HostCommandContext, ListenerError>;

    async fn execute(
        &mut self,
        command: ControllerCommandEnvelope,
        cancel: &CancellationToken,
    ) -> Result<Vec<ControllerResponse>, ListenerError>;
}

pub trait ControllerBackendFactory: Send + Sync {
    fn open(
        &self,
        peer: &AuthenticatedPeer,
    ) -> Result<Box<dyn ControllerConnectionBackend>, ListenerError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListenerRuntimeReport {
    pub instance_id: ListenerInstanceId,
    pub accepted_connections: u64,
    pub authenticated_connections: u64,
    pub rejected_connections: u64,
}

pub struct ListenerRuntime {
    instance_id: ListenerInstanceId,
    budget: ConnectionBudget,
    source_key: SourceBucketKey,
}

impl std::fmt::Debug for ListenerRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ListenerRuntime")
            .field("instance_id", &self.instance_id)
            .field("budget", &self.budget)
            .field("source_key", &"[REDACTED]")
            .finish()
    }
}

impl ListenerRuntime {
    pub fn new(source_key: SourceBucketKey) -> Result<Self, ListenerError> {
        let budget = ConnectionBudget::default();
        budget.validate()?;
        Ok(Self {
            instance_id: ListenerInstanceId::new(),
            budget,
            source_key,
        })
    }

    pub async fn run(
        self,
        listener: TcpListener,
        policy: ControllerListenPolicy,
        interfaces: Arc<dyn InterfaceProvider>,
        authority: Arc<dyn ControllerAuthorityProvider>,
        backends: Arc<dyn ControllerBackendFactory>,
        cancel: CancellationToken,
    ) -> Result<ListenerRuntimeReport, ListenerError> {
        let route = policy
            .route()?
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::Disabled))?;
        if listener.local_addr().map_err(ListenerError::from)?
            != std::net::SocketAddr::new(route.address, route.port.value())
        {
            return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
        }
        resolve_selected_interface(&policy, &interfaces.eligible_interfaces()?)?;

        let preauth = Arc::new(Semaphore::new(self.budget.max_unauthenticated));
        let authenticated = Arc::new(Semaphore::new(self.budget.max_authenticated_per_host));
        let limiter = Arc::new(Mutex::new(AuthRateLimiter::new(self.budget)?));
        let mut tasks = tokio::task::JoinSet::new();
        let mut report = ListenerRuntimeReport {
            instance_id: self.instance_id,
            accepted_connections: 0,
            authenticated_connections: 0,
            rejected_connections: 0,
        };
        let mut interface_refresh = tokio::time::interval(INTERFACE_REFRESH_INTERVAL);
        interface_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut terminal_error = None;

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = interface_refresh.tick() => {
                    let current = interfaces.eligible_interfaces();
                    if current
                        .as_ref()
                        .ok()
                        .and_then(|candidates| resolve_selected_interface(&policy, candidates).ok())
                        .is_none()
                    {
                        terminal_error = Some(ListenerError::new(ListenerErrorCode::InterfaceGone));
                        break;
                    }
                }
                accepted = listener.accept() => {
                    let (stream, source) = match accepted {
                        Ok(value) => value,
                        Err(error) => {
                            terminal_error = Some(ListenerError::from(error));
                            break;
                        }
                    };
                    report.accepted_connections = report.accepted_connections.saturating_add(1);
                    let source = SourceBucket::derive(&self.source_key, source.ip());
                    let permit = match preauth.clone().try_acquire_owned() {
                        Ok(permit) => permit,
                        Err(_) => {
                            report.rejected_connections = report.rejected_connections.saturating_add(1);
                            drop(stream);
                            continue;
                        }
                    };
                    let now = unix_seconds();
                    if limiter
                        .lock()
                        .map_err(|_| ListenerError::new(ListenerErrorCode::Cancelled))?
                        .check(source, now)
                        .is_err()
                    {
                        report.rejected_connections = report.rejected_connections.saturating_add(1);
                        drop(stream);
                        continue;
                    }
                    let authority = Arc::clone(&authority);
                    let backends = Arc::clone(&backends);
                    let authenticated = Arc::clone(&authenticated);
                    let limiter = Arc::clone(&limiter);
                    let connection_cancel = cancel.child_token();
                    tasks.spawn(async move {
                        serve_connection(
                            stream,
                            source,
                            permit,
                            authenticated,
                            limiter,
                            authority,
                            backends,
                            connection_cancel,
                        )
                        .await
                    });
                }
                completed = tasks.join_next(), if !tasks.is_empty() => {
                    if matches!(completed, Some(Ok(Ok(())))) {
                        report.authenticated_connections = report.authenticated_connections.saturating_add(1);
                    } else {
                        report.rejected_connections = report.rejected_connections.saturating_add(1);
                    }
                }
            }
        }

        cancel.cancel();
        let drain = async { while tasks.join_next().await.is_some() {} };
        if tokio::time::timeout(SHUTDOWN_GRACE, drain).await.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        if let Some(error) = terminal_error {
            return Err(error);
        }
        Ok(report)
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_connection(
    mut stream: TcpStream,
    source: SourceBucket,
    preauth_permit: OwnedSemaphorePermit,
    authenticated_limit: Arc<Semaphore>,
    limiter: Arc<Mutex<AuthRateLimiter>>,
    authority_provider: Arc<dyn ControllerAuthorityProvider>,
    backend_factory: Arc<dyn ControllerBackendFactory>,
    cancel: CancellationToken,
) -> Result<(), ListenerError> {
    let initial = authority_provider.snapshot()?;
    let authenticated = match authenticate_controller(
        &mut stream,
        &initial.authority,
        initial.host_private,
        &mut SystemHandshakeEntropy,
    )
    .await
    {
        Ok(connection) => connection,
        Err(error) => {
            let _ = limiter
                .lock()
                .map(|mut limiter| limiter.record_failure(source, unix_seconds()));
            return Err(error);
        }
    };
    limiter
        .lock()
        .map_err(|_| ListenerError::new(ListenerErrorCode::Cancelled))?
        .record_success(source);
    let authenticated_permit = authenticated_limit
        .try_acquire_owned()
        .map_err(|_| ListenerError::new(ListenerErrorCode::ConnectionLimit))?;
    drop(preauth_permit);

    let result = serve_authenticated_stream(
        &mut stream,
        authenticated,
        authority_provider,
        backend_factory,
        cancel,
    )
    .await;
    drop(authenticated_permit);
    result
}

async fn serve_authenticated_stream<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authenticated: crate::AuthenticatedControllerConnection,
    authority_provider: Arc<dyn ControllerAuthorityProvider>,
    backend_factory: Arc<dyn ControllerBackendFactory>,
    cancel: CancellationToken,
) -> Result<(), ListenerError> {
    let peer = authenticated.peer;
    let mut transport = authenticated.connection.transport;
    let mut backend = backend_factory.open(&peer)?;
    let mut refresh = tokio::time::interval(AUTHORITY_REFRESH_INTERVAL);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = refresh.tick() => {
                require_current_peer(&authority_provider.snapshot()?.authority, &peer)?;
            }
            sealed = read_bounded_frame(&mut *stream, MAX_TERMINAL_FRAME_BYTES) => {
                let sealed = sealed?;
                let opened = transport
                    .open(&sealed)
                    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
                if opened.kind != ControllerFrameKind::Control {
                    return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
                }
                let command = decode_command(&opened.payload)?;
                require_frame_capability(opened.capability, &command)?;
                let current = authority_provider.snapshot()?;
                require_current_peer(&current.authority, &peer)?;
                let context = backend.command_context(&command, &cancel).await?;
                let request = ControllerAuthorizationRequest {
                    device_id: peer.device_id,
                    public_key: peer.public_key,
                    identity_generation: peer.identity_generation,
                    capability: command.command.kind().capability(),
                    revocation_epoch: peer.revocation_epoch,
                    session_generation: command.session_generation,
                    now_millis: unix_millis(),
                    deadline_millis: command.deadline_millis,
                };
                BridgeAuthorization::new(&current.authority).authorize(
                    request,
                    command.bridge_command(),
                    context.occupant_generation,
                    context.has_writer_lease,
                )?;
                let response_capability = security_capability(command.command.kind().capability());
                let mut outbound = BoundedFrameQueue::new(ConnectionBudget::default())?;
                for response in backend.execute(command, &cancel).await? {
                    let (kind, capability, maximum) =
                        response_security(&response, response_capability);
                    let payload = encode_response(&response, maximum)?;
                    let sealed = transport
                        .seal(kind, capability, RevocationEpoch(peer.revocation_epoch), &payload)
                        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
                    outbound.push(queue_class(kind), sealed.as_bytes().to_vec())?;
                }
                while let Some((class, sealed)) = outbound.pop() {
                    write_bounded_frame(&mut *stream, &sealed, queue_frame_limit(class)).await?;
                }
            }
        }
    }
    Ok(())
}

fn require_current_peer(
    authority: &ControllerDeviceAuthority,
    peer: &AuthenticatedPeer,
) -> Result<(), ListenerError> {
    let current = authority
        .devices
        .iter()
        .find(|device| device.device_id == peer.device_id && device.public_key == peer.public_key)
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if authority.revocation_epoch != peer.revocation_epoch
        || current.revocation_epoch != peer.revocation_epoch
        || current.identity_generation != peer.identity_generation
        || current.status == termirust_domain::PairedDeviceStatus::Revoked
    {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    Ok(())
}

fn require_frame_capability(
    presented: SecurityCapability,
    command: &ControllerCommandEnvelope,
) -> Result<(), ListenerError> {
    if presented == security_capability(command.command.kind().capability()) {
        Ok(())
    } else {
        Err(ListenerError::new(ListenerErrorCode::Unauthorized))
    }
}

fn security_capability(capability: DomainCapability) -> SecurityCapability {
    match capability {
        DomainCapability::ObserveSessions => SecurityCapability::ObserveSessions,
        DomainCapability::AttachOutput => SecurityCapability::AttachOutput,
        DomainCapability::SendInput => SecurityCapability::SendInput,
        DomainCapability::Resize => SecurityCapability::Resize,
        DomainCapability::RespondToApproval => SecurityCapability::RespondToApproval,
    }
}

fn response_security(
    response: &ControllerResponse,
    command_capability: SecurityCapability,
) -> (ControllerFrameKind, SecurityCapability, usize) {
    match response {
        ControllerResponse::Sessions { .. } => (
            ControllerFrameKind::Control,
            SecurityCapability::ObserveSessions,
            control_payload_limit(),
        ),
        ControllerResponse::Output { .. } => (
            ControllerFrameKind::Terminal,
            SecurityCapability::AttachOutput,
            MAX_TERMINAL_FRAME_BYTES.saturating_sub(64),
        ),
        ControllerResponse::Attached { .. } | ControllerResponse::Detached { .. } => (
            ControllerFrameKind::Control,
            SecurityCapability::AttachOutput,
            control_payload_limit(),
        ),
        ControllerResponse::Completed { .. } | ControllerResponse::Error { .. } => (
            ControllerFrameKind::Control,
            command_capability,
            control_payload_limit(),
        ),
    }
}

const fn control_payload_limit() -> usize {
    termirust_controller_security::MAX_CONTROL_PAYLOAD_BYTES
        .saturating_sub(SEALED_FRAME_OVERHEAD_BUDGET)
}

const fn queue_class(kind: ControllerFrameKind) -> QueueClass {
    match kind {
        ControllerFrameKind::Control => QueueClass::Control,
        ControllerFrameKind::Terminal => QueueClass::Terminal,
    }
}

fn queue_frame_limit(class: QueueClass) -> usize {
    let budget = ConnectionBudget::default();
    match class {
        QueueClass::Control => budget.max_control_frame_bytes,
        QueueClass::Terminal => budget.max_terminal_frame_bytes,
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_controller_security::{
        CONTROLLER_V1, CapabilitySet, ConnectionChallenge, ConnectionInitiator, ConnectionPrelude,
        ControllerFrameKind, HostStaticPublicKey, RevocationEpoch, device_public_key_from_private,
        host_public_key_from_private,
    };
    use termirust_domain::{
        CommandId, ControllerCapabilities, ControllerDeviceId, ControllerProtocolRange,
        DevicePublicKey, HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef,
        HostIdentityState, HostPublicKey, PairedDeviceRecord, PairedDeviceStatus,
        PairingAttemptLedger, PairingOfferId,
    };

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

    struct Backends;
    struct Backend;

    impl ControllerBackendFactory for Backends {
        fn open(
            &self,
            _: &AuthenticatedPeer,
        ) -> Result<Box<dyn ControllerConnectionBackend>, ListenerError> {
            Ok(Box::new(Backend))
        }
    }

    #[async_trait]
    impl ControllerConnectionBackend for Backend {
        async fn command_context(
            &mut self,
            _: &ControllerCommandEnvelope,
            _: &CancellationToken,
        ) -> Result<HostCommandContext, ListenerError> {
            Ok(HostCommandContext::default())
        }

        async fn execute(
            &mut self,
            command: ControllerCommandEnvelope,
            _: &CancellationToken,
        ) -> Result<Vec<ControllerResponse>, ListenerError> {
            Ok(vec![ControllerResponse::Sessions {
                command_id: command.command_id,
                sessions: Vec::new(),
            }])
        }
    }

    struct Entropy(Vec<[u8; 32]>);

    impl crate::HandshakeEntropy for Entropy {
        fn nonce(&mut self) -> Result<[u8; 32], ListenerError> {
            Ok(self.0.remove(0))
        }

        fn ephemeral_private(&mut self) -> Result<StaticPrivateKey, ListenerError> {
            Ok(StaticPrivateKey::from_fixture_bytes(self.0.remove(0)))
        }
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
            secret_ref: Some(HostIdentitySecretRef::new("identity:runtime").unwrap()),
            state: HostIdentityState::Ready,
            revocation_epoch: 2,
            session_generation: 9,
            devices: vec![PairedDeviceRecord {
                device_id: ControllerDeviceId::new(),
                public_key: DevicePublicKey(device_public_key_from_private(device_private).0),
                display_name: "Phone".to_owned(),
                capabilities: ControllerCapabilities::default()
                    .with(DomainCapability::ObserveSessions),
                protocol_range: ControllerProtocolRange::V1,
                created_at: 1,
                last_seen_at: None,
                revocation_epoch: 2,
                identity_generation: HostIdentityGeneration::INITIAL,
                status: PairedDeviceStatus::Online,
                source_offer_id: PairingOfferId::new(),
            }],
            offers: Vec::new(),
            attempts: PairingAttemptLedger::default(),
        }
    }

    #[tokio::test]
    async fn authenticated_stream_dispatches_typed_command_and_closes_after_revocation() {
        let host_private = StaticPrivateKey::from_fixture_bytes([1; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([2; 32]);
        let authority = Arc::new(Authority {
            value: Mutex::new(authority(&host_private, &device_private)),
            host_private: host_private.clone(),
        });
        let (mut client, mut server) = tokio::io::duplex(8 * 1024);
        let server_authority: Arc<dyn ControllerAuthorityProvider> = authority.clone();
        let cancel = CancellationToken::new();
        let server_cancel = cancel.clone();
        let server_task = tokio::spawn(async move {
            let initial = server_authority.snapshot().unwrap();
            let authenticated = authenticate_controller(
                &mut server,
                &initial.authority,
                initial.host_private,
                &mut Entropy(vec![[4; 32], [5; 32]]),
            )
            .await
            .unwrap();
            serve_authenticated_stream(
                &mut server,
                authenticated,
                server_authority,
                Arc::new(Backends),
                server_cancel,
            )
            .await
        });

        let prelude = ConnectionPrelude {
            version: CONTROLLER_V1,
            identity_generation: 1,
            revocation_epoch: RevocationEpoch(2),
            client_nonce: [3; 32],
        };
        tokio::io::AsyncWriteExt::write_all(&mut client, &prelude.encode().unwrap())
            .await
            .unwrap();
        let mut challenge = [0; ConnectionChallenge::ENCODED_BYTES];
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut challenge)
            .await
            .unwrap();
        let challenge = ConnectionChallenge::decode(&challenge).unwrap();
        let mut initiator = ConnectionInitiator::new(
            prelude,
            &challenge,
            HostStaticPublicKey(host_public_key_from_private(&host_private).0),
            device_private,
            StaticPrivateKey::from_fixture_bytes([6; 32]),
            CapabilitySet::default().with(SecurityCapability::ObserveSessions),
            0,
        )
        .unwrap();
        let hello = initiator.write_hello(0).unwrap();
        write_bounded_frame(&mut client, hello.as_bytes(), 1_024)
            .await
            .unwrap();
        let accept = read_bounded_frame(&mut client, 1_024).await.unwrap();
        let mut connection = initiator.read_accept(&accept, 0).unwrap();

        let command_id = CommandId::new();
        let command = ControllerCommandEnvelope::new(
            command_id,
            9,
            unix_millis().saturating_add(10_000),
            crate::ControllerCommand::ListSessions,
        );
        let payload = crate::encode_command(&command).unwrap();
        let sealed = connection
            .transport
            .seal(
                ControllerFrameKind::Control,
                SecurityCapability::ObserveSessions,
                RevocationEpoch(2),
                &payload,
            )
            .unwrap();
        write_bounded_frame(&mut client, sealed.as_bytes(), MAX_TERMINAL_FRAME_BYTES)
            .await
            .unwrap();
        let response = read_bounded_frame(&mut client, MAX_TERMINAL_FRAME_BYTES)
            .await
            .unwrap();
        let opened = connection.transport.open(&response).unwrap();
        assert_eq!(
            crate::decode_response(
                &opened.payload,
                termirust_controller_security::MAX_CONTROL_PAYLOAD_BYTES,
            )
            .unwrap(),
            ControllerResponse::Sessions {
                command_id,
                sessions: Vec::new(),
            }
        );

        {
            let mut current = authority.value.lock().unwrap();
            let device_id = current.devices[0].device_id;
            current.revoke_device(device_id).unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.unwrap_err().code,
            ListenerErrorCode::AuthenticationFailed
        );
        cancel.cancel();
    }
}
