#![cfg(unix)]

#[path = "../../termirust-cli/tests/common/mod.rs"]
mod cli_common;

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use cli_common::*;
use termirust_cli::{
    Cancellation, CliError, ErrorCode, HostLaunchOutcome, HostLauncher, LocalCommandService,
};
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{
    HostInstanceId, HostLifecycle, HostedSessionState, OccupantGeneration, OccupantOwnership,
    PermissionPolicy, PresetDraft, PresetOrigin, ProcessToken, RecognitionConfidence, Revision,
    RuntimeCapability, RuntimeCapabilitySet, RuntimeId, RuntimeOccupant, RuntimeRecognition,
    SessionMutation, WorkingDirectoryRule,
};
use termirust_session_host::process_observation::fingerprint_executable;
use termirust_session_host::{LaunchDescriptor, start};
use termirust_store::{ContinuityRepository, HostLease, HostMetadata, PresetRepository};
use termirust_tui::{LocalResumeExecutor, ResumeExecutor};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const HANDLE: &str = "019cf76d-0493-77d1-8572-3fb4ac801ac8";
const PROVIDER_CANARY: &str = "TUI-RESUME-PROVIDER-CONTENT-MUST-STAY-PRIVATE";
const GENERATION: OccupantGeneration = OccupantGeneration::new(3);

struct ResumeFixture {
    seed: SeededStore,
    codex_sessions: std::path::PathBuf,
    provider_metadata_path: std::path::PathBuf,
    source_revision: Revision,
    source_host_path: std::path::PathBuf,
}

