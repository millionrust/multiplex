//! Synthetic loopback client used only by relay conformance tests and benchmarks.

use crate::RelayServerError;
use futures_util::{SinkExt, StreamExt};
use termirust_relay_protocol::{
    RELAY_LOOPBACK_ORIGIN, RELAY_SUBPROTOCOL, RelayAdmissionChallenge, RelayAdmissionCredential,
    RelayAdmissionResult, RelayClientHello, RelayConnectionId, RelayConnectionSequence,
    RelayDiagnosticCode, RelayDirection, RelayEndpointRole, RelayEnvelopeV1, RelayRouteId,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

pub struct SyntheticRelayClient {
    websocket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    route_id: RelayRouteId,
    role: RelayEndpointRole,
    next_sequence: RelayConnectionSequence,
    connection_id: RelayConnectionId,
}

impl std::fmt::Debug for SyntheticRelayClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SyntheticRelayClient")
            .field("route_id", &"[REDACTED]")
            .field("role", &self.role)
            .field("connection_id", &self.connection_id)
            .finish_non_exhaustive()
    }
}

impl SyntheticRelayClient {
    pub async fn connect(
        url: &str,
        route_id: RelayRouteId,
        role: RelayEndpointRole,
        credential: &RelayAdmissionCredential,
    ) -> Result<Self, RelayServerError> {
        let mut request = url.to_owned().into_client_request().map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::InvalidConfig, error)
        })?;
        request.headers_mut().insert(
            "Origin",
            RELAY_LOOPBACK_ORIGIN.parse().expect("static origin header"),
        );
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            RELAY_SUBPROTOCOL.parse().expect("static protocol header"),
        );
        let (mut websocket, response) = connect_async(request).await.map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error)
        })?;
        if response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|value| value.to_str().ok())
            != Some(RELAY_SUBPROTOCOL)
        {
            return Err(RelayServerError::new(RelayDiagnosticCode::InvalidUpgrade));
        }
        websocket
            .send(Message::Binary(
                RelayClientHello { route_id, role }.encode().to_vec().into(),
            ))
            .await
            .map_err(transport_error)?;
        let first = next_binary(&mut websocket).await?;
        if let Ok(result) = RelayAdmissionResult::decode(&first) {
            return Err(RelayServerError::new(result.diagnostic));
        }
        let challenge = RelayAdmissionChallenge::decode(&first)?;
        if challenge.route_id != route_id || challenge.role != role {
            return Err(RelayServerError::new(RelayDiagnosticCode::InvalidProof));
        }
        websocket
            .send(Message::Binary(
                credential.prove(&challenge).encode().to_vec().into(),
            ))
            .await
            .map_err(transport_error)?;
        let result = RelayAdmissionResult::decode(&next_binary(&mut websocket).await?)?;
        if result.diagnostic != RelayDiagnosticCode::Ready {
            return Err(RelayServerError::new(result.diagnostic));
        }
        let connection_id = result
            .connection_id
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::InvalidProof))?;
        Ok(Self {
            websocket,
            route_id,
            role,
            next_sequence: RelayConnectionSequence(0),
            connection_id,
        })
    }

    pub fn connection_id(&self) -> RelayConnectionId {
        self.connection_id
    }

    pub async fn send_ciphertext(&mut self, ciphertext: Vec<u8>) -> Result<(), RelayServerError> {
        let envelope = RelayEnvelopeV1::new(
            self.route_id,
            RelayDirection::for_sender(self.role),
            self.next_sequence,
            ciphertext,
        )?;
        self.next_sequence.0 = self
            .next_sequence
            .0
            .checked_add(1)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::Internal))?;
        self.send_envelope(envelope).await
    }

    pub async fn send_envelope(
        &mut self,
        envelope: RelayEnvelopeV1,
    ) -> Result<(), RelayServerError> {
        self.websocket
            .send(Message::Binary(envelope.encode().into()))
            .await
            .map_err(transport_error)
    }

    pub async fn receive_envelope(&mut self) -> Result<RelayEnvelopeV1, RelayServerError> {
        RelayEnvelopeV1::decode(&next_binary(&mut self.websocket).await?).map_err(Into::into)
    }

    pub async fn next_message(&mut self) -> Option<Result<Message, RelayServerError>> {
        self.websocket
            .next()
            .await
            .map(|result| result.map_err(transport_error))
    }

    pub async fn close(mut self) {
        let _ = self.websocket.close(None).await;
    }
}

async fn next_binary(
    websocket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
) -> Result<Vec<u8>, RelayServerError> {
    match websocket.next().await {
        Some(Ok(Message::Binary(bytes))) => Ok(bytes.to_vec()),
        Some(Ok(Message::Close(Some(frame)))) => RelayDiagnosticCode::ALL
            .into_iter()
            .find(|code| frame.reason == code.as_str())
            .map_or_else(
                || Err(RelayServerError::new(RelayDiagnosticCode::PeerDisconnected)),
                |code| Err(RelayServerError::new(code)),
            ),
        Some(Ok(_)) => Err(RelayServerError::new(
            RelayDiagnosticCode::MalformedEnvelope,
        )),
        Some(Err(error)) => Err(transport_error(error)),
        None => Err(RelayServerError::new(RelayDiagnosticCode::PeerDisconnected)),
    }
}

fn transport_error(error: tokio_tungstenite::tungstenite::Error) -> RelayServerError {
    RelayServerError::with_source(RelayDiagnosticCode::TransportFailed, error)
}
