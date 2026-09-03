use crate::core::{ConnectedEndpoint, RelayCore};
use crate::{
    RelayCoreSnapshot, RelayDiagnosticSnapshot, RelayMetadataStore, RelayServerConfig,
    RelayServerError,
};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use termirust_relay_protocol::{
    ADMISSION_LIFETIME_SECONDS, IDLE_HEARTBEAT_SECONDS, MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES,
    MAX_QUEUE_MESSAGES, RELAY_SUBPROTOCOL, RelayAdmissionProof, RelayAdmissionResult,
    RelayClientHello, RelayDiagnosticCode, RelayEnvelopeV1, RelayRouteId, RelayRouteRegistration,
    RelayRouteState, RelayServerState, validate_websocket_message_len,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, interval, timeout};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::protocol::{
    CloseFrame, Message, WebSocketConfig, frame::coding::CloseCode,
};
use tokio_tungstenite::{WebSocketStream, accept_hdr_async_with_config};
use tokio_util::sync::CancellationToken;

const RELAY_PATH: &str = "/relay/v1";
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

struct UpgradePolicy {
    allowed_origin: String,
}

impl Callback for UpgradePolicy {
    // Tungstenite fixes this callback's error type to an HTTP response.
    #[allow(clippy::result_large_err)]
    fn on_request(
        self,
        request: &Request,
        mut response: Response,
    ) -> Result<Response, ErrorResponse> {
        if !upgrade_is_valid(request, &self.allowed_origin) {
            return Err(tokio_tungstenite::tungstenite::http::Response::builder()
                .status(403)
                .body(Some(
                    RelayDiagnosticCode::OriginRejected.as_str().to_owned(),
                ))
                .expect("static rejection response"));
        }
        response.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            RELAY_SUBPROTOCOL.parse().expect("static protocol header"),
        );
        Ok(response)
    }
}

struct Shared {
    core: Mutex<RelayCore>,
    store: Mutex<Option<RelayMetadataStore>>,
    state: AtomicU8,
}

pub struct RelayServer;

#[derive(Clone)]
pub struct RelayTlsServerConfig {
    inner: Arc<rustls::ServerConfig>,
}

impl RelayTlsServerConfig {
    pub fn new(config: rustls::ServerConfig) -> Self {
        Self {
            inner: Arc::new(config),
        }
    }
}

impl std::fmt::Debug for RelayTlsServerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RelayTlsServerConfig([PROTECTED])")
    }
}

#[derive(Clone)]
pub struct RelayServerHandle {
    address: SocketAddr,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    server_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    tls: bool,
}

impl std::fmt::Debug for RelayServerHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayServerHandle")
            .field("address", &"[LOOPBACK]")
            .field("state", &self.state())
            .finish()
    }
}

impl RelayServer {
    pub async fn start(config: RelayServerConfig) -> Result<RelayServerHandle, RelayServerError> {
        Self::start_inner(config, None).await
    }

    pub async fn start_tls(
        config: RelayServerConfig,
        tls: RelayTlsServerConfig,
    ) -> Result<RelayServerHandle, RelayServerError> {
        Self::start_inner(config, Some(TlsAcceptor::from(tls.inner))).await
    }

    async fn start_inner(
        config: RelayServerConfig,
        tls: Option<TlsAcceptor>,
    ) -> Result<RelayServerHandle, RelayServerError> {
        let config = config.validate()?;
        let store = RelayMetadataStore::acquire(&config.state_path)?;
        let registrations = store.load()?;
        let core = RelayCore::new(registrations, config.limits, unix_millis())?;
        let listener = TcpListener::bind(config.bind).await.map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error)
        })?;
        let address = listener.local_addr().map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error)
        })?;
        if !address.ip().is_loopback() {
            return Err(RelayServerError::new(RelayDiagnosticCode::LoopbackRequired));
        }

        let shared = Arc::new(Shared {
            core: Mutex::new(core),
            store: Mutex::new(Some(store)),
            state: AtomicU8::new(state_to_u8(RelayServerState::Loading)),
        });
        let shutdown = CancellationToken::new();
        let task_shared = Arc::clone(&shared);
        let task_shutdown = shutdown.clone();
        let allowed_origin = config.allowed_origin.clone();
        let handshake_limit = Arc::new(Semaphore::new(config.limits.unauthenticated_handshakes));
        let uses_tls = tls.is_some();
        let server_task = tokio::spawn(async move {
            serve_loop(
                listener,
                task_shared,
                task_shutdown,
                allowed_origin,
                handshake_limit,
                tls,
            )
            .await;
        });
        shared.state.store(
            state_to_u8(RelayServerState::ListeningLoopback),
            Ordering::Release,
        );
        Ok(RelayServerHandle {
            address,
            shared,
            shutdown,
            server_task: Arc::new(Mutex::new(Some(server_task))),
            tls: uses_tls,
        })
    }
}

