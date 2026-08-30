use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::PublicKey;
use russh::{ChannelMsg, Sig};
use russh_sftp::client::SftpSession;
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
use tokio_util::sync::CancellationToken;

use crate::models::{
    AuthConfig, ConnectRequest, DynamicPortForward, JumpHostConnection, LocalPortForward,
    OutboundProxy, PortForwardRule, RemotePortForward,
};
use crate::storage::{HostKeyDecision, KnownHostStore};
use crate::terminal::TerminalSize;
use termirust_domain::{HostInstanceId, OutputSequence};

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
    HostedBound {
        session_id: u64,
        host_instance_id: HostInstanceId,
    },
    HostedOutput {
        session_id: u64,
        host_instance_id: HostInstanceId,
        output_sequence: OutputSequence,
        data: Vec<u8>,
    },
    HostedSnapshot {
        session_id: u64,
        host_instance_id: HostInstanceId,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshDiagnosticStage {
    RouteAndAuthenticate,
    SessionChannel,
    Sftp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshDiagnosticStageState {
    Started,
    Passed,
}

#[derive(Debug)]
pub struct SshDiagnosticFailure {
    pub stage: SshDiagnosticStage,
    pub error: anyhow::Error,
}

#[derive(Clone, Copy)]
struct SshDiagnosticTimeouts {
    route: std::time::Duration,
    channel: std::time::Duration,
    sftp: std::time::Duration,
}

impl Default for SshDiagnosticTimeouts {
    fn default() -> Self {
        Self {
            route: std::time::Duration::from_secs(30),
            channel: std::time::Duration::from_secs(10),
            sftp: std::time::Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostKeyPolicy {
    TrustOnFirstUse,
    RequireExisting,
}

pub async fn diagnose_connection<F>(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    cancellation: CancellationToken,
    observe: F,
) -> std::result::Result<(), SshDiagnosticFailure>
where
    F: FnMut(SshDiagnosticStage, SshDiagnosticStageState, std::time::Duration),
{
    diagnose_connection_with_timeouts(
        request,
        known_hosts,
        cancellation,
        SshDiagnosticTimeouts::default(),
        observe,
    )
    .await
}

async fn diagnose_connection_with_timeouts<F>(
    mut request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    cancellation: CancellationToken,
    timeouts: SshDiagnosticTimeouts,
    mut observe: F,
) -> std::result::Result<(), SshDiagnosticFailure>
where
    F: FnMut(SshDiagnosticStage, SshDiagnosticStageState, std::time::Duration),
{
    use tokio::time::{Duration, Instant, timeout};

    request.startup_directory = None;
    request.startup_command = None;
    request.persistent_session = false;
    request.persistent_session_name = None;
    request.persistent_session_detach_others = false;
    request.port_forward_rules.clear();
    request.environment.clear();

    let config = Arc::new(client::Config::default());
    let stage = SshDiagnosticStage::RouteAndAuthenticate;
    observe(stage, SshDiagnosticStageState::Started, Duration::ZERO);
    let started = Instant::now();
    let established = tokio::select! {
        _ = cancellation.cancelled() => return Err(SshDiagnosticFailure {
            stage,
            error: anyhow::anyhow!("Connection diagnostic cancelled"),
        }),
        result = timeout(
            timeouts.route,
            establish_session_with_policy(
                config,
                request,
                known_hosts,
                false,
                HostKeyPolicy::RequireExisting,
            ),
        ) => result
            .map_err(|_| SshDiagnosticFailure {
                stage,
                error: anyhow::anyhow!("SSH route and authentication timed out"),
            })?
            .map_err(|error| SshDiagnosticFailure { stage, error })?,
    };
    observe(stage, SshDiagnosticStageState::Passed, started.elapsed());

    let stage = SshDiagnosticStage::SessionChannel;
    observe(stage, SshDiagnosticStageState::Started, Duration::ZERO);
    let started = Instant::now();
    let channel = tokio::select! {
        _ = cancellation.cancelled() => return Err(SshDiagnosticFailure {
            stage,
            error: anyhow::anyhow!("Connection diagnostic cancelled"),
        }),
        result = timeout(timeouts.channel, async {
            let handle = established.target_handle.lock().await;
            handle.channel_open_session().await
        }) => result
            .map_err(|_| SshDiagnosticFailure {
                stage,
                error: anyhow::anyhow!("SSH session channel probe timed out"),
            })?
            .context("Unable to open an SSH session channel")
            .map_err(|error| SshDiagnosticFailure { stage, error })?,
    };
    drop(channel);
    observe(stage, SshDiagnosticStageState::Passed, started.elapsed());

    let stage = SshDiagnosticStage::Sftp;
    observe(stage, SshDiagnosticStageState::Started, Duration::ZERO);
    let started = Instant::now();
    tokio::select! {
        _ = cancellation.cancelled() => return Err(SshDiagnosticFailure {
            stage,
            error: anyhow::anyhow!("Connection diagnostic cancelled"),
        }),
        result = timeout(timeouts.sftp, async {
            let channel = {
                let handle = established.target_handle.lock().await;
                handle
                    .channel_open_session()
                    .await
                    .context("Unable to open an SSH session channel for SFTP")?
            };
            channel
                .request_subsystem(true, "sftp")
                .await
                .context("Unable to start the SFTP subsystem")?;
            let sftp = SftpSession::new(channel.into_stream())
                .await
                .context("Unable to initialize the SFTP session")?;
            sftp.canonicalize(".".to_string())
                .await
                .context("Unable to resolve the SFTP home directory")?;
            let _ = sftp.close().await;
            Ok::<(), anyhow::Error>(())
        }) => result
            .map_err(|_| SshDiagnosticFailure {
                stage,
                error: anyhow::anyhow!("SFTP capability probe timed out"),
            })?
            .map_err(|error| SshDiagnosticFailure { stage, error })?,
    }
    observe(stage, SshDiagnosticStageState::Passed, started.elapsed());
    Ok(())
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
    let established =
        establish_session(Arc::new(config_inner), request, known_hosts, false).await?;
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
    let allow_agent_forwarding = request
        .auth
        .as_ref()
        .is_some_and(|auth| auth.forwarded_agent_socket().is_some());
    let established =
        establish_session(config, request.clone(), known_hosts, allow_agent_forwarding).await?;

    eprintln!("[ssh][{session_id}] authenticated, opening channel...");
    let channel = {
        let handle = established.target_handle.lock().await;
        handle
            .channel_open_session()
            .await
            .context("Unable to open an SSH session channel")?
    };

    if request
        .auth
        .as_ref()
        .is_some_and(|auth| auth.forwarded_agent_socket().is_some())
    {
        channel
            .agent_forward(true)
            .await
            .context("Unable to request one-shot SSH-agent forwarding")?;
    }

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

    let mut forward_tasks = ForwardTaskGuard::default();
    for rule in request.port_forward_rules.clone() {
        match rule {
            PortForwardRule::Local { forward } => {
                forward_tasks.push(
                    start_local_forwarder(established.target_handle.clone(), session_id, forward)
                        .await?,
                );
            }
            PortForwardRule::Dynamic { forward } => {
                forward_tasks.push(
                    start_dynamic_forwarder(established.target_handle.clone(), session_id, forward)
                        .await?,
                );
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

#[derive(Default)]
struct ForwardTaskGuard {
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ForwardTaskGuard {
    fn push(&mut self, task: tokio::task::JoinHandle<()>) {
        self.tasks.push(task);
    }
}

impl Drop for ForwardTaskGuard {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
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

async fn establish_session(
    config: Arc<client::Config>,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    allow_agent_forwarding: bool,
) -> Result<EstablishedSession> {
    establish_session_with_policy(
        config,
        request,
        known_hosts,
        allow_agent_forwarding,
        HostKeyPolicy::TrustOnFirstUse,
    )
    .await
}

async fn establish_session_with_policy(
    config: Arc<client::Config>,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    allow_agent_forwarding: bool,
    host_key_policy: HostKeyPolicy,
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
                allow_agent_forwarding,
                host_key_policy,
            )
            .await?
        } else {
            let (target_handle, target_trusted) = connect_and_authenticate(
                config,
                request.host.clone(),
                request.port,
                request.outbound_proxy.clone(),
                request.known_host_key(),
                request.username.clone(),
                auth,
                remote_forwards,
                known_hosts,
                allow_agent_forwarding,
                host_key_policy,
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

#[allow(clippy::too_many_arguments)]
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
    allow_agent_forwarding: bool,
    host_key_policy: HostKeyPolicy,
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
        first_hop.host.clone(),
        first_hop.port,
        first_hop.outbound_proxy.clone(),
        first_hop.known_host_key(),
        first_hop.username.clone(),
        first_hop.auth.clone(),
        Vec::new(),
        known_hosts.clone(),
        false,
        host_key_policy,
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
            false,
            host_key_policy,
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
        allow_agent_forwarding,
        host_key_policy,
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

#[allow(clippy::too_many_arguments)]
async fn connect_and_authenticate(
    config: Arc<client::Config>,
    host: String,
    port: u16,
    outbound_proxy: Option<OutboundProxy>,
    endpoint: String,
    username: String,
    auth: AuthConfig,
    remote_forwards: Vec<RemotePortForward>,
    known_hosts: Arc<KnownHostStore>,
    allow_agent_forwarding: bool,
    host_key_policy: HostKeyPolicy,
) -> Result<(client::Handle<SessionHandler>, Arc<AtomicBool>)> {
    let trusted_new_host = Arc::new(AtomicBool::new(false));
    let agent_forward_socket = if allow_agent_forwarding {
        auth.forwarded_agent_socket()
            .map(crate::ssh_auth::resolve_local_agent_socket)
            .transpose()?
    } else {
        None
    };
    let handler = SessionHandler {
        endpoint,
        known_hosts,
        trusted_new_host: trusted_new_host.clone(),
        remote_forwards,
        agent_forward_socket,
        host_key_policy,
    };

    let stream = crate::proxy::connect_first_hop(&host, port, outbound_proxy.as_ref()).await?;
    let mut handle = client::connect_stream(config, stream, handler)
        .await
        .context("Unable to open the SSH transport after establishing the network route")?;
    crate::ssh_auth::authenticate(&mut handle, &username, &auth).await?;
    Ok((handle, trusted_new_host))
}

#[allow(clippy::too_many_arguments)]
async fn connect_and_authenticate_stream<R>(
    config: Arc<client::Config>,
    stream: R,
    endpoint: String,
    username: String,
    auth: AuthConfig,
    remote_forwards: Vec<RemotePortForward>,
    known_hosts: Arc<KnownHostStore>,
    allow_agent_forwarding: bool,
    host_key_policy: HostKeyPolicy,
) -> Result<(client::Handle<SessionHandler>, Arc<AtomicBool>)>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let trusted_new_host = Arc::new(AtomicBool::new(false));
    let agent_forward_socket = if allow_agent_forwarding {
        auth.forwarded_agent_socket()
            .map(crate::ssh_auth::resolve_local_agent_socket)
            .transpose()?
    } else {
        None
    };
    let handler = SessionHandler {
        endpoint,
        known_hosts,
        trusted_new_host: trusted_new_host.clone(),
        remote_forwards,
        agent_forward_socket,
        host_key_policy,
    };

    let mut handle = client::connect_stream(config, stream, handler)
        .await
        .context("Unable to open the SSH transport through the jump host")?;
    crate::ssh_auth::authenticate(&mut handle, &username, &auth).await?;
    Ok((handle, trusted_new_host))
}

async fn start_local_forwarder(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    session_id: u64,
    forward: LocalPortForward,
) -> Result<tokio::task::JoinHandle<()>> {
    let bind_addr = format!("{}:{}", forward.local_host, forward.local_port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Unable to bind local forward on {bind_addr}"))?;

    eprintln!(
        "[ssh][{session_id}] local forward listening on {} -> {}:{}",
        bind_addr, forward.remote_host, forward.remote_port
    );

    Ok(tokio::spawn(async move {
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
    }))
}

async fn start_dynamic_forwarder(
    handle: Arc<Mutex<client::Handle<SessionHandler>>>,
    session_id: u64,
    forward: DynamicPortForward,
) -> Result<tokio::task::JoinHandle<()>> {
    let bind_addr = format!("{}:{}", forward.local_host, forward.local_port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Unable to bind dynamic forward on {bind_addr}"))?;

    eprintln!(
        "[ssh][{session_id}] dynamic forward listening on {} (SOCKS5)",
        bind_addr
    );

    Ok(tokio::spawn(async move {
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
    }))
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
    agent_forward_socket: Option<std::path::PathBuf>,
    host_key_policy: HostKeyPolicy,
}

impl client::Handler for SessionHandler {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, server_public_key: &PublicKey) -> Result<bool> {
        eprintln!("[ssh] check_server_key for endpoint={}", self.endpoint);
        let key = server_public_key
            .to_openssh()
            .context("Unable to serialize the server public key")?;

        if self.host_key_policy == HostKeyPolicy::RequireExisting {
            self.known_hosts.verify_existing(&self.endpoint, &key)?;
            eprintln!("[ssh] host key matched existing entry (strict diagnostic)");
            return Ok(true);
        }

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

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _session: &mut client::Session,
    ) -> Result<()> {
        #[cfg(not(unix))]
        {
            let _ = channel;
            bail!("SSH-agent forwarding is unavailable on this platform");
        }
        #[cfg(unix)]
        {
            let socket = self
                .agent_forward_socket
                .clone()
                .context("The server requested SSH-agent forwarding without approval")?;
            tokio::spawn(async move {
                let result = async move {
                    let mut local_agent =
                        tokio::net::UnixStream::connect(socket).await.map_err(|_| {
                            anyhow::anyhow!("Unable to reach the approved local SSH agent")
                        })?;
                    let mut remote_agent = channel.into_stream();
                    tokio::io::copy_bidirectional(&mut local_agent, &mut remote_agent)
                        .await
                        .context("SSH-agent forwarding channel failed")?;
                    Result::<()>::Ok(())
                }
                .await;
                if let Err(error) = result {
                    eprintln!("[ssh] approved agent forwarding channel failed: {error}");
                }
            });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RemoteExecExit, SessionCommand, SshDiagnosticStage, SshDiagnosticTimeouts, SshEvent,
        diagnose_connection, diagnose_connection_with_timeouts, spawn_remote_exec, spawn_session,
        tmux_kill_command,
    };
    use crate::models::{
        AuthConfig, ConnectRequest, ConnectionKind, DynamicPortForward, JumpHostConnection,
        LocalPortForward, OutboundProxy, PortForwardRule, RemotePortForward,
    };
    use crate::storage::KnownHostStore;
    #[cfg(unix)]
    use crate::test_support::TestSshAgent;
    use crate::test_support::{
        DockerSshServer, TestIsolation, TestProxyProtocol, TestTcpProxy, allocate_local_port,
        create_test_user_certificate,
    };
    use crate::ui::shell::{shell_single_quote, startup_bytes_for_request};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

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
            outbound_proxy: None,
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
    fn connection_diagnostic_times_out_and_cancels_a_stalled_transport() {
        let _isolation = TestIsolation::acquire();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (connection, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(2));
            drop(connection);
        });
        let mut request = ssh_request_for_endpoint(port);
        request.session_id = 501;
        request.title = "Stalled SSH".to_string();
        let known_hosts = Arc::new(KnownHostStore::load().unwrap());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let timed_out = runtime
            .block_on(diagnose_connection_with_timeouts(
                request,
                known_hosts,
                CancellationToken::new(),
                SshDiagnosticTimeouts {
                    route: Duration::from_millis(100),
                    channel: Duration::from_millis(100),
                    sftp: Duration::from_millis(100),
                },
                |_, _, _| {},
            ))
            .unwrap_err();
        assert_eq!(timed_out.stage, SshDiagnosticStage::RouteAndAuthenticate);
        assert!(format!("{:#}", timed_out.error).contains("timed out"));
        server.join().unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (connection, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(2));
            drop(connection);
        });
        let mut request = ssh_request_for_endpoint(port);
        request.session_id = 502;
        let known_hosts = Arc::new(KnownHostStore::load().unwrap());
        let cancellation = CancellationToken::new();
        let cancel_from_thread = cancellation.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            cancel_from_thread.cancel();
        });
        let started = Instant::now();
        let cancelled = runtime
            .block_on(diagnose_connection_with_timeouts(
                request,
                known_hosts,
                cancellation,
                SshDiagnosticTimeouts {
                    route: Duration::from_secs(5),
                    channel: Duration::from_secs(5),
                    sftp: Duration::from_secs(5),
                },
                |_, _, _| {},
            ))
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(format!("{:#}", cancelled.error).contains("cancelled"));
        canceller.join().unwrap();
        server.join().unwrap();
    }

    fn ssh_request_for_endpoint(port: u16) -> ConnectRequest {
        ConnectRequest {
            session_id: 1,
            title: "SSH endpoint".to_string(),
            kind: ConnectionKind::Ssh,
            host: "127.0.0.1".to_string(),
            port,
            username: "user".to_string(),
            auth: Some(AuthConfig::Password {
                password: "secret".to_string(),
            }),
            jump_host: None,
            outbound_proxy: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 1_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }

    fn run_connection_diagnostic(
        request: ConnectRequest,
        known_hosts: Arc<KnownHostStore>,
        cancellation: CancellationToken,
    ) -> Result<Vec<SshDiagnosticStage>, super::SshDiagnosticFailure> {
        let stages = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = stages.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(diagnose_connection(
            request,
            known_hosts,
            cancellation,
            move |stage, state, _| {
                if state == super::SshDiagnosticStageState::Passed {
                    observed.lock().unwrap().push(stage);
                }
            },
        ))?;
        Ok(stages.lock().unwrap().clone())
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
                outbound_proxy: None,
            }),
            outbound_proxy: None,
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

    fn wait_for_runtime_error(
        request: &ConnectRequest,
        event_rx: &mpsc::Receiver<SshEvent>,
        label: &str,
    ) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => {
                    assert_eq!(session_id, request.session_id);
                    return message;
                }
                Ok(SshEvent::Connected { .. }) => panic!("{label} unexpectedly authenticated"),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        panic!("{label} did not produce a runtime error");
    }

    fn assert_loopback_port_released(port: u16, label: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => {
                    drop(listener);
                    return;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        panic!("{label} did not release 127.0.0.1:{port}");
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
    fn docker_ssh_connection_diagnostic_is_strict_read_only_and_recovers() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker connection diagnostic e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let mut request = docker_ssh_request(&server);

        let unknown = run_connection_diagnostic(
            request.clone(),
            known_hosts.clone(),
            CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(unknown.stage, SshDiagnosticStage::RouteAndAuthenticate);
        assert!(format!("{:#}", unknown.error).contains("Host key is not trusted"));
        assert!(known_hosts.entries().unwrap().is_empty());

        let (event_tx, event_rx) = mpsc::channel();
        let runtime = spawn_session(request.clone(), known_hosts.clone(), event_tx, 0);
        wait_for_connected(&request, &event_rx, "diagnostic trust setup");
        disconnect_runtime(&request, &runtime, &event_rx, "diagnostic trust setup");
        assert_eq!(known_hosts.entries().unwrap().len(), 1);

        let forward_port = allocate_local_port();
        request.startup_command = Some("touch /home/termirust/diagnostic-startup-ran".to_string());
        request.persistent_session = true;
        request.persistent_session_name = Some("diagnostic-must-not-exist".to_string());
        request.port_forward_rules = vec![PortForwardRule::Dynamic {
            forward: DynamicPortForward {
                local_host: "127.0.0.1".to_string(),
                local_port: forward_port,
            },
        }];
        request.environment = vec![("SECRET_DIAGNOSTIC_VALUE".to_string(), "hidden".to_string())];

        let stages = run_connection_diagnostic(
            request.clone(),
            known_hosts.clone(),
            CancellationToken::new(),
        )
        .expect("strict diagnostic should pass after normal trust review");
        assert_eq!(
            stages,
            vec![
                SshDiagnosticStage::RouteAndAuthenticate,
                SshDiagnosticStage::SessionChannel,
                SshDiagnosticStage::Sftp,
            ]
        );

        let target = format!("{}:{}", server.host(), server.port)
            .parse()
            .expect("invalid Docker SSH endpoint");
        let proxy = TestTcpProxy::start(TestProxyProtocol::HttpConnect, target)
            .expect("unable to start diagnostic proxy");
        let mut proxied = request.clone();
        proxied.outbound_proxy = Some(OutboundProxy::HttpConnect {
            host: "127.0.0.1".to_string(),
            port: proxy.port,
        });
        run_connection_diagnostic(proxied, known_hosts.clone(), CancellationToken::new())
            .expect("strict diagnostic should reuse the saved proxy route");
        assert!(proxy.accepted_connections() >= 1);

        let jump_request = docker_jump_request(&server);
        let (event_tx, event_rx) = mpsc::channel();
        let jump_trust = spawn_session(jump_request.clone(), known_hosts.clone(), event_tx, 0);
        wait_for_connected(&jump_request, &event_rx, "diagnostic jump trust setup");
        disconnect_runtime(
            &jump_request,
            &jump_trust,
            &event_rx,
            "diagnostic jump trust setup",
        );
        run_connection_diagnostic(jump_request, known_hosts.clone(), CancellationToken::new())
            .expect("strict diagnostic should reuse the saved jump route");

        server
            .exec("test ! -e /home/termirust/diagnostic-startup-ran")
            .expect("diagnostic must not run the startup command");
        server
            .exec("! su -s /bin/sh -c 'tmux has-session -t diagnostic-must-not-exist' termirust")
            .expect("diagnostic must not create the configured tmux session");
        let listener = TcpListener::bind(("127.0.0.1", forward_port))
            .expect("diagnostic must not activate configured forwarding");
        drop(listener);

        let mut denied_request = request.clone();
        denied_request.auth = Some(AuthConfig::Password {
            password: "incorrect".to_string(),
        });
        let denied = run_connection_diagnostic(
            denied_request,
            known_hosts.clone(),
            CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(denied.stage, SshDiagnosticStage::RouteAndAuthenticate);
        assert!(format!("{:#}", denied.error).contains("Authentication was rejected"));

        let endpoint = request.address();
        known_hosts.remove(&endpoint).unwrap();
        known_hosts
            .verify_or_trust(&endpoint, "ssh-ed25519 deliberately-wrong")
            .unwrap();
        let mismatch = run_connection_diagnostic(
            request.clone(),
            known_hosts.clone(),
            CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(mismatch.stage, SshDiagnosticStage::RouteAndAuthenticate);
        assert!(format!("{:#}", mismatch.error).contains("Host key mismatch"));
        known_hosts.remove(&endpoint).unwrap();
        let (event_tx, event_rx) = mpsc::channel();
        let trust_recovery = spawn_session(request.clone(), known_hosts.clone(), event_tx, 0);
        wait_for_connected(&request, &event_rx, "diagnostic trust recovery");
        disconnect_runtime(
            &request,
            &trust_recovery,
            &event_rx,
            "diagnostic trust recovery",
        );

        server
            .exec("mv /usr/lib/openssh/sftp-server /usr/lib/openssh/sftp-server.disabled")
            .expect("unable to disable SFTP fixture");
        let no_sftp = run_connection_diagnostic(
            request.clone(),
            known_hosts.clone(),
            CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(no_sftp.stage, SshDiagnosticStage::Sftp);
        server
            .exec("mv /usr/lib/openssh/sftp-server.disabled /usr/lib/openssh/sftp-server")
            .expect("unable to restore SFTP fixture");

        run_connection_diagnostic(
            request.clone(),
            known_hosts.clone(),
            CancellationToken::new(),
        )
        .expect("diagnostic should recover after SFTP is restored");

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let cancellation = run_connection_diagnostic(request, known_hosts, cancelled).unwrap_err();
        assert_eq!(cancellation.stage, SshDiagnosticStage::RouteAndAuthenticate);
        assert!(format!("{:#}", cancellation.error).contains("cancelled"));
    }

    #[test]
    fn docker_ssh_openssh_certificate_connects_and_streams_output() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker certificate ssh e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let certificate_dir = tempfile::TempDir::new().expect("unable to create certificate dir");
        let certificate =
            create_test_user_certificate(certificate_dir.path(), server.username(), true);
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let mut request = docker_ssh_request(&server);
        request.auth = Some(AuthConfig::OpenSshCertificate {
            key_path: docker_private_key_path(),
            certificate_path: certificate.display().to_string(),
            passphrase: None,
        });

        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "certificate ssh");
        runtime
            .command_tx
            .send(SessionCommand::Input(
                b"printf 'certificate-runtime-ok\\n'\n".to_vec(),
            ))
            .expect("unable to send certificate test command");
        wait_for_output_contains(
            &request,
            &event_rx,
            "certificate-runtime-ok",
            "certificate ssh",
        );
        disconnect_runtime(&request, &runtime, &event_rx, "certificate ssh");
    }

    #[test]
    fn docker_ssh_openssh_certificate_rejects_untrusted_signer_without_fallback() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker certificate rejection e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let certificate_dir = tempfile::TempDir::new().expect("unable to create certificate dir");
        let certificate =
            create_test_user_certificate(certificate_dir.path(), server.username(), false);
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let mut request = docker_ssh_request(&server);
        request.auth = Some(AuthConfig::OpenSshCertificate {
            key_path: docker_private_key_path(),
            certificate_path: certificate.display().to_string(),
            passphrase: None,
        });

        let _runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SshEvent::Error {
                    session_id,
                    message,
                }) => {
                    assert_eq!(session_id, request.session_id);
                    assert!(message.contains("Authentication was rejected by the server"));
                    assert!(!message.contains(&docker_private_key_path()));
                    assert!(!message.contains(certificate.to_string_lossy().as_ref()));
                    return;
                }
                Ok(SshEvent::Connected { .. }) => {
                    panic!("untrusted certificate unexpectedly authenticated")
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        panic!("untrusted certificate did not produce an authentication error");
    }

    #[test]
    fn docker_jump_host_accepts_openssh_certificates_on_both_hops() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker jump certificate e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let certificate_dir = tempfile::TempDir::new().expect("unable to create certificate dir");
        let certificate =
            create_test_user_certificate(certificate_dir.path(), server.username(), true);
        let certificate_auth = AuthConfig::OpenSshCertificate {
            key_path: docker_private_key_path(),
            certificate_path: certificate.display().to_string(),
            passphrase: None,
        };
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let mut request = docker_jump_request(&server);
        request.auth = Some(certificate_auth.clone());
        request.jump_host.as_mut().unwrap().auth = certificate_auth;

        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "certificate jump ssh");
        runtime
            .command_tx
            .send(SessionCommand::Input(
                b"printf 'certificate-jump-ok\\n'\n".to_vec(),
            ))
            .expect("unable to send certificate jump command");
        wait_for_output_contains(
            &request,
            &event_rx,
            "certificate-jump-ok",
            "certificate jump ssh",
        );
        disconnect_runtime(&request, &runtime, &event_rx, "certificate jump ssh");
    }

    #[cfg(unix)]
    #[test]
    fn docker_ssh_agent_authenticates_terminal_and_remote_exec() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker SSH-agent auth e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        let agent = TestSshAgent::start_with_fixture_key().expect("unable to start test SSH agent");
        let auth = AuthConfig::LocalAgent {
            socket_path: Some(agent.socket_path().display().to_string()),
            forward_agent: false,
        };

        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let mut request = docker_ssh_request(&server);
        request.auth = Some(auth.clone());
        let runtime = spawn_session(request.clone(), known_hosts.clone(), event_tx, 0);
        wait_for_connected(&request, &event_rx, "SSH-agent terminal auth");
        runtime
            .command_tx
            .send(SessionCommand::Input(
                b"printf 'agent-terminal-ok\\n'\n".to_vec(),
            ))
            .expect("unable to send agent terminal command");
        wait_for_output_contains(
            &request,
            &event_rx,
            "agent-terminal-ok",
            "SSH-agent terminal auth",
        );
        disconnect_runtime(&request, &runtime, &event_rx, "SSH-agent terminal auth");

        request.session_id += 1;
        let AuthConfig::LocalAgent { socket_path, .. } = auth else {
            unreachable!();
        };
        request.auth = Some(AuthConfig::LocalAgent {
            socket_path,
            forward_agent: true,
        });
        let mut process = spawn_remote_exec(
            request,
            known_hosts,
            0,
            "test -z \"${SSH_AUTH_SOCK:-}\" && printf 'agent-remote-exec-ok'".to_string(),
        )
        .expect("unable to start agent-authenticated remote exec");
        let mut stdout = String::new();
        process.stdout.read_to_string(&mut stdout).unwrap();
        let exit = process
            .exit_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("agent remote exec did not report an exit")
            .expect("agent remote exec transport failed");
        assert_eq!(stdout, "agent-remote-exec-ok");
        assert_eq!(exit, RemoteExecExit::Status(0));
    }

    #[cfg(unix)]
    #[test]
    fn docker_ssh_agent_rejects_unavailable_empty_and_untrusted_agents_without_fallback() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker SSH-agent rejection e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");

        let agent = TestSshAgent::start_empty().expect("unable to start empty SSH agent");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let mut request = docker_ssh_request(&server);
        request.session_id = 90;
        request.auth = Some(AuthConfig::LocalAgent {
            socket_path: Some(agent.socket_path().display().to_string()),
            forward_agent: false,
        });
        let _runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        let message = wait_for_runtime_error(&request, &event_rx, "empty agent");
        assert!(message.contains("has no identities"));
        assert!(!message.contains(agent.socket_path().to_string_lossy().as_ref()));
        drop(agent);

        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        request.session_id = 91;
        request.auth = Some(AuthConfig::LocalAgent {
            socket_path: Some("/tmp/customer-secret-missing-agent.sock".to_string()),
            forward_agent: false,
        });
        let _runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        let message = wait_for_runtime_error(&request, &event_rx, "unavailable agent");
        assert!(!message.contains("customer-secret-missing-agent.sock"));

        let agent =
            TestSshAgent::start_with_untrusted_key().expect("unable to start untrusted SSH agent");
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        request.session_id = 92;
        request.auth = Some(AuthConfig::LocalAgent {
            socket_path: Some(agent.socket_path().display().to_string()),
            forward_agent: false,
        });
        let _runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        let message = wait_for_runtime_error(&request, &event_rx, "untrusted agent key");
        assert!(message.contains("Authentication was rejected by the server"));
        assert!(!message.contains(agent.socket_path().to_string_lossy().as_ref()));
    }

    #[cfg(unix)]
    #[test]
    fn docker_jump_host_accepts_ssh_agent_authentication_on_both_hops() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker jump SSH-agent e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        let agent = TestSshAgent::start_with_fixture_key().expect("unable to start test SSH agent");
        let auth = AuthConfig::LocalAgent {
            socket_path: Some(agent.socket_path().display().to_string()),
            forward_agent: false,
        };
        let mut request = docker_jump_request(&server);
        request.auth = Some(auth.clone());
        request.jump_host.as_mut().unwrap().auth = auth;
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();

        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "SSH-agent jump chain");
        runtime
            .command_tx
            .send(SessionCommand::Input(
                b"printf 'agent-jump-ok\\n'\n".to_vec(),
            ))
            .expect("unable to send agent jump command");
        wait_for_output_contains(&request, &event_rx, "agent-jump-ok", "SSH-agent jump chain");
        disconnect_runtime(&request, &runtime, &event_rx, "SSH-agent jump chain");
    }

    #[cfg(unix)]
    #[test]
    fn docker_ssh_agent_forwarding_requires_explicit_per_connection_approval() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker agent-forwarding e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        let agent = TestSshAgent::start_with_fixture_key().expect("unable to start test SSH agent");
        let socket_path = Some(agent.socket_path().display().to_string());

        let mut request = docker_ssh_request(&server);
        request.session_id = 93;
        request.auth = Some(AuthConfig::LocalAgent {
            socket_path: socket_path.clone(),
            forward_agent: false,
        });
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "agent forwarding disabled");
        runtime
            .command_tx
            .send(SessionCommand::Input(
                b"test -z \"${SSH_AUTH_SOCK:-}\" && printf 'agent-forward-disabled-ok\\n'\n"
                    .to_vec(),
            ))
            .expect("unable to test disabled agent forwarding");
        wait_for_output_contains(
            &request,
            &event_rx,
            "agent-forward-disabled-ok",
            "agent forwarding disabled",
        );
        disconnect_runtime(&request, &runtime, &event_rx, "agent forwarding disabled");

        request.session_id = 94;
        request.auth = Some(AuthConfig::LocalAgent {
            socket_path,
            forward_agent: true,
        });
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        wait_for_connected(&request, &event_rx, "agent forwarding approved");
        runtime
            .command_tx
            .send(SessionCommand::Input(
                b"ssh-add -l >/dev/null 2>&1 && printf 'agent-forward-approved-ok\\n'\n".to_vec(),
            ))
            .expect("unable to test approved agent forwarding");
        wait_for_output_contains(
            &request,
            &event_rx,
            "agent-forward-approved-ok",
            "agent forwarding approved",
        );
        disconnect_runtime(&request, &runtime, &event_rx, "agent forwarding approved");
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
    fn docker_ssh_routes_terminal_exec_and_first_jump_hop_through_supported_proxies() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker proxy e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let target = format!("{}:{}", server.host(), server.port)
            .parse()
            .expect("invalid Docker SSH endpoint");

        for (index, protocol) in [TestProxyProtocol::Socks5, TestProxyProtocol::HttpConnect]
            .into_iter()
            .enumerate()
        {
            let proxy = TestTcpProxy::start(protocol, target).expect("unable to start test proxy");
            let route = match protocol {
                TestProxyProtocol::Socks5 => OutboundProxy::Socks5 {
                    host: "127.0.0.1".to_string(),
                    port: proxy.port,
                },
                TestProxyProtocol::HttpConnect => OutboundProxy::HttpConnect {
                    host: "127.0.0.1".to_string(),
                    port: proxy.port,
                },
            };
            let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
            let (event_tx, event_rx) = mpsc::channel();
            let mut request = docker_ssh_request(&server);
            request.session_id = 120 + index as u64;
            request.outbound_proxy = Some(route.clone());
            let runtime = spawn_session(request.clone(), known_hosts.clone(), event_tx, 0);
            wait_for_connected(&request, &event_rx, "proxied terminal");
            runtime
                .command_tx
                .send(SessionCommand::Input(
                    b"printf 'proxy-terminal-ok\\n'\n".to_vec(),
                ))
                .unwrap();
            wait_for_output_contains(&request, &event_rx, "proxy-terminal-ok", "proxied terminal");
            disconnect_runtime(&request, &runtime, &event_rx, "proxied terminal");

            request.session_id += 10;
            let mut process = spawn_remote_exec(
                request,
                known_hosts.clone(),
                0,
                "printf 'proxy-exec-ok'".to_string(),
            )
            .expect("unable to start proxied remote exec");
            let mut stdout = String::new();
            process.stdout.read_to_string(&mut stdout).unwrap();
            let exit = process
                .exit_rx
                .recv_timeout(Duration::from_secs(15))
                .unwrap()
                .unwrap();
            assert_eq!(stdout, "proxy-exec-ok");
            assert_eq!(exit, RemoteExecExit::Status(0));

            let (event_tx, event_rx) = mpsc::channel();
            let mut jump_request = docker_jump_request(&server);
            jump_request.session_id = 140 + index as u64;
            let first_hop = jump_request.jump_host.as_mut().unwrap();
            first_hop.host = "proxy-only.invalid".to_string();
            first_hop.port = 65022;
            first_hop.outbound_proxy = Some(route);
            let runtime = spawn_session(jump_request.clone(), known_hosts, event_tx, 0);
            wait_for_connected(&jump_request, &event_rx, "proxied jump hop");
            disconnect_runtime(&jump_request, &runtime, &event_rx, "proxied jump hop");
            assert!(proxy.accepted_connections() >= 2);
        }
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
        assert_loopback_port_released(local_port, "local forward disconnect");
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
        assert_loopback_port_released(local_port, "dynamic forward disconnect");
    }

    #[test]
    fn docker_ssh_forward_bind_conflict_is_reported_and_prior_listeners_are_cleaned_up() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker forward conflict e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let first_port = allocate_local_port();
        let conflict_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let conflict_port = conflict_listener.local_addr().unwrap().port();
        let mut request = docker_ssh_request(&server);
        request.port_forward_rules = vec![
            PortForwardRule::Local {
                forward: LocalPortForward {
                    local_host: "127.0.0.1".to_string(),
                    local_port: first_port,
                    remote_host: "127.0.0.1".to_string(),
                    remote_port: 22,
                },
            },
            PortForwardRule::Dynamic {
                forward: DynamicPortForward {
                    local_host: "127.0.0.1".to_string(),
                    local_port: conflict_port,
                },
            },
        ];
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let _runtime = spawn_session(request.clone(), known_hosts, event_tx, 0);
        let error = wait_for_runtime_error(&request, &event_rx, "forward bind conflict");
        assert!(error.contains("Unable to bind dynamic forward"));
        assert!(!error.contains(server.password()));
        assert_loopback_port_released(first_port, "partially started forward set");
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
