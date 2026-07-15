use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::agents::process::build_remote_structured_command;
use crate::agents::protocol::{
    AgentEvent, AgentRole, AgentRunState, NormalizedToolCall, ToolOutcome,
};
use crate::agents::stream::{BoundedLine, read_bounded_lines};
use crate::models::{
    AgentBackendKind, AgentLocation, AgentPermissionPolicy, AgentProvider, ConnectRequest,
    SavedAgentDefinition,
};
use crate::ssh::{RemoteExecControl, RemoteExecExit, RemoteExecProcess, spawn_remote_exec};
use crate::storage::KnownHostStore;

const EVENT_CHANNEL_CAPACITY: usize = 512;
const MAX_STDERR_BYTES: usize = 16 * 1024;
const MAX_EVENT_LINE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct HeadlessSessionConfig {
    pub provider: AgentProvider,
    pub executable: PathBuf,
    pub working_directory: PathBuf,
    pub permission_policy: AgentPermissionPolicy,
    pub arguments: Vec<String>,
    pub initial_prompt: Option<String>,
}

pub struct HeadlessSessionHandle {
    config: HeadlessSessionConfig,
    pub event_rx: Receiver<AgentEvent>,
    event_tx: SyncSender<AgentEvent>,
    running: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    child_id: Arc<AtomicU32>,
}

#[derive(Clone)]
pub struct RemoteHeadlessSessionConfig {
    pub definition: SavedAgentDefinition,
    pub request: ConnectRequest,
    pub known_hosts: Arc<KnownHostStore>,
    pub keepalive_secs: u16,
    pub initial_prompt: Option<String>,
}

pub struct RemoteHeadlessSessionHandle {
    config: RemoteHeadlessSessionConfig,
    pub event_rx: Receiver<AgentEvent>,
    event_tx: SyncSender<AgentEvent>,
    running: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    control: Arc<Mutex<Option<RemoteExecControl>>>,
}

impl RemoteHeadlessSessionHandle {
    pub fn send_prompt(&self, prompt: impl Into<String>) -> Result<()> {
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            bail!("Agent prompt cannot be empty");
        }
        if self.running.swap(true, Ordering::AcqRel) {
            bail!("The structured agent already has a running job");
        }
        self.cancel_requested.store(false, Ordering::Release);
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let running = Arc::clone(&self.running);
        let cancel_requested = Arc::clone(&self.cancel_requested);
        let control = Arc::clone(&self.control);
        let spawn_result = thread::Builder::new()
            .name(format!(
                "termirust-remote-{}-structured",
                config
                    .definition
                    .provider
                    .label()
                    .to_ascii_lowercase()
                    .replace(' ', "-")
            ))
            .spawn(move || {
                run_remote_job(config, prompt, event_tx, running, cancel_requested, control)
            });
        if let Err(error) = spawn_result {
            self.running.store(false, Ordering::Release);
            return Err(error).context("Unable to start remote structured agent job");
        }
        Ok(())
    }

    pub fn cancel(&self) -> Result<()> {
        if !self.running.load(Ordering::Acquire) {
            bail!("The structured agent has no active job");
        }
        self.cancel_requested.store(true, Ordering::Release);
        if let Some(control) = self
            .control
            .lock()
            .map_err(|_| anyhow::anyhow!("Remote process control is unavailable"))?
            .as_ref()
        {
            control.terminate()?;
        }
        let _ = self
            .event_tx
            .try_send(AgentEvent::StateChanged(AgentRunState::Cancelled));
        Ok(())
    }
}

impl Drop for RemoteHeadlessSessionHandle {
    fn drop(&mut self) {
        if let Ok(control) = self.control.lock()
            && let Some(control) = control.as_ref()
        {
            let _ = control.terminate();
        }
    }
}

