use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use multiplex_controller_security::{
    ControllerCapability as SecurityCapability, ControllerFrameKind, MAX_SCREEN_FRAME_BYTES,
    MAX_TERMINAL_FRAME_BYTES, RevocationEpoch, StaticPrivateKey,
};
use multiplex_domain::{
    AuthenticatedPeer, ConnectionBudget, ControllerAuthorizationRequest,
    ControllerCapability as DomainCapability, ControllerDeviceAuthority, ControllerPort,
    ListenerInstanceId, ListeningAddress, OccupantGeneration,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{
    AuthRateLimiter, BoundAddress, BoundControllerListeners, BoundedFrameQueue,
    BridgeAuthorization, ControllerBinder, ControllerCommandEnvelope, ControllerConnectionPurpose,
    ControllerPairingAuthority, ControllerResponse, ControllerScreenSession, InterfaceProvider,
    ListenerError, ListenerErrorCode, MAX_SCREEN_PAYLOAD_BYTES, QueueClass, ScreenFrameCapability,
    ScreenGrants, ScreenOutgoing, ScreenTicketStore, SourceBucket, SourceBucketKey, SystemBinder,
    SystemHandshakeEntropy, authenticate_controller, bind_address, decode_command, encode_response,
    pair_controller, pair_controller_with_code, read_bounded_frame, write_bounded_frame,
};

const AUTHORITY_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const INTERFACE_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const HOST_OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);
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

    fn reconcile_authenticated_pairing(
        &self,
        _peer: &AuthenticatedPeer,
    ) -> Result<(), ListenerError> {
        Ok(())
    }
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

    async fn next_response(
        &mut self,
        _cancel: &CancellationToken,
    ) -> Result<Option<ControllerResponse>, ListenerError> {
        Ok(None)
    }
}

pub trait ControllerBackendFactory: Send + Sync {
    fn open(
        &self,
        peer: &AuthenticatedPeer,
    ) -> Result<Box<dyn ControllerConnectionBackend>, ListenerError>;

    /// A Remote Screens session for this device, when this host can serve screens. Whatever the
    /// session sends on `outgoing` reaches the device as screen frames. Hosts that cannot serve
    /// screens keep the default and answer no screen frame.
    fn open_screens(
        &self,
        _peer: &AuthenticatedPeer,
        _outgoing: ScreenOutgoing,
    ) -> Option<Box<dyn ControllerScreenSession>> {
        None
    }
}

/// Called with every address the listener accepts on, whenever that set changes.
pub type ListeningAddressObserver = Arc<dyn Fn(&[ListeningAddress]) + Send + Sync>;

pub struct ListenerServices {
    interfaces: Arc<dyn InterfaceProvider>,
    binder: Arc<dyn ControllerBinder<Listener = std::net::TcpListener>>,
    authority: Arc<dyn ControllerAuthorityProvider>,
    pairing: Arc<dyn ControllerPairingAuthority>,
    backends: Arc<dyn ControllerBackendFactory>,
    addresses_changed: Option<ListeningAddressObserver>,
}

impl ListenerServices {
    pub fn new(
        interfaces: Arc<dyn InterfaceProvider>,
        authority: Arc<dyn ControllerAuthorityProvider>,
        pairing: Arc<dyn ControllerPairingAuthority>,
        backends: Arc<dyn ControllerBackendFactory>,
    ) -> Self {
        Self {
            interfaces,
            binder: Arc::new(SystemBinder),
            authority,
            pairing,
            backends,
            addresses_changed: None,
        }
    }

    pub fn with_binder(
        mut self,
        binder: Arc<dyn ControllerBinder<Listener = std::net::TcpListener>>,
    ) -> Self {
        self.binder = binder;
        self
    }

    pub fn with_address_observer(mut self, observer: ListeningAddressObserver) -> Self {
        self.addresses_changed = Some(observer);
        self
    }
}

type AcceptedConnection = (TcpStream, std::net::SocketAddr);

/// The private addresses currently accepting, each feeding one shared queue.
struct Acceptors {
    port: ControllerPort,
    running: Vec<(ListeningAddress, tokio::task::JoinHandle<()>)>,
    sender: mpsc::Sender<AcceptedConnection>,
}

impl Acceptors {
    fn start(&mut self, bound: BoundAddress<std::net::TcpListener>) -> Result<(), ListenerError> {
        let listener = TcpListener::from_std(bound.listener).map_err(ListenerError::from)?;
        let sender = self.sender.clone();
        let task = tokio::spawn(async move {
            // An accept error ends this address; the next refresh binds it again.
            while let Ok(accepted) = listener.accept().await {
                if sender.send(accepted).await.is_err() {
                    break;
                }
            }
        });
        self.running.push((bound.address, task));
        Ok(())
    }

    /// Closes addresses that left or failed and binds addresses that appeared. Returns whether
    /// the set changed.
    fn refresh(
        &mut self,
        interfaces: &dyn InterfaceProvider,
        binder: &dyn ControllerBinder<Listener = std::net::TcpListener>,
    ) -> bool {
        let Ok(candidates) = interfaces.eligible_interfaces() else {
            return false;
        };
        let port = self.port;
        let before = self.running.len();
        self.running.retain(|(address, task)| {
            let present = candidates.iter().any(|candidate| {
                std::net::SocketAddr::new(candidate.address, port.value()) == address.address
            });
            if !present || task.is_finished() {
                task.abort();
                false
            } else {
                true
            }
        });
        let mut changed = self.running.len() != before;
        for candidate in &candidates {
            let wanted = std::net::SocketAddr::new(candidate.address, port.value());
            if self
                .running
                .iter()
                .any(|(address, _)| address.address == wanted)
            {
                continue;
            }
            if let Ok(bound) = bind_address(candidate, port, binder)
                && self.start(bound).is_ok()
            {
                changed = true;
            }
        }
        changed
    }

