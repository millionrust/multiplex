#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_controller_listener::{
    AuthoritySnapshot, ControllerAuthorityProvider, ControllerClientChannel, ControllerCommand,
    ControllerResponse, ControllerSessionOrigin, DesktopPaneBridgeServer, DesktopPaneRegistration,
    DesktopPaneRegistry, DesktopPaneTransport, HostBackendFactory, ListenerError,
    ListenerErrorCode, SystemHandshakeEntropy, serve_authenticated_stdio_stream,
};
use termirust_controller_security::{
    CapabilitySet, ControllerCapability as SecurityCapability, HostStaticPublicKey,
    StaticPrivateKey, device_public_key_from_private, host_public_key_from_private,
};
use termirust_domain::{
    ActivityAggregate, AddProject, CommandId, ControllerCapabilities,
    ControllerCapability as DomainCapability, ControllerDeviceAuthority, ControllerDeviceId,
    ControllerProtocolRange, DevicePublicKey, HostIdentityGeneration, HostIdentityPublic,
    HostIdentitySecretRef, HostIdentityState, HostInstanceId, HostPublicKey, HostedSession,
    HostedSessionId, HostedSessionState, OccupantGeneration, OutputSequence, PairedDeviceRecord,
    PairedDeviceStatus, PairingOfferId, PositionKey, ProjectId, Revision, RuntimeCapability,
    RuntimeCapabilitySet, RuntimeDetectionResult, RuntimeDetectionStatus, RuntimeId, SessionTitle,
    TitleSource,
};
use termirust_host_protocol::wire;
use termirust_session_host::process_observation::fingerprint_executable;
use termirust_session_host::{LaunchDescriptor, StopDeadlines};
use termirust_store::{JournalLimits, ProjectRepository, SessionRepository, read_host_metadata};
use tokio::io::DuplexStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const SESSION_GENERATION: u64 = 7;
const REVOCATION_EPOCH: u64 = 2;

struct HostProcess {
    child: Child,
}

impl HostProcess {
    fn spawn(binary: &Path, descriptor: &LaunchDescriptor) -> Self {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Session Host process should start");
        serde_json::to_writer(child.stdin.as_mut().unwrap(), descriptor).unwrap();
        child.stdin.take().unwrap().flush().unwrap();

        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        let event: serde_json::Value = serde_json::from_str(&ready).unwrap_or_else(|error| {
            let status = child.try_wait().expect("Session Host status should be readable");
            let diagnostic = if status.is_some() {
                let mut diagnostic = String::new();
                child
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut diagnostic)
                    .unwrap();
                diagnostic
            } else {
                "<host still running>".to_owned()
            };
            panic!(
                "Session Host readiness must be content-free JSON: {error}; status={status:?}; readiness={ready:?}; diagnostic={diagnostic:?}"
            )
        });
        assert_eq!(event["lifecycle"], "ready");
        assert_eq!(event["code"], "host_ready");
        Self { child }
    }

    async fn wait_for_exit(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        while self.child.try_wait().unwrap().is_none() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            self.child.try_wait().unwrap().is_some(),
            "Session Host process did not exit"
        );
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[derive(Clone)]
struct MutableAuthority {
    authority: Arc<Mutex<ControllerDeviceAuthority>>,
    host_private: StaticPrivateKey,
}

impl ControllerAuthorityProvider for MutableAuthority {
    fn snapshot(&self) -> Result<AuthoritySnapshot, ListenerError> {
        Ok(AuthoritySnapshot {
            authority: self.authority.lock().unwrap().clone(),
            host_private: self.host_private.clone(),
        })
    }
}

struct GoldenController {
    channel: ControllerClientChannel<DuplexStream>,
    server: JoinHandle<Result<(), ListenerError>>,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn deadline() -> u64 {
    now_millis().saturating_add(10_000)
}

fn common_environment(home: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".to_owned(), home.to_string_lossy().into_owned()),
        ("LANG".to_owned(), "C".to_owned()),
        ("LC_ALL".to_owned(), "C".to_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("TERM".to_owned(), "xterm-256color".to_owned()),
    ])
}

