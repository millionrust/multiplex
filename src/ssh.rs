use anyhow::{Context, Result, bail};
use russh::ChannelMsg;
use russh::client;
use russh::keys::PublicKey;
use russh::keys::key::PrivateKeyWithHashAlg;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::credentials;
use crate::models::{AuthConfig, ConnectRequest, LocalPortForward};
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

struct EstablishedSession {
    target_handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    trusted_new_host: bool,
    _jump_handle: Option<client::Handle<SessionHandler>>,
}

pub fn spawn_session(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    event_tx: Sender<SshEvent>,
) -> SessionRuntimeHandle {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let session_id = request.session_id;
    let thread_name = format!("ssh-session-{session_id}");

    eprintln!(
        "[ssh][{session_id}] spawn_session: address={}",
        request.address()
    );

    let fallback_tx = event_tx.clone();
    let spawn_result = thread::Builder::new().name(thread_name).spawn(move || {
        eprintln!("[ssh][{session_id}] thread started, building tokio runtime");
        let runtime = match Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[ssh][{session_id}] FATAL: failed to build tokio runtime: {e}");
                let _ = event_tx.send(SshEvent::Error {
                    session_id,
                    message: format!("Failed to build async runtime: {e}"),
                });
                return;
            }
        };

        eprintln!("[ssh][{session_id}] tokio runtime ready, starting session");
        runtime.block_on(async move {
            if let Err(error) =
                run_session(request, known_hosts, command_rx, event_tx.clone()).await
            {
                let message = format!("{error:#}");
                eprintln!("[ssh][{session_id}] session error: {message}");
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
        eprintln!("[ssh][{session_id}] thread exiting");
    });

    if let Err(e) = spawn_result {
        eprintln!("[ssh][{session_id}] FATAL: failed to spawn thread: {e}");
        let _ = fallback_tx.send(SshEvent::Error {
            session_id,
            message: format!("Failed to spawn SSH thread: {e}"),
        });
    }

    SessionRuntimeHandle { command_tx }
}

async fn run_session(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    mut command_rx: UnboundedReceiver<SessionCommand>,
    event_tx: Sender<SshEvent>,
) -> Result<()> {
    let session_id = request.session_id;
    let address = request.address();

    eprintln!("[ssh][{session_id}] connecting to {address}...");
    let config = Arc::new(client::Config::default());
    let established = establish_session(config, request.clone(), known_hosts).await?;

    eprintln!("[ssh][{session_id}] authenticated, opening channel...");
    let channel = {
        let handle = established.target_handle.lock().await;
        handle
            .channel_open_session()
            .await
            .context("Unable to open an SSH session channel")?
    };

    eprintln!("[ssh][{session_id}] channel open, requesting PTY...");
    channel
        .request_pty(true, "xterm-256color", 160, 48, 0, 0, &[])
        .await
        .context("Unable to allocate a remote PTY")?;

    eprintln!("[ssh][{session_id}] PTY allocated, requesting shell...");
    channel
        .request_shell(true)
        .await
        .context("Unable to start an interactive shell")?;

    if let Some(forward) = request.local_forward.clone() {
        start_local_forwarder(established.target_handle.clone(), session_id, forward).await?;
    }

    eprintln!("[ssh][{session_id}] shell started, sending Connected event");
    let _ = event_tx.send(SshEvent::Connected {
        session_id,
        trusted_new_host: established.trusted_new_host,
    });

    let (mut reader_half, write_half) = channel.split();
    let mut reader = reader_half.make_reader();
    let mut writer = write_half.make_writer();
    let mut buffer = vec![0_u8; 4096];

    eprintln!("[ssh][{session_id}] entering I/O loop");
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
                        eprintln!("[ssh][{session_id}] resize to {}x{}", size.cols, size.rows);
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
                        eprintln!("[ssh][{session_id}] disconnect command received");
                        let _ = writer.shutdown().await;
                        break;
                    }
                }
            }
            read = reader.read(&mut buffer) => {
                let bytes_read = read.context("Unable to read from the SSH shell")?;
                if bytes_read == 0 {
                    eprintln!("[ssh][{session_id}] remote EOF (0 bytes read)");
                    break;
                }

                let _ = event_tx.send(SshEvent::Output {
                    session_id,
                    data: buffer[..bytes_read].to_vec(),
                });
            }
        }
    }

    eprintln!("[ssh][{session_id}] I/O loop ended, sending Disconnected event");
    let _ = event_tx.send(SshEvent::Disconnected {
        session_id,
        message: "Remote shell closed".to_string(),
    });

    Ok(())
}

