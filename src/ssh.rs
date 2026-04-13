use anyhow::{Context, Result, bail};
use russh::ChannelMsg;
use russh::client;
use russh::keys::PublicKey;
use russh::keys::key::PrivateKeyWithHashAlg;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::credentials;
use crate::models::{
    AuthConfig, ConnectRequest, DynamicPortForward, JumpHostConnection, LocalPortForward,
    PortForwardRule, RemotePortForward,
};
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
    _jump_handles: Vec<client::Handle<SessionHandler>>,
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

    for rule in request.port_forward_rules.clone() {
        match rule {
            PortForwardRule::Local { forward } => {
                start_local_forwarder(established.target_handle.clone(), session_id, forward)
                    .await?;
            }
            PortForwardRule::Dynamic { forward } => {
                start_dynamic_forwarder(established.target_handle.clone(), session_id, forward)
                    .await?;
            }
            PortForwardRule::Remote { forward } => {
                start_remote_forwarder(established.target_handle.clone(), session_id, forward)
                    .await?;
            }
        }
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
    let auth = request
        .auth
        .clone()
        .context("SSH session is missing authentication settings")?;
    let remote_forwards = request
        .port_forward_rules
        .iter()
        .filter_map(|rule| match rule {
            PortForwardRule::Remote { forward } => Some(forward.clone()),
            PortForwardRule::Local { .. } | PortForwardRule::Dynamic { .. } => None,
        })
        .collect::<Vec<_>>();
    let (target_handle, jump_handles, trusted_new_host) =
        if let Some(jump_host) = request.jump_host.clone() {
            connect_via_jump_chain(
                config,
                request.host.clone(),
                request.port,
                request.known_host_key(),
                request.username.clone(),
                auth,
                jump_host,
                remote_forwards,
                known_hosts,
            )
            .await?
        } else {
            let (target_handle, target_trusted) = connect_and_authenticate(
                config,
                request.address(),
                request.known_host_key(),
                request.username.clone(),
                auth,
                remote_forwards,
                known_hosts,
            )
            .await?;
            (
                target_handle,
                Vec::new(),
                target_trusted.swap(false, Ordering::SeqCst),
            )
        };

    Ok(EstablishedSession {
        target_handle: Arc::new(Mutex::new(target_handle)),
        trusted_new_host,
        _jump_handles: jump_handles,
    })
}

async fn connect_via_jump_chain(
    config: Arc<client::Config>,
    target_host: String,
    target_port: u16,
    target_endpoint: String,
    target_username: String,
    target_auth: AuthConfig,
    jump_host: JumpHostConnection,
    remote_forwards: Vec<RemotePortForward>,
    known_hosts: Arc<KnownHostStore>,
) -> Result<(
    client::Handle<SessionHandler>,
    Vec<client::Handle<SessionHandler>>,
    bool,
)> {
    let chain = flatten_jump_chain(jump_host);
    let first_hop = chain
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Jump host chain is empty"))?;
    let (mut current_handle, current_trusted) = connect_and_authenticate(
        config.clone(),
        first_hop.address(),
        first_hop.known_host_key(),
        first_hop.username.clone(),
        first_hop.auth.clone(),
        Vec::new(),
        known_hosts.clone(),
    )
    .await?;
    let mut jump_handles = Vec::new();
    let mut trusted_new_host = current_trusted.swap(false, Ordering::SeqCst);

    for hop in chain.into_iter().skip(1) {
        eprintln!(
            "[ssh] opening jump channel via {} to {}",
            hop.title,
            hop.address()
        );
        let channel = current_handle
            .channel_open_direct_tcpip(hop.host.clone(), hop.port.into(), "127.0.0.1", 0)
            .await
            .with_context(|| format!("Unable to open jump-host tunnel to {}", hop.address()))?;
        let (next_handle, next_trusted) = connect_and_authenticate_stream(
            config.clone(),
            channel.into_stream(),
            hop.known_host_key(),
            hop.username.clone(),
            hop.auth.clone(),
            Vec::new(),
            known_hosts.clone(),
        )
        .await?;
        trusted_new_host |= next_trusted.swap(false, Ordering::SeqCst);
        jump_handles.push(current_handle);
        current_handle = next_handle;
    }

    eprintln!(
        "[ssh] opening jump channel to target {}:{}",
        target_host, target_port
    );
    let jump_channel = current_handle
        .channel_open_direct_tcpip(target_host, target_port.into(), "127.0.0.1", 0)
        .await
        .context("Unable to open jump-host tunnel to the target")?;

    let (target_handle, target_trusted) = connect_and_authenticate_stream(
        config,
        jump_channel.into_stream(),
        target_endpoint,
        target_username,
        target_auth,
        remote_forwards,
        known_hosts,
    )
    .await?;

    trusted_new_host |= target_trusted.swap(false, Ordering::SeqCst);
    jump_handles.push(current_handle);
    Ok((target_handle, jump_handles, trusted_new_host))
}