impl HeadlessSessionHandle {
    pub fn send_prompt(&self, prompt: impl Into<String>) -> Result<()> {
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            bail!("Agent prompt cannot be empty");
        }
        if self.running.swap(true, Ordering::AcqRel) {
            bail!("The structured agent already has a running job");
        }
        self.cancel_requested.store(false, Ordering::Release);
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let running = Arc::clone(&self.running);
        let cancel_requested = Arc::clone(&self.cancel_requested);
        let child_id = Arc::clone(&self.child_id);
        let spawn_result = thread::Builder::new()
            .name(format!(
                "termirust-{}-structured",
                config
                    .provider
                    .label()
                    .to_ascii_lowercase()
                    .replace(' ', "-")
            ))
            .spawn(move || {
                run_job(
                    config,
                    prompt,
                    event_tx,
                    running,
                    cancel_requested,
                    child_id,
                )
            });
        if let Err(error) = spawn_result {
            self.running.store(false, Ordering::Release);
            return Err(error).context("Unable to start structured agent job");
        }
        Ok(())
    }

    pub fn cancel(&self) -> Result<()> {
        if !self.running.load(Ordering::Acquire) {
            bail!("The structured agent has no active job");
        }
        self.cancel_requested.store(true, Ordering::Release);
        let child_id = self.child_id.load(Ordering::Acquire);
        if child_id != 0 {
            #[cfg(unix)]
            unsafe {
                if libc::kill(child_id as libc::pid_t, libc::SIGTERM) != 0
                    && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                {
                    self.cancel_requested.store(false, Ordering::Release);
                    bail!("Unable to interrupt the structured agent process");
                }
            }
        }
        let _ = self
            .event_tx
            .try_send(AgentEvent::StateChanged(AgentRunState::Cancelled));
        Ok(())
    }
}

impl Drop for HeadlessSessionHandle {
    fn drop(&mut self) {
        let child_id = self.child_id.load(Ordering::Acquire);
        if child_id != 0 {
            #[cfg(unix)]
            unsafe {
                libc::kill(child_id as libc::pid_t, libc::SIGTERM);
            }
        }
    }
}

pub fn spawn_headless_session(config: HeadlessSessionConfig) -> Result<HeadlessSessionHandle> {
    if !matches!(
        config.provider,
        AgentProvider::ClaudeCode | AgentProvider::Gemini
    ) {
        bail!("This provider does not support the headless JSON adapter");
    }
    if !config.working_directory.is_dir() {
        bail!(
            "Agent working directory does not exist: {}",
            config.working_directory.display()
        );
    }
    validate_arguments(config.provider, &config.arguments)?;
    let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CHANNEL_CAPACITY);
    let initial_prompt = config.initial_prompt.clone();
    let handle = HeadlessSessionHandle {
        config,
        event_rx,
        event_tx,
        running: Arc::new(AtomicBool::new(false)),
        cancel_requested: Arc::new(AtomicBool::new(false)),
        child_id: Arc::new(AtomicU32::new(0)),
    };
    handle
        .event_tx
        .send(AgentEvent::StateChanged(AgentRunState::Idle))
        .ok();
    if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.trim().is_empty()) {
        handle.send_prompt(prompt)?;
    }
    Ok(handle)
}

pub fn spawn_remote_headless_session(
    config: RemoteHeadlessSessionConfig,
) -> Result<RemoteHeadlessSessionHandle> {
    if !matches!(
        config.definition.provider,
        AgentProvider::ClaudeCode | AgentProvider::Gemini
    ) {
        bail!("This provider does not support the headless JSON adapter");
    }
    if config.definition.backend != AgentBackendKind::Structured {
        bail!("The remote agent backend is not structured");
    }
    if !matches!(config.definition.location, AgentLocation::SavedHost { .. }) {
        bail!("Remote structured execution requires a saved SSH host");
    }
    validate_arguments(config.definition.provider, &config.definition.arguments)?;
    let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CHANNEL_CAPACITY);
    let initial_prompt = config.initial_prompt.clone();
    let handle = RemoteHeadlessSessionHandle {
        config,
        event_rx,
        event_tx,
        running: Arc::new(AtomicBool::new(false)),
        cancel_requested: Arc::new(AtomicBool::new(false)),
        control: Arc::new(Mutex::new(None)),
    };
    handle
        .event_tx
        .send(AgentEvent::StateChanged(AgentRunState::Idle))
        .ok();
    if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.trim().is_empty()) {
        handle.send_prompt(prompt)?;
    }
    Ok(handle)
}

fn run_job(
    config: HeadlessSessionConfig,
    prompt: String,
    event_tx: SyncSender<AgentEvent>,
    running: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    child_id: Arc<AtomicU32>,
) {
    if cancel_requested.load(Ordering::Acquire) {
        let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Cancelled));
        running.store(false, Ordering::Release);
        return;
    }
    let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Starting));
    let arguments = command_arguments(&config, &prompt);
    let mut child = match Command::new(&config.executable)
        .args(arguments)
        .current_dir(&config.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!("Unable to launch {}: {error}", config.provider.label()),
            });
            running.store(false, Ordering::Release);
            return;
        }
    };
    child_id.store(child.id(), Ordering::Release);
    if cancel_requested.load(Ordering::Acquire) {
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(stderr) = stderr {
        let diagnostics = event_tx.clone();
        thread::spawn(move || capture_stderr(stderr, diagnostics));
    }
    let saw_terminal_event = stdout
        .map(|stdout| read_structured_events(config.provider, stdout, &event_tx))
        .unwrap_or(false);
    let status = child.wait();
    child_id.store(0, Ordering::Release);
    running.store(false, Ordering::Release);
    if cancel_requested.load(Ordering::Acquire) {
        let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Cancelled));
        return;
    }
    if saw_terminal_event {
        return;
    }
    match status {
        Ok(status) if status.success() => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!(
                    "{} exited without a structured completion event",
                    config.provider.label()
                ),
            });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Failed));
        }
        Ok(status) => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!("{} exited with {status}", config.provider.label()),
            });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Failed));
        }
        Err(error) => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!("Unable to wait for {}: {error}", config.provider.label()),
            });
        }
    }
}