async fn authenticate(
    handle: &mut client::Handle<SessionHandler>,
    username: &str,
    auth: &AuthConfig,
) -> Result<()> {
    let auth_result = match auth {
        AuthConfig::Password { password } => handle
            .authenticate_password(username.to_string(), password.clone())
            .await
            .context("Password authentication failed")?,
        AuthConfig::PasswordRef { credential_id } => {
            let password = credentials::load_password(credential_id).with_context(|| {
                format!(
                    "Unable to load password '{}' from the system credential store",
                    credential_id
                )
            })?;

            handle
                .authenticate_password(username.to_string(), password)
                .await
                .context("Password authentication failed")?
        }
        AuthConfig::PrivateKey {
            key_path,
            passphrase,
        } => {
            eprintln!(
                "[ssh] loading private key from {key_path} (passphrase={})",
                passphrase.is_some()
            );
            let key = match russh::keys::load_secret_key(key_path, passphrase.as_deref()) {
                Ok(k) => k,
                Err(e) => {
                    let msg = e.to_string();
                    eprintln!("[ssh] key load failed: {msg}");
                    if msg.to_ascii_lowercase().contains("encrypt") && passphrase.is_none() {
                        bail!(
                            "Private key '{}' is passphrase-protected. Enter the passphrase in the host editor and try again.",
                            key_path
                        );
                    }
                    return Err(e)
                        .with_context(|| format!("Unable to load private key from {}", key_path));
                }
            };
            let rsa_hash = handle
                .best_supported_rsa_hash()
                .await
                .context("Unable to negotiate an RSA signature algorithm")?
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);

            handle
                .authenticate_publickey(username.to_string(), key)
                .await
                .context("Public key authentication failed")?
        }
    };

    if !auth_result.success() {
        bail!("Authentication was rejected by the server");
    }

    Ok(())
}

async fn establish_session(
    config: Arc<client::Config>,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
) -> Result<EstablishedSession> {
    if let Some(jump_host) = request.jump_host.clone() {
        let (jump_handle, jump_trusted) = connect_and_authenticate(
            config.clone(),
            jump_host.address(),
            jump_host.known_host_key(),
            jump_host.username.clone(),
            jump_host.auth.clone(),
            known_hosts.clone(),
        )
        .await?;

        eprintln!(
            "[ssh][{}] opening jump channel via {}",
            request.session_id,
            jump_host.address()
        );
        let jump_channel = jump_handle
            .channel_open_direct_tcpip(request.host.clone(), request.port.into(), "127.0.0.1", 0)
            .await
            .context("Unable to open jump-host tunnel to the target")?;

        let (target_handle, target_trusted) = connect_and_authenticate_stream(
            config,
            jump_channel.into_stream(),
            request.known_host_key(),
            request.username.clone(),
            request.auth.clone(),
            known_hosts,
        )
        .await?;

        Ok(EstablishedSession {
            target_handle: Arc::new(Mutex::new(target_handle)),
            trusted_new_host: jump_trusted.swap(false, Ordering::SeqCst)
                || target_trusted.swap(false, Ordering::SeqCst),
            _jump_handle: Some(jump_handle),
        })
    } else {
        let (target_handle, target_trusted) = connect_and_authenticate(
            config,
            request.address(),
            request.known_host_key(),
            request.username.clone(),
            request.auth.clone(),
            known_hosts,
        )
        .await?;

        Ok(EstablishedSession {
            target_handle: Arc::new(Mutex::new(target_handle)),
            trusted_new_host: target_trusted.swap(false, Ordering::SeqCst),
            _jump_handle: None,
        })
    }
}

async fn connect_and_authenticate(
    config: Arc<client::Config>,
    address: String,
    endpoint: String,
    username: String,
    auth: AuthConfig,
    known_hosts: Arc<KnownHostStore>,
) -> Result<(client::Handle<SessionHandler>, Arc<AtomicBool>)> {
    let trusted_new_host = Arc::new(AtomicBool::new(false));
    let handler = SessionHandler {
        endpoint,
        known_hosts,
        trusted_new_host: trusted_new_host.clone(),
    };

    let mut handle = client::connect(config, address, handler)
        .await
        .context("Unable to open the SSH transport")?;
    authenticate(&mut handle, &username, &auth).await?;
    Ok((handle, trusted_new_host))
}