    fn addresses(&self) -> Vec<ListeningAddress> {
        self.running
            .iter()
            .map(|(address, _)| address.clone())
            .collect()
    }

    fn stop(&mut self) {
        for (_, task) in self.running.drain(..) {
            task.abort();
        }
    }
}

impl std::fmt::Debug for ListenerServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ListenerServices")
            .field("services", &"[OPAQUE]")
            .finish()
    }
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
        listeners: BoundControllerListeners<std::net::TcpListener>,
        services: ListenerServices,
        cancel: CancellationToken,
    ) -> Result<ListenerRuntimeReport, ListenerError> {
        listeners.port.validate()?;
        for bound in &listeners.bound {
            bound.address.validate()?;
            if bound.listener.local_addr().map_err(ListenerError::from)? != bound.address.address {
                return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
            }
        }
        let (sender, mut accepted) = mpsc::channel(self.budget.max_unauthenticated);
        let mut acceptors = Acceptors {
            port: listeners.port,
            running: Vec::new(),
            sender,
        };
        for bound in listeners.bound {
            acceptors.start(bound)?;
        }
        if let Some(observer) = &services.addresses_changed {
            observer(&acceptors.addresses());
        }

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
                    if acceptors.refresh(services.interfaces.as_ref(), services.binder.as_ref())
                        && let Some(observer) = &services.addresses_changed
                    {
                        observer(&acceptors.addresses());
                    }
                }
                connection = accepted.recv() => {
                    let Some((stream, source)) = connection else {
                        terminal_error = Some(ListenerError::new(ListenerErrorCode::Cancelled));
                        break;
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
                    let authority = Arc::clone(&services.authority);
                    let pairing = Arc::clone(&services.pairing);
                    let backends = Arc::clone(&services.backends);
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
                            pairing,
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

        acceptors.stop();
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

pub async fn serve_authenticated_stdio_stream<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authority_provider: Arc<dyn ControllerAuthorityProvider>,
    backend_factory: Arc<dyn ControllerBackendFactory>,
    cancel: CancellationToken,
) -> Result<(), ListenerError> {
    let purpose = tokio::time::timeout(
        Duration::from_secs(ConnectionBudget::default().handshake_timeout_seconds),
        ControllerConnectionPurpose::read_from(stream),
    )
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))??;
    if purpose != ControllerConnectionPurpose::Authenticate {
        return Err(ListenerError::new(ListenerErrorCode::Unauthorized));
    }
    serve_authenticated_stdio_stream_after_purpose(
        stream,
        authority_provider,
        backend_factory,
        cancel,
    )
    .await
}

pub(crate) async fn serve_authenticated_stdio_stream_after_purpose<
    S: AsyncRead + AsyncWrite + Unpin,