impl RelayServerHandle {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn websocket_url(&self) -> String {
        let scheme = if self.tls { "wss" } else { "ws" };
        format!("{scheme}://{}{}", self.address, RELAY_PATH)
    }

    pub fn state(&self) -> RelayServerState {
        u8_to_state(self.shared.state.load(Ordering::Acquire))
    }

    pub async fn register_route(
        &self,
        registration: RelayRouteRegistration,
    ) -> Result<(), RelayServerError> {
        let mut core = self.shared.core.lock().await;
        let next = core.registrations_with_added_route(registration.clone())?;
        self.shared
            .store
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::Shutdown))?
            .commit(&next)?;
        core.insert_registration(registration, unix_millis())
    }

    pub async fn revoke_route(&self, route_id: RelayRouteId) -> Result<(), RelayServerError> {
        let mut core = self.shared.core.lock().await;
        let next = core.revoked_registrations(route_id)?;
        let epoch = next
            .iter()
            .find(|route| route.route_id == route_id)
            .map(|route| route.revocation_epoch.0)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::UnknownRoute))?;
        self.shared
            .store
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::Shutdown))?
            .commit(&next)?;
        core.apply_revocation(route_id, epoch)
    }

    pub async fn route_state(
        &self,
        route_id: RelayRouteId,
    ) -> Result<RelayRouteState, RelayServerError> {
        self.shared.core.lock().await.route_state(route_id)
    }

    pub async fn snapshot(&self) -> RelayCoreSnapshot {
        self.shared.core.lock().await.snapshot()
    }

    pub async fn diagnostics(&self) -> RelayDiagnosticSnapshot {
        self.shared.core.lock().await.diagnostics()
    }

    pub async fn shutdown(&self) -> Result<(), RelayServerError> {
        self.shared
            .state
            .store(state_to_u8(RelayServerState::Draining), Ordering::Release);
        self.shutdown.cancel();
        let Some(task) = self.server_task.lock().await.take() else {
            return Ok(());
        };
        timeout(SHUTDOWN_TIMEOUT, task)
            .await
            .map_err(|_| RelayServerError::new(RelayDiagnosticCode::Shutdown))?
            .map_err(|error| RelayServerError::with_source(RelayDiagnosticCode::Internal, error))?;
        self.shared.store.lock().await.take();
        Ok(())
    }
}

async fn serve_loop(
    listener: TcpListener,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    allowed_origin: String,
    handshake_limit: Arc<Semaphore>,
    tls: Option<TlsAcceptor>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else {
                    shared.state.store(state_to_u8(RelayServerState::Failed), Ordering::Release);
                    break;
                };
                let Ok(permit) = Arc::clone(&handshake_limit).try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let connection_shared = Arc::clone(&shared);
                let connection_shutdown = shutdown.child_token();
                let origin = allowed_origin.clone();
                let tls = tls.clone();
                connections.spawn(async move {
                    if let Some(acceptor) = tls {
                        match timeout(
                            Duration::from_secs(ADMISSION_LIFETIME_SECONDS),
                            acceptor.accept(stream),
                        ).await {
                            Ok(Ok(stream)) => {
                                if let Err(error) = handle_connection(
                                    stream,
                                    peer,
                                    connection_shared,
                                    connection_shutdown,
                                    origin,
                                    permit,
                                ).await {
                                    emit_test_diagnostic("connection", error.code());
                                }
                            }
                            Ok(Err(_)) => emit_test_diagnostic(
                                "tls",
                                RelayDiagnosticCode::TransportFailed,
                            ),
                            Err(_) => emit_test_diagnostic(
                                "tls",
                                RelayDiagnosticCode::ExpiredProof,
                            ),
                        }
                    } else {
                        if let Err(error) = handle_connection(
                            stream,
                            peer,
                            connection_shared,
                            connection_shutdown,
                            origin,
                            permit,
                        ).await {
                            emit_test_diagnostic("connection", error.code());
                        }
                    }
                });
            }
        }
    }

    shutdown.cancel();
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    while !connections.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || timeout(remaining, connections.join_next()).await.is_err() {
            connections.abort_all();
            break;
        }
    }
    shared
        .state
        .store(state_to_u8(RelayServerState::Stopped), Ordering::Release);
}

