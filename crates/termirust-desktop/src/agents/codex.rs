use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::agents::process::build_remote_structured_command;
use crate::agents::protocol::{
    AgentApprovalKind, AgentApprovalRequest, AgentEvent, AgentRole, AgentRunState,
    NormalizedToolCall, ToolOutcome,
};
use crate::agents::stream::{BoundedLine, read_bounded_lines};
use crate::models::{
    AgentBackendKind, AgentLocation, AgentPermissionPolicy, AgentProvider, ConnectRequest,
    SavedAgentDefinition,
};
use crate::ssh::{RemoteExecControl, RemoteExecExit, RemoteExecProcess, spawn_remote_exec};
use crate::storage::KnownHostStore;

const INITIALIZE_REQUEST_ID: u64 = 1;
const THREAD_START_REQUEST_ID: u64 = 2;
const FIRST_USER_REQUEST_ID: u64 = 100;
const EVENT_CHANNEL_CAPACITY: usize = 512;
const COMMAND_CHANNEL_CAPACITY: usize = 64;
const MAX_STDERR_BYTES: usize = 16 * 1024;
const MAX_EVENT_LINE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct CodexSessionConfig {
    pub executable: PathBuf,
    pub working_directory: PathBuf,
    pub permission_policy: AgentPermissionPolicy,
    pub initial_prompt: Option<String>,
}

#[derive(Clone)]
pub struct RemoteCodexSessionConfig {
    pub definition: SavedAgentDefinition,
    pub request: ConnectRequest,
    pub known_hosts: Arc<KnownHostStore>,
    pub keepalive_secs: u16,
    pub initial_prompt: Option<String>,
}

#[derive(Default)]
struct SessionIds {
    thread_id: Option<String>,
    turn_id: Option<String>,
}

pub struct CodexSessionHandle {
    command_tx: SyncSender<Value>,
    pub event_rx: Receiver<AgentEvent>,
    ids: Arc<Mutex<SessionIds>>,
    next_request_id: AtomicU64,
    child_id: Arc<AtomicU32>,
    remote_control: Option<RemoteExecControl>,
}

impl CodexSessionHandle {
    pub fn send_prompt(&self, prompt: impl Into<String>) -> Result<()> {
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            bail!("Codex prompt cannot be empty");
        }
        let thread_id = self
            .ids
            .lock()
            .map_err(|_| anyhow::anyhow!("Codex session state is unavailable"))?
            .thread_id
            .clone()
            .context("Codex session is not ready")?;
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.send_wire(json!({
            "id": id,
            "method": "turn/start",
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": prompt, "text_elements": []}]
            }
        }))
    }

    pub fn cancel(&self) -> Result<()> {
        let ids = self
            .ids
            .lock()
            .map_err(|_| anyhow::anyhow!("Codex session state is unavailable"))?;
        let thread_id = ids
            .thread_id
            .clone()
            .context("Codex session is not ready")?;
        let turn_id = ids.turn_id.clone().context("Codex has no active turn")?;
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        self.send_wire(json!({
            "id": id,
            "method": "turn/interrupt",
            "params": {"threadId": thread_id, "turnId": turn_id}
        }))
    }

    pub fn respond_to_approval(&self, request_id: &str, allow: bool) -> Result<()> {
        let id = parse_request_id(request_id)?;
        self.send_wire(json!({
            "id": id,
            "result": {"decision": if allow { "accept" } else { "decline" }}
        }))
    }

    fn send_wire(&self, value: Value) -> Result<()> {
        self.command_tx
            .try_send(value)
            .map_err(|error| match error {
                std::sync::mpsc::TrySendError::Full(_) => {
                    anyhow::anyhow!("Codex command queue is full; wait for the current request")
                }
                std::sync::mpsc::TrySendError::Disconnected(_) => {
                    anyhow::anyhow!("Codex app-server is no longer accepting commands")
                }
            })
    }
}

impl Drop for CodexSessionHandle {
    fn drop(&mut self) {
        let child_id = self.child_id.load(Ordering::Acquire);
        if child_id != 0 {
            #[cfg(unix)]
            unsafe {
                libc::kill(child_id as libc::pid_t, libc::SIGTERM);
            }
        }
        if let Some(control) = &self.remote_control {
            let _ = control.terminate();
        }
    }
}

