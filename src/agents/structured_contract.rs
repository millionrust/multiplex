use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use tempfile::TempDir;

use super::adapter::{AgentLaunchModes, provider_descriptor};
use super::codex::{CodexSessionConfig, CodexSessionHandle, spawn_codex_session};
use super::headless::{HeadlessSessionConfig, HeadlessSessionHandle, spawn_headless_session};
use super::protocol::{AgentEvent, AgentRole, AgentRunState, ToolOutcome};
use crate::models::{AgentPermissionPolicy, AgentProvider};

const FIXTURE_BYTES: &[u8] =
    include_bytes!("../../tests/fixtures/agents/structured-runtime-contract-v1.json");
const MAX_FIXTURE_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractFixture {
    schema_version: u16,
    max_events: usize,
    deadline_ms: u64,
    oversize_line_bytes: usize,
    providers: Vec<ProviderFixture>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderFixture {
    provider: FixtureProvider,
    capabilities: CapabilityFixture,
    expected_session_id: String,
    expected_message: String,
    expected_tool_id: String,
    expected_tool_name: String,
    expected_failure: String,
    handshake: Option<CodexHandshake>,
    success: Vec<Value>,
    failure: Vec<Value>,
    cancellation: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
enum FixtureProvider {
    Codex,
    ClaudeCode,
    Gemini,
}

impl FixtureProvider {
    fn agent_provider(self) -> AgentProvider {
        match self {
            Self::Codex => AgentProvider::Codex,
            Self::ClaudeCode => AgentProvider::ClaudeCode,
            Self::Gemini => AgentProvider::Gemini,
        }
    }

    fn file_stem(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct CapabilityFixture {
    interactive_pty: bool,
    structured_events: bool,
    approvals: bool,
    cancellation: bool,
    context_handoff: bool,
    remote: bool,
}

impl From<CapabilityFixture> for AgentLaunchModes {
    fn from(value: CapabilityFixture) -> Self {
        Self {
            interactive_pty: value.interactive_pty,
            structured_events: value.structured_events,
            approvals: value.approvals,
            cancellation: value.cancellation,
            context_handoff: value.context_handoff,
            remote: value.remote,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexHandshake {
    initialize: Value,
    session: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    Success,
    Failure,
    Cancellation,
    Recovery,
}

impl Scenario {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Cancellation => "cancellation",
            Self::Recovery => "recovery",
        }
    }
}

enum ContractHandle {
    Codex(CodexSessionHandle),
    Headless(HeadlessSessionHandle),
}

impl ContractHandle {
    fn receiver(&self) -> &Receiver<AgentEvent> {
        match self {
            Self::Codex(handle) => &handle.event_rx,
            Self::Headless(handle) => &handle.event_rx,
        }
    }

    fn cancel(&self) -> anyhow::Result<()> {
        match self {
            Self::Codex(handle) => handle.cancel(),
            Self::Headless(handle) => handle.cancel(),
        }
    }
}

#[derive(Default, Debug)]
struct Projection {
    starting: bool,
    running: bool,
    ready: bool,
    message: bool,
    tool_started: bool,
    tool_succeeded: bool,
    completed: bool,
    succeeded: bool,
    failed: bool,
    cancelled: bool,
    completion_events: usize,
    succeeded_states: usize,
    failure_events: usize,
    failed_states: usize,
    cancelled_states: usize,
    diagnostic_count: usize,
    unexpected: Vec<&'static str>,
}

impl Projection {
    fn observe(&mut self, event: &AgentEvent, fixture: &ProviderFixture) {
        match event {
            AgentEvent::StateChanged(AgentRunState::Starting) => self.starting = true,
            AgentEvent::StateChanged(AgentRunState::Running) => self.running = true,
            AgentEvent::StateChanged(AgentRunState::Succeeded) => {
                self.succeeded = true;
                self.succeeded_states += 1;
            }
            AgentEvent::StateChanged(AgentRunState::Failed) => {
                self.failed = true;
                self.failed_states += 1;
            }
            AgentEvent::StateChanged(AgentRunState::Cancelled) => {
                self.cancelled = true;
                self.cancelled_states += 1;
            }
            AgentEvent::SessionReady {
                provider_session_id,
            } if provider_session_id == &fixture.expected_session_id => self.ready = true,
            AgentEvent::SessionReady { .. } => self.unexpected.push("session_id"),
            AgentEvent::MessageDelta {
                role: AgentRole::Assistant,
                text,
            } if text == &fixture.expected_message => self.message = true,
            AgentEvent::MessageDelta { .. } => self.unexpected.push("message"),
            AgentEvent::ToolStarted(call)
                if call.call_id == fixture.expected_tool_id
                    && call.name == fixture.expected_tool_name =>
            {
                self.tool_started = true;
            }
            AgentEvent::ToolStarted(_) => self.unexpected.push("tool_started"),
            AgentEvent::ToolFinished {
                call_id,
                outcome: ToolOutcome::Succeeded,
            } if call_id == &fixture.expected_tool_id => self.tool_succeeded = true,
            AgentEvent::ToolFinished { .. } => self.unexpected.push("tool_finished"),
            AgentEvent::Completed { .. } => {
                self.completed = true;
                self.completion_events += 1;
            }
            AgentEvent::ApprovalRequested(_) => self.unexpected.push("approval"),
            AgentEvent::Failed { error } if error == &fixture.expected_failure => {
                self.failed = true;
                self.failure_events += 1;
            }
            AgentEvent::Failed { .. } => self.unexpected.push("failure"),
            AgentEvent::Diagnostic { .. } => self.diagnostic_count += 1,
            AgentEvent::StateChanged(
                AgentRunState::Idle
                | AgentRunState::WaitingForApproval
                | AgentRunState::Blocked
                | AgentRunState::Disconnected,
            ) => {}
        }
    }

    fn success_complete(&self) -> bool {
        self.completed && self.succeeded
    }

    fn failure_complete(&self) -> bool {
        self.failure_events >= 1 && self.failed_states >= 1
    }

    fn recovery_complete(&self) -> bool {
        self.success_complete() && self.diagnostic_count >= 2
    }
}

#[test]
fn structured_runtime_contract_all_providers_match_shared_fixture() {
    let fixture = load_fixture();
    for provider in &fixture.providers {
        assert_eq!(
            provider_descriptor(provider.provider.agent_provider()).launch_modes,
            AgentLaunchModes::from(provider.capabilities),
            "{} capability projection drifted",
            provider.provider.file_stem()
        );

        let success = run_scenario(&fixture, provider, Scenario::Success);
        assert_success(provider, &success);

        let failure = run_scenario(&fixture, provider, Scenario::Failure);
        assert!(failure.starting && failure.running && failure.ready);
        assert_eq!(failure.failure_events, 1, "{failure:?}");
        assert_eq!(failure.failed_states, 1, "{failure:?}");
        assert!(!failure.succeeded && !failure.completed && !failure.cancelled);
        assert!(failure.unexpected.is_empty(), "{failure:?}");

        let cancellation = run_cancellation(&fixture, provider);
        assert!(cancellation.starting && cancellation.running && cancellation.ready);
        assert_eq!(cancellation.cancelled_states, 1, "{cancellation:?}");
        assert!(!cancellation.succeeded && !cancellation.completed && !cancellation.failed);
        assert!(cancellation.unexpected.is_empty(), "{cancellation:?}");

        let recovery = run_scenario(&fixture, provider, Scenario::Recovery);
        assert_success(provider, &recovery);
        assert_eq!(recovery.diagnostic_count, 2, "{recovery:?}");
    }
}

fn load_fixture() -> ContractFixture {
    assert!(FIXTURE_BYTES.len() <= MAX_FIXTURE_BYTES);
    let fixture: ContractFixture = serde_json::from_slice(FIXTURE_BYTES).expect("valid fixture");
    assert_eq!(fixture.schema_version, 1);
    assert!((1..=256).contains(&fixture.max_events));
    assert!((1..=5_000).contains(&fixture.deadline_ms));
    assert!((1_048_577..=2 * 1_048_576).contains(&fixture.oversize_line_bytes));
    assert_eq!(fixture.providers.len(), 3);
    let providers: BTreeSet<_> = fixture
        .providers
        .iter()
        .map(|provider| provider.provider)
        .collect();
    assert_eq!(providers.len(), fixture.providers.len());
    assert!(providers.contains(&FixtureProvider::Codex));
    assert!(providers.contains(&FixtureProvider::ClaudeCode));
    assert!(providers.contains(&FixtureProvider::Gemini));
    for provider in &fixture.providers {
        match provider.provider {
            FixtureProvider::Codex => assert!(provider.handshake.is_some()),
            FixtureProvider::ClaudeCode | FixtureProvider::Gemini => {
                assert!(provider.handshake.is_none());
            }
        }
        assert!(!provider.success.is_empty());
        assert!(!provider.failure.is_empty());
        assert!(!provider.cancellation.is_empty());
    }
    fixture
}

fn run_scenario(
    contract: &ContractFixture,
    provider: &ProviderFixture,
    scenario: Scenario,
) -> Projection {
    let fixture_process = FixtureProcess::new(contract, provider, scenario);
    let marker = fixture_process.root.path().join("must-not-exist");
    let canary = format!("$(touch {})", marker.display());
    let handle = launch(provider.provider, &fixture_process.executable, &canary);
    let mut projection = collect(
        &handle,
        contract,
        provider,
        |projection| match scenario {
            Scenario::Success => projection.success_complete(),
            Scenario::Failure => projection.failure_complete(),
            Scenario::Recovery => projection.recovery_complete(),
            Scenario::Cancellation => false,
        },
        &canary,
    );
    assert!(!marker.exists(), "fixture prompt was shell-evaluated");
    fixture_process.assert_exited();
    drain_settled(&handle, provider, &mut projection, &canary);
    drop(handle);
    projection
}

fn run_cancellation(contract: &ContractFixture, provider: &ProviderFixture) -> Projection {
    let fixture_process = FixtureProcess::new(contract, provider, Scenario::Cancellation);
    let canary = "TERMIRUST_CONTRACT_CANCEL_CANARY";
    let handle = launch(provider.provider, &fixture_process.executable, canary);
    let mut projection = collect(
        &handle,
        contract,
        provider,
        |projection| projection.running && projection.ready,
        canary,
    );
    handle.cancel().expect("fixture cancellation");
    collect_into(
        &handle,
        contract,
        provider,
        &mut projection,
        |projection| projection.cancelled,
        canary,
    );
    fixture_process.assert_exited();
    drain_settled(&handle, provider, &mut projection, canary);
    drop(handle);
    projection
}

fn launch(provider: FixtureProvider, executable: &Path, prompt: &str) -> ContractHandle {
    let working_directory = executable
        .parent()
        .expect("fixture executable parent")
        .to_path_buf();
    match provider {
        FixtureProvider::Codex => ContractHandle::Codex(
            spawn_codex_session(CodexSessionConfig {
                executable: executable.to_path_buf(),
                working_directory,
                permission_policy: AgentPermissionPolicy::ReadOnly,
                initial_prompt: Some(prompt.to_string()),
            })
            .expect("launch Codex fixture"),
        ),
        FixtureProvider::ClaudeCode | FixtureProvider::Gemini => ContractHandle::Headless(
            spawn_headless_session(HeadlessSessionConfig {
                provider: provider.agent_provider(),
                executable: executable.to_path_buf(),
                working_directory,
                permission_policy: AgentPermissionPolicy::ReadOnly,
                arguments: vec![prompt.to_string()],
                initial_prompt: Some(prompt.to_string()),
            })
            .expect("launch headless fixture"),
        ),
    }
}

fn collect(
    handle: &ContractHandle,
    contract: &ContractFixture,
    provider: &ProviderFixture,
    done: impl Fn(&Projection) -> bool,
    canary: &str,
) -> Projection {
    let mut projection = Projection::default();
    collect_into(handle, contract, provider, &mut projection, done, canary);
    projection
}

fn collect_into(
    handle: &ContractHandle,
    contract: &ContractFixture,
    provider: &ProviderFixture,
    projection: &mut Projection,
    done: impl Fn(&Projection) -> bool,
    canary: &str,
) {
    let deadline = Instant::now() + Duration::from_millis(contract.deadline_ms);
    for _ in 0..contract.max_events {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "fixture event deadline exceeded");
        let event = match handle.receiver().recv_timeout(remaining) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => panic!("fixture event deadline exceeded"),
            Err(RecvTimeoutError::Disconnected) => panic!("fixture event channel disconnected"),
        };
        assert!(!format!("{event:?}").contains(canary));
        projection.observe(&event, provider);
        if done(projection) {
            return;
        }
    }
    panic!("fixture event limit exceeded: {projection:?}");
}

fn drain_settled(
    handle: &ContractHandle,
    provider: &ProviderFixture,
    projection: &mut Projection,
    canary: &str,
) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        match handle.receiver().recv_timeout(Duration::from_millis(20)) {
            Ok(event) => {
                assert!(!format!("{event:?}").contains(canary));
                projection.observe(&event, provider);
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn assert_success(provider: &ProviderFixture, projection: &Projection) {
    assert!(projection.starting && projection.running && projection.ready);
    assert!(projection.message && projection.tool_started && projection.tool_succeeded);
    assert!(projection.completed && projection.succeeded);
    assert_eq!(projection.completion_events, 1, "{projection:?}");
    assert_eq!(projection.succeeded_states, 1, "{projection:?}");
    assert_eq!(projection.failure_events, 0, "{projection:?}");
    assert_eq!(projection.failed_states, 0, "{projection:?}");
    assert_eq!(projection.cancelled_states, 0, "{projection:?}");
    assert!(!projection.failed && !projection.cancelled);
    assert!(projection.unexpected.is_empty(), "{projection:?}");
    assert!(provider.capabilities.structured_events);
}

struct FixtureProcess {
    root: TempDir,
    executable: PathBuf,
    pid_file: PathBuf,
}

impl FixtureProcess {
    fn new(contract: &ContractFixture, provider: &ProviderFixture, scenario: Scenario) -> Self {
        let root = tempfile::tempdir().expect("private fixture root");
        let executable = root.path().join(format!(
            "{}-{}",
            provider.provider.file_stem(),
            scenario.label()
        ));
        let pid_file = root.path().join("fixture.pid");
        fs::write(
            &executable,
            render_script(contract, provider, scenario, &pid_file),
        )
        .expect("write fixture executable");
        let mut permissions = fs::metadata(&executable)
            .expect("fixture executable metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).expect("private fixture executable");
        Self {
            root,
            executable,
            pid_file,
        }
    }

    fn assert_exited(&self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        let pid = loop {
            if let Ok(value) = fs::read_to_string(&self.pid_file)
                && let Ok(pid) = value.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "fixture pid was not recorded");
            std::thread::sleep(Duration::from_millis(10));
        };
        while process_exists(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!process_exists(pid), "fixture process {pid} remained alive");
    }
}

fn render_script(
    contract: &ContractFixture,
    provider: &ProviderFixture,
    scenario: Scenario,
    pid_file: &Path,
) -> String {
    let mut script = format!(
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" > {}\n",
        shell_single_quote(&pid_file.to_string_lossy())
    );
    if provider.provider == FixtureProvider::Codex {
        let handshake = provider.handshake.as_ref().expect("Codex handshake");
        script.push_str("IFS= read -r _initialize\n");
        push_record(&mut script, &handshake.initialize);
        script.push_str("IFS= read -r _initialized\nIFS= read -r _session\n");
        push_record(&mut script, &handshake.session);
        script.push_str("IFS= read -r _turn\n");
    }

    match scenario {
        Scenario::Success => push_records(&mut script, &provider.success),
        Scenario::Failure => push_records(&mut script, &provider.failure),
        Scenario::Recovery => {
            script.push_str(&format!(
                "dd if=/dev/zero bs={} count=1 2>/dev/null | tr '\\000' x\nprintf '\\n'\nprintf '%s\\n' '{{\"broken\":'\n",
                contract.oversize_line_bytes
            ));
            push_records(&mut script, &provider.success);
        }
        Scenario::Cancellation if provider.provider == FixtureProvider::Codex => {
            let split = provider.cancellation.len() - 1;
            push_records(&mut script, &provider.cancellation[..split]);
            script.push_str("IFS= read -r _interrupt\n");
            push_records(&mut script, &provider.cancellation[split..]);
        }
        Scenario::Cancellation => {
            push_records(&mut script, &provider.cancellation);
            script.push_str("trap 'exit 0' TERM INT\nwhile :; do :; done\n");
        }
    }
    script
}

fn push_records(script: &mut String, records: &[Value]) {
    for record in records {
        push_record(script, record);
    }
}

fn push_record(script: &mut String, record: &Value) {
    let line = serde_json::to_string(record).expect("serialize fixture record");
    script.push_str("printf '%s\\n' ");
    script.push_str(&shell_single_quote(&line));
    script.push('\n');
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}

fn process_exists(process_id: u32) -> bool {
    let result = unsafe { libc::kill(process_id as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