fn emit_test_diagnostic(stage: &str, code: RelayDiagnosticCode) {
    if std::env::var_os("TERMIRUST_RELAY_TEST_DIAGNOSTICS").is_some() {
        eprintln!(
            "relay test diagnostic: stage={stage} code={}",
            code.as_str()
        );
    }
}

fn emit_endpoint_test_diagnostic(
    event: &str,
    role: termirust_relay_protocol::RelayEndpointRole,
    code: Option<RelayDiagnosticCode>,
) {
    if std::env::var_os("TERMIRUST_RELAY_TEST_DIAGNOSTICS").is_none() {
        return;
    }
    let role = match role {
        termirust_relay_protocol::RelayEndpointRole::Host => "host",
        termirust_relay_protocol::RelayEndpointRole::Controller => "controller",
    };
    if let Some(code) = code {
        eprintln!(
            "relay test diagnostic: event={event} role={role} code={}",
            code.as_str()
        );
    } else {
        eprintln!("relay test diagnostic: event={event} role={role}");
    }
}

async fn handle_connection<S>(
    stream: S,
    peer: SocketAddr,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    allowed_origin: String,
    handshake_permit: OwnedSemaphorePermit,
) -> Result<(), RelayServerError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    if !peer.ip().is_loopback() {
        return Err(RelayServerError::new(RelayDiagnosticCode::LoopbackRequired));
    }
    let callback = UpgradePolicy { allowed_origin };
    let websocket_config = WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(16 * 1024)
        .max_write_buffer_size(MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES + 32 * 1024)
        .max_message_size(Some(MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES));
    let mut websocket = accept_hdr_async_with_config(stream, callback, Some(websocket_config))
        .await
        .map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::InvalidUpgrade, error)
        })?;

    let endpoint_cancel = shutdown.child_token();
    let (outgoing_tx, outgoing_rx) = mpsc::channel(MAX_QUEUE_MESSAGES);
    let admitted = timeout(
        Duration::from_secs(ADMISSION_LIFETIME_SECONDS),
        authenticate(
            &mut websocket,
            peer,
            &shared,
            outgoing_tx,
            endpoint_cancel.clone(),
        ),
    )
    .await
    .map_err(|_| RelayServerError::new(RelayDiagnosticCode::ExpiredProof))??;
    drop(handshake_permit);

    emit_endpoint_test_diagnostic("admitted", admitted.role, None);
    let result = run_forwarding(websocket, outgoing_rx, &shared, &admitted, endpoint_cancel).await;
    emit_endpoint_test_diagnostic(
        "forwarding-ended",
        admitted.role,
        result.as_ref().err().map(RelayServerError::code),
    );
    shared.core.lock().await.disconnect(&admitted);
    result
}

async fn authenticate<S>(
    websocket: &mut WebSocketStream<S>,
    peer: SocketAddr,
    shared: &Arc<Shared>,
    outgoing_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
) -> Result<ConnectedEndpoint, RelayServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let hello_bytes = next_binary(websocket).await?;
    let hello = RelayClientHello::decode(&hello_bytes)?;
    let challenge = shared.core.lock().await.issue_challenge(
        hello.route_id,
        hello.role,
        peer.ip(),
        unix_seconds(),
    );
    let challenge = match challenge {
        Ok(challenge) => challenge,
        Err(error) => {
            send_rejection(websocket, error.code()).await;
            return Err(error);
        }
    };
    websocket
        .send(Message::Binary(challenge.encode().to_vec().into()))
        .await
        .map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error)
        })?;
    let proof_bytes = next_binary(websocket).await?;
    let proof = RelayAdmissionProof::decode(&proof_bytes)?;
    let admitted =
        shared
            .core
            .lock()
            .await
            .admit(proof, peer.ip(), unix_seconds(), outgoing_tx, cancel);
    let admitted = match admitted {
        Ok(admitted) => admitted,
        Err(error) => {
            send_rejection(websocket, error.code()).await;
            return Err(error);
        }
    };
    websocket
        .send(Message::Binary(
            RelayAdmissionResult::accepted(admitted.connection_id)
                .encode()
                .to_vec()
                .into(),
        ))
        .await
        .map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error)
        })?;
    Ok(admitted)
}