fn descriptor(
    fixture: &Path,
    runtime_parent: &Path,
    sessions: &SessionRepository,
    session_id: HostedSessionId,
    executable: PathBuf,
    arguments: Vec<String>,
    runtime_detection: Option<RuntimeDetectionResult>,
) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: runtime_parent.join(session_id.to_string()),
        session_dir: sessions.session_data_path(session_id),
        executable,
        runtime_detection,
        arguments,
        environment: common_environment(fixture),
        cwd: Some(fixture.to_path_buf()),
        columns: 100,
        rows: 30,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines {
            interrupt_millis: 100,
            terminate_millis: 500,
            total_millis: 2_000,
        },
    }
}

fn insert_session(
    sessions: &SessionRepository,
    project_id: ProjectId,
    session_id: HostedSessionId,
    title: &str,
) {
    let expected = sessions.load().unwrap().revision;
    sessions
        .create_session(
            HostedSession {
                id: session_id,
                project_id,
                group_id: None,
                preset_id: None,
                title: SessionTitle::new(title).unwrap(),
                title_source: TitleSource::Manual,
                lifecycle: HostedSessionState::Live,
                activity: ActivityAggregate::default(),
                pinned: false,
                position: PositionKey::FIRST,
                last_output_sequence: OutputSequence::ZERO,
                read_through_sequence: OutputSequence::ZERO,
                unread_sequence: None,
                archived_at: None,
                created_at: 1,
                updated_at: 1,
                revision: Revision::ZERO,
            },
            expected,
        )
        .unwrap();
}

async fn connect_controller(
    authority: Arc<MutableAuthority>,
    backends: Arc<HostBackendFactory>,
    host_private: &StaticPrivateKey,
    device_private: &StaticPrivateKey,
) -> GoldenController {
    let (client, mut server_stream) = tokio::io::duplex(512 * 1024);
    let server_authority: Arc<dyn ControllerAuthorityProvider> = authority;
    let server = tokio::spawn(async move {
        serve_authenticated_stdio_stream(
            &mut server_stream,
            server_authority,
            backends,
            CancellationToken::new(),
        )
        .await
    });
    let requested = CapabilitySet::default()
        .with(SecurityCapability::ObserveSessions)
        .with(SecurityCapability::AttachOutput)
        .with(SecurityCapability::SendInput)
        .with(SecurityCapability::Resize)
        .with(SecurityCapability::RespondToApproval);
    let channel = ControllerClientChannel::connect(
        client,
        1,
        REVOCATION_EPOCH,
        SESSION_GENERATION,
        HostStaticPublicKey(host_public_key_from_private(host_private).0),
        device_private.clone(),
        requested,
        &mut SystemHandshakeEntropy,
    )
    .await
    .unwrap();
    GoldenController { channel, server }
}

