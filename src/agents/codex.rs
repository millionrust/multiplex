use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::agents::protocol::{
    AgentApprovalKind, AgentApprovalRequest, AgentEvent, AgentRole, AgentRunState,
    NormalizedToolCall, ToolOutcome,
};
use crate::models::AgentPermissionPolicy;

const INITIALIZE_REQUEST_ID: u64 = 1;
const THREAD_START_REQUEST_ID: u64 = 2;
const FIRST_USER_REQUEST_ID: u64 = 100;
const EVENT_CHANNEL_CAPACITY: usize = 512;
const COMMAND_CHANNEL_CAPACITY: usize = 64;
const MAX_STDERR_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct CodexSessionConfig {
    pub executable: PathBuf,
    pub working_directory: PathBuf,
    pub permission_policy: AgentPermissionPolicy,
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
    child_id: u32,
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
            .send(value)
            .context("Codex app-server is no longer accepting commands")
    }
}

impl Drop for CodexSessionHandle {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(self.child_id as libc::pid_t, libc::SIGTERM);
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
    let child_id = child.id();
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
    thread::Builder::new()
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
    thread::Builder::new()
        .name("termirust-codex-monitor".to_string())
        .spawn(move || match child.wait() {
            Ok(status) if !status.success() => {
                let _ = monitor_events.send(AgentEvent::Failed {
                    error: format!("Codex app-server exited with {status}"),
                });
                let _ = monitor_events.send(AgentEvent::StateChanged(AgentRunState::Disconnected));
            }
            Err(error) => {
                let _ = monitor_events.send(AgentEvent::Failed {
                    error: format!("Unable to wait for Codex app-server: {error}"),
                });
            }
            _ => {}
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
    let mut pending_methods = HashMap::<String, String>::new();
    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                emit(
                    &event_tx,
                    AgentEvent::Failed {
                        error: format!("Unable to read Codex app-server output: {error}"),
                    },
                );
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                emit(
                    &event_tx,
                    AgentEvent::Diagnostic {
                        message: format!("Ignored malformed Codex JSONL message: {error}"),
                    },
                );
                continue;
            }
        };
        handle_message(
            message,
            &command_tx,
            &event_tx,
            &ids,
            &config,
            &mut pending_methods,
        );
    }
}

fn handle_message(
    message: Value,
    command_tx: &SyncSender<Value>,
    event_tx: &SyncSender<AgentEvent>,
    ids: &Arc<Mutex<SessionIds>>,
    config: &CodexSessionConfig,
    pending_methods: &mut HashMap<String, String>,
) {
    if message.get("method").is_some() && message.get("id").is_some() {
        handle_server_request(&message, event_tx, pending_methods);
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
        } else if let Some(turn_id) = message.pointer("/result/turn/id").and_then(Value::as_str) {
            if let Ok(mut current) = ids.lock() {
                current.turn_id = Some(turn_id.to_string());
            }
        }
        return;
    }

    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = message.get("params").unwrap_or(&Value::Null);
    match method {
        "turn/started" => {
            if let Some(turn_id) = params.pointer("/turn/id").and_then(Value::as_str) {
                if let Ok(mut current) = ids.lock() {
                    current.turn_id = Some(turn_id.to_string());
                }
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

fn handle_server_request(
    message: &Value,
    event_tx: &SyncSender<AgentEvent>,
    pending_methods: &mut HashMap<String, String>,
) {
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
    pending_methods.insert(request_id.clone(), method.to_string());
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

fn capture_stderr(stderr: impl std::io::Read, event_tx: SyncSender<AgentEvent>) {
    let mut captured = String::new();
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if captured.len() >= MAX_STDERR_BYTES {
            break;
        }
        if !captured.is_empty() {
            captured.push('\n');
        }
        captured.push_str(&line);
    }
    if !captured.trim().is_empty() {
        emit(&event_tx, AgentEvent::Diagnostic { message: captured });
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
    use super::{CodexSessionConfig, spawn_codex_session};
    use crate::agents::{AgentEvent, AgentRole, AgentRunState};
    use crate::models::AgentPermissionPolicy;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
}