fn run_remote_job(
    config: RemoteHeadlessSessionConfig,
    prompt: String,
    event_tx: SyncSender<AgentEvent>,
    running: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    active_control: Arc<Mutex<Option<RemoteExecControl>>>,
) {
    if cancel_requested.load(Ordering::Acquire) {
        let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Cancelled));
        running.store(false, Ordering::Release);
        return;
    }
    let provider = config.definition.provider;
    let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Starting));
    let arguments = command_arguments_for(
        provider,
        config.definition.permission_policy,
        &config.definition.arguments,
        &prompt,
    );
    let command = match build_remote_structured_command(
        &config.definition,
        &arguments,
        &config.request.environment,
    ) {
        Ok(command) => command,
        Err(error) => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: error.to_string(),
            });
            running.store(false, Ordering::Release);
            return;
        }
    };
    let process = match spawn_remote_exec(
        config.request,
        config.known_hosts,
        config.keepalive_secs,
        command,
    ) {
        Ok(process) => process,
        Err(error) => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!("Unable to launch remote {}: {error:#}", provider.label()),
            });
            running.store(false, Ordering::Release);
            return;
        }
    };
    let RemoteExecProcess {
        stdout,
        stderr,
        exit_rx,
        control,
        ..
    } = process;
    if let Ok(mut active) = active_control.lock() {
        *active = Some(control.clone());
    }
    if cancel_requested.load(Ordering::Acquire) {
        let _ = control.terminate();
    }
    let diagnostics = event_tx.clone();
    thread::spawn(move || capture_stderr(stderr, diagnostics));
    let saw_terminal_event = read_structured_events(provider, stdout, &event_tx);
    let exit = exit_rx
        .recv()
        .unwrap_or_else(|_| Err("Remote process exit channel disconnected".to_string()));
    if let Ok(mut active) = active_control.lock() {
        *active = None;
    }
    running.store(false, Ordering::Release);
    if cancel_requested.load(Ordering::Acquire) {
        let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Cancelled));
        return;
    }
    if saw_terminal_event {
        return;
    }
    match exit {
        Ok(RemoteExecExit::Status(0)) => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!(
                    "Remote {} exited without a structured completion event",
                    provider.label()
                ),
            });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Failed));
        }
        Ok(RemoteExecExit::Status(status)) => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!("Remote {} exited with status {status}", provider.label()),
            });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Failed));
        }
        Ok(RemoteExecExit::Signal { signal, message }) => {
            let detail = if message.trim().is_empty() {
                signal
            } else {
                format!("{signal}: {message}")
            };
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!("Remote {} was terminated by {detail}", provider.label()),
            });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Failed));
        }
        Err(error) => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: format!("Remote {} transport failed: {error}", provider.label()),
            });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Disconnected));
        }
    }
}

fn read_structured_events(
    provider: AgentProvider,
    stdout: impl Read,
    event_tx: &SyncSender<AgentEvent>,
) -> bool {
    let mut saw_terminal_event = false;
    let read_result =
        read_bounded_lines(
            BufReader::new(stdout),
            MAX_EVENT_LINE_BYTES,
            |line| match line {
                BoundedLine::TooLong => {
                    let _ = event_tx.send(AgentEvent::Diagnostic {
                        message: format!(
                            "Ignored structured event larger than {MAX_EVENT_LINE_BYTES} bytes"
                        ),
                    });
                }
                BoundedLine::Bytes(line) if line.iter().all(u8::is_ascii_whitespace) => {}
                BoundedLine::Bytes(line) => match serde_json::from_slice::<Value>(&line) {
                    Ok(value) => saw_terminal_event |= normalize_event(provider, &value, event_tx),
                    Err(error) => {
                        let _ = event_tx.send(AgentEvent::Diagnostic {
                            message: format!("Ignored malformed structured event: {error}"),
                        });
                    }
                },
            },
        );
    if let Err(error) = read_result {
        let _ = event_tx.send(AgentEvent::Failed {
            error: format!("Unable to read structured output: {error}"),
        });
    }
    saw_terminal_event
}