async fn close_controller(controller: GoldenController) {
    let GoldenController { channel, server } = controller;
    drop(channel);
    let result = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("Controller server should notice client close")
        .unwrap();
    assert!(
        result.is_ok()
            || result
                .as_ref()
                .is_err_and(|error| error.code == ListenerErrorCode::Io)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_desktop_pane_round_trips_through_authenticated_controller() {
    let fixture = tempfile::tempdir().unwrap();
    let metadata_root = fixture.path().join("metadata");
    let sessions =
        SessionRepository::open(&metadata_root, fixture.path().join("session-data")).unwrap();
    let projects = ProjectRepository::open(&metadata_root).unwrap();
    let registry = DesktopPaneRegistry::default();
    let session_id = HostedSessionId::new();
    let received_input = Arc::new(Mutex::new(Vec::new()));
    let received_by_transport = received_input.clone();
    registry.register(DesktopPaneRegistration {
        session_id,
        title: "Live local terminal".to_owned(),
        runtime: "local_shell".to_owned(),
        columns: 100,
        rows: 30,
        transport: DesktopPaneTransport::new(move |bytes| {
            received_by_transport.lock().unwrap().extend(bytes);
            true
        }),
    });
    let expected_history = (0..80)
        .flat_map(|index| format!("VISIBLE-DESKTOP-HISTORY-{index}\r\n").into_bytes())
        .collect::<Vec<_>>();
    for index in 0..80 {
        registry.append_output(
            session_id,
            format!("VISIBLE-DESKTOP-HISTORY-{index}\r\n").as_bytes(),
            Vec::new,
        );
    }
    let bridge =
        DesktopPaneBridgeServer::start(fixture.path().join("desktop-bridge"), registry).unwrap();

    let host_private = StaticPrivateKey::from_fixture_bytes([31; 32]);
    let device_private = StaticPrivateKey::from_fixture_bytes([32; 32]);
    let capabilities = ControllerCapabilities::default()
        .with(DomainCapability::ObserveSessions)
        .with(DomainCapability::AttachOutput)
        .with(DomainCapability::SendInput);
    let authority_state = Arc::new(Mutex::new(ControllerDeviceAuthority {
        identity: Some(HostIdentityPublic::new(
            HostIdentityGeneration::INITIAL,
            HostPublicKey(host_public_key_from_private(&host_private).0),
        )),
        secret_ref: Some(HostIdentitySecretRef::new("identity:desktop-pane-test").unwrap()),
        state: HostIdentityState::Ready,
        revocation_epoch: REVOCATION_EPOCH,
        session_generation: SESSION_GENERATION,
        devices: vec![PairedDeviceRecord {
            device_id: ControllerDeviceId::new(),
            public_key: DevicePublicKey(device_public_key_from_private(&device_private).0),
            display_name: "Mobile controller".to_owned(),
            capabilities,
            protocol_range: ControllerProtocolRange::V1,
            created_at: 1,
            last_seen_at: None,
            revocation_epoch: REVOCATION_EPOCH,
            identity_generation: HostIdentityGeneration::INITIAL,
            status: PairedDeviceStatus::Online,
            source_offer_id: PairingOfferId::new(),
        }],
        offers: Vec::new(),
        attempts: Default::default(),
    }));
    let authority = Arc::new(MutableAuthority {
        authority: authority_state,
        host_private: host_private.clone(),
    });
    let backends = Arc::new(
        HostBackendFactory::new(sessions, projects, fixture.path().join("runtime"))
            .with_desktop_pane_bridge(Some(bridge.endpoint())),
    );
    let mut controller =
        connect_controller(authority, backends, &host_private, &device_private).await;

    let list_id = controller
        .channel
        .send(
            ControllerCommand::ListSessions {
                offset: 0,
                limit: 100,
                expected_revision: None,
            },
            deadline(),
        )
        .await
        .unwrap();
    let generation = match controller.channel.read_response().await.unwrap() {
        ControllerResponse::Sessions {
            command_id,
            sessions,
            ..
        } if command_id == list_id => {
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].session_id, session_id);
            sessions[0].occupant_generation.unwrap()
        }
        response => panic!("unexpected session list response: {response:?}"),
    };
    let output = attach_from(
        &mut controller.channel,
        session_id,
        generation,
        OutputSequence::ZERO,
    )
    .await;
    assert_eq!(
        output
            .into_iter()
            .flat_map(|(_, bytes)| bytes)
            .collect::<Vec<_>>(),
        expected_history
    );
    expect_completed(
        &mut controller.channel,
        ControllerCommand::AcquireWriter {
            session_id,
            occupant_generation: generation,
        },
    )
    .await;
    expect_completed(
        &mut controller.channel,
        ControllerCommand::Input {
            session_id,
            occupant_generation: generation,
            bytes: b"PHONE-INPUT\n".to_vec(),
        },
    )
    .await;
    assert_eq!(*received_input.lock().unwrap(), b"PHONE-INPUT\n");

    close_controller(controller).await;
}

async fn expect_completed(
    channel: &mut ControllerClientChannel<DuplexStream>,
    command: ControllerCommand,
) {
    let command_id = channel.send(command, deadline()).await.unwrap();
    loop {
        match channel.read_response().await.unwrap() {
            ControllerResponse::Completed {
                command_id: actual,
                applied,
            } if actual == command_id => {
                assert!(applied);
                return;
            }
            ControllerResponse::Output { .. } => {}
            response => panic!("unexpected Controller response: {response:?}"),
        }
    }
}

