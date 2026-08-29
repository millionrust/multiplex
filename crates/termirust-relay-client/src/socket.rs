use futures_util::{SinkExt, StreamExt};
use rustls::{ClientConfig, RootCertStore};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use termirust_relay_protocol::{
    ADMISSION_LIFETIME_SECONDS, MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES, RELAY_LOOPBACK_ORIGIN,
    RELAY_SUBPROTOCOL, RelayAdmissionChallenge, RelayAdmissionCredential, RelayAdmissionResult,
    RelayClientHello, RelayConnectionId, RelayConnectionSequence, RelayDiagnosticCode,
    RelayDirection, RelayEnvelopeV1,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
};
use x509_parser::parse_x509_certificate;

use crate::{
    RelayClientRole, RelayEndpointConfig, RelayRouteError, RelayRouteErrorCode, RelaySpkiPin,
};

type RelayWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone)]
pub struct RelayTlsClientConfig {
    config: Arc<ClientConfig>,
}

impl std::fmt::Debug for RelayTlsClientConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RelayTlsClientConfig([PROTECTED])")
    }
}

impl RelayTlsClientConfig {
    pub fn from_rustls(config: ClientConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    pub fn system_roots() -> Result<Self, RelayRouteError> {
        let native = rustls_native_certs::load_native_certs();
        if native.certs.is_empty() {
            return Err(RelayRouteError::new(RelayRouteErrorCode::TlsFailed));
        }
        let mut roots = RootCertStore::empty();
        let (added, _) = roots.add_parsable_certificates(native.certs);
        if added == 0 {
            return Err(RelayRouteError::new(RelayRouteErrorCode::TlsFailed));
        }
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::TlsFailed))?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self::from_rustls(config))
    }
}

pub struct RelaySocket {
    websocket: RelayWebSocket,
    route_id: termirust_relay_protocol::RelayRouteId,
    role: RelayClientRole,
    next_send_sequence: RelayConnectionSequence,
    next_receive_sequence: RelayConnectionSequence,
    connection_id: RelayConnectionId,
}

impl std::fmt::Debug for RelaySocket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelaySocket")
            .field("route", &"[REDACTED]")
            .field("role", &self.role)
            .field("connection", &self.connection_id)
            .finish_non_exhaustive()
    }
}

impl RelaySocket {
    pub async fn connect(
        endpoint: &RelayEndpointConfig,
        role: RelayClientRole,
        credential: &RelayAdmissionCredential,
    ) -> Result<Self, RelayRouteError> {
        let tls = RelayTlsClientConfig::system_roots()?;
        Self::connect_with_tls(endpoint, role, credential, tls).await
    }