>(
    stream: &mut S,
    authority_provider: Arc<dyn ControllerAuthorityProvider>,
    backend_factory: Arc<dyn ControllerBackendFactory>,
    cancel: CancellationToken,
) -> Result<(), ListenerError> {
    let initial = authority_provider.snapshot()?;
    let authenticated = authenticate_controller(
        stream,
        &initial.authority,
        initial.host_private,
        &mut SystemHandshakeEntropy,
    )
    .await?;
    authority_provider.reconcile_authenticated_pairing(&authenticated.peer)?;
    serve_authenticated_stream(
        stream,
        authenticated,
        authority_provider,
        backend_factory,
        cancel,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn serve_connection(
    mut stream: TcpStream,
    source: SourceBucket,
    preauth_permit: OwnedSemaphorePermit,
    authenticated_limit: Arc<Semaphore>,
    limiter: Arc<Mutex<AuthRateLimiter>>,
    authority_provider: Arc<dyn ControllerAuthorityProvider>,
    pairing_authority: Arc<dyn ControllerPairingAuthority>,
    backend_factory: Arc<dyn ControllerBackendFactory>,
    cancel: CancellationToken,
) -> Result<(), ListenerError> {
    let purpose = tokio::time::timeout(
        Duration::from_secs(ConnectionBudget::default().handshake_timeout_seconds),
        ControllerConnectionPurpose::read_from(&mut stream),
    )
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))??;
    if matches!(
        purpose,
        ControllerConnectionPurpose::Pair | ControllerConnectionPurpose::PairCode
    ) {
        let result = if purpose == ControllerConnectionPurpose::Pair {
            pair_controller(
                &mut stream,
                pairing_authority.as_ref(),
                &mut SystemHandshakeEntropy,
                cancel,
            )
            .await
        } else {
            pair_controller_with_code(
                &mut stream,
                pairing_authority.as_ref(),
                &mut SystemHandshakeEntropy,
                cancel,
            )
            .await
        };
        match result {
            Ok(_) => {
                limiter
                    .lock()
                    .map_err(|_| ListenerError::new(ListenerErrorCode::Cancelled))?
                    .record_success(source);
                drop(preauth_permit);
                return Ok(());
            }
            Err(error) => {
                let _ = limiter
                    .lock()
                    .map(|mut limiter| limiter.record_failure(source, unix_seconds()));
                return Err(error);
            }
        }
    }
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
    authority_provider.reconcile_authenticated_pairing(&authenticated.peer)?;
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
    let (screen_sender, mut screen_receiver) = mpsc::unbounded_channel();
    let mut screens = backend_factory.open_screens(&peer, screen_sender);
    let mut screen_tickets = ScreenTicketStore::default();
    let mut screen_entropy = SystemHandshakeEntropy;
    let mut refresh = tokio::time::interval(AUTHORITY_REFRESH_INTERVAL);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut host_output_poll = tokio::time::interval(HOST_OUTPUT_POLL_INTERVAL);
    host_output_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut incoming = Box::pin(read_bounded_frame(&mut reader, MAX_TERMINAL_FRAME_BYTES));
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = refresh.tick() => {
                require_current_peer(&authority_provider.snapshot()?.authority, &peer)?;
            }
            sealed = &mut incoming => {
                let sealed = sealed?;
                let opened = transport
                    .open(&sealed)
                    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
                if opened.kind == ControllerFrameKind::Screen {
                    let capability = ScreenFrameCapability::from_security(opened.capability)?;
                    let current = authority_provider.snapshot()?;
                    require_current_peer(&current.authority, &peer)?;
                    require_screen_capability(&current.authority, &peer, capability)?;
                    let session = screens
                        .as_mut()
                        .ok_or_else(|| ListenerError::new(ListenerErrorCode::Unauthorized))?;
                    session
                        .receive(capability, &opened.payload, &mut screen_tickets)
                        .await?;
                    drop(incoming);
                    incoming = Box::pin(read_bounded_frame(&mut reader, MAX_TERMINAL_FRAME_BYTES));
                    continue;
                }
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
                // Screen sessions live on this connection, so the listener answers for them.
                let responses = match command.command {
                    // A ticket used to be issued whether or not this computer had a screen to
                    // serve. With sharing switched off no frame ever followed, so a device sat
                    // on a spinner for ever rather than being told.
                    crate::ControllerCommand::OpenScreen if screens.is_none() => {
                        vec![ControllerResponse::Error {
                            command_id: command.command_id,
                            code: "screen_sharing_off".to_owned(),
                            completion_unknown: false,
                        }]
                    }
                    crate::ControllerCommand::OpenScreen => {
                        let grants = ScreenGrants::from_capabilities(peer.device_id, peer.capabilities);
                        let ticket = screen_tickets.issue(grants, &mut screen_entropy)?;
                        vec![ControllerResponse::ScreenOpened {
                            command_id: command.command_id,
                            ticket: ticket.to_vec(),
                            can_control_pointer: grants.can_control_pointer,
                            can_control_keyboard: grants.can_control_keyboard,
                        }]
                    }
                    crate::ControllerCommand::CloseScreen => {
                        screen_tickets.close();
                        if let Some(session) = screens.as_mut() {
                            session.close();
                        }
                        vec![ControllerResponse::Completed {
                            command_id: command.command_id,
                            applied: true,
                        }]
                    }
                    _ => backend.execute(command, &cancel).await?,
                };
                for response in responses {
                    let (kind, capability, maximum) =
                        response_security(&response, response_capability);
                    let payload = encode_response(&response, maximum)?;
                    let sealed = transport
                        .seal(kind, capability, RevocationEpoch(peer.revocation_epoch), &payload)
                        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
                    outbound.push(queue_class(kind), sealed.as_bytes().to_vec())?;
                    while let Some((class, sealed)) = outbound.pop() {
                        write_bounded_frame(&mut writer, &sealed, queue_frame_limit(class)).await?;
                    }
                }
                drop(incoming);
                incoming = Box::pin(read_bounded_frame(&mut reader, MAX_TERMINAL_FRAME_BYTES));
            }
            // When every sender is dropped the host ended the screen session; this branch stops
            // matching and the Controller connection carries on.
            Some(bytes) = screen_receiver.recv() => {
                for chunk in bytes.chunks(MAX_SCREEN_PAYLOAD_BYTES) {
                    let sealed = transport
                        .seal(
                            ControllerFrameKind::Screen,
                            SecurityCapability::ObserveScreens,
                            RevocationEpoch(peer.revocation_epoch),
                            chunk,
                        )
                        .map_err(|_| {
                            ListenerError::new(ListenerErrorCode::AuthenticationFailed)
                        })?;
                    write_bounded_frame(&mut writer, sealed.as_bytes(), MAX_SCREEN_FRAME_BYTES)
                        .await?;
                }
            }
            _ = host_output_poll.tick() => {
                if let Some(response) = backend.next_response(&cancel).await? {
                    let (kind, capability, maximum) =
                        response_security(&response, SecurityCapability::AttachOutput);
                    let payload = encode_response(&response, maximum)?;
                    let sealed = transport
                        .seal(
                            kind,
                            capability,
                            RevocationEpoch(peer.revocation_epoch),
                            &payload,
                        )
                        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
                    let class = queue_class(kind);
                    let mut outbound = BoundedFrameQueue::new(ConnectionBudget::default())?;
                    outbound.push(class, sealed.as_bytes().to_vec())?;
                    while let Some((class, sealed)) = outbound.pop() {
                        write_bounded_frame(&mut writer, &sealed, queue_frame_limit(class)).await?;
                    }
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
        || current.status == multiplex_domain::PairedDeviceStatus::Revoked
    {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    Ok(())
}

/// Screen frames are authorized per frame against the device's current record, so withdrawing a
/// capability stops that traffic without waiting for the session to end.
fn require_screen_capability(
    authority: &ControllerDeviceAuthority,
    peer: &AuthenticatedPeer,
    capability: ScreenFrameCapability,
) -> Result<(), ListenerError> {
    let domain = match capability {
        ScreenFrameCapability::Observe => DomainCapability::ObserveScreens,
        ScreenFrameCapability::Pointer => DomainCapability::ControlPointer,
        ScreenFrameCapability::Keyboard => DomainCapability::ControlKeyboard,
    };
    let request = ControllerAuthorizationRequest {
        device_id: peer.device_id,
        public_key: peer.public_key,
        identity_generation: peer.identity_generation,
        capability: domain,
        revocation_epoch: peer.revocation_epoch,
        session_generation: authority.session_generation,
        now_millis: unix_millis(),
        deadline_millis: unix_millis().saturating_add(1_000),
    };
    match authority.authorize(request) {
        multiplex_domain::ControllerAuthorizationDecision::Allow => Ok(()),
        multiplex_domain::ControllerAuthorizationDecision::Deny(denial) => {
            Err(ListenerError::authorization(denial))
        }
    }
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
        DomainCapability::ObserveScreens => SecurityCapability::ObserveScreens,
        DomainCapability::ControlPointer => SecurityCapability::ControlPointer,
        DomainCapability::ControlKeyboard => SecurityCapability::ControlKeyboard,
        DomainCapability::CreateSession => SecurityCapability::CreateSession,
    }
}

fn response_security(
    response: &ControllerResponse,
    command_capability: SecurityCapability,
) -> (ControllerFrameKind, SecurityCapability, usize) {
    match response {
        // Never sent by this build; a reader that met one skipped it before reaching here.
        ControllerResponse::Unknown | ControllerResponse::Sessions { .. } => (
            ControllerFrameKind::Control,
            SecurityCapability::ObserveSessions,
            control_payload_limit(),
        ),
        ControllerResponse::SessionCreated { .. } => (
            ControllerFrameKind::Control,
            SecurityCapability::CreateSession,
            control_payload_limit(),
        ),
        ControllerResponse::Snapshot { .. } | ControllerResponse::Output { .. } => (
            ControllerFrameKind::Terminal,
            SecurityCapability::AttachOutput,
            MAX_TERMINAL_FRAME_BYTES.saturating_sub(64),
        ),
        ControllerResponse::Attached { .. } | ControllerResponse::Detached { .. } => (
            ControllerFrameKind::Control,
            SecurityCapability::AttachOutput,
            control_payload_limit(),
        ),
        ControllerResponse::ScreenOpened { .. } => (
            ControllerFrameKind::Control,
            SecurityCapability::ObserveScreens,
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
    multiplex_controller_security::MAX_CONTROL_PAYLOAD_BYTES
        .saturating_sub(SEALED_FRAME_OVERHEAD_BUDGET)
}

const fn queue_class(kind: ControllerFrameKind) -> QueueClass {
    match kind {
        ControllerFrameKind::Control => QueueClass::Control,
        // Screen frames share the terminal queue class: the same 1 MiB frame limit, and no
        // screen sessions are served over this listener yet.
        ControllerFrameKind::Terminal | ControllerFrameKind::Screen => QueueClass::Terminal,
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
mod address_tests {
    use std::net::SocketAddr;

    use multiplex_domain::{
        AddressFamily, NetworkInterfaceCandidate, NetworkInterfaceId, NetworkInterfaceKind,
    };

    use super::*;

    /// Interfaces the test changes between refreshes.
    #[derive(Default)]
    struct ChangingInterfaces(Mutex<Vec<NetworkInterfaceCandidate>>);

    impl InterfaceProvider for ChangingInterfaces {
        fn eligible_interfaces(&self) -> Result<Vec<NetworkInterfaceCandidate>, ListenerError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    /// Binds a loopback socket for each private address, since tests cannot own real ones.
    struct LoopbackBinder;

    impl ControllerBinder for LoopbackBinder {
        type Listener = std::net::TcpListener;

        fn bind_exact(&self, _address: SocketAddr) -> std::io::Result<Self::Listener> {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            listener.set_nonblocking(true)?;
            Ok(listener)
        }
    }

    fn candidate(address: &str) -> NetworkInterfaceCandidate {
        let address: std::net::IpAddr = address.parse().unwrap();
        NetworkInterfaceCandidate {
            id: NetworkInterfaceId::new("4:en0").unwrap(),
            label: "en0".into(),
            kind: NetworkInterfaceKind::Lan,
            address_family: if address.is_ipv4() {
                AddressFamily::Ipv4
            } else {
                AddressFamily::Ipv6
            },
            address,
        }
    }

    fn listening(acceptors: &Acceptors) -> Vec<String> {
        acceptors
            .addresses()
            .iter()
            .map(|address| address.address.to_string())
            .collect()
    }

    #[tokio::test]
    async fn addresses_follow_networks_that_come_and_go() {
        let interfaces = ChangingInterfaces::default();
        let (sender, _accepted) = mpsc::channel(4);
        let mut acceptors = Acceptors {
            port: ControllerPort::Generated(55_000),
            running: Vec::new(),
            sender,
        };

        assert!(!acceptors.refresh(&interfaces, &LoopbackBinder));
        assert!(listening(&acceptors).is_empty());

        *interfaces.0.lock().unwrap() = vec![candidate("192.168.88.4"), candidate("100.81.253.53")];
        assert!(acceptors.refresh(&interfaces, &LoopbackBinder));
        assert_eq!(
            listening(&acceptors),
            ["192.168.88.4:55000", "100.81.253.53:55000"]
        );
        assert!(!acceptors.refresh(&interfaces, &LoopbackBinder));

        // Tailscale gets a new address and the Wi-Fi network is left.
        *interfaces.0.lock().unwrap() = vec![candidate("100.81.203.77")];
        assert!(acceptors.refresh(&interfaces, &LoopbackBinder));
        assert_eq!(listening(&acceptors), ["100.81.203.77:55000"]);

        acceptors.stop();
        assert!(listening(&acceptors).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use multiplex_controller_security::{
        CONTROLLER_V2, CapabilitySet, ConnectionChallenge, ConnectionInitiator, ConnectionPrelude,
        ControllerFrameKind, HostStaticPublicKey, RevocationEpoch, device_public_key_from_private,
        host_public_key_from_private,
    };
    use multiplex_domain::{
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

    #[derive(Default)]
    struct Backends {
        pending: Arc<Mutex<std::collections::VecDeque<ControllerResponse>>>,
        screens: bool,
        screen_frames: Arc<Mutex<Vec<(ScreenFrameCapability, usize)>>>,
    }

    struct Backend {
        pending: Arc<Mutex<std::collections::VecDeque<ControllerResponse>>>,
    }

    impl ControllerBackendFactory for Backends {
        fn open(
            &self,
            _: &AuthenticatedPeer,
        ) -> Result<Box<dyn ControllerConnectionBackend>, ListenerError> {
            Ok(Box::new(Backend {
                pending: self.pending.clone(),
            }))
        }

        fn open_screens(
            &self,
            _: &AuthenticatedPeer,
            outgoing: ScreenOutgoing,
        ) -> Option<Box<dyn ControllerScreenSession>> {
            self.screens.then(|| {
                Box::new(EchoScreens {
                    outgoing,
                    seen: self.screen_frames.clone(),
                }) as Box<dyn ControllerScreenSession>
            })
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
                revision: 1,
                update_sequence: 1,
                sessions: Vec::new(),
                next_offset: None,
            }])
        }

        async fn next_response(
            &mut self,
            _: &CancellationToken,
        ) -> Result<Option<ControllerResponse>, ListenerError> {
            Ok(self.pending.lock().unwrap().pop_front())
        }
    }

    fn authority_with_screens(
        host_private: &StaticPrivateKey,
        device_private: &StaticPrivateKey,
    ) -> ControllerDeviceAuthority {
        let mut authority = authority(host_private, device_private);
        for device in &mut authority.devices {
            device.capabilities = device
                .capabilities
                .with(DomainCapability::ObserveScreens)
                .with(DomainCapability::ControlPointer);
        }
        authority
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
                    .with(DomainCapability::ObserveSessions)
                    .with(DomainCapability::AttachOutput),
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
    async fn authenticated_stdio_dispatches_typed_command_and_closes_after_revocation() {
        let host_private = StaticPrivateKey::from_fixture_bytes([1; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([2; 32]);
        let authority = Arc::new(Authority {
            value: Mutex::new(authority(&host_private, &device_private)),
            host_private: host_private.clone(),
        });
        let (mut client, mut server) = tokio::io::duplex(8 * 1024);
        let server_authority: Arc<dyn ControllerAuthorityProvider> = authority.clone();
        let backends = Arc::new(Backends::default());
        let pending = backends.pending.clone();
        let cancel = CancellationToken::new();
        let server_cancel = cancel.clone();
        let server_task = tokio::spawn(async move {
            serve_authenticated_stdio_stream(&mut server, server_authority, backends, server_cancel)
                .await
        });

        ControllerConnectionPurpose::Authenticate
            .write_to(&mut client)
            .await
            .unwrap();
        let prelude = ConnectionPrelude {
            version: CONTROLLER_V2,
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
            CapabilitySet::default()
                .with(SecurityCapability::ObserveSessions)
                .with(SecurityCapability::AttachOutput),
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
            crate::ControllerCommand::ListSessions {
                offset: 0,
                limit: 1_000,
                expected_revision: None,
            },
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
                multiplex_controller_security::MAX_CONTROL_PAYLOAD_BYTES,
            )
            .unwrap(),
            ControllerResponse::Sessions {
                command_id,
                revision: 1,
                update_sequence: 1,
                sessions: Vec::new(),
                next_offset: None,
            }
        );

        let session_id = multiplex_domain::HostedSessionId::new();
        pending
            .lock()
            .unwrap()
            .push_back(ControllerResponse::Output {
                session_id,
                sequence: multiplex_domain::OutputSequence::new(1),
                bytes: b"live output".to_vec(),
            });
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            read_bounded_frame(&mut client, MAX_TERMINAL_FRAME_BYTES),
        )
        .await
        .unwrap()
        .unwrap();
        let opened = connection.transport.open(&response).unwrap();
        assert_eq!(opened.kind, ControllerFrameKind::Terminal);
        assert_eq!(opened.capability, SecurityCapability::AttachOutput);
        assert_eq!(
            crate::decode_response(&opened.payload, MAX_TERMINAL_FRAME_BYTES).unwrap(),
            ControllerResponse::Output {
                session_id,
                sequence: multiplex_domain::OutputSequence::new(1),
                bytes: b"live output".to_vec(),
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

    /// A screen session that answers the first message, which presents the ticket, with more
    /// bytes than one screen frame can carry.
    struct EchoScreens {
        outgoing: ScreenOutgoing,
        seen: Arc<Mutex<Vec<(ScreenFrameCapability, usize)>>>,
    }

    const ECHO_SCREEN_BYTES: usize = 1_500_000;

    #[async_trait]
    impl ControllerScreenSession for EchoScreens {
        async fn receive(
            &mut self,
            capability: ScreenFrameCapability,
            bytes: &[u8],
            tickets: &mut ScreenTicketStore,
        ) -> Result<(), ListenerError> {
            if tickets.active().is_none() {
                let proof: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
                tickets
                    .spend(&proof)
                    .ok_or_else(|| ListenerError::new(ListenerErrorCode::Unauthorized))?;
                self.outgoing
                    .send(vec![7; ECHO_SCREEN_BYTES])
                    .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
            }
            self.seen.lock().unwrap().push((capability, bytes.len()));
            Ok(())
        }

        fn close(&mut self) {}
    }

    /// A phone can watch a computer's screen and use one of its terminals at the same time.
    ///
    /// Each thing the phone does opens its own connection, so "one session at a time" was never
    /// the wire's rule — it was the phone serialising itself. The listener serves both at once:
    /// a screen session on one connection and a command on another, for the same device.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_device_can_watch_a_screen_and_use_a_terminal_at_the_same_time() {
        let host_private = StaticPrivateKey::from_fixture_bytes([45; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([46; 32]);
        let authority = Arc::new(Authority {
            value: Mutex::new(authority_with_screens(&host_private, &device_private)),
            host_private: host_private.clone(),
        });
        let backends = Arc::new(Backends {
            screens: true,
            ..Backends::default()
        });
        let host_public = HostStaticPublicKey(host_public_key_from_private(&host_private).0);

        // The connection the screen runs on.
        let (watch_client, mut watch_server) = tokio::io::duplex(8 * 1024);
        let watching = tokio::spawn({
            let provider: Arc<dyn ControllerAuthorityProvider> = authority.clone();
            let backends = backends.clone();
            async move {
                serve_authenticated_stdio_stream(
                    &mut watch_server,
                    provider,
                    backends,
                    CancellationToken::new(),
                )
                .await
            }
        });
        let mut watch = crate::ControllerClientChannel::connect(
            watch_client,
            1,
            2,
            9,
            host_public,
            StaticPrivateKey::from_fixture_bytes([46; 32]),
            CapabilitySet::default()
                .with(SecurityCapability::ObserveScreens)
                .with(SecurityCapability::ObserveSessions),
            &mut crate::SystemHandshakeEntropy,
        )
        .await
        .unwrap();
        let deadline = unix_millis().saturating_add(10_000);
        watch
            .send(crate::ControllerCommand::OpenScreen, deadline)
            .await
            .unwrap();
        let ControllerResponse::ScreenOpened { ticket, .. } = watch.read_response().await.unwrap()
        else {
            panic!("the screen did not open");
        };

        // A second connection for the same device, while the first is still watching.
        let (work_client, mut work_server) = tokio::io::duplex(8 * 1024);
        let working = tokio::spawn({
            let provider: Arc<dyn ControllerAuthorityProvider> = authority.clone();
            let backends = backends.clone();
            async move {
                serve_authenticated_stdio_stream(
                    &mut work_server,
                    provider,
                    backends,
                    CancellationToken::new(),
                )
                .await
            }
        });
        let mut work = crate::ControllerClientChannel::connect(
            work_client,
            1,
            2,
            9,
            host_public,
            StaticPrivateKey::from_fixture_bytes([46; 32]),
            CapabilitySet::default().with(SecurityCapability::ObserveSessions),
            &mut crate::SystemHandshakeEntropy,
        )
        .await
        .unwrap();
        let listed = work
            .send(
                crate::ControllerCommand::ListSessions {
                    offset: 0,
                    limit: 10,
                    expected_revision: None,
                },
                deadline,
            )
            .await
            .unwrap();
        match work.read_response().await.unwrap() {
            ControllerResponse::Sessions { command_id, .. } => assert_eq!(command_id, listed),
            other => panic!("the second connection was not served: {other:?}"),
        }

        // And the screen is still the first connection's to drive.
        watch
            .send_screen(ScreenFrameCapability::Observe, &ticket)
            .await
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), watch.read_incoming())
            .await
            .expect("the screen stopped when the second connection opened")
            .unwrap();
        assert!(
            matches!(frame, crate::ControllerIncoming::Screen(_)),
            "the watching connection stopped carrying screen frames",
        );

        drop(watch);
        drop(work);
        let _ = watching.await;
        let _ = working.await;
    }

    /// A computer with screen sharing switched off says so.
    ///
    /// A ticket used to be issued whether or not there was a screen behind it, and then no frame
    /// ever came: the phone showed an enabled Open screen and a spinner that never resolved.
    #[tokio::test]
    async fn a_computer_that_shares_no_screen_says_so_rather_than_sending_nothing() {
        let host_private = StaticPrivateKey::from_fixture_bytes([43; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([44; 32]);
        let authority = Arc::new(Authority {
            value: Mutex::new(authority_with_screens(&host_private, &device_private)),
            host_private: host_private.clone(),
        });
        let provider: Arc<dyn ControllerAuthorityProvider> = authority.clone();
        // The screen source a computer that is not sharing has: none.
        let backends = Arc::new(Backends {
            screens: false,
            ..Backends::default()
        });
        let (client, mut server) = tokio::io::duplex(8 * 1024);
        let server_task = tokio::spawn(async move {
            serve_authenticated_stdio_stream(
                &mut server,
                provider,
                backends,
                CancellationToken::new(),
            )
            .await
        });
        let mut channel = crate::ControllerClientChannel::connect(
            client,
            1,
            2,
            9,
            HostStaticPublicKey(host_public_key_from_private(&host_private).0),
            device_private,
            CapabilitySet::default().with(SecurityCapability::ObserveScreens),
            &mut crate::SystemHandshakeEntropy,
        )
        .await
        .unwrap();

        let deadline = unix_millis().saturating_add(10_000);
        let command_id = channel
            .send(crate::ControllerCommand::OpenScreen, deadline)
            .await
            .unwrap();
        match channel.read_response().await.unwrap() {
            ControllerResponse::Error {
                command_id: answered,
                code,
                completion_unknown,
            } => {
                assert_eq!(answered, command_id);
                assert_eq!(code, "screen_sharing_off");
                assert!(!completion_unknown);
            }
            other => panic!("a ticket was issued for a screen that is not shared: {other:?}"),
        }
        drop(channel);
        let _ = server_task.await;
    }

    #[tokio::test]
    async fn screen_frames_carry_the_session_and_stop_when_a_capability_is_withdrawn() {
        let host_private = StaticPrivateKey::from_fixture_bytes([41; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([42; 32]);
        let authority = Arc::new(Authority {
            value: Mutex::new(authority_with_screens(&host_private, &device_private)),
            host_private: host_private.clone(),
        });
        let provider: Arc<dyn ControllerAuthorityProvider> = authority.clone();
        let backends = Arc::new(Backends {
            screens: true,
            ..Backends::default()
        });
        let seen = backends.screen_frames.clone();
        let (client, mut server) = tokio::io::duplex(8 * 1024);
        let server_task = tokio::spawn(async move {
            serve_authenticated_stdio_stream(
                &mut server,
                provider,
                backends,
                CancellationToken::new(),
            )
            .await
        });
        let mut channel = crate::ControllerClientChannel::connect(
            client,
            1,
            2,
            9,
            HostStaticPublicKey(host_public_key_from_private(&host_private).0),
            device_private,
            CapabilitySet::default()
                .with(SecurityCapability::ObserveScreens)
                .with(SecurityCapability::ControlPointer),
            &mut crate::SystemHandshakeEntropy,
        )
        .await
        .unwrap();

        let deadline = unix_millis().saturating_add(10_000);
        channel
            .send(crate::ControllerCommand::OpenScreen, deadline)
            .await
            .unwrap();
        let ControllerResponse::ScreenOpened { ticket, .. } =
            channel.read_response().await.unwrap()
        else {
            panic!("a screen session was not opened");
        };

        channel
            .send_screen(ScreenFrameCapability::Observe, &ticket)
            .await
            .unwrap();
        let mut received = Vec::new();
        while received.len() < ECHO_SCREEN_BYTES {
            match tokio::time::timeout(Duration::from_secs(2), channel.read_incoming())
                .await
                .unwrap()
                .unwrap()
            {
                crate::ControllerIncoming::Screen(bytes) => {
                    assert!(bytes.len() <= MAX_SCREEN_PAYLOAD_BYTES);
                    received.extend(bytes);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(received, vec![7; ECHO_SCREEN_BYTES], "sent in two frames");

        // Pointer input rides its own capability, and a wrong ticket never starts a session.
        channel
            .send_screen(ScreenFrameCapability::Pointer, b"pointer-move")
            .await
            .unwrap();
        assert_eq!(
            channel
                .send_screen(ScreenFrameCapability::Keyboard, b"typing")
                .await
                .map_err(|error| error.code),
            Err(ListenerErrorCode::Unauthorized),
            "the device was never granted the keyboard"
        );

        // Wait for that pointer frame to land, so withdrawing the capability cannot race it.
        tokio::time::timeout(Duration::from_secs(2), async {
            while seen.lock().unwrap().len() < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the pointer frame reached the screen session");

        {
            let mut current = authority.value.lock().unwrap();
            current.devices[0].capabilities =
                ControllerCapabilities::default().with(DomainCapability::ObserveScreens);
        }
        channel
            .send_screen(ScreenFrameCapability::Pointer, b"pointer-move")
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), server_task)
                .await
                .unwrap()
                .unwrap()
                .is_err(),
            "a withdrawn pointer capability ends the connection"
        );
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            [
                (ScreenFrameCapability::Observe, 32),
                (ScreenFrameCapability::Pointer, 12),
            ]
        );
    }

    #[tokio::test]
    async fn screen_tickets_are_issued_and_withdrawn_with_the_screen_capabilities() {
        let host_private = StaticPrivateKey::from_fixture_bytes([31; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([32; 32]);
        let authority = Arc::new(Authority {
            value: Mutex::new(authority_with_screens(&host_private, &device_private)),
            host_private: host_private.clone(),
        });
        let provider: Arc<dyn ControllerAuthorityProvider> = authority.clone();
        let (client, mut server) = tokio::io::duplex(8 * 1024);
        // A computer that shares its screen: this is about what happens to the ticket, and a
        // computer with nothing to show is now refused before a ticket is ever issued.
        let backends = Arc::new(Backends {
            screens: true,
            ..Backends::default()
        });
        let server_task = tokio::spawn(async move {
            serve_authenticated_stdio_stream(
                &mut server,
                provider,
                backends,
                CancellationToken::new(),
            )
            .await
        });
        let mut channel = crate::ControllerClientChannel::connect(
            client,
            1,
            2,
            9,
            HostStaticPublicKey(host_public_key_from_private(&host_private).0),
            device_private,
            CapabilitySet::default()
                .with(SecurityCapability::ObserveScreens)
                .with(SecurityCapability::ControlPointer),
            &mut crate::SystemHandshakeEntropy,
        )
        .await
        .unwrap();

        let deadline = unix_millis().saturating_add(10_000);
        let command_id = channel
            .send(crate::ControllerCommand::OpenScreen, deadline)
            .await
            .unwrap();
        let ControllerResponse::ScreenOpened {
            command_id: answered,
            ticket,
            can_control_pointer,
            can_control_keyboard,
        } = channel.read_response().await.unwrap()
        else {
            panic!("a screen session was not opened");
        };
        assert_eq!(answered, command_id);
        assert_eq!(ticket.len(), 32);
        assert!(can_control_pointer, "the device may point");
        assert!(!can_control_keyboard, "the device may not type");

        // A second request replaces the ticket rather than repeating it.
        channel
            .send(crate::ControllerCommand::OpenScreen, deadline)
            .await
            .unwrap();
        let ControllerResponse::ScreenOpened { ticket: second, .. } =
            channel.read_response().await.unwrap()
        else {
            panic!("a screen session was not opened");
        };
        assert_ne!(second, ticket);

        channel
            .send(crate::ControllerCommand::CloseScreen, deadline)
            .await
            .unwrap();
        assert!(matches!(
            channel.read_response().await.unwrap(),
            ControllerResponse::Completed { applied: true, .. }
        ));

        // Taking the capability away stops the next request, even on the open connection.
        {
            let mut current = authority.value.lock().unwrap();
            current.devices[0].capabilities = ControllerCapabilities::default()
                .with(DomainCapability::ObserveSessions)
                .with(DomainCapability::AttachOutput);
        }
        channel
            .send(crate::ControllerCommand::OpenScreen, deadline)
            .await
            .unwrap();
        assert!(
            channel.read_response().await.is_err(),
            "a withdrawn capability ends the connection"
        );
        assert!(server_task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn reusable_client_channel_round_trips_a_bounded_command() {
        let host_private = StaticPrivateKey::from_fixture_bytes([21; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([22; 32]);
        let authority: Arc<dyn ControllerAuthorityProvider> = Arc::new(Authority {
            value: Mutex::new(authority(&host_private, &device_private)),
            host_private: host_private.clone(),
        });
        let (client, mut server) = tokio::io::duplex(8 * 1024);
        let server_task = tokio::spawn(async move {
            serve_authenticated_stdio_stream(
                &mut server,
                authority,
                Arc::new(Backends::default()),
                CancellationToken::new(),
            )
            .await
        });
        let mut channel = crate::ControllerClientChannel::connect(
            client,
            1,
            2,
            9,
            HostStaticPublicKey(host_public_key_from_private(&host_private).0),
            device_private,
            CapabilitySet::default().with(SecurityCapability::ObserveSessions),
            &mut crate::SystemHandshakeEntropy,
        )
        .await
        .unwrap();
        let command_id = channel
            .send(
                crate::ControllerCommand::ListSessions {
                    offset: 0,
                    limit: 100,
                    expected_revision: None,
                },
                unix_millis().saturating_add(10_000),
            )
            .await
            .unwrap();
        assert_eq!(
            channel.read_response().await.unwrap(),
            ControllerResponse::Sessions {
                command_id,
                revision: 1,
                update_sequence: 1,
                sessions: Vec::new(),
                next_offset: None,
            }
        );
        drop(channel);
        assert!(server_task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn stdio_bridge_rejects_pairing_before_any_security_payload() {
        let host_private = StaticPrivateKey::from_fixture_bytes([1; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([2; 32]);
        let authority: Arc<dyn ControllerAuthorityProvider> = Arc::new(Authority {
            value: Mutex::new(authority(&host_private, &device_private)),
            host_private,
        });
        let (mut client, mut server) = tokio::io::duplex(64);
        let task = tokio::spawn(async move {
            serve_authenticated_stdio_stream(
                &mut server,
                authority,
                Arc::new(Backends::default()),
                CancellationToken::new(),
            )
            .await
        });
        ControllerConnectionPurpose::Pair
            .write_to(&mut client)
            .await
            .unwrap();
        assert_eq!(
            task.await.unwrap().unwrap_err().code,
            ListenerErrorCode::Unauthorized
        );
    }
}