fn command_arguments(config: &HeadlessSessionConfig, prompt: &str) -> Vec<String> {
    command_arguments_for(
        config.provider,
        config.permission_policy,
        &config.arguments,
        prompt,
    )
}

fn command_arguments_for(
    provider: AgentProvider,
    permission_policy: AgentPermissionPolicy,
    custom_arguments: &[String],
    prompt: &str,
) -> Vec<String> {
    let mut arguments = match provider {
        AgentProvider::ClaudeCode => vec![
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ],
        AgentProvider::Gemini => vec![
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ],
        _ => Vec::new(),
    };
    match (provider, permission_policy) {
        (AgentProvider::ClaudeCode, AgentPermissionPolicy::ReadOnly) => {
            arguments.extend(["--permission-mode".to_string(), "plan".to_string()]);
        }
        (AgentProvider::Gemini, AgentPermissionPolicy::ReadOnly) => {
            arguments.extend(["--approval-mode".to_string(), "plan".to_string()]);
        }
        _ => {}
    }
    arguments.extend(custom_arguments.iter().cloned());
    arguments
}

fn normalize_event(
    provider: AgentProvider,
    value: &Value,
    event_tx: &SyncSender<AgentEvent>,
) -> bool {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match provider {
        AgentProvider::ClaudeCode => normalize_claude(kind, value, event_tx),
        AgentProvider::Gemini => normalize_gemini(kind, value, event_tx),
        _ => false,
    }
}

fn normalize_claude(kind: &str, value: &Value, event_tx: &SyncSender<AgentEvent>) -> bool {
    match kind {
        "system" if value.get("subtype").and_then(Value::as_str) == Some("init") => {
            if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
                let _ = event_tx.send(AgentEvent::SessionReady {
                    provider_session_id: session_id.to_string(),
                });
            }
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Running));
            false
        }
        "assistant" => {
            if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
                normalize_content_blocks(content, event_tx);
            }
            false
        }
        "user" => {
            if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
                for block in content {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result")
                        && let Some(call_id) = block.get("tool_use_id").and_then(Value::as_str)
                    {
                        let _ = event_tx.send(AgentEvent::ToolFinished {
                            call_id: call_id.to_string(),
                            outcome: if block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                            {
                                ToolOutcome::Failed
                            } else {
                                ToolOutcome::Succeeded
                            },
                        });
                    }
                }
            }
            false
        }
        "result" => {
            let failed = value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if failed {
                let _ = event_tx.send(AgentEvent::Failed {
                    error: value
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or("Claude job failed")
                        .to_string(),
                });
                let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Failed));
            } else {
                let summary = value
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let _ = event_tx.send(AgentEvent::Completed { summary });
                let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Succeeded));
            }
            true
        }
        _ => false,
    }
}

fn normalize_gemini(kind: &str, value: &Value, event_tx: &SyncSender<AgentEvent>) -> bool {
    match kind {
        "init" => {
            if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
                let _ = event_tx.send(AgentEvent::SessionReady {
                    provider_session_id: session_id.to_string(),
                });
            }
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Running));
            false
        }
        "message" if value.get("role").and_then(Value::as_str) == Some("assistant") => {
            if let Some(text) = value.get("content").and_then(Value::as_str) {
                let _ = event_tx.send(AgentEvent::MessageDelta {
                    role: AgentRole::Assistant,
                    text: text.to_string(),
                });
            }
            false
        }
        "tool_use" => {
            let call_id = value
                .get("tool_id")
                .and_then(Value::as_str)
                .unwrap_or("gemini-tool")
                .to_string();
            let name = value
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let _ = event_tx.send(AgentEvent::ToolStarted(NormalizedToolCall {
                call_id,
                name,
                summary: value.get("parameters").map(Value::to_string),
            }));
            false
        }
        "tool_result" => {
            if let Some(call_id) = value.get("tool_id").and_then(Value::as_str) {
                let _ = event_tx.send(AgentEvent::ToolFinished {
                    call_id: call_id.to_string(),
                    outcome: if value.get("status").and_then(Value::as_str) == Some("error") {
                        ToolOutcome::Failed
                    } else {
                        ToolOutcome::Succeeded
                    },
                });
            }
            false
        }
        "error" => {
            let _ = event_tx.send(AgentEvent::Failed {
                error: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Gemini job failed")
                    .to_string(),
            });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Failed));
            true
        }
        "result" => {
            let _ = event_tx.send(AgentEvent::Completed { summary: None });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Succeeded));
            true
        }
        _ => false,
    }
}