async fn attach_from(
    channel: &mut ControllerClientChannel<DuplexStream>,
    session_id: HostedSessionId,
    generation: OccupantGeneration,
    from: OutputSequence,
) -> Vec<(OutputSequence, Vec<u8>)> {
    let command_id = channel
        .send(
            ControllerCommand::Attach {
                session_id,
                occupant_generation: generation,
                from_sequence: from,
                columns: 100,
                rows: 30,
            },
            deadline(),
        )
        .await
        .unwrap();
    let mut through = None;
    let mut outputs = Vec::new();
    loop {
        match channel.read_response().await.unwrap() {
            ControllerResponse::Attached {
                command_id: actual,
                replay_through_sequence,
                has_writer_lease,
                ..
            } if actual == command_id => {
                assert!(!has_writer_lease);
                through = Some(replay_through_sequence);
            }
            ControllerResponse::Output {
                session_id: actual,
                sequence,
                bytes,
            } if actual == session_id => outputs.push((sequence, bytes)),
            ControllerResponse::Snapshot { .. } => {
                panic!("small golden run should not require a compacted snapshot")
            }
            response => panic!("unexpected Controller attach response: {response:?}"),
        }
        if let Some(through) = through
            && outputs
                .last()
                .map(|(sequence, _)| *sequence)
                .unwrap_or(from)
                >= through
        {
            return outputs;
        }
    }
}

