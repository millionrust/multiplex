use std::io::{IsTerminal as _, Write};
use std::time::Duration;

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use multiplex_client::{ConnectOptions, HostClient, LocalEndpoint, SequencedOutput};
use multiplex_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use rand::RngCore as _;
use tokio_util::sync::CancellationToken;

use crate::detach_keys::DetachKeys;
use crate::local::{
    ValidatedSessionAttach, decode_host_lifecycle, host_lifecycle_name, map_client,
    map_input_after_dispatch, map_resize_after_dispatch,
};
use crate::terminal_input::{TerminalInput, WindowResizes, window_output, window_size};
use crate::{Cancellation, CliCommand, CliError, CliPaths, ErrorCode, LocalCommandService};

const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(40);
/// A shell someone is typing into shows each keystroke's echo within this, where inspecting a
/// session can wait longer.
const SHELL_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long after the last keystroke a shell window lets go of the writer lease, so a paired device
/// can take over a terminal nobody is typing into.
const SHELL_LEASE_IDLE: Duration = Duration::from_millis(1_500);

/// How an interactive attach behaves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttachStyle {
    /// `session attach`: announces itself, and Ctrl-C detaches.
    Inspect,
    /// `shell`: the window is the terminal. Nothing is announced, its input reaches the program
    /// byte for byte, and only Ctrl-] then d detaches, leaving the session running. The window
    /// holds the writer lease only while it is typed in, so a paired device can take a terminal
    /// nobody here is using, and keys typed while a device holds it ring the bell instead of
    /// mixing in.
    Shell,
    /// Native terminal bridge: preserve every input byte, including terminal protocols.
    RawShell,
}

/// The writer lease as a shell window holds it: taken on the first keystroke, let go when the
/// typing stops.
struct ShellLease {
    held: bool,
    last_used: std::time::Instant,
}

impl ShellLease {
    fn new() -> Self {
        Self {
            held: false,
            last_used: std::time::Instant::now(),
        }
    }

    /// Whether this window may write now, taking the lease when nobody holds it.
    async fn take(&mut self, client: &mut HostClient, cancel: &CancellationToken) -> bool {
        if !self.held {
            self.held = client
                .set_writer_lease(CommandId::new(), true, cancel)
                .await
                .unwrap_or(false);
        }
        if self.held {
            self.last_used = std::time::Instant::now();
        }
        self.held
    }

    async fn release_if_idle(&mut self, client: &mut HostClient, cancel: &CancellationToken) {
        if self.held && self.last_used.elapsed() >= SHELL_LEASE_IDLE {
            let _ = client
                .set_writer_lease(CommandId::new(), false, cancel)
                .await;
            self.held = false;
        }
    }
}

/// Sends what was typed in a shell window, or rings the bell when a paired device is typing.
async fn send_shell_input(
    client: &mut HostClient,
    lease: &mut ShellLease,
    bytes: Vec<u8>,
    stdout: &mut impl Write,
    cancel: &CancellationToken,
) -> Result<(), CliError> {
    if bytes.is_empty() {
        return Ok(());
    }
    if lease.take(client, cancel).await {
        match client.input_all(&bytes, cancel).await {
            Ok(true) => return Ok(()),
            Ok(false) => lease.held = false,
            Err(error) => return Err(map_input_after_dispatch(error)),
        }
    }
    stdout
        .write_all(b"\x07")
        .and_then(|()| stdout.flush())
        .map_err(|_| output_unavailable())
}
pub struct LocalSessionAttachExecutor {
    paths: CliPaths,
}

impl LocalSessionAttachExecutor {
    pub fn new(paths: CliPaths) -> Self {
        Self { paths }
    }

    pub fn execute(
        &self,
        mut command: CliCommand,
        cancellation: &Cancellation,
    ) -> Result<(), CliError> {
        if !std::io::stdin().is_terminal()
            || !std::io::stdout().is_terminal()
            || !std::io::stderr().is_terminal()
        {
            return Err(CliError::new(
                ErrorCode::InteractionRequired,
                "interactive local Session attach requires a terminal",
                "Run attach without --json and without redirecting stdin, stdout, or stderr.",
            ));
        }
        let CliCommand::SessionAttach {
            session_id,
            from_sequence,
            columns,
            rows,
            request_control,
        } = &mut command
        else {
            return Err(CliError::new(
                ErrorCode::Usage,
                "interactive execution requires the local Session attach command",
                "Run multiplex-cli session attach <id> without --json.",
            ));
        };
        if let Some((terminal_columns, terminal_rows)) = window_size() {
            *columns = terminal_columns;
            *rows = terminal_rows;
        }
        let service = LocalCommandService::open(self.paths.clone());
        let validated = service.validate_session_attach(
            *session_id,
            *from_sequence,
            *columns,
            *rows,
            *request_control,
            cancellation,
        )?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| terminal_unavailable("unable to initialize interactive attach"))?;
        runtime.block_on(run_attach(
            *session_id,
            validated,
            cancellation,
            AttachStyle::Inspect,
        ))
    }
}

