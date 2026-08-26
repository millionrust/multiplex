use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::PublicKey;
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::{ChannelMsg, Sig};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, sync_channel};
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
    KillTmuxSession { session_name: String },
    StopDurable,
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
    HostedSnapshot {
        session_id: u64,
        data: Vec<u8>,
        boundary_sequence: u64,
    },
    HostedStatus {
        session_id: u64,
        state: termirust_domain::HostedSessionState,
        last_sequence: u64,
        durable_sequence: u64,
        activity: termirust_domain::ActivityAggregate,
        has_writer_lease: bool,
        detail: String,
    },
    Error {
        session_id: u64,
        message: String,
    },
    TmuxSessionKilled {
        session_id: u64,
        session_name: String,
    },
    Disconnected {
        session_id: u64,
        message: String,
    },
}

pub struct SessionRuntimeHandle {
    pub command_tx: UnboundedSender<SessionCommand>,
}

const REMOTE_EXEC_STREAM_CAPACITY: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteExecExit {
    Status(u32),
    Signal { signal: String, message: String },
}

enum RemoteExecCommand {
    Input(Vec<u8>),
    Terminate,
}

#[derive(Clone)]
pub struct RemoteExecControl {
    command_tx: UnboundedSender<RemoteExecCommand>,
}

impl RemoteExecControl {
    pub fn terminate(&self) -> Result<()> {
        self.command_tx
            .send(RemoteExecCommand::Terminate)
            .map_err(|_| anyhow::anyhow!("Remote process is no longer running"))
    }
}

pub struct RemoteExecWriter {
    command_tx: UnboundedSender<RemoteExecCommand>,
}

impl Write for RemoteExecWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.command_tx
            .send(RemoteExecCommand::Input(buffer.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "remote stdin is closed"))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct RemoteExecReader {
    rx: Receiver<Vec<u8>>,
    pending: Vec<u8>,
    offset: usize,
}

impl RemoteExecReader {
    fn new(rx: Receiver<Vec<u8>>) -> Self {
        Self {
            rx,
            pending: Vec::new(),
            offset: 0,
        }
    }
}

impl Read for RemoteExecReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        while self.offset >= self.pending.len() {
            match self.rx.recv() {
                Ok(data) if data.is_empty() => continue,
                Ok(data) => {
                    self.pending = data;
                    self.offset = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let available = &self.pending[self.offset..];
        let copied = available.len().min(buffer.len());
        buffer[..copied].copy_from_slice(&available[..copied]);
        self.offset += copied;
        Ok(copied)
    }
}

pub struct RemoteExecProcess {
    pub stdin: RemoteExecWriter,
    pub stdout: RemoteExecReader,
    pub stderr: RemoteExecReader,
    pub exit_rx: Receiver<Result<RemoteExecExit, String>>,
    pub control: RemoteExecControl,
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
    keepalive_secs: u16,
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
            if let Err(error) = run_session(
                request,
                known_hosts,
                command_rx,
                event_tx.clone(),
                keepalive_secs,
            )
            .await
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

pub fn spawn_remote_exec(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    keepalive_secs: u16,
    command: String,
) -> Result<RemoteExecProcess> {
    if request.is_local_shell() {
        bail!("Remote execution requires an SSH connection request");
    }
    if command.trim().is_empty() {
        bail!("Remote execution command cannot be empty");
    }
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (stdout_tx, stdout_rx) = sync_channel(REMOTE_EXEC_STREAM_CAPACITY);
    let (stderr_tx, stderr_rx) = sync_channel(REMOTE_EXEC_STREAM_CAPACITY);
    let (exit_tx, exit_rx) = sync_channel(1);
    let session_id = request.session_id;
    let fallback_exit = exit_tx.clone();

    thread::Builder::new()
        .name(format!("ssh-exec-{session_id}"))
        .spawn(move || {
            let runtime = match Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = exit_tx.send(Err(format!(
                        "Unable to build remote execution runtime: {error}"
                    )));
                    return;
                }
            };
            let result = runtime.block_on(run_remote_exec(
                request,
                known_hosts,
                keepalive_secs,
                command,
                command_rx,
                stdout_tx,
                stderr_tx,
            ));
            let _ = exit_tx.send(result.map_err(|error| format!("{error:#}")));
        })
        .map_err(|error| {
            let message = format!("Unable to start remote execution thread: {error}");
            let _ = fallback_exit.send(Err(message.clone()));
            anyhow::anyhow!(message)
        })?;

