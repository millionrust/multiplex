#![allow(dead_code)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use termirust_cli::{
    Cancellation, CliClock, CliError, CliIds, CliPaths, HostAttachRequest, HostAttachSummary,
    HostController, HostLaunchOutcome, HostLauncher, HostResizeRequest, LocalCommandService,
};
use termirust_domain::{
    ActivityAggregate, AddProject, CommandId, ExecutableSpec, HostInstanceId, HostLifecycle,
    HostedSession, HostedSessionId, HostedSessionState, LaunchPreset, OsStringValue,
    OutputSequence, PermissionPolicy, PositionKey, PresetDraft, PresetId, PresetOrigin, PresetRisk,
    ProjectId, Revision, SessionTitle, TitleSource, WorkingDirectoryRule,
};
use termirust_session_host::LaunchDescriptor;
use termirust_store::{
    HostLease, HostMetadata, PresetRepository, ProjectRepository, SessionRepository,
};
use uuid::Uuid;

pub const PROJECT_ID: ProjectId = ProjectId::from_uuid(Uuid::from_u128(1));
pub const PRESET_ID: PresetId = PresetId::from_uuid(Uuid::from_u128(2));
pub const SESSION_ID: HostedSessionId = HostedSessionId::from_uuid(Uuid::from_u128(3));
pub const LAUNCH_SESSION_ID: HostedSessionId = HostedSessionId::from_uuid(Uuid::from_u128(4));
const COMMAND_ID: CommandId = CommandId::from_uuid(Uuid::from_u128(5));
const HOST_ID: HostInstanceId = HostInstanceId::from_uuid(Uuid::from_u128(6));

pub struct SeededStore {
    pub temp: tempfile::TempDir,
    pub config_root: std::path::PathBuf,
    pub metadata_root: std::path::PathBuf,
    pub project_root: std::path::PathBuf,
}

impl SeededStore {
    pub fn paths(&self) -> CliPaths {
        CliPaths::new(
            self.config_root.clone(),
            self.temp.path().join("missing-session-host"),
        )
    }

    pub fn sessions(&self) -> SessionRepository {
        SessionRepository::open(
            self.metadata_root.clone(),
            self.config_root.join("durable-sessions"),
        )
        .unwrap()
    }
}

pub fn seed_store() -> SeededStore {
    let temp = tempfile::tempdir().unwrap();
    let config_root = temp.path().join("config");
    let metadata_root = config_root.join("agent-workspace");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();
    let projects = ProjectRepository::open(&metadata_root).unwrap();
    projects
        .add_project(AddProject {
            id: PROJECT_ID,
            root: project_root.clone(),
            display_name: Some("Example Project".into()),
            expected: Revision::ZERO,
        })
        .unwrap();
    let presets = PresetRepository::open(&metadata_root).unwrap();
    presets
        .save_preset(
            PresetDraft {
                id: PRESET_ID,
                label: "Counter".into(),
                executable: "/bin/sh".into(),
                args: vec!["-c".into(), "sleep 30".into()],
                working_directory: WorkingDirectoryRule::ProjectRoot,
                runtime: None,
                enabled: true,
                favorite: true,
                permission_policy: PermissionPolicy::AskAsNeeded,
                origin: PresetOrigin::User,
                confirm_risky_favorite: false,
            },
            Revision::ZERO,
        )
        .unwrap();
    SessionRepository::open(&metadata_root, config_root.join("durable-sessions")).unwrap();
    SeededStore {
        temp,
        config_root,
        metadata_root,
        project_root,
    }
}

pub fn insert_session(
    seed: &SeededStore,
    state: HostedSessionState,
    archived: bool,
) -> HostedSession {
    let repository = seed.sessions();
    let revision = repository.load().unwrap().revision;
    repository
        .create_session(
            HostedSession {
                id: SESSION_ID,
                project_id: PROJECT_ID,
                group_id: None,
                preset_id: Some(PRESET_ID),
                title: SessionTitle::new("Counter session").unwrap(),
                title_source: TitleSource::Manual,
                lifecycle: state,
                activity: ActivityAggregate::default(),
                pinned: false,
                position: PositionKey::FIRST,
                last_output_sequence: OutputSequence::ZERO,
                read_through_sequence: OutputSequence::ZERO,
                unread_sequence: None,
                archived_at: archived.then_some(1),
                created_at: 1,
                updated_at: 1,
                revision: Revision::ZERO,
            },
            revision,
        )
        .unwrap()
}

pub fn write_ready_host_metadata(seed: &SeededStore, host_id: HostInstanceId) -> HostLease {
    let lease = HostLease::acquire(
        seed.config_root
            .join("durable-sessions")
            .join(SESSION_ID.to_string()),
        host_id,
    )
    .unwrap();
    lease
        .write_metadata(&HostMetadata {
            format_version: HostMetadata::FORMAT_VERSION,
            session_id: SESSION_ID,
            host_instance_id: host_id,
            process_token: None,
            runtime_recognition: None,
            activity: Default::default(),
            lifecycle: HostLifecycle::Ready,
            endpoint_name: "fixture.sock".into(),
            heartbeat_monotonic_nanos: 1,
            durability_watermark: None,
        })
        .unwrap();
    lease
}

#[derive(Default)]
pub struct FakeLauncherState {
    pub calls: usize,
    pub outcome: Option<Result<HostLaunchOutcome, CliError>>,
    pub cancel_after_ready: bool,
    pub descriptors: Vec<LaunchDescriptor>,
}

