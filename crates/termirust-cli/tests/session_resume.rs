#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use common::*;
use termirust_cli::{
    Cancellation, CliCommand, CliData, CliError, CommandService, ErrorCode, HostLaunchOutcome,
    HostLauncher, LocalCommandService, RenderOptions, render_success,
};
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{
    CommandId, ContinuityLink, HostInstanceId, HostLifecycle, HostedSessionId, HostedSessionState,
    OccupantGeneration, OccupantOwnership, PermissionPolicy, PresetDraft, PresetOrigin,
    ProcessToken, RecognitionConfidence, Revision, RuntimeCapability, RuntimeCapabilitySet,
    RuntimeId, RuntimeOccupant, RuntimeRecognition, SessionMutation, WorkingDirectoryRule,
};
use termirust_session_host::process_observation::fingerprint_executable;
use termirust_session_host::{LaunchDescriptor, start};
use termirust_store::{ContinuityRepository, HostLease, HostMetadata, PresetRepository};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const HANDLE: &str = "019cf76d-0493-77d1-8572-3fb4ac801ac8";
const PROVIDER_CANARY: &str = "RESUME-PROVIDER-CONTENT-MUST-STAY-PRIVATE";
const GENERATION: OccupantGeneration = OccupantGeneration::new(3);

struct ResumeFixture {
    seed: SeededStore,
    codex_home: std::path::PathBuf,
    codex_sessions: std::path::PathBuf,
    provider_metadata_path: std::path::PathBuf,
    source_revision: Revision,
    source_host_path: std::path::PathBuf,
}