fn normalize_content_blocks(content: &[Value], event_tx: &SyncSender<AgentEvent>) {
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let _ = event_tx.send(AgentEvent::MessageDelta {
                        role: AgentRole::Assistant,
                        text: text.to_string(),
                    });
                }
            }
            Some("tool_use") => {
                let _ = event_tx.send(AgentEvent::ToolStarted(NormalizedToolCall {
                    call_id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("claude-tool")
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string(),
                    summary: block.get("input").map(Value::to_string),
                }));
            }
            _ => {}
        }
    }
}

fn validate_arguments(provider: AgentProvider, arguments: &[String]) -> Result<()> {
    let forbidden: &[&str] = match provider {
        AgentProvider::ClaudeCode => &["--dangerously-skip-permissions"],
        AgentProvider::Gemini => &["--yolo"],
        _ => &[],
    };
    if let Some(argument) = arguments.iter().find(|argument| {
        let lowered = argument.to_ascii_lowercase();
        forbidden.iter().any(|item| lowered.contains(item))
    }) {
        bail!("Unsafe permission-bypass argument is not allowed: {argument}");
    }
    Ok(())
}

fn capture_stderr(mut stderr: impl Read, event_tx: SyncSender<AgentEvent>) {
    let mut bytes = Vec::new();
    let _ = stderr
        .by_ref()
        .take(MAX_STDERR_BYTES as u64)
        .read_to_end(&mut bytes);
    let message = String::from_utf8_lossy(&bytes).trim().to_string();
    if !message.is_empty() {
        let _ = event_tx.send(AgentEvent::Diagnostic { message });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HeadlessSessionConfig, HeadlessSessionHandle, MAX_EVENT_LINE_BYTES,
        RemoteHeadlessSessionConfig, command_arguments, normalize_event, spawn_headless_session,
        spawn_remote_headless_session,
    };
    use crate::agents::protocol::{AgentEvent, AgentRole, AgentRunState};
    use crate::models::{
        AgentBackendKind, AgentLocation, AgentPermissionPolicy, AgentProvider, AuthConfig,
        ConnectRequest, ConnectionKind, SavedAgentDefinition, SavedWorktreePolicy,
    };
    use crate::storage::KnownHostStore;
    use crate::test_support::{DockerSshServer, TestIsolation};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn config(provider: AgentProvider) -> HeadlessSessionConfig {
        HeadlessSessionConfig {
            provider,
            executable: PathBuf::from("agent"),
            working_directory: PathBuf::from("/tmp"),
            permission_policy: AgentPermissionPolicy::ReadOnly,
            arguments: vec!["argument with spaces".to_string()],
            initial_prompt: None,
        }
    }

    fn docker_request(server: &DockerSshServer) -> ConnectRequest {
        ConnectRequest {
            session_id: 701,
            title: "Remote structured agent".to_string(),
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
    fn cancellation_does_not_block_when_the_event_queue_is_full() {
        let (event_tx, event_rx) = mpsc::sync_channel(1);
        event_tx
            .send(AgentEvent::Diagnostic {
                message: "fill queue".to_string(),
            })
            .unwrap();
        let handle = HeadlessSessionHandle {
            config: config(AgentProvider::ClaudeCode),
            event_rx,
            event_tx,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            cancel_requested: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            child_id: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        };
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(handle.cancel());
        });
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("cancel should not wait for event queue capacity")
                .is_ok()
        );
    }

    #[test]
    fn builds_documented_headless_arguments_without_shell_joining() {
        assert_eq!(
            command_arguments(&config(AgentProvider::ClaudeCode), "do work"),
            vec![
                "-p",
                "do work",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "plan",
                "argument with spaces",
            ]
        );
        assert_eq!(
            command_arguments(&config(AgentProvider::Gemini), "do work"),
            vec![
                "-p",
                "do work",
                "--output-format",
                "stream-json",
                "--approval-mode",
                "plan",
                "argument with spaces",
            ]
        );
    }

    #[test]
    fn docker_remote_headless_session_normalizes_structured_events() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping remote headless e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let executable = "/usr/local/bin/termirust-fake-claude";
        server
            .exec(&format!(
                "cat > {executable} <<'TERMIRUST_EOF'\n#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'fake-claude 1.0\\n'; exit 0; fi\nprintf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"remote-session\"}}'\nprintf '%s\\n' '{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"remote-ok\"}}]}}}}'\nprintf '%s\\n' '{{\"type\":\"result\",\"is_error\":false,\"result\":\"remote-done\"}}'\nTERMIRUST_EOF\nchmod 755 {executable}"
            ))
            .expect("unable to install fake remote provider");
        let definition = SavedAgentDefinition {
            provider: AgentProvider::ClaudeCode,
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
        let handle = spawn_remote_headless_session(RemoteHeadlessSessionConfig {
            definition,
            request: docker_request(&server),
            known_hosts: Arc::new(KnownHostStore::load().unwrap()),
            keepalive_secs: 0,
            initial_prompt: Some("review remotely".to_string()),
        })
        .expect("unable to start remote headless session");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut saw_ready = false;
        let mut saw_message = false;
        let mut saw_completed = false;
        let mut saw_succeeded = false;
        while std::time::Instant::now() < deadline && !saw_succeeded {
            match handle.event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(AgentEvent::SessionReady {
                    provider_session_id,
                }) => saw_ready = provider_session_id == "remote-session",
                Ok(AgentEvent::MessageDelta { text, .. }) => {
                    saw_message |= text.contains("remote-ok")
                }
                Ok(AgentEvent::Completed { summary }) => {
                    saw_completed = summary.as_deref() == Some("remote-done")
                }
                Ok(AgentEvent::StateChanged(AgentRunState::Succeeded)) => saw_succeeded = true,
                Ok(AgentEvent::Failed { error }) => panic!("remote provider failed: {error}"),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("remote provider event stream closed: {error}"),
            }
        }
        assert!(saw_ready);
        assert!(saw_message);
        assert!(saw_completed);
        assert!(saw_succeeded);
    }

    #[test]
    fn docker_remote_headless_missing_provider_reports_install_guidance() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping remote missing-provider e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let definition = SavedAgentDefinition {
            provider: AgentProvider::ClaudeCode,
            backend: AgentBackendKind::Structured,
            location: AgentLocation::SavedHost {
                profile_id: "docker".to_string(),
            },
            working_directory: Some("/home/termirust".to_string()),
            executable_override: Some("/opt/termirust-missing-claude".to_string()),
            permission_policy: AgentPermissionPolicy::ReadOnly,
            worktree: SavedWorktreePolicy::ReadOnly,
            ..SavedAgentDefinition::default()
        };
        let handle = spawn_remote_headless_session(RemoteHeadlessSessionConfig {
            definition,
            request: docker_request(&server),
            known_hosts: Arc::new(KnownHostStore::load().unwrap()),
            keepalive_secs: 0,
            initial_prompt: Some("this must not run".to_string()),
        })
        .expect("unable to start remote missing-provider check");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut diagnostic = String::new();
        let mut saw_failed = false;
        while std::time::Instant::now() < deadline
            && (!saw_failed || !diagnostic.contains("code.claude.com/docs/en/setup"))
        {
            match handle.event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(AgentEvent::Diagnostic { message }) => diagnostic.push_str(&message),
                Ok(AgentEvent::StateChanged(AgentRunState::Failed)) => saw_failed = true,
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("remote provider event stream closed: {error}"),
            }
        }

        assert!(saw_failed, "missing remote provider did not fail");
        assert!(
            diagnostic.contains("Install Claude Code")
                && diagnostic.contains("code.claude.com/docs/en/setup")
                && diagnostic.contains("Check again"),
            "missing remote provider guidance was not actionable: {diagnostic:?}"
        );
    }

    #[test]
    fn docker_remote_headless_cancellation_terminates_the_active_job() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping remote cancellation e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let executable = "/usr/local/bin/termirust-cancellable-claude";
        server
            .exec(&format!(
                "cat > {executable} <<'TERMIRUST_EOF'\n#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'fake-claude 1.0\\n'; exit 0; fi\ntrap 'printf cancelled > /tmp/termirust-remote-cancelled; exit 143' TERM\nprintf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"remote-cancel\"}}'\nwhile :; do sleep 1; done\nTERMIRUST_EOF\nchmod 755 {executable}; rm -f /tmp/termirust-remote-cancelled"
            ))
            .expect("unable to install cancellable remote provider");
        let definition = SavedAgentDefinition {
            provider: AgentProvider::ClaudeCode,
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
        let handle = spawn_remote_headless_session(RemoteHeadlessSessionConfig {
            definition,
            request: docker_request(&server),
            known_hosts: Arc::new(KnownHostStore::load().unwrap()),
            keepalive_secs: 0,
            initial_prompt: Some("wait until cancelled".to_string()),
        })
        .expect("unable to start cancellable remote provider");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut saw_running = false;
        while std::time::Instant::now() < deadline && !saw_running {
            match handle.event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(AgentEvent::StateChanged(AgentRunState::Running)) => saw_running = true,
                Ok(AgentEvent::Failed { error }) => panic!("remote provider failed: {error}"),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("remote provider event stream closed: {error}"),
            }
        }
        assert!(saw_running, "remote provider did not become running");
        handle
            .cancel()
            .expect("remote cancellation should send TERM");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut saw_cancelled = false;
        while std::time::Instant::now() < deadline && !saw_cancelled {
            match handle.event_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(AgentEvent::StateChanged(AgentRunState::Cancelled)) => saw_cancelled = true,
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("remote provider event stream closed: {error}"),
            }
        }
        assert!(saw_cancelled, "remote provider did not report cancellation");

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if server
                .exec("test \"$(cat /tmp/termirust-remote-cancelled 2>/dev/null)\" = cancelled")
                .is_ok()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("remote provider did not receive the cancellation signal");
    }

    #[test]
    fn normalizes_claude_and_gemini_stream_events() {
        let (tx, rx) = mpsc::sync_channel(32);
        normalize_event(
            AgentProvider::ClaudeCode,
            &json!({"type":"assistant","message":{"content":[{"type":"text","text":"claude"}]}}),
            &tx,
        );
        normalize_event(
            AgentProvider::Gemini,
            &json!({"type":"message","role":"assistant","content":"gemini"}),
            &tx,
        );
        normalize_event(AgentProvider::Gemini, &json!({"type":"result"}), &tx);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(events.contains(&AgentEvent::MessageDelta {
            role: AgentRole::Assistant,
            text: "claude".to_string(),
        }));
        assert!(events.contains(&AgentEvent::MessageDelta {
            role: AgentRole::Assistant,
            text: "gemini".to_string(),
        }));
        assert!(events.contains(&AgentEvent::StateChanged(AgentRunState::Succeeded)));
    }

    fn run_live_headless_smoke(provider: AgentProvider, executable: &str, marker: &str) {
        if std::env::var("TERMIRUST_RUN_LIVE_AGENT_TESTS").as_deref() != Ok("1") {
            eprintln!("skipping live provider smoke; set TERMIRUST_RUN_LIVE_AGENT_TESTS=1");
            return;
        }
        let working_directory = std::env::temp_dir().join(format!(
            "termirust-live-{}-smoke",
            provider.label().to_ascii_lowercase().replace(' ', "-")
        ));
        std::fs::create_dir_all(&working_directory).unwrap();
        let session = spawn_headless_session(HeadlessSessionConfig {
            provider,
            executable: PathBuf::from(executable),
            working_directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            arguments: Vec::new(),
            initial_prompt: Some(format!(
                "Reply with exactly {marker}. Do not use tools or modify files."
            )),
        })
        .expect("launch live headless provider");
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let mut response = String::new();
        let mut succeeded = false;
        while std::time::Instant::now() < deadline {
            match session.event_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(AgentEvent::MessageDelta { text, .. }) => response.push_str(&text),
                Ok(AgentEvent::Completed { summary }) => {
                    if let Some(summary) = summary {
                        response.push_str(&summary);
                    }
                    succeeded = true;
                }
                Ok(AgentEvent::StateChanged(AgentRunState::Succeeded)) => succeeded = true,
                Ok(AgentEvent::Failed { error }) => {
                    panic!("live {} failed: {error}", provider.label())
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if succeeded => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => panic!("live {} event channel failed: {error}", provider.label()),
            }
            if succeeded && response.contains(marker) {
                break;
            }
        }
        assert!(succeeded, "{} did not report success", provider.label());
        assert!(
            response.contains(marker),
            "{} response did not contain {marker}: {response:?}",
            provider.label()
        );
    }

    #[test]
    #[ignore = "requires TERMIRUST_RUN_LIVE_AGENT_TESTS=1, authenticated Claude Code, and network access"]
    fn live_claude_headless_smoke() {
        run_live_headless_smoke(
            AgentProvider::ClaudeCode,
            "claude",
            "TERMIRUST_CLAUDE_LIVE_OK",
        );
    }

    #[test]
    #[ignore = "requires TERMIRUST_RUN_LIVE_AGENT_TESTS=1, authenticated Gemini CLI, and network access"]
    fn live_gemini_headless_smoke() {
        run_live_headless_smoke(AgentProvider::Gemini, "gemini", "TERMIRUST_GEMINI_LIVE_OK");
    }

    #[test]
    #[cfg(unix)]
    fn cancellation_remains_cancelled_after_the_child_exits() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("termirust-headless-cancel-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake claude");
        fs::write(&executable, "#!/bin/sh\nexec sleep 30\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let handle = spawn_headless_session(HeadlessSessionConfig {
            provider: AgentProvider::ClaudeCode,
            executable,
            working_directory: directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            arguments: Vec::new(),
            initial_prompt: Some("cancel this".to_string()),
        })
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while handle.child_id.load(Ordering::Acquire) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(handle.child_id.load(Ordering::Acquire), 0);

        handle.cancel().unwrap();
        while handle.running.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!handle.running.load(Ordering::Acquire));
        let events: Vec<_> = handle.event_rx.try_iter().collect();
        assert!(events.contains(&AgentEvent::StateChanged(AgentRunState::Cancelled)));
        assert!(!events.contains(&AgentEvent::StateChanged(AgentRunState::Failed)));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::Failed { .. }))
        );
    }

    #[test]
    #[cfg(unix)]
    fn successful_exit_without_completion_is_reported_as_failed() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("termirust-headless-empty-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake claude");
        fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let handle = spawn_headless_session(HeadlessSessionConfig {
            provider: AgentProvider::ClaudeCode,
            executable,
            working_directory: directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            arguments: Vec::new(),
            initial_prompt: Some("finish this".to_string()),
        })
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while handle.running.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!handle.running.load(Ordering::Acquire));
        let events: Vec<_> = handle.event_rx.try_iter().collect();
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Failed { error }
                if error.contains("exited without a structured completion event")
        )));
        assert!(events.contains(&AgentEvent::StateChanged(AgentRunState::Failed)));
        assert!(!events.contains(&AgentEvent::StateChanged(AgentRunState::Succeeded)));
    }

    #[test]
    #[cfg(unix)]
    fn structured_provider_failure_is_not_overwritten_by_exit_status() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("termirust-headless-failure-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake claude");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"is_error\":true,\"result\":\"Account access is disabled\"}'\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let handle = spawn_headless_session(HeadlessSessionConfig {
            provider: AgentProvider::ClaudeCode,
            executable,
            working_directory: directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            arguments: Vec::new(),
            initial_prompt: Some("fail clearly".to_string()),
        })
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while handle.running.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }

        let events: Vec<_> = handle.event_rx.try_iter().collect();
        assert!(events.contains(&AgentEvent::Failed {
            error: "Account access is disabled".to_string(),
        }));
        assert!(!events.iter().any(|event| matches!(
            event,
            AgentEvent::Failed { error } if error.contains("exited with")
        )));
    }

    #[test]
    #[cfg(unix)]
    fn headless_process_streams_events_without_shell_parsing_arguments() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("termirust-headless-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake claude");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"session-1\"}'\nprintf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"finished\"}]}}'\nprintf '%s\\n' '{\"type\":\"result\",\"is_error\":false,\"result\":\"ok\"}'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let marker = directory.join("must-not-exist");
        let handle = spawn_headless_session(HeadlessSessionConfig {
            provider: AgentProvider::ClaudeCode,
            executable: executable.clone(),
            working_directory: directory.clone(),
            permission_policy: AgentPermissionPolicy::ReadOnly,
            arguments: vec![format!("$(touch {})", marker.display())],
            initial_prompt: Some("test prompt".to_string()),
        })
        .unwrap();
        let mut events = Vec::new();
        while !events.contains(&AgentEvent::StateChanged(AgentRunState::Succeeded)) {
            events.push(
                handle
                    .event_rx
                    .recv_timeout(Duration::from_secs(3))
                    .expect("expected structured event"),
            );
        }
        let arguments = fs::read_to_string(format!("{}.args", executable.display())).unwrap();
        assert!(arguments.lines().any(|line| line == "test prompt"));
        assert!(
            arguments
                .lines()
                .any(|line| line == format!("$(touch {})", marker.display()))
        );
        assert!(!marker.exists());
        assert!(events.contains(&AgentEvent::MessageDelta {
            role: AgentRole::Assistant,
            text: "finished".to_string(),
        }));
    }

    #[test]
    #[cfg(unix)]
    fn headless_process_discards_oversized_events_and_continues() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("termirust-headless-bounded-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake gemini");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\nprintf '%s\\n' '{{\"type\":\"result\"}}'\n",
            "x".repeat(MAX_EVENT_LINE_BYTES + 1)
        );
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let handle = spawn_headless_session(HeadlessSessionConfig {
            provider: AgentProvider::Gemini,
            executable,
            working_directory: directory,
            permission_policy: AgentPermissionPolicy::ReadOnly,
            arguments: Vec::new(),
            initial_prompt: Some("bounded input".to_string()),
        })
        .unwrap();
        let mut events = Vec::new();
        while !events.contains(&AgentEvent::StateChanged(AgentRunState::Succeeded)) {
            events.push(
                handle
                    .event_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("expected bounded structured event"),
            );
        }
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Diagnostic { message } if message.contains("larger than")
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::Failed { .. }))
        );
    }
}