pub fn spawn_codex_session(config: CodexSessionConfig) -> Result<CodexSessionHandle> {
    if !config.working_directory.is_dir() {
        bail!(
            "Codex working directory does not exist: {}",
            config.working_directory.display()
        );
    }
    let mut child = Command::new(&config.executable)
        .args(["app-server", "--stdio"])
        .current_dir(&config.working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "Unable to launch {} app-server",
                config.executable.display()
            )
        })?;
    let child_id = Arc::new(AtomicU32::new(child.id()));
    let stdin = child
        .stdin
        .take()
        .context("Codex app-server stdin is unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("Codex app-server stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Codex app-server stderr is unavailable")?;
    start_codex_transport(
        config,
        stdin,
        stdout,
        stderr,
        move || {
            child
                .wait()
                .map(|status| (status.success(), status.to_string()))
                .map_err(|error| format!("Unable to wait for Codex app-server: {error}"))
        },
        child_id,
        None,
    )
}

pub fn spawn_remote_codex_session(config: RemoteCodexSessionConfig) -> Result<CodexSessionHandle> {
    if config.definition.provider != AgentProvider::Codex {
        bail!("The remote Codex adapter requires the Codex provider");
    }
    if config.definition.backend != AgentBackendKind::Structured {
        bail!("The remote Codex backend is not structured");
    }
    if !matches!(config.definition.location, AgentLocation::SavedHost { .. }) {
        bail!("Remote structured execution requires a saved SSH host");
    }
    let working_directory = config
        .definition
        .working_directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("Choose a remote working directory")?;
    let executable = config
        .definition
        .executable_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("codex");
    let command = build_remote_structured_command(
        &config.definition,
        &["app-server".to_string(), "--stdio".to_string()],
        &config.request.environment,
    )?;
    let protocol_config = CodexSessionConfig {
        executable: PathBuf::from(executable),
        working_directory: PathBuf::from(working_directory),
        permission_policy: config.definition.permission_policy,
        initial_prompt: config.initial_prompt,
    };
    let process = spawn_remote_exec(
        config.request,
        config.known_hosts,
        config.keepalive_secs,
        command,
    )?;
    let RemoteExecProcess {
        stdin,
        stdout,
        stderr,
        exit_rx,
        control,
    } = process;
    let monitor_control = control.clone();
    start_codex_transport(
        protocol_config,
        stdin,
        stdout,
        stderr,
        move || match exit_rx
            .recv()
            .map_err(|_| "Remote Codex exit channel disconnected".to_string())??
        {
            RemoteExecExit::Status(status) => {
                Ok((status == 0, format!("remote exit status {status}")))
            }
            RemoteExecExit::Signal { signal, message } => {
                let detail = if message.trim().is_empty() {
                    signal
                } else {
                    format!("{signal}: {message}")
                };
                Ok((false, format!("remote signal {detail}")))
            }
        },
        Arc::new(AtomicU32::new(0)),
        Some(monitor_control),
    )
}