#[test]
fn preview_is_non_mutating_bounded_and_content_free() {
    let fixture = resume_fixture("0.150.1");
    let launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = resume_service(&fixture, launcher.clone(), controller);
    let before_sessions = fixture.seed.sessions().load().unwrap();
    let before_host = fs::read(&fixture.source_host_path).unwrap();

    let data = service
        .execute(preview_command(), &Cancellation::default())
        .unwrap();
    let CliData::ResumePreview(preview) = &data else {
        panic!("expected resume preview");
    };
    assert_eq!(preview.source_session_id, SESSION_ID.to_string());
    assert_eq!(preview.source_revision, fixture.source_revision.get());
    assert_eq!(preview.provider, "codex");
    assert_eq!(preview.provider_version, "0.150.1");
    assert_eq!(preview.permission_policy, "read_only");
    assert_eq!(preview.replacement_generation, 4);
    assert!(preview.confirmation_required);
    assert_eq!(launcher.lock().unwrap().calls, 0);
    assert!(fixture.seed.sessions().load().unwrap() == before_sessions);
    assert_eq!(fs::read(&fixture.source_host_path).unwrap(), before_host);
    assert!(
        !fixture
            .seed
            .metadata_root
            .join("resume-continuity.json")
            .exists()
    );

    let json = String::from_utf8(
        render_success(
            &data,
            &[],
            RenderOptions {
                json: true,
                terminal_width: 80,
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(!json.contains(HANDLE));
    assert!(!json.contains(PROVIDER_CANARY));
    assert!(!json.contains(fixture.seed.project_root.to_string_lossy().as_ref()));
}

#[test]
fn confirmed_resume_launches_one_read_only_successor_and_commits_continuity() {
    let fixture = resume_fixture("0.150.1");
    let launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = resume_service(&fixture, launcher.clone(), controller.clone());
    let before_host = fs::read(&fixture.source_host_path).unwrap();

    let data = service
        .execute(
            commit_command(fixture.source_revision),
            &Cancellation::default(),
        )
        .unwrap();
    let CliData::Resume(resumed) = data else {
        panic!("expected resume result");
    };
    assert_eq!(resumed.source_session_id, SESSION_ID.to_string());
    assert_eq!(resumed.successor_session_id, LAUNCH_SESSION_ID.to_string());
    assert_eq!(resumed.permission_policy, "read_only");
    assert_eq!(resumed.replacement_generation, 4);
    assert_eq!(resumed.lifecycle, "live");
    assert!(resumed.continuity_committed);

    let launcher = launcher.lock().unwrap();
    assert_eq!(launcher.calls, 1);
    let descriptor = launcher.descriptors.first().unwrap();
    assert_eq!(descriptor.session_id, LAUNCH_SESSION_ID);
    assert_eq!(
        descriptor.expected_occupant_generation,
        Some(OccupantGeneration::new(4))
    );
    assert_eq!(
        descriptor.arguments,
        vec![
            "resume".to_string(),
            "--cd".to_string(),
            fs::canonicalize(&fixture.seed.project_root)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            "--sandbox".to_string(),
            "read-only".to_string(),
            HANDLE.to_string(),
        ]
    );
    drop(launcher);
    let snapshot = fixture.seed.sessions().load().unwrap();
    assert_eq!(snapshot.sessions.len(), 2);
    let source = snapshot
        .sessions
        .iter()
        .find(|session| session.id == SESSION_ID)
        .unwrap();
    let successor = snapshot
        .sessions
        .iter()
        .find(|session| session.id == LAUNCH_SESSION_ID)
        .unwrap();
    assert_eq!(source.revision, fixture.source_revision);
    assert_eq!(source.lifecycle, HostedSessionState::Exited);
    assert_eq!(successor.lifecycle, HostedSessionState::Live);
    assert_eq!(successor.activity.generation, OccupantGeneration::new(4));
    assert_eq!(fs::read(&fixture.source_host_path).unwrap(), before_host);
    let continuity = ContinuityRepository::open(&fixture.seed.metadata_root)
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(continuity.links.len(), 1);
    assert_eq!(continuity.links[0].source_session_id, SESSION_ID);
    assert_eq!(
        continuity.links[0].replacement_session_id,
        LAUNCH_SESSION_ID
    );
    assert_eq!(controller.lock().unwrap().calls, 0);
}

#[test]
fn stale_unsupported_archived_and_cancelled_requests_never_launch() {
    let fixture = resume_fixture("0.150.1");
    let launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = resume_service(&fixture, launcher.clone(), controller);
    assert_eq!(
        service
            .execute(
                commit_command(Revision::new(fixture.source_revision.get() + 1)),
                &Cancellation::default(),
            )
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        service
            .execute(preview_command(), &cancelled)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(launcher.lock().unwrap().calls, 0);

    let unsupported = resume_fixture("0.150.0");
    let unsupported_launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let mut service = resume_service(
        &unsupported,
        unsupported_launcher.clone(),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    assert_eq!(
        service
            .execute(preview_command(), &Cancellation::default())
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    assert_eq!(unsupported_launcher.lock().unwrap().calls, 0);

    let archived = resume_fixture("0.150.1");
    let archived_sessions = archived.seed.sessions();
    archived_sessions
        .mutate_session(
            SESSION_ID,
            archived.source_revision,
            SessionMutation::Archive { at: 2 },
            2,
        )
        .unwrap();
    let archived_launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let mut service = resume_service(
        &archived,
        archived_launcher.clone(),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    assert_eq!(
        service
            .execute(preview_command(), &Cancellation::default())
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(archived_launcher.lock().unwrap().calls, 0);
}

#[test]
fn continuity_race_stops_only_the_replacement_and_closes_its_record() {
    let fixture = resume_fixture("0.150.1");
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let launcher = Arc::new(ContinuityRaceLauncher {
        metadata_root: fixture.seed.metadata_root.clone(),
    });
    let mut service = LocalCommandService::with_adapters(
        fixture
            .seed
            .paths()
            .with_codex_conversation_root(fixture.codex_sessions.clone()),
        launcher,
        Arc::new(FakeController {
            state: controller.clone(),
        }),
        Arc::new(FixedClock(10)),
        Arc::new(FixedIds),
    );
    assert_eq!(
        service
            .execute(
                commit_command(fixture.source_revision),
                &Cancellation::default()
            )
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let controller = controller.lock().unwrap();
    assert_eq!(controller.calls, 1);
    assert_eq!(
        controller.expected_hosts,
        vec![Some(HostInstanceId::from_uuid(Uuid::from_u128(6)))]
    );
    drop(controller);
    let sessions = fixture.seed.sessions().load().unwrap();
    assert_eq!(sessions.sessions.len(), 2);
    assert_eq!(
        sessions
            .sessions
            .iter()
            .find(|session| session.id == LAUNCH_SESSION_ID)
            .unwrap()
            .lifecycle,
        HostedSessionState::Failed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_host_resume_survives_cli_service_return_and_packaged_preview_is_private() {
    let fixture = resume_fixture("0.150.1");
    let provider_metadata_before = fs::read(&fixture.provider_metadata_path).unwrap();
    let launcher = Arc::new(RealHostLauncher::default());
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = LocalCommandService::with_adapters(
        fixture
            .seed
            .paths()
            .with_codex_conversation_root(fixture.codex_sessions.clone()),
        launcher.clone(),
        Arc::new(FakeController { state: controller }),
        Arc::new(FixedClock(10)),
        Arc::new(FixedIds),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_termirust-cli"))
        .args(["session", "resume", &SESSION_ID.to_string(), "--json"])
        .env("TERMIRUST_CONFIG_DIR", &fixture.seed.config_root)
        .env("CODEX_HOME", &fixture.codex_home)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains(HANDLE));
    assert!(!stdout.contains(PROVIDER_CANARY));
    assert!(!stdout.contains(fixture.seed.project_root.to_string_lossy().as_ref()));

    let data = service
        .execute(
            commit_command(fixture.source_revision),
            &Cancellation::default(),
        )
        .unwrap();
    let CliData::Resume(resumed) = data else {
        panic!("expected real resume result");
    };
    let endpoint = LocalEndpoint::for_config_root(&fixture.seed.config_root, LAUNCH_SESSION_ID);
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local_read_only(LAUNCH_SESSION_ID, [8; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let state = client.get_state(&cancel).await.unwrap();
    assert_eq!(
        state.lifecycle,
        termirust_host_protocol::wire::Lifecycle::Ready as i32
    );
    let mut saw_canary = false;
    for _ in 0..40 {
        let outputs = client
            .attach(termirust_domain::OutputSequence::ZERO, 100, 30, &cancel)
            .await
            .unwrap();
        let replay = outputs
            .iter()
            .flat_map(|output| output.bytes.iter().copied())
            .collect::<Vec<_>>();
        let replay_contains_canary = replay
            .windows(PROVIDER_CANARY.len())
            .any(|window| window == PROVIDER_CANARY.as_bytes());
        let snapshot_contains_canary = client.take_last_snapshot().is_some_and(|snapshot| {
            snapshot
                .terminal_bytes
                .windows(PROVIDER_CANARY.len())
                .any(|window| window == PROVIDER_CANARY.as_bytes())
        });
        if replay_contains_canary || snapshot_contains_canary {
            saw_canary = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(saw_canary);
    client.disconnect();
    assert_eq!(resumed.lifecycle, "live");
    assert_eq!(
        fs::read(&fixture.provider_metadata_path).unwrap(),
        provider_metadata_before
    );
    launcher.shutdown();
}

fn preview_command() -> CliCommand {
    CliCommand::SessionResume {
        session_id: SESSION_ID,
        expected_revision: None,
        confirmed: false,
    }
}

fn commit_command(revision: Revision) -> CliCommand {
    CliCommand::SessionResume {
        session_id: SESSION_ID,
        expected_revision: Some(revision),
        confirmed: true,
    }
}

fn resume_service(
    fixture: &ResumeFixture,
    launcher: Arc<Mutex<FakeLauncherState>>,
    controller: Arc<Mutex<FakeControllerState>>,
) -> LocalCommandService {
    LocalCommandService::with_adapters(
        fixture
            .seed
            .paths()
            .with_codex_conversation_root(fixture.codex_sessions.clone()),
        Arc::new(FakeLauncher { state: launcher }),
        Arc::new(FakeController { state: controller }),
        Arc::new(FixedClock(10)),
        Arc::new(FixedIds),
    )
}

fn resume_fixture(version: &str) -> ResumeFixture {
    let seed = seed_store();
    let executable = seed.temp.path().join("codex-fixture");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '{PROVIDER_CANARY}\\n'\nwhile IFS= read -r line; do printf 'RESUME:%s\\n' \"$line\"; done\n"
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();
    let presets = PresetRepository::open(&seed.metadata_root).unwrap();
    let preset_revision = presets.load().unwrap().revision;
    presets
        .save_preset(
            PresetDraft {
                id: PRESET_ID,
                label: "Codex".into(),
                executable: executable.to_string_lossy().into_owned(),
                args: Vec::new(),
                working_directory: WorkingDirectoryRule::ProjectRoot,
                runtime: Some("codex".into()),
                enabled: true,
                favorite: true,
                permission_policy: PermissionPolicy::WorkspaceWrite,
                origin: PresetOrigin::User,
                confirm_risky_favorite: false,
            },
            preset_revision,
        )
        .unwrap();
    let source = insert_session(&seed, HostedSessionState::Exited, false);
    let host_id = HostInstanceId::from_uuid(Uuid::from_u128(40));
    let process_token = ProcessToken::new(host_id, 41, 1);
    let fingerprint = fingerprint_executable(&executable).unwrap();
    let recognition = RuntimeRecognition {
        occupant: Some(RuntimeOccupant {
            runtime_id: RuntimeId::new("codex").unwrap(),
            descriptor_version: 1,
            safe_version: Some(version.to_string()),
            executable_fingerprint: Some(fingerprint),
            generation: GENERATION,
            ownership: OccupantOwnership::Managed {
                host_instance: host_id,
                child_token: process_token,
            },
            capabilities: RuntimeCapabilitySet::new([
                RuntimeCapability::InteractivePty,
                RuntimeCapability::Cancellation,
                RuntimeCapability::Resume,
            ]),
            stale: false,
        }),
        confidence: RecognitionConfidence::Verified,
        observed_at_nanos: 1,
    };
    let source_dir = seed
        .config_root
        .join("durable-sessions")
        .join(SESSION_ID.to_string());
    let lease = HostLease::acquire(&source_dir, host_id).unwrap();
    lease
        .write_metadata(&HostMetadata {
            format_version: HostMetadata::FORMAT_VERSION,
            session_id: SESSION_ID,
            host_instance_id: host_id,
            process_token: Some(process_token),
            runtime_recognition: Some(recognition),
            activity: Default::default(),
            lifecycle: HostLifecycle::Exited,
            endpoint_name: "source.sock".into(),
            heartbeat_monotonic_nanos: 1,
            durability_watermark: None,
        })
        .unwrap();
    drop(lease);
    let codex_home = seed.temp.path().join("codex-home");
    let codex_sessions = codex_home.join("sessions");
    let provider_metadata_path = write_codex_metadata(&codex_sessions, &seed.project_root, version);
    ResumeFixture {
        source_host_path: source_dir.join("host.json"),
        seed,
        codex_home,
        codex_sessions,
        provider_metadata_path,
        source_revision: source.revision,
    }
}

fn write_codex_metadata(root: &Path, cwd: &Path, version: &str) -> std::path::PathBuf {
    let directory = root.join("2026/08/29");
    fs::create_dir_all(&directory).unwrap();
    let value = serde_json::json!({
        "ordinal": 0,
        "timestamp": "2026-08-29T00:00:00Z",
        "type": "session_meta",
        "payload": {
            "id": HANDLE,
            "session_id": HANDLE,
            "cli_version": version,
            "cwd": cwd,
            "ignored_content": {"canary": PROVIDER_CANARY}
        }
    });
    let path = directory.join("rollout.jsonl");
    fs::write(&path, format!("{value}\n")).unwrap();
    path
}

struct ContinuityRaceLauncher {
    metadata_root: std::path::PathBuf,
}

impl HostLauncher for ContinuityRaceLauncher {
    fn launch(
        &self,
        _descriptor: &LaunchDescriptor,
        _host_executable: &Path,
        _cancellation: &Cancellation,
    ) -> Result<HostLaunchOutcome, CliError> {
        let repository = ContinuityRepository::open(&self.metadata_root).unwrap();
        let snapshot = repository.load().unwrap();
        repository
            .record(
                snapshot.revision,
                ContinuityLink {
                    command_id: CommandId::from_uuid(Uuid::from_u128(90)),
                    source_session_id: SESSION_ID,
                    replacement_session_id: HostedSessionId::from_uuid(Uuid::from_u128(91)),
                    runtime_id: RuntimeId::new("codex").unwrap(),
                    prior_generation: GENERATION,
                    replacement_generation: GENERATION.next(),
                    committed_at: 9,
                },
            )
            .unwrap();
        Ok(HostLaunchOutcome::Ready)
    }
}

#[derive(Default)]
struct RealHostLauncher {
    state: Mutex<RealHostState>,
}

#[derive(Default)]
struct RealHostState {
    shutdown: Option<std::sync::mpsc::Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

impl RealHostLauncher {
    fn shutdown(&self) {
        let (shutdown, join) = {
            let mut state = self.state.lock().unwrap();
            (state.shutdown.take(), state.join.take())
        };
        if let Some(shutdown) = shutdown {
            let _ = shutdown.send(());
        }
        if let Some(join) = join {
            join.join().unwrap();
        }
    }
}

impl HostLauncher for RealHostLauncher {
    fn launch(
        &self,
        descriptor: &LaunchDescriptor,
        _host_executable: &Path,
        cancellation: &Cancellation,
    ) -> Result<HostLaunchOutcome, CliError> {
        if cancellation.is_cancelled() {
            return Err(CliError::new(
                ErrorCode::Cancelled,
                "fixture launch cancelled",
                "inspect state",
            ));
        }
        let descriptor = descriptor.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        let join = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                match start(descriptor).await {
                    Ok(host) => {
                        let _ = ready_tx.send(Ok(()));
                        let _ = tokio::task::spawn_blocking(move || shutdown_rx.recv()).await;
                        host.shutdown().await.unwrap();
                    }
                    Err(_) => {
                        let _ = ready_tx.send(Err(()));
                    }
                }
            });
        });
        if ready_rx.recv().ok() != Some(Ok(())) {
            join.join().unwrap();
            return Err(CliError::new(
                ErrorCode::OperationFailed,
                "fixture Host failed",
                "inspect fixture",
            ));
        }
        let mut state = self.state.lock().unwrap();
        state.shutdown = Some(shutdown_tx);
        state.join = Some(join);
        Ok(HostLaunchOutcome::Ready)
    }
}