fn flatten_jump_chain(jump_host: JumpHostConnection) -> Vec<JumpHostConnection> {
    let mut chain = Vec::new();
    let mut current = Some(jump_host);

    while let Some(jump) = current {
        current = jump.jump_host.as_deref().cloned();
        chain.push(JumpHostConnection {
            jump_host: None,
            ..jump
        });
    }

    chain.reverse();
    chain
}

async fn connect_and_authenticate(
    config: Arc<client::Config>,
    address: String,
    endpoint: String,
    username: String,
    auth: AuthConfig,
    remote_forwards: Vec<RemotePortForward>,
    known_hosts: Arc<KnownHostStore>,
) -> Result<(client::Handle<SessionHandler>, Arc<AtomicBool>)> {
    let trusted_new_host = Arc::new(AtomicBool::new(false));
    let handler = SessionHandler {
        endpoint,
        known_hosts,
        trusted_new_host: trusted_new_host.clone(),
        remote_forwards,
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
    remote_forwards: Vec<RemotePortForward>,
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
        remote_forwards,
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
                        if let Err(error) = proxy_direct_tcpip_connection(
                            handle,
                            stream,
                            originator_addr,
                            forward.remote_host.clone(),
                            forward.remote_port,
                            forward.display_name(),
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

async fn start_dynamic_forwarder(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    session_id: u64,
    forward: DynamicPortForward,
) -> Result<()> {
    let bind_addr = format!("{}:{}", forward.local_host, forward.local_port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Unable to bind dynamic forward on {bind_addr}"))?;

    eprintln!(
        "[ssh][{session_id}] dynamic forward listening on {} (SOCKS5)",
        bind_addr
    );

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, originator_addr)) => {
                    let handle = handle.clone();
                    let forward = forward.clone();
                    tokio::spawn(async move {
                        if let Err(error) = proxy_dynamic_forward_connection(
                            handle,
                            stream,
                            originator_addr,
                            &forward,
                        )
                        .await
                        {
                            eprintln!(
                                "[ssh][{session_id}] dynamic forward {} failed: {error:#}",
                                forward.display_name()
                            );
                        }
                    });
                }
                Err(error) => {
                    eprintln!("[ssh][{session_id}] dynamic forward accept loop stopped: {error:#}");
                    break;
                }
            }
        }
    });

    Ok(())
}

async fn start_remote_forwarder(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    session_id: u64,
    forward: RemotePortForward,
) -> Result<()> {
    let requested_port = forward.remote_port;
    let assigned_port = {
        let mut handle = handle.lock().await;
        handle
            .tcpip_forward(forward.remote_host.clone(), requested_port.into())
            .await
            .with_context(|| {
                format!(
                    "Unable to request remote forward {}",
                    forward.display_name()
                )
            })?
    };

    eprintln!(
        "[ssh][{session_id}] remote forward listening on {}:{} -> {}:{}",
        forward.remote_host, assigned_port, forward.local_host, forward.local_port
    );

    Ok(())
}

async fn proxy_dynamic_forward_connection(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    mut stream: TcpStream,
    originator_addr: std::net::SocketAddr,
    forward: &DynamicPortForward,
) -> Result<()> {
    let (target_host, target_port) = negotiate_socks5_target(&mut stream).await?;
    let display_name = format!("{} -> {target_host}:{target_port}", forward.display_name());
    proxy_direct_tcpip_connection(
        handle,
        stream,
        originator_addr,
        target_host,
        target_port,
        display_name,
    )
    .await
}