fn start_codex_transport(
    config: CodexSessionConfig,
    stdin: impl Write + Send + 'static,
    stdout: impl Read + Send + 'static,
    stderr: impl Read + Send + 'static,
    wait_for_exit: impl FnOnce() -> Result<(bool, String), String> + Send + 'static,
    child_id: Arc<AtomicU32>,
    remote_control: Option<RemoteExecControl>,
) -> Result<CodexSessionHandle> {
    let (command_tx, command_rx) = mpsc::sync_channel::<Value>(COMMAND_CHANNEL_CAPACITY);
    let (event_tx, event_rx) = mpsc::sync_channel::<AgentEvent>(EVENT_CHANNEL_CAPACITY);
    let ids = Arc::new(Mutex::new(SessionIds::default()));

    thread::Builder::new()
        .name("termirust-codex-writer".to_string())
        .spawn(move || write_commands(stdin, command_rx))
        .context("Unable to start Codex command writer")?;

    let reader_commands = command_tx.clone();
    let reader_events = event_tx.clone();
    let reader_ids = Arc::clone(&ids);
    let reader_config = config.clone();
    let reader_thread = thread::Builder::new()
        .name("termirust-codex-reader".to_string())
        .spawn(move || {
            read_messages(
                stdout,
                reader_commands,
                reader_events,
                reader_ids,
                reader_config,
            )
        })
        .context("Unable to start Codex event reader")?;

    let stderr_events = event_tx.clone();
    thread::Builder::new()
        .name("termirust-codex-stderr".to_string())
        .spawn(move || capture_stderr(stderr, stderr_events))
        .context("Unable to start Codex diagnostic reader")?;

    let monitor_events = event_tx.clone();
    let monitor_child_id = Arc::clone(&child_id);
    thread::Builder::new()
        .name("termirust-codex-monitor".to_string())
        .spawn(move || {
            let result = wait_for_exit();
            let _ = reader_thread.join();
            monitor_child_id.store(0, Ordering::Release);
            match result {
                Ok((success, description)) => {
                    if !success {
                        let _ = monitor_events.send(AgentEvent::Failed {
                            error: format!("Codex app-server exited with {description}"),
                        });
                    }
                    let _ =
                        monitor_events.send(AgentEvent::StateChanged(AgentRunState::Disconnected));
                }
                Err(error) => {
                    let _ = monitor_events.send(AgentEvent::Failed { error });
                    let _ =
                        monitor_events.send(AgentEvent::StateChanged(AgentRunState::Disconnected));
                }
            }
        })
        .context("Unable to start Codex process monitor")?;

    command_tx
        .send(json!({
            "id": INITIALIZE_REQUEST_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "termirust",
                    "title": "TermiRust",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {"experimentalApi": false}
            }
        }))
        .context("Unable to initialize Codex app-server")?;
    event_tx
        .send(AgentEvent::StateChanged(AgentRunState::Starting))
        .ok();

    Ok(CodexSessionHandle {
        command_tx,
        event_rx,
        ids,
        next_request_id: AtomicU64::new(
            if config
                .initial_prompt
                .as_deref()
                .is_some_and(|prompt| !prompt.trim().is_empty())
            {
                FIRST_USER_REQUEST_ID + 1
            } else {
                FIRST_USER_REQUEST_ID
            },
        ),
        child_id,
        remote_control,
    })
}

fn write_commands(mut stdin: impl Write, command_rx: Receiver<Value>) {
    for command in command_rx {
        if serde_json::to_writer(&mut stdin, &command).is_err()
            || stdin.write_all(b"\n").is_err()
            || stdin.flush().is_err()
        {
            break;
        }
    }
}

fn read_messages(
    stdout: impl std::io::Read,
    command_tx: SyncSender<Value>,
    event_tx: SyncSender<AgentEvent>,
    ids: Arc<Mutex<SessionIds>>,
    config: CodexSessionConfig,
) {
    let result = read_bounded_lines(
        BufReader::new(stdout),
        MAX_EVENT_LINE_BYTES,
        |line| match line {
            BoundedLine::TooLong => emit(
                &event_tx,
                AgentEvent::Diagnostic {
                    message: format!(
                        "Ignored Codex JSONL message larger than {MAX_EVENT_LINE_BYTES} bytes"
                    ),
                },
            ),
            BoundedLine::Bytes(line) if line.iter().all(u8::is_ascii_whitespace) => {}
            BoundedLine::Bytes(line) => {
                let message: Value = match serde_json::from_slice(&line) {
                    Ok(message) => message,
                    Err(error) => {
                        emit(
                            &event_tx,
                            AgentEvent::Diagnostic {
                                message: format!("Ignored malformed Codex JSONL message: {error}"),
                            },
                        );
                        return;
                    }
                };
                handle_message(message, &command_tx, &event_tx, &ids, &config);
            }
        },
    );
    if let Err(error) = result {
        emit(
            &event_tx,
            AgentEvent::Failed {
                error: format!("Unable to read Codex app-server output: {error}"),
            },
        );
    }
}

