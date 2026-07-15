use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::agents::{AgentEvent, AgentRole, AgentRunState, NormalizedToolCall, ToolOutcome};
use crate::models::{AgentPermissionPolicy, AgentProvider};

const EVENT_CHANNEL_CAPACITY: usize = 512;
const MAX_STDERR_BYTES: usize = 16 * 1024;

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
    child_id: Arc<AtomicU32>,
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
        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let running = Arc::clone(&self.running);
        let child_id = Arc::clone(&self.child_id);
        thread::Builder::new()
            .name(format!(
                "termirust-{}-structured",
                config
                    .provider
                    .label()
                    .to_ascii_lowercase()
                    .replace(' ', "-")
            ))
            .spawn(move || run_job(config, prompt, event_tx, running, child_id))
            .context("Unable to start structured agent job")?;
        Ok(())
    }

    pub fn cancel(&self) -> Result<()> {
        let child_id = self.child_id.load(Ordering::Acquire);
        if child_id == 0 {
            bail!("The structured agent has no active job");
        }
        #[cfg(unix)]
        unsafe {
            if libc::kill(child_id as libc::pid_t, libc::SIGTERM) != 0 {
                bail!("Unable to interrupt the structured agent process");
            }
        }
        let _ = self
            .event_tx
            .send(AgentEvent::StateChanged(AgentRunState::Cancelled));
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

fn run_job(
    config: HeadlessSessionConfig,
    prompt: String,
    event_tx: SyncSender<AgentEvent>,
    running: Arc<AtomicBool>,
    child_id: Arc<AtomicU32>,
) {
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
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(stderr) = stderr {
        let diagnostics = event_tx.clone();
        thread::spawn(move || capture_stderr(stderr, diagnostics));
    }
    if let Some(stdout) = stdout {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => match serde_json::from_str::<Value>(&line) {
                    Ok(value) => normalize_event(config.provider, &value, &event_tx),
                    Err(error) => {
                        let _ = event_tx.send(AgentEvent::Diagnostic {
                            message: format!("Ignored malformed structured event: {error}"),
                        });
                    }
                },
                Ok(_) => {}
                Err(error) => {
                    let _ = event_tx.send(AgentEvent::Failed {
                        error: format!("Unable to read structured output: {error}"),
                    });
                    break;
                }
            }
        }
    }
    let status = child.wait();
    child_id.store(0, Ordering::Release);
    running.store(false, Ordering::Release);
    match status {
        Ok(status) if status.success() => {}
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

fn command_arguments(config: &HeadlessSessionConfig, prompt: &str) -> Vec<String> {
    let mut arguments = match config.provider {
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
    match (config.provider, config.permission_policy) {
        (AgentProvider::ClaudeCode, AgentPermissionPolicy::ReadOnly) => {
            arguments.extend(["--permission-mode".to_string(), "plan".to_string()]);
        }
        (AgentProvider::Gemini, AgentPermissionPolicy::ReadOnly) => {
            arguments.extend(["--approval-mode".to_string(), "plan".to_string()]);
        }
        _ => {}
    }
    arguments.extend(config.arguments.clone());
    arguments
}

fn normalize_event(provider: AgentProvider, value: &Value, event_tx: &SyncSender<AgentEvent>) {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match provider {
        AgentProvider::ClaudeCode => normalize_claude(kind, value, event_tx),
        AgentProvider::Gemini => normalize_gemini(kind, value, event_tx),
        _ => {}
    }
}

fn normalize_claude(kind: &str, value: &Value, event_tx: &SyncSender<AgentEvent>) {
    match kind {
        "system" if value.get("subtype").and_then(Value::as_str) == Some("init") => {
            if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
                let _ = event_tx.send(AgentEvent::SessionReady {
                    provider_session_id: session_id.to_string(),
                });
            }
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Running));
        }
        "assistant" => {
            if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
                normalize_content_blocks(content, event_tx);
            }
        }
        "user" => {
            if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
                for block in content {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        if let Some(call_id) = block.get("tool_use_id").and_then(Value::as_str) {
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
            }
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
        }
        _ => {}
    }
}

fn normalize_gemini(kind: &str, value: &Value, event_tx: &SyncSender<AgentEvent>) {
    match kind {
        "init" => {
            if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
                let _ = event_tx.send(AgentEvent::SessionReady {
                    provider_session_id: session_id.to_string(),
                });
            }
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Running));
        }
        "message" if value.get("role").and_then(Value::as_str) == Some("assistant") => {
            if let Some(text) = value.get("content").and_then(Value::as_str) {
                let _ = event_tx.send(AgentEvent::MessageDelta {
                    role: AgentRole::Assistant,
                    text: text.to_string(),
                });
            }
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
        }
        "result" => {
            let _ = event_tx.send(AgentEvent::Completed { summary: None });
            let _ = event_tx.send(AgentEvent::StateChanged(AgentRunState::Succeeded));
        }
        _ => {}
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
        HeadlessSessionConfig, command_arguments, normalize_event, spawn_headless_session,
    };
    use crate::agents::{AgentEvent, AgentRole, AgentRunState};
    use crate::models::{AgentPermissionPolicy, AgentProvider};
    use serde_json::json;
    use std::path::PathBuf;
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
}
