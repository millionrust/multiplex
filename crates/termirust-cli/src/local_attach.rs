use std::io::{IsTerminal as _, Write};
use std::time::Duration;

use crossterm::event::{Event as TerminalEvent, EventStream, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures_util::StreamExt as _;
use rand::RngCore as _;
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint, SequencedOutput};
use termirust_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use tokio_util::sync::CancellationToken;

use crate::local::{
    ValidatedSessionAttach, decode_host_lifecycle, host_lifecycle_name, map_client,
    map_input_after_dispatch, map_resize_after_dispatch,
};
use crate::remote_ssh::{append_key_bytes, is_cancel_key, is_leader_key};
use crate::{Cancellation, CliCommand, CliError, CliPaths, ErrorCode, LocalCommandService};

const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(40);
const MAX_INTERACTIVE_INPUT_BYTES: usize = 16 * 1024;

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
                "Run termirust-cli session attach <id> without --json.",
            ));
        };
        if let Ok((terminal_columns, terminal_rows)) = crossterm::terminal::size() {
            *columns = terminal_columns.clamp(1, 1_000);
            *rows = terminal_rows.clamp(1, 1_000);
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
        runtime.block_on(run_attach(*session_id, validated, cancellation))
    }
}

async fn run_attach(
    session_id: HostedSessionId,
    validated: ValidatedSessionAttach,
    cancellation: &Cancellation,
) -> Result<(), CliError> {
    let async_cancel = CancellationToken::new();
    let mut nonce = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let options = if validated.request.request_control {
        ConnectOptions::local(session_id, nonce)
    } else {
        ConnectOptions::local_read_only(session_id, nonce)
    };
    let endpoint = LocalEndpoint::new(&validated.runtime_root, session_id);
    let mut client = HostClient::connect(endpoint, options, &async_cancel)
        .await
        .map_err(map_client)?;
    verify_host_identity(&mut client, validated.request.expected_host_instance_id)?;

    let mut stdout = std::io::stdout().lock();
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
    if validated.request.request_control && !state.has_writer_lease {
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
    enable_raw_mode()
        .map_err(|_| terminal_unavailable("interactive terminal mode is unavailable"))?;
    let _raw_mode = RawModeGuard;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(LIVE_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut columns = validated.request.columns;
    let mut rows = validated.request.rows;
    let mut leader = false;

    loop {
        tokio::select! {
            biased;
            event = events.next() => {
                let event = event
                    .ok_or_else(|| terminal_unavailable("interactive terminal event stream ended"))?
                    .map_err(|_| terminal_unavailable("interactive terminal input is unavailable"))?;
                match event {
                    TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        if is_cancel_key(&key) {
                            break;
                        }
                        if leader {
                            leader = false;
                            if matches!(key.code, KeyCode::Char('d' | 'D')) && key.modifiers.is_empty() {
                                break;
                            }
                            if validated.request.request_control {
                                send_input(&mut client, vec![0x1d], &async_cancel).await?;
                            }
                        } else if is_leader_key(&key) {
                            leader = true;
                            continue;
                        }
                        if validated.request.request_control {
                            let mut bytes = Vec::with_capacity(8);
                            append_key_bytes(&mut bytes, key)?;
                            if !bytes.is_empty() {
                                send_input(&mut client, bytes, &async_cancel).await?;
                            }
                        }
                    }
                    TerminalEvent::Paste(value) if validated.request.request_control => {
                        if value.len() > MAX_INTERACTIVE_INPUT_BYTES {
                            return Err(CliError::new(
                                ErrorCode::ResourceLimit,
                                "interactive paste exceeds the 16 KiB input limit",
                                "Paste a smaller payload or send it in separate chunks.",
                            ));
                        }
                        send_input(&mut client, value.into_bytes(), &async_cancel).await?;
                    }
                    TerminalEvent::Resize(next_columns, next_rows) if validated.request.request_control => {
                        columns = next_columns.clamp(1, 1_000);
                        rows = next_rows.clamp(1, 1_000);
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
                    _ => {}
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

async fn send_input(
    client: &mut HostClient,
    bytes: Vec<u8>,
    cancel: &CancellationToken,
) -> Result<(), CliError> {
    if bytes.len() > MAX_INTERACTIVE_INPUT_BYTES {
        return Err(CliError::new(
            ErrorCode::ResourceLimit,
            "interactive terminal input exceeds the 16 KiB limit",
            "Send a smaller input payload.",
        ));
    }
    let applied = client
        .input(CommandId::new(), bytes, cancel)
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
) -> Result<termirust_host_protocol::wire::StateEvent, CliError> {
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

const fn lifecycle_terminal(lifecycle: termirust_domain::HostLifecycle) -> bool {
    matches!(
        lifecycle,
        termirust_domain::HostLifecycle::Exited
            | termirust_domain::HostLifecycle::Failed
            | termirust_domain::HostLifecycle::Orphaned
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