pub(crate) async fn run_attach(
    session_id: HostedSessionId,
    validated: ValidatedSessionAttach,
    cancellation: &Cancellation,
    style: AttachStyle,
) -> Result<(), CliError> {
    let async_cancel = CancellationToken::new();
    let mut nonce = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    // A shell window starts without the lease and takes it when typed in.
    let options = if validated.request.request_control && style == AttachStyle::Inspect {
        ConnectOptions::local(session_id, nonce)
    } else {
        ConnectOptions::local_read_only(session_id, nonce)
    };
    let endpoint = LocalEndpoint::new(&validated.runtime_root, session_id);
    let mut client = HostClient::connect(endpoint, options, &async_cancel)
        .await
        .map_err(map_client)?;
    verify_host_identity(&mut client, validated.request.expected_host_instance_id)?;

    // Raw mode, entered below, drops before these console modes are put back, including when
    // readiness or replay fails.
    let (_window_modes, mut stdout) = window_output(std::io::stdout().lock())
        .map_err(|_| terminal_unavailable("virtual terminal mode is unavailable"))?;
    let shell = style != AttachStyle::Inspect;
    // Enter raw input before announcing readiness or replaying output. Otherwise tcsetattr can
    // discard keys queued during restore, even though the native PTY already exists.
    let _native_raw_mode = if shell {
        enable_raw_mode()
            .map_err(|_| terminal_unavailable("interactive terminal mode is unavailable"))?;
        let guard = RawModeGuard;
        // Only the app's own bridge waits for this; a terminal window would ignore it.
        if style == AttachStyle::RawShell {
            stdout
                .write_all(crate::SHELL_READY_MARKER)
                .and_then(|()| stdout.flush())
                .map_err(|_| output_unavailable())?;
        }
        Some(guard)
    } else {
        None
    };
    let mut watermark = validated.request.from_sequence;
    let state = poll_output(
        &mut client,
        &mut stdout,
        &mut watermark,
        validated.request.columns,
        validated.request.rows,
        &async_cancel,
    )
    .await?;
    if style == AttachStyle::Inspect && validated.request.request_control && !state.has_writer_lease
    {
        client.disconnect();
        return Err(CliError::new(
            ErrorCode::InteractionRequired,
            "another Controller holds the Session writer lease",
            "Detach the current writer or attach without --write for read-only access.",
        ));
    }
    let lifecycle = decode_host_lifecycle(state.lifecycle)?;
    if lifecycle_terminal(lifecycle) {
        writeln!(
            std::io::stderr(),
            "Session replay complete; Host lifecycle is {}.",
            host_lifecycle_name(lifecycle)
        )
        .map_err(|_| output_unavailable())?;
        client.disconnect();
        return Ok(());
    }

    if style == AttachStyle::Inspect {
        let mode = if validated.request.request_control {
            "writer input enabled"
        } else {
            "read-only observer"
        };
        writeln!(
            std::io::stderr(),
            "Attached to the durable local Host ({mode}). Detach with Ctrl-] then d."
        )
        .map_err(|_| output_unavailable())?;
    }
    let _raw_mode = if shell {
        None
    } else {
        enable_raw_mode()
            .map_err(|_| terminal_unavailable("interactive terminal mode is unavailable"))?;
        Some(RawModeGuard)
    };
    if shell {
        return run_raw_shell(
            &mut client,
            &mut stdout,
            &mut watermark,
            (validated.request.columns, validated.request.rows),
            state
                .viewport
                .map(|viewport| (viewport.columns, viewport.rows)),
            (style == AttachStyle::Shell).then(DetachKeys::default),
            cancellation,
            &async_cancel,
        )
        .await;
    }
    let mut input = TerminalInput::stdin()
        .map_err(|_| terminal_unavailable("interactive terminal input is unavailable"))?;
    let mut detach_keys = DetachKeys::with_ctrl_c();
    let mut ticker = tokio::time::interval(LIVE_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut columns = validated.request.columns;
    let mut rows = validated.request.rows;
    // Attaching does not resize the Host. Watch from the size it reports, so a writer whose
    // window differs, including one resized while the attach started, resizes it at once.
    let host_size = state.viewport.and_then(|viewport| {
        Some((
            u16::try_from(viewport.columns).ok()?,
            u16::try_from(viewport.rows).ok()?,
        ))
    });
    let watched_from = match host_size {
        Some(size) if validated.request.request_control => size,
        _ => (columns, rows),
    };
    let mut resizes = WindowResizes::new(watched_from)
        .map_err(|_| terminal_unavailable("interactive terminal resizes are unavailable"))?;

    loop {
        tokio::select! {
            biased;
            bytes = input.recv() => {
                let bytes = bytes
                    .ok_or_else(|| terminal_unavailable("interactive terminal input ended"))?;
                let (bytes, detached) = detach_keys.filter(&bytes);
                // A read-only attach sends nothing on; its window only watches.
                if validated.request.request_control && !bytes.is_empty() {
                    send_input(&mut client, bytes, &async_cancel).await?;
                }
                if detached {
                    break;
                }
            }
            (next_columns, next_rows) = resizes.changed() => {
                columns = next_columns;
                rows = next_rows;
                if validated.request.request_control {
                    let applied = client
                        .resize(CommandId::new(), u32::from(columns), u32::from(rows), &async_cancel)
                        .await
                        .map_err(map_resize_after_dispatch)?;
                    if !applied {
                        return Err(CliError::new(
                            ErrorCode::Conflict,
                            "interactive terminal resize was not applied",
                            "Detach and inspect the current writer lease before retrying.",
                        ));
                    }
                }
            }
            _ = ticker.tick() => {
                if cancellation.is_cancelled() {
                    break;
                }
                let state = poll_output(
                    &mut client,
                    &mut stdout,
                    &mut watermark,
                    columns,
                    rows,
                    &async_cancel,
                ).await?;
                let lifecycle = decode_host_lifecycle(state.lifecycle)?;
                if validated.request.request_control && !state.has_writer_lease {
                    return Err(CliError::new(
                        ErrorCode::InteractionRequired,
                        "the Session writer lease was lost during attach",
                        "Reattach read-only or retry --write after the current writer detaches.",
                    ));
                }
                if lifecycle_terminal(lifecycle) {
                    break;
                }
            }
        }
    }
    let _ = client.detach(&async_cancel).await;
    Ok(())
}

/// A terminal window, or GPUI, already encodes keys, paste, focus, mouse reports and its replies
/// to the program's questions. Parsing that stream as key events loses protocol bytes, and stops
/// every key behind a reply the parser cannot finish. Read it verbatim, with bounded
/// backpressure, while polling the frontend PTY size so resize signals reach the durable Host
/// too. A shell window also watches for its detach keys.
#[allow(clippy::too_many_arguments)]
async fn run_raw_shell(
    client: &mut HostClient,
    stdout: &mut impl Write,
    watermark: &mut OutputSequence,
    viewport: (u16, u16),
    mut host_viewport: Option<(u32, u32)>,
    mut detach_keys: Option<DetachKeys>,
    cancellation: &Cancellation,
    cancel: &CancellationToken,
) -> Result<(), CliError> {
    let (mut columns, mut rows) = viewport;
    let mut input_rx = TerminalInput::stdin()
        .map_err(|_| terminal_unavailable("raw terminal input is unavailable"))?;
    let mut resizes = WindowResizes::new(viewport)
        .map_err(|_| terminal_unavailable("raw terminal resizes are unavailable"))?;
    let mut lease = ShellLease::new();
    let mut ticker = tokio::time::interval(SHELL_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            bytes = input_rx.recv() => {
                let Some(bytes) = bytes else { break; };
                let (bytes, detached) = match detach_keys.as_mut() {
                    Some(keys) => keys.filter(&bytes),
                    None => (bytes, false),
                };
                if bytes.is_empty() {
                    if detached { break; }
                    continue;
                }
                let viewport = (u32::from(columns), u32::from(rows));
                if lease.take(client, cancel).await && host_viewport != Some(viewport)
                    && client.resize(CommandId::new(), viewport.0, viewport.1, cancel).await.map_err(map_resize_after_dispatch)? {
                    host_viewport = Some(viewport);
                }
                send_shell_input(client, &mut lease, bytes, stdout, cancel).await?;
                if detached { break; }
            }
            (next_columns, next_rows) = resizes.changed() => {
                columns = next_columns;
                rows = next_rows;
                if lease.take(client, cancel).await
                    && client.resize(CommandId::new(), u32::from(columns), u32::from(rows), cancel).await.map_err(map_resize_after_dispatch)? {
                    host_viewport = Some((u32::from(columns), u32::from(rows)));
                }
            }
            _ = ticker.tick() => {
                if cancellation.is_cancelled() { break; }
                let state = poll_output(client, stdout, watermark, columns, rows, cancel).await?;
                if let Some(viewport) = state.viewport {
                    host_viewport = Some((viewport.columns, viewport.rows));
                }
                lease.release_if_idle(client, cancel).await;
                if lifecycle_terminal(decode_host_lifecycle(state.lifecycle)?) { break; }
            }
        }
    }
    let _ = client.detach(cancel).await;
    Ok(())
}