    Ok(RemoteExecProcess {
        stdin: RemoteExecWriter {
            command_tx: command_tx.clone(),
        },
        stdout: RemoteExecReader::new(stdout_rx),
        stderr: RemoteExecReader::new(stderr_rx),
        exit_rx,
        control: RemoteExecControl { command_tx },
    })
}

async fn run_remote_exec(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    keepalive_secs: u16,
    command: String,
    mut command_rx: UnboundedReceiver<RemoteExecCommand>,
    stdout_tx: SyncSender<Vec<u8>>,
    stderr_tx: SyncSender<Vec<u8>>,
) -> Result<RemoteExecExit> {
    let session_id = request.session_id;
    let endpoint = request.address();
    eprintln!("[ssh][{session_id}] remote exec connecting to {endpoint}...");
    let mut config_inner = client::Config::default();
    if keepalive_secs > 0 {
        config_inner.keepalive_interval =
            Some(std::time::Duration::from_secs(u64::from(keepalive_secs)));
    }
    let established = establish_session(Arc::new(config_inner), request, known_hosts).await?;
    eprintln!("[ssh][{session_id}] remote exec authenticated");
    let mut channel = {
        let handle = established.target_handle.lock().await;
        handle
            .channel_open_session()
            .await
            .context("Unable to open remote execution channel")?
    };
    eprintln!("[ssh][{session_id}] remote exec channel open");
    channel
        .exec(true, command)
        .await
        .context("Unable to start remote process")?;
    eprintln!("[ssh][{session_id}] remote process started");

    let mut exit = None;
    let mut commands_open = true;
    loop {
        tokio::select! {
            maybe_command = command_rx.recv(), if commands_open => {
                match maybe_command {
                    Some(RemoteExecCommand::Input(data)) => {
                        channel
                            .data(&data[..])
                            .await
                            .context("Unable to write remote process input")?;
                    }
                    Some(RemoteExecCommand::Terminate) => {
                        channel
                            .signal(Sig::TERM)
                            .await
                            .context("Unable to terminate remote process")?;
                    }
                    None => {
                        commands_open = false;
                        let _ = channel.signal(Sig::TERM).await;
                    }
                }
            }
            maybe_message = channel.wait() => {
                match maybe_message {
                    Some(ChannelMsg::Data { data }) => {
                        stdout_tx
                            .send(data.to_vec())
                            .map_err(|_| anyhow::anyhow!("Remote stdout consumer disconnected"))?;
                    }
                    Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
                        stderr_tx
                            .send(data.to_vec())
                            .map_err(|_| anyhow::anyhow!("Remote stderr consumer disconnected"))?;
                    }
                    Some(ChannelMsg::ExtendedData { .. }) => {}
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        eprintln!("[ssh][{session_id}] remote process exited with status {exit_status}");
                        exit = Some(RemoteExecExit::Status(exit_status));
                    }
                    Some(ChannelMsg::ExitSignal {
                        signal_name,
                        error_message,
                        ..
                    }) => {
                        eprintln!("[ssh][{session_id}] remote process exited by signal {signal_name:?}");
                        exit = Some(RemoteExecExit::Signal {
                            signal: format!("{signal_name:?}"),
                            message: error_message,
                        });
                    }
                    Some(ChannelMsg::Eof) => {}
                    Some(ChannelMsg::Close) | None => break,
                    Some(_) => {}
                }
            }
        }
    }

    eprintln!("[ssh][{session_id}] remote exec channel closed");
    exit.context("Remote process closed without an exit status")
}