async fn connect_and_authenticate_stream<R>(
    config: Arc<client::Config>,
    stream: R,
    endpoint: String,
    username: String,
    auth: AuthConfig,
    known_hosts: Arc<KnownHostStore>,
) -> Result<(client::Handle<SessionHandler>, Arc<AtomicBool>)>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let trusted_new_host = Arc::new(AtomicBool::new(false));
    let handler = SessionHandler {
        endpoint,
        known_hosts,
        trusted_new_host: trusted_new_host.clone(),
    };

    let mut handle = client::connect_stream(config, stream, handler)
        .await
        .context("Unable to open the SSH transport through the jump host")?;
    authenticate(&mut handle, &username, &auth).await?;
    Ok((handle, trusted_new_host))
}

async fn start_local_forwarder(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    session_id: u64,
    forward: LocalPortForward,
) -> Result<()> {
    let bind_addr = format!("{}:{}", forward.local_host, forward.local_port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Unable to bind local forward on {bind_addr}"))?;

    eprintln!(
        "[ssh][{session_id}] local forward listening on {} -> {}:{}",
        bind_addr, forward.remote_host, forward.remote_port
    );

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, originator_addr)) => {
                    let handle = handle.clone();
                    let forward = forward.clone();
                    tokio::spawn(async move {
                        if let Err(error) = proxy_local_forward_connection(
                            handle,
                            stream,
                            originator_addr,
                            &forward,
                        )
                        .await
                        {
                            eprintln!(
                                "[ssh][{session_id}] local forward {} failed: {error:#}",
                                forward.display_name()
                            );
                        }
                    });
                }
                Err(error) => {
                    eprintln!("[ssh][{session_id}] local forward accept loop stopped: {error:#}");
                    break;
                }
            }
        }
    });

    Ok(())
}

async fn proxy_local_forward_connection(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    mut stream: TcpStream,
    originator_addr: std::net::SocketAddr,
    forward: &LocalPortForward,
) -> Result<()> {
    let mut channel = {
        let handle = handle.lock().await;
        handle
            .channel_open_direct_tcpip(
                forward.remote_host.clone(),
                forward.remote_port.into(),
                originator_addr.ip().to_string(),
                originator_addr.port().into(),
            )
            .await
            .with_context(|| format!("Unable to open remote forward {}", forward.display_name()))?
    };

    let mut socket_closed = false;
    let mut buffer = vec![0_u8; 65536];
    loop {
        tokio::select! {
            read = stream.read(&mut buffer), if !socket_closed => {
                match read {
                    Ok(0) => {
                        socket_closed = true;
                        channel.eof().await.context("Unable to signal EOF to forwarded channel")?;
                    }
                    Ok(bytes_read) => {
                        channel
                            .data(&buffer[..bytes_read])
                            .await
                            .context("Unable to write to forwarded SSH channel")?;
                    }
                    Err(error) => return Err(error).context("Unable to read from local forwarded socket"),
                }
            }
            maybe_msg = channel.wait() => {
                match maybe_msg {
                    Some(ChannelMsg::Data { data }) => {
                        stream
                            .write_all(&data)
                            .await
                            .context("Unable to write forwarded data to local socket")?;
                    }
                    Some(ChannelMsg::Eof) | None => {
                        if !socket_closed {
                            let _ = channel.eof().await;
                        }
                        break;
                    }
                    Some(ChannelMsg::WindowAdjusted { .. }) => {}
                    Some(_) => {}
                }
            }
        }
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
        eprintln!("[ssh] check_server_key for endpoint={}", self.endpoint);
        let key = server_public_key
            .to_openssh()
            .context("Unable to serialize the server public key")?;

        match self.known_hosts.verify_or_trust(&self.endpoint, &key)? {
            HostKeyDecision::Existing => {
                eprintln!("[ssh] host key matched existing entry");
                Ok(true)
            }
            HostKeyDecision::Added => {
                eprintln!("[ssh] new host key trusted and pinned");
                self.trusted_new_host.store(true, Ordering::SeqCst);
                Ok(true)
            }
        }
    }
}