pub struct FakeLauncher {
    pub state: Arc<Mutex<FakeLauncherState>>,
}

impl HostLauncher for FakeLauncher {
    fn launch(
        &self,
        descriptor: &LaunchDescriptor,
        _host_executable: &Path,
        cancellation: &Cancellation,
    ) -> Result<HostLaunchOutcome, CliError> {
        assert!(!format!("{descriptor:?}").contains("sleep 30"));
        let mut state = self.state.lock().unwrap();
        state.calls += 1;
        state.descriptors.push(descriptor.clone());
        if state.cancel_after_ready {
            cancellation.cancel();
        }
        state.outcome.clone().unwrap_or_else(|| {
            if cancellation.is_cancelled() {
                Ok(HostLaunchOutcome::ReadyAfterPreReadyCancellation)
            } else {
                Ok(HostLaunchOutcome::Ready)
            }
        })
    }
}

#[derive(Default)]
pub struct FakeControllerState {
    pub calls: usize,
    pub input_calls: usize,
    pub resize_calls: usize,
    pub attach_calls: usize,
    pub inputs: Vec<Vec<u8>>,
    pub resize_requests: Vec<(u16, u16)>,
    pub attach_requests: Vec<HostAttachRequest>,
    pub expected_hosts: Vec<Option<HostInstanceId>>,
    pub result: Option<Result<(), CliError>>,
    pub input_result: Option<Result<bool, CliError>>,
    pub resize_result: Option<Result<bool, CliError>>,
    pub attach_result: Option<Result<HostAttachSummary, CliError>>,
}

pub struct FakeController {
    pub state: Arc<Mutex<FakeControllerState>>,
}

impl HostController for FakeController {
    fn attach(
        &self,
        _runtime_root: &Path,
        _session_id: HostedSessionId,
        request: HostAttachRequest,
        _cancellation: &Cancellation,
    ) -> Result<HostAttachSummary, CliError> {
        let mut state = self.state.lock().unwrap();
        state.attach_calls += 1;
        state
            .expected_hosts
            .push(Some(request.expected_host_instance_id));
        state.attach_requests.push(request);
        state.attach_result.clone().unwrap_or(Ok(HostAttachSummary {
            lifecycle: HostLifecycle::Ready,
            latest_sequence: OutputSequence::new(9),
            replayed_records: 2,
            replayed_bytes: 12,
            snapshot: false,
            writer_lease: request.request_control,
        }))
    }

    fn input(
        &self,
        _runtime_root: &Path,
        _session_id: HostedSessionId,
        expected_host_instance_id: HostInstanceId,
        _command_id: CommandId,
        bytes: Vec<u8>,
        _cancellation: &Cancellation,
    ) -> Result<bool, CliError> {
        let mut state = self.state.lock().unwrap();
        state.input_calls += 1;
        state.inputs.push(bytes);
        state.expected_hosts.push(Some(expected_host_instance_id));
        state.input_result.clone().unwrap_or(Ok(true))
    }

    fn resize(
        &self,
        _runtime_root: &Path,
        _session_id: HostedSessionId,
        request: HostResizeRequest,
        _cancellation: &Cancellation,
    ) -> Result<bool, CliError> {
        let mut state = self.state.lock().unwrap();
        state.resize_calls += 1;
        state.resize_requests.push((request.columns, request.rows));
        state
            .expected_hosts
            .push(Some(request.expected_host_instance_id));
        state.resize_result.clone().unwrap_or(Ok(true))
    }

    fn stop(
        &self,
        _runtime_root: &Path,
        _session_id: HostedSessionId,
        expected_host_instance_id: Option<HostInstanceId>,
        _command_id: CommandId,
        _cancellation: &Cancellation,
    ) -> Result<(), CliError> {
        let mut state = self.state.lock().unwrap();
        state.calls += 1;
        state.expected_hosts.push(expected_host_instance_id);
        state.result.clone().unwrap_or(Ok(()))
    }
}

pub struct FixedClock(pub u64);

impl CliClock for FixedClock {
    fn now_millis(&self) -> u64 {
        self.0
    }
}

pub struct FixedIds;

impl CliIds for FixedIds {
    fn session_id(&self) -> HostedSessionId {
        LAUNCH_SESSION_ID
    }

    fn command_id(&self) -> CommandId {
        COMMAND_ID
    }

    fn host_instance_id(&self) -> HostInstanceId {
        HOST_ID
    }
}

pub fn service(
    seed: &SeededStore,
    launcher: Arc<Mutex<FakeLauncherState>>,
    controller: Arc<Mutex<FakeControllerState>>,
) -> LocalCommandService {
    LocalCommandService::with_adapters(
        seed.paths(),
        Arc::new(FakeLauncher { state: launcher }),
        Arc::new(FakeController { state: controller }),
        Arc::new(FixedClock(10)),
        Arc::new(FixedIds),
    )
}

pub fn preset_fixture() -> LaunchPreset {
    LaunchPreset {
        id: PRESET_ID,
        label: termirust_domain::LocalizedUserText::new("Counter").unwrap(),
        executable: ExecutableSpec::parse("/bin/sh").unwrap(),
        args: vec![OsStringValue::new("-c").unwrap()],
        working_directory: WorkingDirectoryRule::ProjectRoot,
        runtime: None,
        enabled: true,
        favorite: true,
        position: PositionKey::FIRST,
        permission_policy: PermissionPolicy::AskAsNeeded,
        origin: PresetOrigin::User,
        risk: PresetRisk::Safe,
        revision: Revision::new(1),
    }
}