async fn run_session(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    mut command_rx: UnboundedReceiver<SessionCommand>,
    event_tx: Sender<SshEvent>,
    keepalive_secs: u16,
) -> Result<()> {
    let session_id = request.session_id;
    let address = request.address();

    eprintln!("[ssh][{session_id}] connecting to {address}...");
    let mut config_inner = client::Config::default();
    if keepalive_secs > 0 {
        config_inner.keepalive_interval =
            Some(std::time::Duration::from_secs(u64::from(keepalive_secs)));
    }
    let config = Arc::new(config_inner);
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
                    Some(SessionCommand::KillTmuxSession { session_name }) => {
                        kill_tmux_session(established.target_handle.clone(), &session_name)
                            .await
                            .with_context(|| format!("Unable to kill tmux session {session_name:?}"))?;
                        let _ = event_tx.send(SshEvent::TmuxSessionKilled {
                            session_id,
                            session_name,
                        });
                    }
                    Some(SessionCommand::StopDurable | SessionCommand::Disconnect) | None => {
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

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn tmux_kill_command(session_name: &str) -> String {
    format!("tmux kill-session -t {}", shell_single_quote(session_name))
}

async fn kill_tmux_session(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    session_name: &str,
) -> Result<()> {
    let mut channel = {
        let handle = handle.lock().await;
        handle
            .channel_open_session()
            .await
            .context("Unable to open tmux control channel")?
    };
    channel
        .exec(true, tmux_kill_command(session_name))
        .await
        .context("Unable to execute tmux kill-session")?;

    let mut exit_status = None;
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::ExitStatus {
                exit_status: status,
            } => exit_status = Some(status),
            ChannelMsg::ExitSignal {
                signal_name,
                error_message,
                ..
            } => anyhow::bail!("tmux kill-session terminated by {signal_name:?}: {error_message}"),
            _ => {}
        }
    }
    match exit_status {
        Some(0) => Ok(()),
        Some(status) => anyhow::bail!("tmux kill-session exited with status {status}"),
        None => anyhow::bail!("tmux kill-session returned no exit status"),
    }
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

#[cfg(test)]
mod tests {
    use super::{
        RemoteExecExit, SessionCommand, SshEvent, spawn_remote_exec, spawn_session,
        tmux_kill_command,
    };
    use crate::models::{
        AuthConfig, ConnectRequest, ConnectionKind, DynamicPortForward, JumpHostConnection,
        LocalPortForward, PortForwardRule, RemotePortForward,
    };
    use crate::storage::KnownHostStore;
    use crate::test_support::{DockerSshServer, TestIsolation, allocate_local_port};
    use crate::ui::shell::{shell_single_quote, startup_bytes_for_request};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};

    fn docker_ssh_request(server: &DockerSshServer) -> ConnectRequest {
        ConnectRequest {
            session_id: 42,
            title: "Docker SSH".to_string(),
            kind: ConnectionKind::Ssh,
            host: server.host().to_string(),
            port: server.port,
            username: server.username().to_string(),
            auth: Some(AuthConfig::Password {
                password: server.password().to_string(),
            }),
            jump_host: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }

    #[test]
    fn remote_exec_reader_preserves_chunk_boundaries_as_a_byte_stream() {
        let (tx, rx) = mpsc::sync_channel(2);
        tx.send(b"first".to_vec()).unwrap();
        tx.send(b"-second".to_vec()).unwrap();
        drop(tx);
        let mut reader = super::RemoteExecReader::new(rx);
        let mut output = String::new();
        reader.read_to_string(&mut output).unwrap();
        assert_eq!(output, "first-second");
    }

    #[test]
    fn tmux_kill_command_quotes_the_session_name_once() {
        assert_eq!(
            tmux_kill_command("client's session; touch /tmp/nope"),
            "tmux kill-session -t 'client'\\''s session; touch /tmp/nope'"
        );
    }

    fn docker_private_key_path() -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ssh-server/id_ed25519")
            .display()
            .to_string()
    }

    fn docker_jump_request(server: &DockerSshServer) -> ConnectRequest {
        let key_path = docker_private_key_path();
        ConnectRequest {
            session_id: 77,
            title: "Docker Via Jump".to_string(),
            kind: ConnectionKind::Ssh,
            host: "127.0.0.1".to_string(),
            port: 22,
            username: server.username().to_string(),
            auth: Some(AuthConfig::PrivateKey {
                key_path: key_path.clone(),
                passphrase: None,
            }),
            jump_host: Some(JumpHostConnection {
                title: "Docker Bastion".to_string(),
                host: server.host().to_string(),
                port: server.port,
                username: server.username().to_string(),
                auth: AuthConfig::PrivateKey {
                    key_path,
                    passphrase: None,
                },
                jump_host: None,
            }),
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }

    fn wait_for_connected(
        request: &ConnectRequest,
        event_rx: &mpsc::Receiver<SshEvent>,
        label: &str,
    ) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SshEvent::Connected { session_id, .. }) => {
                    assert_eq!(session_id, request.session_id);
                    return;
                }
                Ok(SshEvent::Output { .. }) => {}
                Ok(SshEvent::TmuxSessionKilled { .. }) => {
                    panic!("{label} killed tmux before connect")
                }
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => {
                    panic!("{label} emitted error for session {session_id}: {message}");
                }
                Ok(SshEvent::Disconnected {
                    session_id,
                    message,
                }) => {
                    panic!(
                        "{label} disconnected before connect for session {session_id}: {message}"
                    );
                }
                Ok(_) => panic!("{label} received an event reserved for durable sessions"),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("{label} channel disconnected unexpectedly");
                }
            }
        }

        panic!("did not observe connected event for {label}");
    }

    fn disconnect_runtime(
        request: &ConnectRequest,
        runtime: &super::SessionRuntimeHandle,
        event_rx: &mpsc::Receiver<SshEvent>,
        label: &str,
    ) {
        runtime
            .command_tx
            .send(SessionCommand::Disconnect)
            .expect("unable to disconnect ssh runtime");

        let disconnected = loop {
            match event_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(SshEvent::Disconnected { session_id, .. }) => break session_id,
                Ok(
                    SshEvent::Output { .. }
                    | SshEvent::Connected { .. }
                    | SshEvent::TmuxSessionKilled { .. },
                ) => continue,
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => {
                    panic!("unexpected {label} error after disconnect for {session_id}: {message}")
                }
                Ok(_) => panic!("{label} received an event reserved for durable sessions"),
                Err(error) => panic!("did not observe {label} disconnect event: {error}"),
            }
        };
        assert_eq!(disconnected, request.session_id);
    }

    fn send_startup_payload(request: &ConnectRequest, runtime: &super::SessionRuntimeHandle) {
        let bytes = startup_bytes_for_request(request, None)
            .expect("persistent ssh request should generate startup bytes");
        runtime
            .command_tx
            .send(SessionCommand::Input(bytes))
            .expect("unable to send startup payload");
    }

    fn wait_for_remote_success(server: &DockerSshServer, command: &str, label: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_error = String::new();
        while Instant::now() < deadline {
            match server.exec(command) {
                Ok(output) => return output,
                Err(error) => last_error = error,
            }
            std::thread::sleep(Duration::from_millis(250));
        }

        panic!("{label} did not become true before timeout.\nlast error:\n{last_error}");
    }

    fn docker_fixture_user_command(command: &str) -> String {
        format!("su -s /bin/sh -c {} termirust", shell_single_quote(command))
    }

    fn assert_remote_success_stays_true(
        server: &DockerSshServer,
        command: &str,
        duration: Duration,
        label: &str,
    ) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            if let Err(error) = server.exec(command) {
                panic!("{label} stopped being true.\nerror:\n{error}");
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn wait_for_output_contains(
        request: &ConnectRequest,
        event_rx: &mpsc::Receiver<SshEvent>,
        needle: &str,
        label: &str,
    ) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SshEvent::Output { session_id, data }) => {
                    assert_eq!(session_id, request.session_id);
                    if String::from_utf8_lossy(&data).contains(needle) {
                        return;
                    }
                }
                Ok(SshEvent::Connected { .. }) => {}
                Ok(SshEvent::TmuxSessionKilled { .. }) => {
                    panic!("{label} unexpectedly killed a tmux session")
                }
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => panic!("{label} emitted error for session {session_id}: {message}"),
                Ok(SshEvent::Disconnected {
                    session_id,
                    message,
                }) => {
                    panic!(
                        "{label} disconnected before expected output for {session_id}: {message}"
                    )
                }
                Ok(_) => panic!("{label} received an event reserved for durable sessions"),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("{label} channel disconnected unexpectedly");
                }
            }
        }

        panic!("{label} did not emit expected output containing {needle:?}");
    }

    #[test]
    fn docker_ssh_session_connects_and_streams_output() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker ssh e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();

        let request = docker_ssh_request(&server);
        let runtime = spawn_session(request.clone(), known_hosts.clone(), event_tx, 0);

        let mut saw_connected = false;
        let mut saw_output = false;
        let deadline = Instant::now() + Duration::from_secs(20);

        while Instant::now() < deadline && (!saw_connected || !saw_output) {
            match event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SshEvent::Connected { session_id, .. }) => {
                    assert_eq!(session_id, request.session_id);
                    saw_connected = true;
                    runtime
                        .command_tx
                        .send(SessionCommand::Input(
                            b"printf 'runtime-e2e-ok\\n'\n".to_vec(),
                        ))
                        .expect("unable to send test command");
                }
                Ok(SshEvent::Output { session_id, data }) => {
                    assert_eq!(session_id, request.session_id);
                    let output = String::from_utf8_lossy(&data);
                    if output.contains("runtime-e2e-ok") {
                        saw_output = true;
                        break;
                    }
                }
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => {
                    panic!("ssh runtime emitted error for session {session_id}: {message}");
                }
                Ok(SshEvent::TmuxSessionKilled { .. }) => {
                    panic!("ordinary SSH test unexpectedly killed a tmux session")
                }
                Ok(SshEvent::Disconnected {
                    session_id,
                    message,
                }) => {
                    panic!(
                        "ssh runtime disconnected before output for session {session_id}: {message}"
                    );
                }
                Ok(_) => panic!("SSH runtime received an event reserved for durable sessions"),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("ssh runtime channel disconnected unexpectedly");
                }
            }
        }

        assert!(saw_connected, "did not observe ssh connected event");
        assert!(
            saw_output,
            "did not observe runtime output from docker ssh server"
        );

        runtime
            .command_tx
            .send(SessionCommand::Disconnect)
            .expect("unable to disconnect ssh runtime");

        let disconnected = loop {
            match event_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(SshEvent::Disconnected { session_id, .. }) => break session_id,
                Ok(
                    SshEvent::Output { .. }
                    | SshEvent::Connected { .. }
                    | SshEvent::TmuxSessionKilled { .. },
                ) => continue,
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => panic!("unexpected ssh error after disconnect for {session_id}: {message}"),
                Ok(_) => panic!("SSH runtime received an event reserved for durable sessions"),
                Err(error) => panic!("did not observe disconnect event: {error}"),
            }
        };
        assert_eq!(disconnected, request.session_id);

        let saved_keys = known_hosts.entries().expect("unable to read known hosts");
        assert_eq!(saved_keys.len(), 1);
        assert_eq!(saved_keys[0].0, request.known_host_key());
    }

    #[test]
    fn docker_remote_exec_separates_streams_accepts_input_and_reports_status() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker remote exec e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let request = docker_ssh_request(&server);
        let mut process = spawn_remote_exec(
            request,
            known_hosts,
            0,
            "IFS= read -r value; printf 'stdout:%s' \"$value\"; printf 'stderr:%s' \"$value\" >&2; exit 7"
                .to_string(),
        )
        .expect("unable to start remote exec");

        process
            .stdin
            .write_all(b"round-trip\n")
            .expect("unable to write remote stdin");
        let mut stdout = String::new();
        let mut stderr = String::new();
        process.stdout.read_to_string(&mut stdout).unwrap();
        process.stderr.read_to_string(&mut stderr).unwrap();
        let exit = process
            .exit_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("remote exec did not report an exit")
            .expect("remote exec transport failed");

        assert_eq!(stdout, "stdout:round-trip");
        assert_eq!(stderr, "stderr:round-trip");
        assert_eq!(exit, RemoteExecExit::Status(7));
    }

    #[test]
    fn docker_ssh_persistent_tmux_session_survives_reconnect_without_rerunning_startup() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker tmux persistence e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let startup_dir = "/home/termirust/tmux e2e dir";
        let marker_path = "/home/termirust/tmux-startup-count";
        let pwd_path = "/home/termirust/tmux-startup-pwd";
        let session_name = format!("tr-e2e-{}", server.port);
        server
            .exec(&format!(
                "rm -f {marker_path} {pwd_path}; mkdir -p {}; chown -R termirust:termirust {}",
                shell_single_quote(startup_dir),
                shell_single_quote(startup_dir)
            ))
            .expect("unable to prepare remote tmux persistence fixture");

        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();

        let mut request = docker_ssh_request(&server);
        request.session_id = 501;
        request.persistent_session = true;
        request.persistent_session_name = Some(session_name.clone());
        request.startup_directory = Some(startup_dir.to_string());
        request.startup_command = Some(format!(
            "pwd > {pwd_path}; printf 'startup-ran\\n' >> {marker_path}"
        ));

        let runtime = spawn_session(request.clone(), known_hosts.clone(), event_tx, 0);
        wait_for_connected(&request, &event_rx, "tmux persistence first connect");
        send_startup_payload(&request, &runtime);

        wait_for_remote_success(
            &server,
            &docker_fixture_user_command(&format!(
                "tmux has-session -t {}",
                shell_single_quote(&session_name)
            )),
            "tmux session creation",
        );
        wait_for_remote_success(
            &server,
            &format!(
                "test \"$(cat {pwd_path})\" = {}",
                shell_single_quote(startup_dir)
            ),
            "startup directory application",
        );
        wait_for_remote_success(
            &server,
            &format!("test \"$(wc -l < {marker_path})\" = '1'"),
            "startup command first run",
        );

        disconnect_runtime(
            &request,
            &runtime,
            &event_rx,
            "tmux persistence first connect",
        );
        wait_for_remote_success(
            &server,
            &docker_fixture_user_command(&format!(
                "tmux has-session -t {}",
                shell_single_quote(&session_name)
            )),
            "tmux session survives ssh disconnect",
        );

        let (reconnect_tx, reconnect_rx) = mpsc::channel();
        let mut reconnect_request = request.clone();
        reconnect_request.session_id = 502;
        let reconnect_runtime =
            spawn_session(reconnect_request.clone(), known_hosts, reconnect_tx, 0);
        wait_for_connected(
            &reconnect_request,
            &reconnect_rx,
            "tmux persistence reconnect",
        );
        send_startup_payload(&reconnect_request, &reconnect_runtime);

        assert_remote_success_stays_true(
            &server,
            &format!("test \"$(wc -l < {marker_path})\" = '1'"),
            Duration::from_secs(2),
            "startup command should not rerun on tmux attach",
        );

        disconnect_runtime(
            &reconnect_request,
            &reconnect_runtime,
            &reconnect_rx,
            "tmux persistence reconnect",
        );
        let _ = server.exec(&docker_fixture_user_command(&format!(
            "tmux kill-session -t {}",
            shell_single_quote(&session_name)
        )));
    }

    #[test]
    fn docker_ssh_persistent_tmux_kill_command_removes_session() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker tmux kill e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let session_name = format!("tr-kill-e2e-{}", server.port);
        let mut request = docker_ssh_request(&server);
        request.session_id = 504;
        request.persistent_session = true;
        request.persistent_session_name = Some(session_name.clone());

        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "tmux kill connect");
        send_startup_payload(&request, &runtime);
        wait_for_remote_success(
            &server,
            &docker_fixture_user_command(&format!(
                "tmux has-session -t {}",
                shell_single_quote(&session_name)
            )),
            "tmux kill fixture creation",
        );

        runtime
            .command_tx
            .send(SessionCommand::KillTmuxSession {
                session_name: session_name.clone(),
            })
            .expect("unable to request tmux session kill");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_killed = false;
        loop {
            assert!(
                Instant::now() < deadline,
                "tmux kill did not disconnect the attached shell"
            );
            match event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SshEvent::Disconnected { session_id, .. }) => {
                    assert_eq!(session_id, request.session_id);
                    break;
                }
                Ok(SshEvent::Output { .. } | SshEvent::Connected { .. }) => {}
                Ok(SshEvent::TmuxSessionKilled {
                    session_id,
                    session_name: killed_name,
                }) => {
                    assert_eq!(session_id, request.session_id);
                    assert_eq!(killed_name, session_name);
                    saw_killed = true;
                }
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => panic!("tmux kill failed for session {session_id}: {message}"),
                Ok(_) => panic!("SSH runtime received an event reserved for durable sessions"),
                Err(RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("tmux kill event channel failed: {error}"),
            }
        }
        assert!(
            saw_killed,
            "runtime did not report successful tmux deletion"
        );
        wait_for_remote_success(
            &server,
            &docker_fixture_user_command(&format!(
                "! tmux has-session -t {} 2>/dev/null",
                shell_single_quote(&session_name)
            )),
            "tmux session deletion",
        );
    }

    #[test]
    fn docker_ssh_persistent_tmux_missing_binary_prints_fallback_message() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker tmux fallback e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();

        let mut request = docker_ssh_request(&server);
        request.session_id = 503;
        request.persistent_session = true;
        request.persistent_session_name = Some("tr-no-tmux-e2e".to_string());
        request.environment = vec![("PATH".to_string(), "/tmp/termirust-no-tmux".to_string())];

        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "tmux missing fallback");
        send_startup_payload(&request, &runtime);
        wait_for_output_contains(
            &request,
            &event_rx,
            "TermiRust Persistent Session could not start because tmux is not installed on this host.",
            "tmux missing fallback",
        );
        disconnect_runtime(&request, &runtime, &event_rx, "tmux missing fallback");
    }

    #[test]
    fn docker_ssh_session_connects_through_jump_host_chain() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker jump-host e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();

        let request = docker_jump_request(&server);
        let runtime = spawn_session(request.clone(), known_hosts.clone(), event_tx, 0);

        let mut saw_connected = false;
        let mut saw_output = false;
        let deadline = Instant::now() + Duration::from_secs(20);

        while Instant::now() < deadline && (!saw_connected || !saw_output) {
            match event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SshEvent::Connected { session_id, .. }) => {
                    assert_eq!(session_id, request.session_id);
                    saw_connected = true;
                    runtime
                        .command_tx
                        .send(SessionCommand::Input(
                            b"printf 'jump-runtime-e2e-ok\\n'\n".to_vec(),
                        ))
                        .expect("unable to send jump-host test command");
                }
                Ok(SshEvent::Output { session_id, data }) => {
                    assert_eq!(session_id, request.session_id);
                    let output = String::from_utf8_lossy(&data);
                    if output.contains("jump-runtime-e2e-ok") {
                        saw_output = true;
                        break;
                    }
                }
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => {
                    panic!(
                        "jump-host ssh runtime emitted error for session {session_id}: {message}"
                    );
                }
                Ok(SshEvent::TmuxSessionKilled { .. }) => {
                    panic!("jump-host test unexpectedly killed a tmux session")
                }
                Ok(SshEvent::Disconnected {
                    session_id,
                    message,
                }) => {
                    panic!(
                        "jump-host ssh runtime disconnected before output for session {session_id}: {message}"
                    );
                }
                Ok(_) => panic!("SSH runtime received an event reserved for durable sessions"),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("jump-host ssh runtime channel disconnected unexpectedly");
                }
            }
        }

        assert!(
            saw_connected,
            "did not observe jump-host ssh connected event"
        );
        assert!(
            saw_output,
            "did not observe runtime output from docker ssh server through jump host"
        );

        runtime
            .command_tx
            .send(SessionCommand::Disconnect)
            .expect("unable to disconnect jump-host ssh runtime");

        let disconnected = loop {
            match event_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(SshEvent::Disconnected { session_id, .. }) => break session_id,
                Ok(
                    SshEvent::Output { .. }
                    | SshEvent::Connected { .. }
                    | SshEvent::TmuxSessionKilled { .. },
                ) => continue,
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => panic!(
                    "unexpected jump-host ssh error after disconnect for {session_id}: {message}"
                ),
                Ok(_) => panic!("SSH runtime received an event reserved for durable sessions"),
                Err(error) => panic!("did not observe jump-host disconnect event: {error}"),
            }
        };
        assert_eq!(disconnected, request.session_id);

        let saved_keys = known_hosts.entries().expect("unable to read known hosts");
        assert!(saved_keys.len() >= 2);
        let endpoints = saved_keys
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        assert!(endpoints.contains(&"127.0.0.1:22".to_string()));
        assert!(endpoints.contains(&format!("127.0.0.1:{}", server.port)));
    }

    #[test]
    fn docker_ssh_local_port_forward_proxies_remote_service() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker local forward e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        server
            .exec(
                "nohup sh -lc 'while true; do printf \"forward-local-ok\\n\" | nc -l -p 39001 -q 1; done' >/tmp/forward-local.log 2>&1 &",
            )
            .expect("unable to start remote forwarded fixture service");

        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let local_port = allocate_local_port();

        let mut request = docker_ssh_request(&server);
        request.port_forward_rules = vec![PortForwardRule::Local {
            forward: LocalPortForward {
                local_host: "127.0.0.1".to_string(),
                local_port,
                remote_host: "127.0.0.1".to_string(),
                remote_port: 39001,
            },
        }];

        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "local forward");
        std::thread::sleep(Duration::from_millis(250));

        let mut stream =
            TcpStream::connect(("127.0.0.1", local_port)).expect("local forward should accept");
        let mut buffer = String::new();
        stream
            .read_to_string(&mut buffer)
            .expect("local forward should return remote payload");
        assert!(buffer.contains("forward-local-ok"));

        disconnect_runtime(&request, &runtime, &event_rx, "local forward");
    }

    #[test]
    fn docker_ssh_dynamic_port_forward_proxies_remote_service() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker dynamic forward e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        server
            .exec(
                "nohup sh -lc 'while true; do printf \"forward-socks-ok\\n\" | nc -l -p 39002 -q 1; done' >/tmp/forward-socks.log 2>&1 &",
            )
            .expect("unable to start remote socks fixture service");

        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let local_port = allocate_local_port();

        let mut request = docker_ssh_request(&server);
        request.port_forward_rules = vec![PortForwardRule::Dynamic {
            forward: DynamicPortForward {
                local_host: "127.0.0.1".to_string(),
                local_port,
            },
        }];

        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "dynamic forward");
        std::thread::sleep(Duration::from_millis(250));

        let mut stream =
            TcpStream::connect(("127.0.0.1", local_port)).expect("socks proxy should accept");
        stream
            .write_all(&[0x05, 0x01, 0x00])
            .expect("should send socks greeting");
        let mut greeting = [0u8; 2];
        stream
            .read_exact(&mut greeting)
            .expect("should read socks greeting");
        assert_eq!(greeting, [0x05, 0x00]);

        let request_bytes = [
            0x05, 0x01, 0x00, 0x03, 9, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x98,
            0x5a,
        ];
        stream
            .write_all(&request_bytes)
            .expect("should send socks connect");
        let mut response = [0u8; 10];
        stream
            .read_exact(&mut response)
            .expect("should read socks connect response");
        assert_eq!(response[0], 0x05);
        assert_eq!(response[1], 0x00);

        let mut buffer = String::new();
        stream
            .read_to_string(&mut buffer)
            .expect("socks forward should return remote payload");
        assert!(buffer.contains("forward-socks-ok"));

        disconnect_runtime(&request, &runtime, &event_rx, "dynamic forward");
    }

    #[test]
    fn docker_ssh_remote_port_forward_proxies_local_service() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker remote forward e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let local_listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("should bind local forwarded target");
        let local_port = local_listener
            .local_addr()
            .expect("should read local forwarded target addr")
            .port();
        let remote_port = allocate_local_port();

        let mut request = docker_ssh_request(&server);
        request.port_forward_rules = vec![PortForwardRule::Remote {
            forward: RemotePortForward {
                local_host: "127.0.0.1".to_string(),
                local_port,
                remote_host: "127.0.0.1".to_string(),
                remote_port,
            },
        }];

        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "remote forward");
        std::thread::sleep(Duration::from_millis(250));

        server
            .exec(&format!(
                "timeout 5 sh -lc \"printf 'reverse-forward-ok\\\\n' | nc -w 3 127.0.0.1 {}\"",
                remote_port
            ))
            .expect("remote forward should accept remote connection");

        let (mut accepted, _) = local_listener
            .accept()
            .expect("local forwarded target should accept");
        let mut payload = String::new();
        accepted
            .read_to_string(&mut payload)
            .expect("local forwarded target should read payload");
        assert!(payload.contains("reverse-forward-ok"));

        disconnect_runtime(&request, &runtime, &event_rx, "remote forward");
    }
}