async fn send_input(
    client: &mut HostClient,
    bytes: Vec<u8>,
    cancel: &CancellationToken,
) -> Result<(), CliError> {
    let applied = client
        .input_all(&bytes, cancel)
        .await
        .map_err(map_input_after_dispatch)?;
    if applied {
        Ok(())
    } else {
        Err(CliError::new(
            ErrorCode::Conflict,
            "interactive terminal input was not applied",
            "Detach and inspect the current writer lease before retrying.",
        ))
    }
}

async fn poll_output(
    client: &mut HostClient,
    stdout: &mut impl Write,
    watermark: &mut OutputSequence,
    columns: u16,
    rows: u16,
    cancel: &CancellationToken,
) -> Result<multiplex_host_protocol::wire::StateEvent, CliError> {
    let outputs = client
        .attach(*watermark, u32::from(columns), u32::from(rows), cancel)
        .await
        .map_err(map_client)?;
    if let Some(snapshot) = client.take_last_snapshot() {
        if snapshot.boundary_sequence < watermark.get() {
            return Err(sequence_gap());
        }
        stdout
            .write_all(&snapshot.terminal_bytes)
            .and_then(|()| stdout.flush())
            .map_err(|_| output_unavailable())?;
        *watermark = OutputSequence::new(snapshot.boundary_sequence);
    }
    write_outputs(stdout, watermark, outputs)?;
    client.take_last_state().ok_or_else(|| {
        terminal_unavailable("durable Host attach completed without an authoritative state")
    })
}