async fn input_and_collect(
    channel: &mut ControllerClientChannel<DuplexStream>,
    session_id: HostedSessionId,
    generation: OccupantGeneration,
    marker: &[u8],
    expected: &[u8],
) -> Vec<(OutputSequence, Vec<u8>)> {
    let command_id = channel
        .send(
            ControllerCommand::Input {
                session_id,
                occupant_generation: generation,
                bytes: marker.to_vec(),
            },
            deadline(),
        )
        .await
        .unwrap();
    let mut completed = false;
    let mut outputs = Vec::new();
    let read = async {
        loop {
            match channel.read_response().await.unwrap() {
                ControllerResponse::Completed {
                    command_id: actual,
                    applied,
                } if actual == command_id => {
                    assert!(applied);
                    completed = true;
                }
                ControllerResponse::Output {
                    session_id: actual,
                    sequence,
                    bytes,
                } if actual == session_id => outputs.push((sequence, bytes)),
                response => panic!("unexpected Controller input response: {response:?}"),
            }
            let bytes = outputs
                .iter()
                .flat_map(|(_, bytes)| bytes.iter().copied())
                .collect::<Vec<_>>();
            if completed
                && bytes
                    .windows(expected.len())
                    .any(|window| window == expected)
            {
                return outputs;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .expect("Controller should observe input echo")
}

async fn direct_input_from(
    endpoint: LocalEndpoint,
    session_id: HostedSessionId,
    from: OutputSequence,
    input: &[u8],
    expected: &[u8],
) -> Vec<(OutputSequence, Vec<u8>)> {
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [41; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(
        client
            .input(CommandId::new(), input.to_vec(), &cancel)
            .await
            .unwrap()
    );
    let mut outputs = Vec::new();
    for _ in 0..100 {
        let cursor = outputs
            .last()
            .map(|(sequence, _)| *sequence)
            .unwrap_or(from);
        outputs.extend(
            client
                .attach(cursor, 100, 30, &cancel)
                .await
                .unwrap()
                .into_iter()
                .map(|output| (output.sequence, output.bytes)),
        );
        let bytes = outputs
            .iter()
            .flat_map(|(_, bytes)| bytes.iter().copied())
            .collect::<Vec<_>>();
        if bytes
            .windows(expected.len())
            .any(|window| window == expected)
        {
            client.disconnect();
            return outputs;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("direct Host client did not observe expected output")
}

async fn prove_terminal_session(
    endpoint: LocalEndpoint,
    session_id: HostedSessionId,
    input: &[u8],
    expected: &[u8],
) {
    let outputs =
        direct_input_from(endpoint, session_id, OutputSequence::ZERO, input, expected).await;
    let bytes = outputs
        .iter()
        .flat_map(|(_, bytes)| bytes.iter().copied())
        .collect::<Vec<_>>();
    assert!(
        bytes
            .windows(expected.len())
            .any(|window| window == expected)
    );
}

async fn stop_host(endpoint: LocalEndpoint, session_id: HostedSessionId) {
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [77; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(
        client
            .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundled_desktop_host_controller_golden_run() {
    let Ok(host_binary) = std::env::var("TERMIRUST_N02_HOST_BIN") else {
        eprintln!("SKIPPED N02 live golden run: TERMIRUST_N02_HOST_BIN is not set");
        return;
    };
    let ssh_port: u16 = std::env::var("TERMIRUST_N02_SSH_PORT")
        .expect("N02 SSH port is required")
        .parse()
        .expect("N02 SSH port should be numeric");
    let ssh_key =
        PathBuf::from(std::env::var_os("TERMIRUST_N02_SSH_KEY").expect("N02 SSH key is required"));
    let fixture = tempfile::Builder::new()
        .prefix("tr-n02-")
        .tempdir_in("/tmp")
        .unwrap();
    let project_root = fixture.path().join("project");
    let metadata_root = fixture.path().join("metadata");
    let session_data_root = fixture.path().join("sessions");
    let runtime_parent = fixture.path().join("runtime");
    fs::create_dir_all(&project_root).unwrap();
    fs::create_dir_all(&session_data_root).unwrap();
    fs::create_dir_all(&runtime_parent).unwrap();

    let project_id = ProjectId::new();
    ProjectRepository::open(&metadata_root)
        .unwrap()
        .add_project(AddProject {
            id: project_id,
            root: project_root,
            display_name: Some("N02 golden project".to_owned()),
            expected: Revision::ZERO,
        })
        .unwrap();
    let sessions = SessionRepository::open(&metadata_root, &session_data_root).unwrap();
    let local_id = HostedSessionId::new();
    let ssh_id = HostedSessionId::new();
    let agent_id = HostedSessionId::new();

    let agent_executable = fixture.path().join("deterministic-agent");
    fs::write(
        &agent_executable,
        "#!/bin/sh\ntrap 'exit 0' INT TERM\nprintf 'AGENT-READY\\n'\nwhile IFS= read -r line; do printf 'AGENT-OUT:%s\\n' \"$line\"; done\n",
    )
    .unwrap();
    fs::set_permissions(&agent_executable, fs::Permissions::from_mode(0o700)).unwrap();
    let agent_detection = RuntimeDetectionResult {
        runtime_id: RuntimeId::new("golden-agent").unwrap(),
        descriptor_version: 1,
        status: RuntimeDetectionStatus::Available,
        fingerprint: Some(fingerprint_executable(&agent_executable).unwrap()),
        safe_version: Some("1.0.0".to_owned()),
        capabilities: RuntimeCapabilitySet::new([
            RuntimeCapability::InteractivePty,
            RuntimeCapability::Cancellation,
        ]),
        diagnostic_code: None,
    };

    let local = descriptor(
        fixture.path(),
        &runtime_parent,
        &sessions,
        local_id,
        PathBuf::from("/bin/sh"),
        vec![
            "-c".to_owned(),
            "trap 'exit 0' INT TERM; printf 'LOCAL-READY\\n'; while IFS= read -r line; do printf 'LOCAL-OUT:%s\\n' \"$line\"; done".to_owned(),
        ],
        None,
    );
    let ssh = descriptor(
        fixture.path(),
        &runtime_parent,
        &sessions,
        ssh_id,
        PathBuf::from("/usr/bin/ssh"),
        vec![
            "-tt".to_owned(),
            "-i".to_owned(),
            ssh_key.to_string_lossy().into_owned(),
            "-p".to_owned(),
            ssh_port.to_string(),
            "-o".to_owned(),
            "BatchMode=yes".to_owned(),
            "-o".to_owned(),
            "StrictHostKeyChecking=no".to_owned(),
            "-o".to_owned(),
            "UserKnownHostsFile=/dev/null".to_owned(),
            "-o".to_owned(),
            "LogLevel=ERROR".to_owned(),
            "termirust@127.0.0.1".to_owned(),
            "printf 'SSH-READY\\n'; while IFS= read -r line; do printf 'SSH-OUT:%s\\n' \"$line\"; done".to_owned(),
        ],
        None,
    );
    let agent = descriptor(
        fixture.path(),
        &runtime_parent,
        &sessions,
        agent_id,
        agent_executable,
        Vec::new(),
        Some(agent_detection),
    );

    let host_binary = PathBuf::from(host_binary);
    let mut local_process = HostProcess::spawn(&host_binary, &local);
    let mut ssh_process = HostProcess::spawn(&host_binary, &ssh);
    let mut agent_process = HostProcess::spawn(&host_binary, &agent);
    insert_session(&sessions, project_id, local_id, "Local PTY");
    insert_session(&sessions, project_id, ssh_id, "Docker SSH");
    insert_session(&sessions, project_id, agent_id, "Deterministic agent");

    prove_terminal_session(
        LocalEndpoint::new(&local.runtime_root, local_id),
        local_id,
        b"local-proof\n",
        b"LOCAL-OUT:local-proof",
    )
    .await;
    prove_terminal_session(
        LocalEndpoint::new(&ssh.runtime_root, ssh_id),
        ssh_id,
        b"ssh-proof\n",
        b"SSH-OUT:ssh-proof",
    )
    .await;

    let host_private = StaticPrivateKey::from_fixture_bytes([11; 32]);
    let device_private = StaticPrivateKey::from_fixture_bytes([12; 32]);
    let device_id = ControllerDeviceId::new();
    let capabilities = ControllerCapabilities::default()
        .with(DomainCapability::ObserveSessions)
        .with(DomainCapability::AttachOutput)
        .with(DomainCapability::SendInput)
        .with(DomainCapability::Resize)
        .with(DomainCapability::RespondToApproval);
    let authority_state = Arc::new(Mutex::new(ControllerDeviceAuthority {
        identity: Some(HostIdentityPublic::new(
            HostIdentityGeneration::INITIAL,
            HostPublicKey(host_public_key_from_private(&host_private).0),
        )),
        secret_ref: Some(HostIdentitySecretRef::new("identity:n02-golden").unwrap()),
        state: HostIdentityState::Ready,
        revocation_epoch: REVOCATION_EPOCH,
        session_generation: SESSION_GENERATION,
        devices: vec![PairedDeviceRecord {
            device_id,
            public_key: DevicePublicKey(device_public_key_from_private(&device_private).0),
            display_name: "N02 Controller".to_owned(),
            capabilities,
            protocol_range: ControllerProtocolRange::V1,
            created_at: 1,
            last_seen_at: None,
            revocation_epoch: REVOCATION_EPOCH,
            identity_generation: HostIdentityGeneration::INITIAL,
            status: PairedDeviceStatus::Online,
            source_offer_id: PairingOfferId::new(),
        }],
        offers: Vec::new(),
        attempts: Default::default(),
    }));
    let authority = Arc::new(MutableAuthority {
        authority: authority_state.clone(),
        host_private: host_private.clone(),
    });
    let backends = Arc::new(HostBackendFactory::new(
        sessions.clone(),
        ProjectRepository::open(&metadata_root).unwrap(),
        &runtime_parent,
    ));

    let mut first = connect_controller(
        authority.clone(),
        backends.clone(),
        &host_private,
        &device_private,
    )
    .await;
    let list_id = first
        .channel
        .send(
            ControllerCommand::ListSessions {
                offset: 0,
                limit: 100,
                expected_revision: None,
            },
            deadline(),
        )
        .await
        .unwrap();
    let summaries = match first.channel.read_response().await.unwrap() {
        ControllerResponse::Sessions {
            command_id,
            sessions,
            ..
        } if command_id == list_id => sessions,
        response => panic!("unexpected session list response: {response:?}"),
    };
    assert_eq!(summaries.len(), 3);
    assert_eq!(
        summaries
            .iter()
            .filter(|session| session.origin == ControllerSessionOrigin::Terminal)
            .count(),
        2
    );
    assert!(
        summaries
            .iter()
            .all(|session| session.occupant_generation.is_some()),
        "every live terminal or agent must expose its authoritative occupant generation"
    );
    let agent_summary = summaries
        .iter()
        .find(|session| session.session_id == agent_id)
        .unwrap();
    assert_eq!(agent_summary.origin, ControllerSessionOrigin::ManagedAgent);
    assert_eq!(agent_summary.runtime.as_deref(), Some("golden-agent"));
    let generation = agent_summary.occupant_generation.unwrap();

    let mut ordered = attach_from(
        &mut first.channel,
        agent_id,
        generation,
        OutputSequence::ZERO,
    )
    .await;
    expect_completed(
        &mut first.channel,
        ControllerCommand::AcquireWriter {
            session_id: agent_id,
            occupant_generation: generation,
        },
    )
    .await;
    ordered.extend(
        input_and_collect(
            &mut first.channel,
            agent_id,
            generation,
            b"first-controller\n",
            b"AGENT-OUT:first-controller",
        )
        .await,
    );
    expect_completed(
        &mut first.channel,
        ControllerCommand::ReleaseWriter {
            session_id: agent_id,
            occupant_generation: generation,
        },
    )
    .await;
    let watermark = ordered.last().unwrap().0;
    close_controller(first).await;

    let offline = direct_input_from(
        LocalEndpoint::new(&agent.runtime_root, agent_id),
        agent_id,
        watermark,
        b"between-controllers\n",
        b"AGENT-OUT:between-controllers",
    )
    .await;
    let mut second =
        connect_controller(authority.clone(), backends, &host_private, &device_private).await;
    let replay = attach_from(&mut second.channel, agent_id, generation, watermark).await;
    assert_eq!(
        replay, offline,
        "Controller replay must match the exact watermark tail"
    );
    ordered.extend(replay);
    expect_completed(
        &mut second.channel,
        ControllerCommand::AcquireWriter {
            session_id: agent_id,
            occupant_generation: generation,
        },
    )
    .await;
    ordered.extend(
        input_and_collect(
            &mut second.channel,
            agent_id,
            generation,
            b"second-controller\n",
            b"AGENT-OUT:second-controller",
        )
        .await,
    );
    assert!(
        ordered
            .windows(2)
            .all(|pair| { pair[1].0.get() == pair[0].0.get().saturating_add(1) })
    );

    authority_state
        .lock()
        .unwrap()
        .revoke_device(device_id)
        .unwrap();
    let server_result = tokio::time::timeout(Duration::from_secs(2), second.server)
        .await
        .expect("revocation should close the Controller server")
        .unwrap()
        .unwrap_err();
    assert_eq!(server_result.code, ListenerErrorCode::AuthenticationFailed);
    assert!(
        second
            .channel
            .send(
                ControllerCommand::Input {
                    session_id: agent_id,
                    occupant_generation: generation,
                    bytes: b"revoked-input\n".to_vec(),
                },
                deadline(),
            )
            .await
            .is_err()
    );

    stop_host(LocalEndpoint::new(&local.runtime_root, local_id), local_id).await;
    stop_host(LocalEndpoint::new(&ssh.runtime_root, ssh_id), ssh_id).await;
    stop_host(LocalEndpoint::new(&agent.runtime_root, agent_id), agent_id).await;
    local_process.wait_for_exit().await;
    ssh_process.wait_for_exit().await;
    agent_process.wait_for_exit().await;
    for descriptor in [&local, &ssh, &agent] {
        assert_eq!(
            read_host_metadata(&descriptor.session_dir)
                .unwrap()
                .lifecycle,
            termirust_domain::HostLifecycle::Exited
        );
    }
}