async fn proxy_direct_tcpip_connection(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    mut stream: TcpStream,
    originator_addr: std::net::SocketAddr,
    target_host: String,
    target_port: u16,
    display_name: String,
) -> Result<()> {
    let mut channel = {
        let handle = handle.lock().await;
        handle
            .channel_open_direct_tcpip(
                target_host,
                target_port.into(),
                originator_addr.ip().to_string(),
                originator_addr.port().into(),
            )
            .await
            .with_context(|| format!("Unable to open remote forward {display_name}"))?
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

async fn proxy_remote_forward_connection(
    channel: russh::Channel<russh::client::Msg>,
    forward: RemotePortForward,
) -> Result<()> {
    let local_addr = format!("{}:{}", forward.local_host, forward.local_port);
    let mut local_stream = TcpStream::connect(&local_addr)
        .await
        .with_context(|| format!("Unable to connect local target {local_addr}"))?;
    let mut channel_stream = channel.into_stream();
    copy_bidirectional(&mut local_stream, &mut channel_stream)
        .await
        .with_context(|| format!("Unable to proxy remote forward to {local_addr}"))?;
    Ok(())
}

async fn negotiate_socks5_target(stream: &mut TcpStream) -> Result<(String, u16)> {
    let mut header = [0u8; 2];
    stream
        .read_exact(&mut header)
        .await
        .context("Unable to read SOCKS5 greeting")?;
    if header[0] != 0x05 {
        bail!("Unsupported SOCKS version {}", header[0]);
    }

    let method_count = header[1] as usize;
    let mut methods = vec![0u8; method_count];
    stream
        .read_exact(&mut methods)
        .await
        .context("Unable to read SOCKS5 methods")?;
    if !methods.contains(&0x00) {
        stream
            .write_all(&[0x05, 0xff])
            .await
            .context("Unable to reject SOCKS5 authentication methods")?;
        bail!("SOCKS5 client did not offer no-auth mode");
    }

    stream
        .write_all(&[0x05, 0x00])
        .await
        .context("Unable to acknowledge SOCKS5 method")?;

    let mut request_header = [0u8; 4];
    stream
        .read_exact(&mut request_header)
        .await
        .context("Unable to read SOCKS5 request header")?;
    if request_header[0] != 0x05 {
        bail!("Unsupported SOCKS request version {}", request_header[0]);
    }
    if request_header[1] != 0x01 {
        stream
            .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await
            .context("Unable to reject SOCKS5 command")?;
        bail!("Only SOCKS5 CONNECT is supported");
    }

    let host = match request_header[3] {
        0x01 => {
            let mut addr = [0u8; 4];
            stream
                .read_exact(&mut addr)
                .await
                .context("Unable to read SOCKS5 IPv4 target")?;
            std::net::Ipv4Addr::from(addr).to_string()
        }
        0x03 => {
            let mut length = [0u8; 1];
            stream
                .read_exact(&mut length)
                .await
                .context("Unable to read SOCKS5 domain length")?;
            let mut domain = vec![0u8; length[0] as usize];
            stream
                .read_exact(&mut domain)
                .await
                .context("Unable to read SOCKS5 domain target")?;
            String::from_utf8(domain).context("SOCKS5 domain target is not valid UTF-8")?
        }
        0x04 => {
            let mut addr = [0u8; 16];
            stream
                .read_exact(&mut addr)
                .await
                .context("Unable to read SOCKS5 IPv6 target")?;
            std::net::Ipv6Addr::from(addr).to_string()
        }
        atyp => {
            stream
                .write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .context("Unable to reject SOCKS5 address type")?;
            bail!("Unsupported SOCKS5 address type {atyp}");
        }
    };

    let mut port_bytes = [0u8; 2];
    stream
        .read_exact(&mut port_bytes)
        .await
        .context("Unable to read SOCKS5 target port")?;
    let port = u16::from_be_bytes(port_bytes);

    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
        .context("Unable to acknowledge SOCKS5 connect request")?;

    Ok((host, port))
}

#[derive(Clone)]
struct SessionHandler {
    endpoint: String,
    known_hosts: Arc<KnownHostStore>,
    trusted_new_host: Arc<AtomicBool>,
    remote_forwards: Vec<RemotePortForward>,
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

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut client::Session,
    ) -> Result<()> {
        let target = self
            .remote_forwards
            .iter()
            .find(|forward| {
                u32::from(forward.remote_port) == connected_port
                    && (forward.remote_host == connected_address
                        || forward.remote_host == "0.0.0.0"
                        || forward.remote_host == "::"
                        || self
                            .remote_forwards
                            .iter()
                            .filter(|candidate| u32::from(candidate.remote_port) == connected_port)
                            .count()
                            == 1)
            })
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No remote forward target registered for {}:{}",
                    connected_address,
                    connected_port
                )
            })?;

        tokio::spawn(async move {
            if let Err(error) = proxy_remote_forward_connection(channel, target.clone()).await {
                eprintln!(
                    "[ssh] remote forward {} proxy failed: {error:#}",
                    target.display_name()
                );
            }
        });

        Ok(())
    }
}