fn write_outputs(
    stdout: &mut impl Write,
    watermark: &mut OutputSequence,
    outputs: Vec<SequencedOutput>,
) -> Result<(), CliError> {
    for output in outputs {
        if output.sequence <= *watermark {
            continue;
        }
        if watermark.checked_next() != Some(output.sequence) {
            return Err(sequence_gap());
        }
        stdout
            .write_all(&output.bytes)
            .map_err(|_| output_unavailable())?;
        *watermark = output.sequence;
    }
    stdout.flush().map_err(|_| output_unavailable())
}

fn verify_host_identity(client: &mut HostClient, expected: HostInstanceId) -> Result<(), CliError> {
    if client.host_instance_id() == Some(expected) {
        return Ok(());
    }
    client.disconnect();
    Err(CliError::new(
        ErrorCode::PermissionDenied,
        "durable Host identity changed before attach",
        "Inspect the Session and retry only after its current Host is confirmed.",
    ))
}

const fn lifecycle_terminal(lifecycle: multiplex_domain::HostLifecycle) -> bool {
    matches!(
        lifecycle,
        multiplex_domain::HostLifecycle::Exited
            | multiplex_domain::HostLifecycle::Failed
            | multiplex_domain::HostLifecycle::Orphaned
    )
}

fn sequence_gap() -> CliError {
    CliError::new(
        ErrorCode::Conflict,
        "local terminal output has a sequence gap",
        "Detach and reattach from an earlier output sequence to recover the snapshot.",
    )
}

fn terminal_unavailable(message: &str) -> CliError {
    CliError::new(
        ErrorCode::Unavailable,
        message,
        "Run attach from a supported local terminal and retry.",
    )
}

fn output_unavailable() -> CliError {
    CliError::new(
        ErrorCode::OperationFailed,
        "interactive terminal output is unavailable",
        "Restore stdout and reattach; the durable Host Session continues running.",
    )
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}
