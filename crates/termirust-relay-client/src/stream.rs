use std::sync::Arc;
use std::time::Duration;
use termirust_relay_protocol::MAX_CIPHERTEXT_PAYLOAD_BYTES;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    RelayClientRole, RelayClientState, RelayEndpointConfig, RelayRouteError, RelayRouteErrorCode,
    RelaySecretStore, RelaySecretStoreError, RelaySocket, RelayTlsClientConfig,
};

const STREAM_BUFFER_BYTES: usize = 256 * 1024;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub struct RelayByteStream {
    inner: DuplexStream,
}

impl std::fmt::Debug for RelayByteStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RelayByteStream([REDACTED])")
    }
}

impl AsyncRead for RelayByteStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl AsyncWrite for RelayByteStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

pub struct RelayConnectionHandle {
    stream: Option<RelayByteStream>,
    cancel: CancellationToken,
    task: Option<JoinHandle<Result<(), RelayRouteError>>>,
    state: watch::Receiver<RelayClientState>,
}

impl std::fmt::Debug for RelayConnectionHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayConnectionHandle")
            .field("state", &*self.state.borrow())
            .field("transport", &"[REDACTED]")
            .finish()
    }
}

impl RelayConnectionHandle {
    pub async fn connect(
        endpoint: RelayEndpointConfig,
        role: RelayClientRole,
        secrets: Arc<dyn RelaySecretStore>,
    ) -> Result<Self, RelayRouteError> {
        let tls = RelayTlsClientConfig::system_roots()?;
        Self::connect_with_tls(endpoint, role, secrets, tls).await
    }

    pub async fn connect_with_tls(
        endpoint: RelayEndpointConfig,
        role: RelayClientRole,
        secrets: Arc<dyn RelaySecretStore>,
        tls: RelayTlsClientConfig,
    ) -> Result<Self, RelayRouteError> {
        let secret = secrets
            .get(&endpoint.credential_ref)
            .map_err(map_store_error)?;
        let credential = secret.admission_credential();
        let socket = RelaySocket::connect_with_tls(&endpoint, role, &credential, tls).await?;
        let (application, relay) = tokio::io::duplex(STREAM_BUFFER_BYTES);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let (state_tx, state) = watch::channel(RelayClientState::WaitingPeer);
        let task = tokio::spawn(async move {
            let result = pump(socket, relay, task_cancel, &state_tx).await;
            let final_state = match &result {
                Ok(()) => RelayClientState::Disabled,
                Err(error) if error.code == RelayRouteErrorCode::Cancelled => {
                    RelayClientState::Disabled
                }
                Err(error) if error.code == RelayRouteErrorCode::RelayEpochMismatch => {
                    RelayClientState::Revoked
                }
                Err(_) => RelayClientState::Failed,
            };
            let _ = state_tx.send(final_state);
            result
        });
        Ok(Self {
            stream: Some(RelayByteStream { inner: application }),
            cancel,
            task: Some(task),
            state,
        })
    }

    pub fn take_stream(&mut self) -> Option<RelayByteStream> {
        self.stream.take()
    }

    pub fn state(&self) -> RelayClientState {
        *self.state.borrow()
    }

    pub fn subscribe_state(&self) -> watch::Receiver<RelayClientState> {
        self.state.clone()
    }

    pub async fn shutdown(mut self) -> Result<(), RelayRouteError> {
        self.cancel.cancel();
        self.stream.take();
        let Some(mut task) = self.task.take() else {
            return Ok(());
        };
        match tokio::time::timeout(SHUTDOWN_GRACE, &mut task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) if error.code == RelayRouteErrorCode::Cancelled => Ok(()),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(RelayRouteError::new(RelayRouteErrorCode::Internal)),
            Err(_) => {
                task.abort();
                Err(RelayRouteError::new(RelayRouteErrorCode::Cancelled))
            }
        }
    }
}

impl Drop for RelayConnectionHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn pump(
    mut socket: RelaySocket,
    stream: DuplexStream,
    cancel: CancellationToken,
    state: &watch::Sender<RelayClientState>,
) -> Result<(), RelayRouteError> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut outbound = vec![0_u8; MAX_CIPHERTEXT_PAYLOAD_BYTES.min(64 * 1024)];
    let _ = state.send(RelayClientState::AuthenticatingController);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                socket.close().await;
                return Err(RelayRouteError::new(RelayRouteErrorCode::Cancelled));
            }
            incoming = socket.receive() => {
                let bytes = incoming?;
                writer.write_all(&bytes).await.map_err(|_| {
                    RelayRouteError::new(RelayRouteErrorCode::PeerDisconnected)
                })?;
                let _ = state.send(RelayClientState::Ready);
            }
            read = reader.read(&mut outbound) => {
                let read = read.map_err(|_| RelayRouteError::new(RelayRouteErrorCode::PeerDisconnected))?;
                if read == 0 {
                    socket.close().await;
                    return Ok(());
                }
                socket.send(outbound[..read].to_vec()).await?;
            }
        }
    }
}

fn map_store_error(error: RelaySecretStoreError) -> RelayRouteError {
    let code = match error {
        RelaySecretStoreError::Missing | RelaySecretStoreError::Invalid => {
            RelayRouteErrorCode::CredentialLost
        }
        RelaySecretStoreError::Locked | RelaySecretStoreError::PermissionDenied => {
            RelayRouteErrorCode::CredentialLocked
        }
        RelaySecretStoreError::Unavailable => RelayRouteErrorCode::CredentialLost,
    };
    RelayRouteError::new(code)
}