    pub async fn connect_with_tls(
        endpoint: &RelayEndpointConfig,
        role: RelayClientRole,
        credential: &RelayAdmissionCredential,
        tls: RelayTlsClientConfig,
    ) -> Result<Self, RelayRouteError> {
        let mut request = endpoint
            .wss_url
            .expose_for_connection()
            .into_client_request()
            .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::InvalidConfig))?;
        request.headers_mut().insert(
            "Origin",
            RELAY_LOOPBACK_ORIGIN
                .parse()
                .expect("static relay Origin header"),
        );
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            RELAY_SUBPROTOCOL
                .parse()
                .expect("static relay protocol header"),
        );
        let websocket_config = WebSocketConfig::default()
            .read_buffer_size(64 * 1024)
            .write_buffer_size(64 * 1024)
            .max_write_buffer_size(2 * MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES)
            .max_message_size(Some(MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES));
        let (mut websocket, response) = connect_async_tls_with_config(
            request,
            Some(websocket_config),
            true,
            Some(Connector::Rustls(tls.config)),
        )
        .await
        .map_err(map_connect_error)?;
        if response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|value| value.to_str().ok())
            != Some(RELAY_SUBPROTOCOL)
        {
            return Err(RelayRouteError::new(RelayRouteErrorCode::UpgradeRejected));
        }
        verify_peer_pin(&websocket, endpoint.expected_spki_pin)?;

        let protocol_role = role.protocol_role();
        websocket
            .send(Message::Binary(
                RelayClientHello {
                    route_id: endpoint.route_id,
                    role: protocol_role,
                }
                .encode()
                .to_vec()
                .into(),
            ))
            .await
            .map_err(map_transport_error)?;
        let first = next_binary(&mut websocket).await?;
        if let Ok(result) = RelayAdmissionResult::decode(&first) {
            return Err(map_admission_result(result.diagnostic));
        }
        let challenge = RelayAdmissionChallenge::decode(&first)
            .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::MalformedFrame))?;
        validate_challenge(endpoint, protocol_role, credential, &challenge)?;
        websocket
            .send(Message::Binary(
                credential.prove(&challenge).encode().to_vec().into(),
            ))
            .await
            .map_err(map_transport_error)?;
        let result = RelayAdmissionResult::decode(&next_binary(&mut websocket).await?)
            .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::MalformedFrame))?;
        if result.diagnostic != RelayDiagnosticCode::Ready {
            return Err(map_admission_result(result.diagnostic));
        }
        let connection_id = result
            .connection_id
            .ok_or_else(|| RelayRouteError::new(RelayRouteErrorCode::AdmissionRejected))?;
        Ok(Self {
            websocket,
            route_id: endpoint.route_id,
            role,
            next_send_sequence: RelayConnectionSequence(0),
            next_receive_sequence: RelayConnectionSequence(0),
            connection_id,
        })
    }

    pub async fn send(&mut self, bytes: Vec<u8>) -> Result<(), RelayRouteError> {
        let envelope = RelayEnvelopeV1::new(
            self.route_id,
            RelayDirection::for_sender(self.role.protocol_role()),
            self.next_send_sequence,
            bytes,
        )
        .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::FrameLimit))?;
        self.next_send_sequence.0 = self
            .next_send_sequence
            .0
            .checked_add(1)
            .ok_or_else(|| RelayRouteError::new(RelayRouteErrorCode::FrameLimit))?;
        self.websocket
            .send(Message::Binary(envelope.encode().into()))
            .await
            .map_err(map_transport_error)
    }

    pub async fn receive(&mut self) -> Result<Vec<u8>, RelayRouteError> {
        loop {
            match self.websocket.next().await {
                Some(Ok(Message::Binary(bytes))) => {
                    let envelope = RelayEnvelopeV1::decode(&bytes)
                        .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::MalformedFrame))?;
                    let expected_direction = RelayDirection::for_sender(match self.role {
                        RelayClientRole::Host => {
                            termirust_relay_protocol::RelayEndpointRole::Controller
                        }
                        RelayClientRole::DesktopController => {
                            termirust_relay_protocol::RelayEndpointRole::Host
                        }
                    });
                    if envelope.route_id() != self.route_id
                        || envelope.direction() != expected_direction
                        || envelope.sequence() != self.next_receive_sequence
                    {
                        return Err(RelayRouteError::new(RelayRouteErrorCode::SequenceMismatch));
                    }
                    self.next_receive_sequence.0 =
                        self.next_receive_sequence.0.checked_add(1).ok_or_else(|| {
                            RelayRouteError::new(RelayRouteErrorCode::SequenceMismatch)
                        })?;
                    return Ok(envelope.ciphertext().to_vec());
                }
                Some(Ok(Message::Ping(_))) => {
                    self.websocket.flush().await.map_err(map_transport_error)?;
                }
                Some(Ok(Message::Close(_))) | None => {
                    return Err(RelayRouteError::new(RelayRouteErrorCode::PeerDisconnected));
                }
                Some(Ok(_)) => {
                    return Err(RelayRouteError::new(RelayRouteErrorCode::MalformedFrame));
                }
                Some(Err(error)) => return Err(map_transport_error(error)),
            }
        }
    }

    pub async fn close(mut self) {
        let _ = self.websocket.close(None).await;
    }
}