fn handle_message(
    message: Value,
    command_tx: &SyncSender<Value>,
    event_tx: &SyncSender<AgentEvent>,
    ids: &Arc<Mutex<SessionIds>>,
    config: &CodexSessionConfig,
) {
    if message.get("method").is_some() && message.get("id").is_some() {
        handle_server_request(&message, event_tx);
        return;
    }
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        if let Some(error) = message.get("error") {
            emit(
                event_tx,
                AgentEvent::Failed {
                    error: error_message(error),
                },
            );
            emit(event_tx, AgentEvent::StateChanged(AgentRunState::Failed));
            return;
        }
        if id == INITIALIZE_REQUEST_ID {
            let _ = command_tx.send(json!({"method": "initialized", "params": {}}));
            let _ = command_tx.send(thread_start_request(config));
        } else if id == THREAD_START_REQUEST_ID {
            let thread_id = message
                .pointer("/result/thread/id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let Some(thread_id) = thread_id else {
                emit(
                    event_tx,
                    AgentEvent::Failed {
                        error: "Codex thread/start response did not contain a thread id"
                            .to_string(),
                    },
                );
                return;
            };
            if let Ok(mut current) = ids.lock() {
                current.thread_id = Some(thread_id.clone());
            }
            emit(
                event_tx,
                AgentEvent::SessionReady {
                    provider_session_id: thread_id,
                },
            );
            emit(event_tx, AgentEvent::StateChanged(AgentRunState::Idle));
            if let Some(prompt) = config
                .initial_prompt
                .as_deref()
                .filter(|prompt| !prompt.trim().is_empty())
            {
                let _ = send_turn(command_tx, ids, FIRST_USER_REQUEST_ID, prompt);
            }
        } else if let Some(turn_id) = message.pointer("/result/turn/id").and_then(Value::as_str)
            && let Ok(mut current) = ids.lock()
        {
            current.turn_id = Some(turn_id.to_string());
        }
        return;
    }

    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = message.get("params").unwrap_or(&Value::Null);
    match method {
        "turn/started" => {
            if let Some(turn_id) = params.pointer("/turn/id").and_then(Value::as_str)
                && let Ok(mut current) = ids.lock()
            {
                current.turn_id = Some(turn_id.to_string());
            }
            emit(event_tx, AgentEvent::StateChanged(AgentRunState::Running));
        }
        "item/agentMessage/delta" => {
            if let Some(text) = params.get("delta").and_then(Value::as_str) {
                emit(
                    event_tx,
                    AgentEvent::MessageDelta {
                        role: AgentRole::Assistant,
                        text: text.to_string(),
                    },
                );
            }
        }
        "item/started" => {
            if let Some(call) = tool_call(params) {
                emit(event_tx, AgentEvent::ToolStarted(call));
            }
        }
        "item/completed" => {
            if let Some(call_id) = params.pointer("/item/id").and_then(Value::as_str) {
                let status = params
                    .pointer("/item/status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                emit(
                    event_tx,
                    AgentEvent::ToolFinished {
                        call_id: call_id.to_string(),
                        outcome: if matches!(status, "failed" | "declined") {
                            if status == "declined" {
                                ToolOutcome::Declined
                            } else {
                                ToolOutcome::Failed
                            }
                        } else {
                            ToolOutcome::Succeeded
                        },
                    },
                );
            }
        }
        "turn/completed" => {
            let status = params
                .pointer("/turn/status")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            let state = match status {
                "completed" => AgentRunState::Succeeded,
                "interrupted" | "cancelled" => AgentRunState::Cancelled,
                _ => AgentRunState::Failed,
            };
            emit(event_tx, AgentEvent::StateChanged(state));
            if state == AgentRunState::Succeeded {
                emit(event_tx, AgentEvent::Completed { summary: None });
            }
            if let Ok(mut current) = ids.lock() {
                current.turn_id = None;
            }
        }
        "error" => {
            emit(
                event_tx,
                AgentEvent::Failed {
                    error: params
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex reported an unknown error")
                        .to_string(),
                },
            );
            emit(event_tx, AgentEvent::StateChanged(AgentRunState::Failed));
        }
        _ => {}
    }
}