#[test]
fn tui_resume_preview_is_read_only_private_and_commit_is_exact() {
    let fixture = resume_fixture("0.150.1");
    let launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let executor = fake_executor(&fixture, launcher.clone(), controller.clone());
    let before_sessions = fixture.seed.sessions().load().unwrap();
    let before_host = fs::read(&fixture.source_host_path).unwrap();
    let before_provider = fs::read(&fixture.provider_metadata_path).unwrap();

    let review = executor
        .preview(&SESSION_ID.to_string(), &Cancellation::default())
        .unwrap();
    assert_eq!(review.source_session_id, SESSION_ID.to_string());
    assert_eq!(review.source_revision, fixture.source_revision.get());
    assert_eq!(review.provider, "codex");
    assert_eq!(review.provider_version, "0.150.1");
    assert_eq!(review.permission_policy, "read_only");
    assert_eq!(review.replacement_generation, 4);
    assert_eq!(launcher.lock().unwrap().calls, 0);
    assert!(fixture.seed.sessions().load().unwrap() == before_sessions);
    assert_eq!(fs::read(&fixture.source_host_path).unwrap(), before_host);
    assert_eq!(
        fs::read(&fixture.provider_metadata_path).unwrap(),
        before_provider
    );
    assert!(
        !fixture
            .seed
            .metadata_root
            .join("resume-continuity.json")
            .exists()
    );
    let debug = format!("{review:?}");
    assert!(!debug.contains(HANDLE));
    assert!(!debug.contains(PROVIDER_CANARY));
    assert!(!debug.contains(fixture.seed.project_root.to_string_lossy().as_ref()));
    assert!(!debug.contains(&SESSION_ID.to_string()));

    let result = executor
        .commit(
            &review.source_session_id,
            review.source_revision,
            &Cancellation::default(),
        )
        .unwrap();
    assert_eq!(result.source_session_id, SESSION_ID.to_string());
    assert_eq!(result.successor_session_id, LAUNCH_SESSION_ID.to_string());
    assert_eq!(result.permission_policy, "read_only");
    assert_eq!(result.lifecycle, "live");
    assert!(result.continuity_committed);
    assert_eq!(launcher.lock().unwrap().calls, 1);
    let sessions = fixture.seed.sessions().load().unwrap();
    let source = sessions
        .sessions
        .iter()
        .find(|session| session.id == SESSION_ID)
        .unwrap();
    let successor = sessions
        .sessions
        .iter()
        .find(|session| session.id == LAUNCH_SESSION_ID)
        .unwrap();
    assert_eq!(source.revision, fixture.source_revision);
    assert_eq!(source.lifecycle, HostedSessionState::Exited);
    assert_eq!(successor.lifecycle, HostedSessionState::Live);
    assert_eq!(successor.activity.generation, OccupantGeneration::new(4));
    assert_eq!(fs::read(&fixture.source_host_path).unwrap(), before_host);
    assert_eq!(
        fs::read(&fixture.provider_metadata_path).unwrap(),
        before_provider
    );
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
fn tui_resume_stale_cancelled_unsupported_and_archived_never_launch() {
    let fixture = resume_fixture("0.150.1");
    let launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let executor = fake_executor(
        &fixture,
        launcher.clone(),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let before = fixture.seed.sessions().load().unwrap();
    assert_eq!(
        executor
            .commit(
                &SESSION_ID.to_string(),
                fixture.source_revision.get() + 1,
                &Cancellation::default(),
            )
            .unwrap_err()
            .code,
        "conflict"
    );
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        executor
            .preview(&SESSION_ID.to_string(), &cancelled)
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert!(fixture.seed.sessions().load().unwrap() == before);
    assert_eq!(launcher.lock().unwrap().calls, 0);

    let unsupported = resume_fixture("0.150.0");
    let unsupported_launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let executor = fake_executor(
        &unsupported,
        unsupported_launcher.clone(),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    assert_eq!(
        executor
            .preview(&SESSION_ID.to_string(), &Cancellation::default())
            .unwrap_err()
            .code,
        "unavailable"
    );
    assert_eq!(unsupported_launcher.lock().unwrap().calls, 0);

    let archived = resume_fixture("0.150.1");
    archived
        .seed
        .sessions()
        .mutate_session(
            SESSION_ID,
            archived.source_revision,
            SessionMutation::Archive { at: 2 },
            2,
        )
        .unwrap();
    let archived_launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let executor = fake_executor(
        &archived,
        archived_launcher.clone(),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    assert_eq!(
        executor
            .preview(&SESSION_ID.to_string(), &Cancellation::default())
            .unwrap_err()
            .code,
        "validation"
    );
    assert_eq!(archived_launcher.lock().unwrap().calls, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_resume_real_successor_host_survives_executor_return() {
    let fixture = resume_fixture("0.150.1");
    let before_host = fs::read(&fixture.source_host_path).unwrap();
    let before_provider = fs::read(&fixture.provider_metadata_path).unwrap();
    let launcher = Arc::new(RealHostLauncher::default());
    let service = LocalCommandService::with_adapters(
        fixture
            .seed
            .paths()
            .with_codex_conversation_root(fixture.codex_sessions.clone()),
        launcher.clone(),
        Arc::new(FakeController {
            state: Arc::new(Mutex::new(FakeControllerState::default())),
        }),
        Arc::new(FixedClock(10)),
        Arc::new(FixedIds),
    );
    let executor = LocalResumeExecutor::with_service(service);
    let review = executor
        .preview(&SESSION_ID.to_string(), &Cancellation::default())
        .unwrap();
    let result = executor
        .commit(
            &review.source_session_id,
            review.source_revision,
            &Cancellation::default(),
        )
        .unwrap();

    let endpoint = LocalEndpoint::for_config_root(&fixture.seed.config_root, LAUNCH_SESSION_ID);
    let cancellation = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local_read_only(LAUNCH_SESSION_ID, [8; 32]),
        &cancellation,
    )
    .await
    .unwrap();
    let state = client.get_state(&cancellation).await.unwrap();
    assert_eq!(
        state.lifecycle,
        termirust_host_protocol::wire::Lifecycle::Ready as i32
    );
    let mut saw_canary = false;
    for _ in 0..40 {
        let outputs = client
            .attach(
                termirust_domain::OutputSequence::ZERO,
                100,
                30,
                &cancellation,
            )
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
    assert_eq!(result.lifecycle, "live");
    assert_eq!(result.permission_policy, "read_only");
    assert_eq!(fs::read(&fixture.source_host_path).unwrap(), before_host);
    assert_eq!(
        fs::read(&fixture.provider_metadata_path).unwrap(),
        before_provider
    );
    launcher.shutdown();
}

fn fake_executor(
    fixture: &ResumeFixture,
    launcher: Arc<Mutex<FakeLauncherState>>,
    controller: Arc<Mutex<FakeControllerState>>,
) -> LocalResumeExecutor {
    LocalResumeExecutor::with_service(LocalCommandService::with_adapters(
        fixture
            .seed
            .paths()
            .with_codex_conversation_root(fixture.codex_sessions.clone()),
        Arc::new(FakeLauncher { state: launcher }),
        Arc::new(FakeController { state: controller }),
        Arc::new(FixedClock(10)),
        Arc::new(FixedIds),
    ))
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
    let codex_sessions = seed.temp.path().join("codex-home/sessions");
    let provider_metadata_path = write_codex_metadata(&codex_sessions, &seed.project_root, version);
    ResumeFixture {
        source_host_path: source_dir.join("host.json"),
        seed,
        codex_sessions,
        provider_metadata_path,
        source_revision: source.revision,
    }
}

fn write_codex_metadata(root: &Path, cwd: &Path, version: &str) -> std::path::PathBuf {
    let directory = root.join("2026/08/31");
    fs::create_dir_all(&directory).unwrap();
    let value = serde_json::json!({
        "ordinal": 0,
        "timestamp": "2026-08-31T00:00:00Z",
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