pub fn spki_pin_from_certificate(certificate_der: &[u8]) -> Result<RelaySpkiPin, RelayRouteError> {
    let (_, certificate) = parse_x509_certificate(certificate_der)
        .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::TlsFailed))?;
    let digest = Sha256::digest(certificate.tbs_certificate.subject_pki.raw);
    Ok(RelaySpkiPin(digest.into()))
}

fn verify_peer_pin(
    websocket: &RelayWebSocket,
    expected: RelaySpkiPin,
) -> Result<(), RelayRouteError> {
    let MaybeTlsStream::Rustls(stream) = websocket.get_ref() else {
        return Err(RelayRouteError::new(RelayRouteErrorCode::TlsFailed));
    };
    let certificate = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certificates| certificates.first())
        .ok_or_else(|| RelayRouteError::new(RelayRouteErrorCode::TlsFailed))?;
    if spki_pin_from_certificate(certificate.as_ref())? != expected {
        return Err(RelayRouteError::new(RelayRouteErrorCode::SpkiPinMismatch));
    }
    Ok(())
}

fn validate_challenge(
    endpoint: &RelayEndpointConfig,
    role: termirust_relay_protocol::RelayEndpointRole,
    credential: &RelayAdmissionCredential,
    challenge: &RelayAdmissionChallenge,
) -> Result<(), RelayRouteError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::Internal))?
        .as_secs();
    if challenge.route_id != endpoint.route_id
        || challenge.role != role
        || challenge.verifier != credential.verifier()
        || challenge.expires_at_unix_seconds < now
        || challenge.expires_at_unix_seconds.saturating_sub(now) > ADMISSION_LIFETIME_SECONDS
    {
        return Err(RelayRouteError::new(RelayRouteErrorCode::AdmissionRejected));
    }
    if challenge.revocation_epoch != endpoint.binding.relay_epoch {
        return Err(RelayRouteError::new(
            RelayRouteErrorCode::RelayEpochMismatch,
        ));
    }
    Ok(())
}

async fn next_binary(websocket: &mut RelayWebSocket) -> Result<Vec<u8>, RelayRouteError> {
    match websocket.next().await {
        Some(Ok(Message::Binary(bytes))) => Ok(bytes.to_vec()),
        Some(Ok(Message::Close(_))) | None => {
            Err(RelayRouteError::new(RelayRouteErrorCode::PeerDisconnected))
        }
        Some(Ok(_)) => Err(RelayRouteError::new(RelayRouteErrorCode::MalformedFrame)),
        Some(Err(error)) => Err(map_transport_error(error)),
    }
}

fn map_connect_error(error: tokio_tungstenite::tungstenite::Error) -> RelayRouteError {
    use tokio_tungstenite::tungstenite::Error;
    let code = match error {
        Error::Tls(_) => RelayRouteErrorCode::TlsFailed,
        Error::Http(_) | Error::HttpFormat(_) => RelayRouteErrorCode::UpgradeRejected,
        Error::Url(_) => RelayRouteErrorCode::InvalidConfig,
        _ => RelayRouteErrorCode::ConnectFailed,
    };
    RelayRouteError::new(code)
}

fn map_transport_error(_: tokio_tungstenite::tungstenite::Error) -> RelayRouteError {
    RelayRouteError::new(RelayRouteErrorCode::PeerDisconnected)
}

fn map_admission_result(diagnostic: RelayDiagnosticCode) -> RelayRouteError {
    let code = match diagnostic {
        RelayDiagnosticCode::Revoked | RelayDiagnosticCode::RevokedLive => {
            RelayRouteErrorCode::RelayEpochMismatch
        }
        RelayDiagnosticCode::FrameLimit => RelayRouteErrorCode::FrameLimit,
        RelayDiagnosticCode::QueueLimit => RelayRouteErrorCode::QueuePressure,
        _ => RelayRouteErrorCode::AdmissionRejected,
    };
    RelayRouteError::new(code)
}