fn thread_start_request(config: &CodexSessionConfig) -> Value {
    let sandbox = match config.permission_policy {
        AgentPermissionPolicy::ReadOnly => "read-only",
        AgentPermissionPolicy::ProviderDefault | AgentPermissionPolicy::WorkspaceWrite => {
            "workspace-write"
        }
    };
    json!({
        "id": THREAD_START_REQUEST_ID,
        "method": "thread/start",
        "params": {
            "cwd": config.working_directory,
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "sandbox": sandbox
        }
    })
}

fn send_turn(
    command_tx: &SyncSender<Value>,
    ids: &Arc<Mutex<SessionIds>>,
    request_id: u64,
    prompt: &str,
) -> Result<()> {
    let thread_id = ids
        .lock()
        .map_err(|_| anyhow::anyhow!("Codex session state is unavailable"))?
        .thread_id
        .clone()
        .context("Codex session is not ready")?;
    command_tx
        .send(json!({
            "id": request_id,
            "method": "turn/start",
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": prompt, "text_elements": []}]
            }
        }))
        .context("Codex app-server is no longer accepting commands")
}

fn handle_server_request(message: &Value, event_tx: &SyncSender<AgentEvent>) {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let request_id = request_id_string(message.get("id").unwrap_or(&Value::Null));
    let Some(kind) = (match method {
        "item/commandExecution/requestApproval" | "execCommandApproval" => {
            Some(AgentApprovalKind::Command)
        }
        "item/fileChange/requestApproval" | "applyPatchApproval" => {
            Some(AgentApprovalKind::FileChange)
        }
        "item/permissions/requestApproval" => Some(AgentApprovalKind::Permissions),
        _ => None,
    }) else {
        return;
    };
    let params = message.get("params").unwrap_or(&Value::Null);
    let operation = params
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| params.get("grantRoot").and_then(Value::as_str))
        .unwrap_or(method)
        .to_string();
    emit(
        event_tx,
        AgentEvent::StateChanged(AgentRunState::WaitingForApproval),
    );
    emit(
        event_tx,
        AgentEvent::ApprovalRequested(AgentApprovalRequest {
            request_id,
            kind,
            operation,
            working_directory: params
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string),
            reason: params
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
    );
}

fn tool_call(params: &Value) -> Option<NormalizedToolCall> {
    let item = params.get("item")?;
    let call_id = item.get("id")?.as_str()?.to_string();
    let name = item.get("type")?.as_str()?.to_string();
    if matches!(name.as_str(), "agentMessage" | "reasoning" | "userMessage") {
        return None;
    }
    let summary = item
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| item.get("path").and_then(Value::as_str))
        .map(str::to_string);
    Some(NormalizedToolCall {
        call_id,
        name,
        summary,
    })
}

fn capture_stderr(mut stderr: impl Read, event_tx: SyncSender<AgentEvent>) {
    let mut bytes = Vec::new();
    let _ = stderr
        .by_ref()
        .take(MAX_STDERR_BYTES as u64)
        .read_to_end(&mut bytes);
    let message = String::from_utf8_lossy(&bytes).trim().to_string();
    if !message.is_empty() {
        emit(&event_tx, AgentEvent::Diagnostic { message });
    }
}

