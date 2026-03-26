use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::PublicKey;
use russh::keys::key::PrivateKeyWithHashAlg;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Builder;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::models::{AuthConfig, ConnectRequest};
use crate::storage::{HostKeyDecision, KnownHostStore};
use crate::terminal::TerminalSize;

#[derive(Debug)]
pub enum SessionCommand {
    Input(Vec<u8>),
    Resize(TerminalSize),
    Disconnect,
}

#[derive(Clone, Debug)]
pub enum SshEvent {
    Connected {
        session_id: u64,
        trusted_new_host: bool,
    },
    Output {
        session_id: u64,
        data: Vec<u8>,
    },
    Error {
        session_id: u64,
        message: String,
    },
    Disconnected {
        session_id: u64,
        message: String,
    },
}

pub struct SessionRuntimeHandle {
    pub command_tx: UnboundedSender<SessionCommand>,
}

pub fn spawn_session(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    event_tx: Sender<SshEvent>,
) -> SessionRuntimeHandle {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let session_id = request.session_id;
    let thread_name = format!("ssh-session-{session_id}");

    let _ = thread::Builder::new().name(thread_name).spawn(move || {
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .expect("failed to build tokio runtime for SSH session");

        runtime.block_on(async move {
            if let Err(error) =
                run_session(request, known_hosts, command_rx, event_tx.clone()).await
            {
                let message = error.to_string();
                let _ = event_tx.send(SshEvent::Error {
                    session_id,
                    message: message.clone(),
                });
                let _ = event_tx.send(SshEvent::Disconnected {
                    session_id,
                    message,
                });
            }
        });
    });

    SessionRuntimeHandle { command_tx }
}

async fn run_session(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    mut command_rx: UnboundedReceiver<SessionCommand>,
    event_tx: Sender<SshEvent>,
) -> Result<()> {
    let trusted_new_host = Arc::new(AtomicBool::new(false));
    let handler = SessionHandler {
        endpoint: request.known_host_key(),
        known_hosts,
        trusted_new_host: trusted_new_host.clone(),
    };

    let config = Arc::new(client::Config::default());
    let mut handle = client::connect(config, request.address(), handler)
        .await
        .context("Unable to open the SSH transport")?;

    authenticate(&mut handle, &request).await?;

    let channel = handle
        .channel_open_session()
        .await
        .context("Unable to open an SSH session channel")?;

    channel
        .request_pty(true, "xterm-256color", 160, 48, 0, 0, &[])
        .await
        .context("Unable to allocate a remote PTY")?;
    channel
        .request_shell(true)
        .await
        .context("Unable to start an interactive shell")?;

    let _ = event_tx.send(SshEvent::Connected {
        session_id: request.session_id,
        trusted_new_host: trusted_new_host.swap(false, Ordering::SeqCst),
    });

    let (mut reader_half, write_half) = channel.split();
    let mut reader = reader_half.make_reader();
    let mut writer = write_half.make_writer();
    let mut buffer = vec![0_u8; 4096];

    loop {
        tokio::select! {
            maybe_command = command_rx.recv() => {
                match maybe_command {
                    Some(SessionCommand::Input(data)) => {
                        writer
                            .write_all(&data)
                            .await
                            .context("Unable to write to the SSH shell")?;
                        let _ = writer.flush().await;
                    }
                    Some(SessionCommand::Resize(size)) => {
                        write_half
                            .window_change(
                                size.cols as u32,
                                size.rows as u32,
                                size.pixel_width as u32,
                                size.pixel_height as u32,
                            )
                            .await
                            .context("Unable to resize the remote PTY")?;
                    }
                    Some(SessionCommand::Disconnect) | None => {
                        let _ = writer.shutdown().await;
                        break;
                    }
                }
            }
            read = reader.read(&mut buffer) => {
                let bytes_read = read.context("Unable to read from the SSH shell")?;
                if bytes_read == 0 {
                    break;
                }

                let _ = event_tx.send(SshEvent::Output {
                    session_id: request.session_id,
                    data: buffer[..bytes_read].to_vec(),
                });
            }
        }
    }

    let _ = event_tx.send(SshEvent::Disconnected {
        session_id: request.session_id,
        message: "Remote shell closed".to_string(),
    });

    Ok(())
}

async fn authenticate(
    handle: &mut client::Handle<SessionHandler>,
    request: &ConnectRequest,
) -> Result<()> {
    let auth_result = match &request.auth {
        AuthConfig::Password { password } => handle
            .authenticate_password(request.username.clone(), password.clone())
            .await
            .context("Password authentication failed")?,
        AuthConfig::PrivateKey {
            key_path,
            passphrase,
        } => {
            let key = russh::keys::load_secret_key(key_path, passphrase.as_deref())
                .with_context(|| format!("Unable to load private key from {}", key_path))?;
            let rsa_hash = handle
                .best_supported_rsa_hash()
                .await
                .context("Unable to negotiate an RSA signature algorithm")?
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);

            handle
                .authenticate_publickey(request.username.clone(), key)
                .await
                .context("Public key authentication failed")?
        }
    };

    if !auth_result.success() {
        bail!("Authentication was rejected by the server");
    }

    Ok(())
}

#[derive(Clone)]
struct SessionHandler {
    endpoint: String,
    known_hosts: Arc<KnownHostStore>,
    trusted_new_host: Arc<AtomicBool>,
}

impl client::Handler for SessionHandler {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, server_public_key: &PublicKey) -> Result<bool> {
        let key = server_public_key
            .to_openssh()
            .context("Unable to serialize the server public key")?;

        match self.known_hosts.verify_or_trust(&self.endpoint, &key)? {
            HostKeyDecision::Existing => Ok(true),
            HostKeyDecision::Added => {
                self.trusted_new_host.store(true, Ordering::SeqCst);
                Ok(true)
            }
        }
    }
}