async fn run_forwarding<S>(
    websocket: WebSocketStream<S>,
    mut outgoing_rx: mpsc::Receiver<Vec<u8>>,
    shared: &Arc<Shared>,
    endpoint: &ConnectedEndpoint,
    cancel: CancellationToken,
) -> Result<(), RelayServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut source) = websocket.split();
    let mut heartbeat = interval(Duration::from_secs(IDLE_HEARTBEAT_SECONDS / 2));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let code = cancellation_code(shared, endpoint).await;
                let _ = sink.send(close_message(code)).await;
                return Ok(());
            }
            maybe_outgoing = outgoing_rx.recv() => {
                let Some(encoded) = maybe_outgoing else {
                    emit_endpoint_test_diagnostic("outgoing-closed", endpoint.role, None);
                    return Ok(());
                };
                let encoded_len = encoded.len();
                sink.send(Message::Binary(encoded.into())).await.map_err(|error| {
                    RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error)
                })?;
                emit_endpoint_test_diagnostic("outgoing-sent", endpoint.role, None);
                shared.core.lock().await.delivered(endpoint, encoded_len);
            }
            maybe_message = source.next() => {
                let Some(message) = maybe_message else {
                    return Ok(());
                };
                match message.map_err(|error| RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error))? {
                    Message::Binary(bytes) => {
                        validate_websocket_message_len(bytes.len())?;
                        let encoded = bytes.to_vec();
                        let envelope = RelayEnvelopeV1::decode(&encoded)?;
                        if let Err(error) = shared.core.lock().await.forward(endpoint, envelope, encoded, unix_millis()) {
                            let _ = sink.send(close_message(error.code())).await;
                            return Err(error);
                        }
                        emit_endpoint_test_diagnostic("incoming-forwarded", endpoint.role, None);
                    }
                    Message::Ping(payload) => {
                        sink.send(Message::Pong(payload)).await.map_err(|error| {
                            RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error)
                        })?;
                    }
                    Message::Pong(_) => {}
                    Message::Close(_) => {
                        emit_endpoint_test_diagnostic("peer-close", endpoint.role, None);
                        return Ok(());
                    }
                    Message::Text(_) | Message::Frame(_) => {
                        let error = RelayServerError::new(RelayDiagnosticCode::MalformedEnvelope);
                        let _ = sink.send(close_message(error.code())).await;
                        return Err(error);
                    }
                }
            }
            _ = heartbeat.tick() => {
                sink.send(Message::Ping(Vec::new().into())).await.map_err(|error| {
                    RelayServerError::with_source(RelayDiagnosticCode::IdleTimeout, error)
                })?;
            }
        }
    }
}

async fn cancellation_code(
    shared: &Arc<Shared>,
    endpoint: &ConnectedEndpoint,
) -> RelayDiagnosticCode {
    if u8_to_state(shared.state.load(Ordering::Acquire)) == RelayServerState::Draining {
        return RelayDiagnosticCode::Shutdown;
    }
    shared
        .core
        .lock()
        .await
        .close_code(endpoint.route_id)
        .unwrap_or(RelayDiagnosticCode::PeerDisconnected)
}

async fn next_binary<S>(websocket: &mut WebSocketStream<S>) -> Result<Vec<u8>, RelayServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match websocket.next().await {
        Some(Ok(Message::Binary(bytes))) => Ok(bytes.to_vec()),
        Some(Ok(_)) => Err(RelayServerError::new(RelayDiagnosticCode::InvalidProof)),
        Some(Err(error)) => Err(RelayServerError::with_source(
            RelayDiagnosticCode::TransportFailed,
            error,
        )),
        None => Err(RelayServerError::new(RelayDiagnosticCode::PeerDisconnected)),
    }
}

async fn send_rejection<S>(websocket: &mut WebSocketStream<S>, code: RelayDiagnosticCode)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _ = websocket
        .send(Message::Binary(
            RelayAdmissionResult::rejected(code)
                .encode()
                .to_vec()
                .into(),
        ))
        .await;
    let _ = websocket.send(close_message(code)).await;
}

fn upgrade_is_valid(request: &Request, allowed_origin: &str) -> bool {
    let origin_matches = request
        .headers()
        .get("Origin")
        .and_then(|value| value.to_str().ok())
        == Some(allowed_origin);
    let protocol_matches = request
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|protocol| protocol == RELAY_SUBPROTOCOL)
        });
    request.uri().path() == RELAY_PATH && origin_matches && protocol_matches
}

fn close_message(code: RelayDiagnosticCode) -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::Policy,
        reason: code.as_str().into(),
    }))
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

fn state_to_u8(state: RelayServerState) -> u8 {
    match state {
        RelayServerState::Stopped => 0,
        RelayServerState::Loading => 1,
        RelayServerState::ListeningLoopback => 2,
        RelayServerState::Draining => 3,
        RelayServerState::Failed => 4,
    }
}

fn u8_to_state(value: u8) -> RelayServerState {
    match value {
        0 => RelayServerState::Stopped,
        1 => RelayServerState::Loading,
        2 => RelayServerState::ListeningLoopback,
        3 => RelayServerState::Draining,
        _ => RelayServerState::Failed,
    }
}