fn request_id_string(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .or_else(|| id.as_u64().map(|value| value.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_request_id(id: &str) -> Result<Value> {
    if let Ok(value) = id.parse::<u64>() {
        Ok(Value::from(value))
    } else if !id.trim().is_empty() && id != "unknown" {
        Ok(Value::from(id))
    } else {
        bail!("Approval request id is invalid")
    }
}

fn error_message(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex app-server request failed")
        .to_string()
}

fn emit(event_tx: &SyncSender<AgentEvent>, event: AgentEvent) {
    let _ = event_tx.send(event);
}

#[cfg(test)]
mod tests {
    use super::{
        CodexSessionConfig, MAX_STDERR_BYTES, RemoteCodexSessionConfig, capture_stderr,
        spawn_codex_session, spawn_remote_codex_session,
    };
    use crate::agents::protocol::{AgentApprovalKind, AgentEvent, AgentRole, AgentRunState};
    use crate::models::{
        AgentBackendKind, AgentLocation, AgentPermissionPolicy, AgentProvider, AuthConfig,
        ConnectRequest, ConnectionKind, SavedAgentDefinition, SavedWorktreePolicy,
    };
    use crate::storage::KnownHostStore;
    use crate::test_support::{DockerSshServer, TestIsolation};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn stderr_capture_enforces_its_byte_limit_without_newlines() {
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(1);
        capture_stderr(
            std::io::Cursor::new(vec![b'x'; MAX_STDERR_BYTES * 4]),
            event_tx,
        );
        let AgentEvent::Diagnostic { message } = event_rx.recv().unwrap() else {
            panic!("expected bounded diagnostic event");
        };
        assert_eq!(message.len(), MAX_STDERR_BYTES);
    }

    #[test]
    fn user_commands_do_not_block_when_the_writer_queue_is_full() {
        let (command_tx, _command_rx) = std::sync::mpsc::sync_channel(1);
        command_tx.send(json!({"fill": true})).unwrap();
        let (_event_tx, event_rx) = std::sync::mpsc::sync_channel(1);
        let session = super::CodexSessionHandle {
            command_tx,
            event_rx,
            ids: std::sync::Arc::new(std::sync::Mutex::new(super::SessionIds::default())),
            next_request_id: std::sync::atomic::AtomicU64::new(1),
            child_id: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            remote_control: None,
        };
        assert!(
            session
                .send_wire(json!({"next": true}))
                .unwrap_err()
                .to_string()
                .contains("queue is full")
        );
    }

    #[cfg(unix)]
    fn fake_codex() -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("termirust-codex-server-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake codex");
        fs::write(
            &executable,
            r#"#!/bin/sh
read initialize
printf '%s\n' '{"id":1,"result":{"userAgent":"fake"}}'
read initialized
read thread_start
printf '%s\n' '{"id":2,"result":{"thread":{"id":"thread-1"},"model":"fake","modelProvider":"fake","cwd":"/tmp","approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite","writableRoots":[],"readOnlyAccess":{"type":"fullAccess"},"networkAccess":false,"excludeTmpdirEnvVar":false,"excludeSlashTmp":false}}}'
read turn_start
printf '%s\n' '{"id":100,"result":{"turn":{"id":"turn-1","items":[],"status":"inProgress","error":null}}}'
printf '%s\n' '{"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1","items":[],"status":"inProgress","error":null}}}'
printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","delta":"done"}}'
printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","items":[],"status":"completed","error":null}}}'
sleep 1
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        (executable, directory)
    }

    #[test]
    #[cfg(unix)]
    fn performs_handshake_and_normalizes_turn_events() {
        let (executable, working_directory) = fake_codex();
        let session = spawn_codex_session(CodexSessionConfig {
            executable,
            working_directory,
            permission_policy: AgentPermissionPolicy::WorkspaceWrite,
            initial_prompt: Some("finish the task".to_string()),
        })
        .unwrap();
        let mut events = Vec::new();
        while events.len() < 7 {
            events.push(
                session
                    .event_rx
                    .recv_timeout(Duration::from_secs(3))
                    .expect("expected Codex event"),
            );
        }
        assert!(events.contains(&AgentEvent::SessionReady {
            provider_session_id: "thread-1".to_string(),
        }));
        assert!(events.contains(&AgentEvent::StateChanged(AgentRunState::Running)));
        assert!(events.contains(&AgentEvent::MessageDelta {
            role: AgentRole::Assistant,
            text: "done".to_string(),
        }));
        assert!(events.contains(&AgentEvent::StateChanged(AgentRunState::Succeeded)));
    }

    #[test]
    fn docker_remote_codex_preserves_bidirectional_app_server_protocol() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping remote Codex e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let executable = "/usr/local/bin/termirust-fake-codex";
        server
            .exec(&format!(
                "cat > {executable} <<'TERMIRUST_EOF'\n#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'fake-codex 1.0\\n'; exit 0; fi\nread initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"fake\"}}}}'\nread initialized\nread thread_start\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"remote-thread\"}},\"model\":\"fake\",\"modelProvider\":\"fake\",\"cwd\":\"/home/termirust\",\"approvalPolicy\":\"on-request\",\"approvalsReviewer\":\"user\",\"sandbox\":{{\"type\":\"readOnly\",\"networkAccess\":false}}}}}}'\nread turn_start\nprintf '%s\\n' '{{\"id\":100,\"result\":{{\"turn\":{{\"id\":\"remote-turn\",\"items\":[],\"status\":\"inProgress\",\"error\":null}}}}}}'\nprintf '%s\\n' '{{\"method\":\"turn/started\",\"params\":{{\"threadId\":\"remote-thread\",\"turn\":{{\"id\":\"remote-turn\",\"items\":[],\"status\":\"inProgress\",\"error\":null}}}}}}'\nprintf '%s\\n' '{{\"method\":\"item/agentMessage/delta\",\"params\":{{\"threadId\":\"remote-thread\",\"turnId\":\"remote-turn\",\"itemId\":\"item-1\",\"delta\":\"remote-codex-ok\"}}}}'\nprintf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"remote-thread\",\"turn\":{{\"id\":\"remote-turn\",\"items\":[],\"status\":\"completed\",\"error\":null}}}}}}'\nTERMIRUST_EOF\nchmod 755 {executable}"
            ))
            .expect("unable to install fake remote Codex");
        let request = ConnectRequest {
            session_id: 702,
            title: "Remote Codex".to_string(),
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
        };
        let definition = SavedAgentDefinition {
            provider: AgentProvider::Codex,
            backend: AgentBackendKind::Structured,
            location: AgentLocation::SavedHost {
                profile_id: "docker".to_string(),
            },
            working_directory: Some("/home/termirust".to_string()),
            executable_override: Some(executable.to_string()),
            permission_policy: AgentPermissionPolicy::ReadOnly,
            worktree: SavedWorktreePolicy::ReadOnly,
            ..SavedAgentDefinition::default()
        };
        let session = spawn_remote_codex_session(RemoteCodexSessionConfig {
            definition,
            request,
            known_hosts: std::sync::Arc::new(KnownHostStore::load().unwrap()),
            keepalive_secs: 0,
            initial_prompt: Some("review remotely".to_string()),
        })
        .expect("unable to launch remote Codex");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut saw_ready = false;
        let mut saw_message = false;
        let mut saw_succeeded = false;
        while std::time::Instant::now() < deadline && !saw_succeeded {
            match session.event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(AgentEvent::SessionReady {
                    provider_session_id,
                }) => saw_ready = provider_session_id == "remote-thread",
                Ok(AgentEvent::MessageDelta { text, .. }) => {
                    saw_message |= text.contains("remote-codex-ok")
                }
                Ok(AgentEvent::StateChanged(AgentRunState::Succeeded)) => saw_succeeded = true,
                Ok(AgentEvent::Failed { error }) => panic!("remote Codex failed: {error}"),
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("remote Codex event stream closed: {error}"),
            }
        }
        assert!(saw_ready);
        assert!(saw_message);
        assert!(saw_succeeded);
    }

    #[test]
    #[cfg(unix)]
    fn clears_the_process_id_after_the_app_server_exits() {
        let (executable, working_directory) = fake_codex();
        let session = spawn_codex_session(CodexSessionConfig {
            executable,
            working_directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            initial_prompt: Some("finish the task".to_string()),
        })
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while session.child_id.load(std::sync::atomic::Ordering::Acquire) != 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            session.child_id.load(std::sync::atomic::Ordering::Acquire),
            0,
            "process monitor must clear stale child ids before handle drop"
        );
    }

    #[test]
    #[cfg(unix)]
    fn app_server_exit_is_reported_after_buffered_turn_events() {
        let (executable, working_directory) = fake_codex();
        let session = spawn_codex_session(CodexSessionConfig {
            executable,
            working_directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            initial_prompt: Some("finish the task".to_string()),
        })
        .unwrap();
        let mut states = Vec::new();
        loop {
            if let AgentEvent::StateChanged(state) = session
                .event_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("expected Codex lifecycle event")
            {
                states.push(state);
                if state == AgentRunState::Disconnected {
                    break;
                }
            }
        }
        assert_eq!(states.last(), Some(&AgentRunState::Disconnected));
        assert!(states.contains(&AgentRunState::Succeeded));
    }

    #[test]
    #[cfg(unix)]
    fn dropping_a_session_stops_an_active_app_server() {
        let (executable, working_directory) = fake_codex();
        let session = spawn_codex_session(CodexSessionConfig {
            executable,
            working_directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            initial_prompt: None,
        })
        .unwrap();
        let child_id = session.child_id.load(std::sync::atomic::Ordering::Acquire);
        assert_ne!(child_id, 0);

        drop(session);

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while process_exists(child_id) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !process_exists(child_id),
            "Codex child process remained alive"
        );
    }

    #[test]
    #[cfg(unix)]
    fn surfaces_and_responds_to_server_approval_requests() {
        use std::os::unix::fs::PermissionsExt as _;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("termirust-codex-approval-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake codex approval");
        fs::write(
            &executable,
            r#"#!/bin/sh
read initialize
printf '%s\n' '{"id":1,"result":{"userAgent":"fake"}}'
read initialized
read thread_start
printf '%s\n' '{"id":2,"result":{"thread":{"id":"thread-approval"}}}'
printf '%s\n' '{"id":77,"method":"item/commandExecution/requestApproval","params":{"command":"cargo test","cwd":"/tmp/project","reason":"run tests"}}'
read approval
printf '%s\n' "$approval" > "$0.response"
sleep 1
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let response_path = PathBuf::from(format!("{}.response", executable.display()));
        let session = spawn_codex_session(CodexSessionConfig {
            executable,
            working_directory: directory,
            permission_policy: AgentPermissionPolicy::WorkspaceWrite,
            initial_prompt: None,
        })
        .unwrap();
        let approval = loop {
            match session
                .event_rx
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
            {
                AgentEvent::ApprovalRequested(approval) => break approval,
                _ => continue,
            }
        };
        assert_eq!(approval.request_id, "77");
        assert_eq!(approval.kind, AgentApprovalKind::Command);
        assert_eq!(approval.operation, "cargo test");
        assert_eq!(approval.working_directory.as_deref(), Some("/tmp/project"));
        assert_eq!(approval.reason.as_deref(), Some("run tests"));

        session
            .respond_to_approval(&approval.request_id, false)
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !response_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let response: Value = serde_json::from_slice(&fs::read(response_path).unwrap()).unwrap();
        assert_eq!(
            response,
            json!({"id": 77, "result": {"decision": "decline"}})
        );
    }

    #[cfg(unix)]
    fn process_exists(process_id: u32) -> bool {
        let result = unsafe { libc::kill(process_id as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[test]
    #[ignore = "requires TERMIRUST_RUN_LIVE_AGENT_TESTS=1, an authenticated Codex CLI, and network access"]
    fn live_codex_app_server_smoke() {
        if std::env::var("TERMIRUST_RUN_LIVE_AGENT_TESTS").as_deref() != Ok("1") {
            eprintln!("skipping live Codex smoke; set TERMIRUST_RUN_LIVE_AGENT_TESTS=1");
            return;
        }
        let working_directory = std::env::temp_dir().join("termirust-live-codex-smoke");
        fs::create_dir_all(&working_directory).unwrap();
        let session = spawn_codex_session(CodexSessionConfig {
            executable: PathBuf::from("codex"),
            working_directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            initial_prompt: Some(
                "Reply with exactly TERMIRUST_CODEX_LIVE_OK. Do not use tools.".to_string(),
            ),
        })
        .expect("launch live Codex app-server");
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let mut response = String::new();
        let mut succeeded = false;
        while std::time::Instant::now() < deadline {
            match session.event_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(AgentEvent::MessageDelta { text, .. }) => response.push_str(&text),
                Ok(AgentEvent::StateChanged(AgentRunState::Succeeded))
                | Ok(AgentEvent::Completed { .. }) => succeeded = true,
                Ok(AgentEvent::Failed { error }) => panic!("live Codex failed: {error}"),
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if succeeded => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => panic!("live Codex event channel failed: {error}"),
            }
            if succeeded && response.contains("TERMIRUST_CODEX_LIVE_OK") {
                break;
            }
        }
        assert!(succeeded, "Codex did not report a successful turn");
        assert!(
            response.contains("TERMIRUST_CODEX_LIVE_OK"),
            "Codex response did not contain the expected marker: {response:?}"
        );
    }
}
