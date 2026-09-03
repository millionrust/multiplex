#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_controller_listener::{
    ControllerDeviceService, InterfaceProvider as _, ListenerControlCommand,
    ListenerLaunchDescriptor, ListenerProcessEvent, NoControllerChannels, ProcessPairingDecision,
    SystemInterfaceProvider, run_listener_worker,
};
use termirust_controller_security::{StaticPrivateKey, host_public_key_from_private};
use termirust_domain::{
    ActivityAggregate, AddProject, AddressFamily, CommandId, ControllerCapabilities,
    ControllerCapability, ControllerDeviceAuthority, ControllerDeviceId, ControllerListenPolicy,
    ControllerPort, DiscoveryPolicy, HostIdentityGeneration, HostIdentityPublic,
    HostIdentitySecretRef, HostIdentityState, HostInstanceId, HostPublicKey, HostedSession,
    HostedSessionId, HostedSessionState, NetworkInterfaceCandidate, NetworkInterfaceKind,
    OutputSequence, PairingOfferId, PositionKey, ProjectId, Revision, SessionTitle, TitleSource,
};
use termirust_host_protocol::wire;
use termirust_session_host::{LaunchDescriptor, StopDeadlines};
use termirust_store::{
    ControllerDeviceRepository, ControllerNetworkRepository, JournalLimits, ProjectRepository,
    SessionRepository,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const SESSION_GENERATION: u64 = 1;
const REVOCATION_EPOCH: u64 = 1;
const MAX_CONTROL_BYTES: u64 = 4 * 1024;

#[derive(Serialize)]
struct FixtureConfig {
    schema_version: u16,
    fixture_id: Uuid,
    offer_text: String,
    controller_address: String,
    controller_port: u16,
    control_address: String,
    control_port: u16,
    control_token: String,
    session_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRequest {
    token: String,
    command: String,
}

#[derive(Serialize)]
struct ControlResponse<'a> {
    ok: bool,
    value: Option<&'a str>,
}

#[derive(Default)]
struct FixtureStatus {
    sas: Option<String>,
    offer_id: Option<PairingOfferId>,
    device_id: Option<ControllerDeviceId>,
    revoked: bool,
}

struct HostProcess {
    child: Child,
    endpoint: LocalEndpoint,
    session_id: HostedSessionId,
}

impl HostProcess {
    fn spawn(binary: &Path, descriptor: &LaunchDescriptor) -> Result<Self, String> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| "host_spawn")?;
        serde_json::to_writer(child.stdin.as_mut().ok_or("host_stdin")?, descriptor)
            .map_err(|_| "host_descriptor")?;
        child
            .stdin
            .take()
            .ok_or("host_stdin")?
            .flush()
            .map_err(|_| "host_descriptor")?;

        let mut ready = String::new();
        BufReader::new(child.stdout.take().ok_or("host_stdout")?)
            .read_line(&mut ready)
            .map_err(|_| "host_readiness")?;
        let event: serde_json::Value = serde_json::from_str(&ready).map_err(|_| {
            let diagnostic = if child.try_wait().ok().flatten().is_some() {
                let mut value = String::new();
                if let Some(stderr) = child.stderr.take() {
                    let _ = stderr.take(512).read_to_string(&mut value);
                }
                value
            } else {
                "host_still_running".to_owned()
            };
            format!("host_readiness:{diagnostic}")
        })?;
        if event["lifecycle"] != "ready" || event["code"] != "host_ready" {
            return Err("host_readiness".to_owned());
        }
        Ok(Self {
            child,
            endpoint: LocalEndpoint::new(&descriptor.runtime_root, descriptor.session_id),
            session_id: descriptor.session_id,
        })
    }

    fn stop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            let endpoint = self.endpoint.clone();
            let session_id = self.session_id;
            runtime.block_on(async move {
                let cancel = CancellationToken::new();
                if let Ok(mut client) = HostClient::connect(
                    endpoint,
                    ConnectOptions::local(session_id, [91; 32]),
                    &cancel,
                )
                .await
                {
                    let _ = client
                        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
                        .await;
                }
            });
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn main() {
    if let Err(code) = run() {
        eprintln!("mobile Controller fixture failed: {code}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (root, session_host_binary, config_path) = arguments()?;
    prepare_root(&root)?;
    let controller_root = root.join("controller");
    let project_root = root.join("projects");
    let session_data_root = root.join("sessions");
    let runtime_parent = root.join("runtime");
    for path in [
        &controller_root,
        &project_root,
        &session_data_root,
        &runtime_parent,
    ] {
        fs::create_dir_all(path).map_err(|_| "fixture_directories")?;
    }

    let interface = select_interface()?;
    let host_private = StaticPrivateKey::from_fixture_bytes([31; 32]);
    initialize_authority(&controller_root, &host_private)?;
    let policy = ControllerListenPolicy {
        enabled: true,
        interface_id: Some(interface.id.clone()),
        address_family: Some(interface.address_family),
        selected_address: Some(interface.address),
        port: Some(ControllerPort::generated(49_152).map_err(|_| "controller_port")?),
        discovery: DiscoveryPolicy::Off,
    };
    let network =
        ControllerNetworkRepository::open(&controller_root).map_err(|_| "controller_network")?;
    let network_snapshot = network.load().map_err(|_| "controller_network")?;
    let saved_network = network
        .save(network_snapshot.revision, policy.clone())
        .map_err(|_| "controller_network")?;

    let workspace_root = root.join("workspace");
    fs::create_dir_all(&workspace_root).map_err(|_| "project_directory")?;
    let sessions =
        SessionRepository::open(&project_root, &session_data_root).map_err(|_| "session_store")?;
    let project_id = ProjectId::new();
    ProjectRepository::open(&project_root)
        .map_err(|_| "project_store")?
        .add_project(AddProject {
            id: project_id,
            root: workspace_root,
            display_name: Some("Mobile Controller fixture".to_owned()),
            expected: Revision::ZERO,
        })
        .map_err(|_| "project_store")?;
    let session_id = HostedSessionId::new();
    let descriptor = host_descriptor(&root, &runtime_parent, &sessions, session_id);
    let mut host = HostProcess::spawn(&session_host_binary, &descriptor)?;
    insert_session(&sessions, project_id, session_id)?;

    let launch = ListenerLaunchDescriptor::new(
        controller_root.clone(),
        project_root,
        session_data_root,
        runtime_parent,
        saved_network.revision,
        saved_network.policy,
        &host_private,
    )
    .map_err(|_| "listener_descriptor")?;
    let (mut listener_control, listener_input) =
        UnixStream::pair().map_err(|_| "listener_control")?;
    let (listener_output, event_input) = UnixStream::pair().map_err(|_| "listener_events")?;
    let listener_thread =
        thread::spawn(move || run_listener_worker(BufReader::new(listener_input), listener_output));
    launch
        .write(&mut listener_control)
        .map_err(|_| "listener_descriptor")?;
    let (event_tx, event_rx) = mpsc::sync_channel(32);
    let event_thread = thread::spawn(move || {
        let mut reader = BufReader::new(event_input);
        loop {
            match ListenerProcessEvent::read(&mut reader) {
                Ok(Some(event)) => {
                    if event_tx.send(event).is_err() {
                        return;
                    }
                }
                _ => return,
            }
        }
    });
    let ready_port = match event_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(ListenerProcessEvent::Ready { port, .. }) => port,
        _ => return Err("listener_readiness".to_owned()),
    };

    ListenerControlCommand::begin_pairing()
        .write(&mut listener_control)
        .map_err(|_| "pairing_begin")?;
    let (offer_id, offer_text) = match event_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(ListenerProcessEvent::PairingOffer {
            offer_id,
            offer_text,
            ..
        }) => (offer_id, offer_text),
        _ => return Err("pairing_offer".to_owned()),
    };

    let control_listener =
        TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0))).map_err(|_| "control_bind")?;
    let control_port = control_listener
        .local_addr()
        .map_err(|_| "control_bind")?
        .port();
    control_listener
        .set_nonblocking(true)
        .map_err(|_| "control_bind")?;
    let control_token = Uuid::new_v4().simple().to_string();
    write_config(
        &config_path,
        &FixtureConfig {
            schema_version: 1,
            fixture_id: Uuid::new_v4(),
            offer_text,
            controller_address: interface.address.to_string(),
            controller_port: ready_port,
            control_address: interface.address.to_string(),
            control_port,
            control_token: control_token.clone(),
            session_id: session_id.into_uuid(),
        },
    )?;

    let status = Arc::new(Mutex::new(FixtureStatus::default()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let control_status = status.clone();
    let control_shutdown = shutdown.clone();
    let control_writer = listener_control
        .try_clone()
        .map_err(|_| "listener_control")?;
    let control_repository =
        ControllerDeviceRepository::open(&controller_root).map_err(|_| "controller_store")?;
    let control_thread = thread::spawn(move || {
        serve_control(
            control_listener,
            control_token,
            control_repository,
            control_writer,
            control_status,
            control_shutdown,
        )
    });

    println!("{{\"schema_version\":1,\"state\":\"ready\"}}");
    while !shutdown.load(Ordering::Acquire) {
        match event_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ListenerProcessEvent::PairingSasReady {
                offer_id: actual,
                sas,
                ..
            }) if actual == offer_id => {
                let mut locked = status.lock().map_err(|_| "fixture_status")?;
                locked.sas = Some(sas);
                locked.offer_id = Some(offer_id);
            }
            Ok(ListenerProcessEvent::PairingComplete {
                offer_id: actual,
                device_id,
                ..
            }) if actual == offer_id => {
                status.lock().map_err(|_| "fixture_status")?.device_id = Some(device_id);
            }
            Ok(ListenerProcessEvent::PairingFailed { code, .. }) => {
                return Err(format!("pairing_failed:{code}"));
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("listener_events".to_owned());
            }
        }
    }

    drop(listener_control);
    let _ = control_thread.join();
    let _ = listener_thread.join();
    let _ = event_thread.join();
    host.stop();
    let _ = fs::remove_file(config_path);
    Ok(())
}

fn arguments() -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let mut root = None;
    let mut binary = None;
    let mut config = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| "arguments".to_owned())?;
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(value)),
            "--session-host-bin" => binary = Some(PathBuf::from(value)),
            "--config" => config = Some(PathBuf::from(value)),
            _ => return Err("arguments".to_owned()),
        }
    }
    let root = root.ok_or_else(|| "arguments".to_owned())?;
    let binary = binary.ok_or_else(|| "arguments".to_owned())?;
    let config = config.ok_or_else(|| "arguments".to_owned())?;
    if !root.is_absolute() || !binary.is_absolute() || !config.is_absolute() {
        return Err("arguments".to_owned());
    }
    Ok((root, binary, config))
}

fn prepare_root(root: &Path) -> Result<(), String> {
    if root.exists() {
        return Err("fixture_root_exists".to_owned());
    }
    fs::create_dir_all(root).map_err(|_| "fixture_root")?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))
        .map_err(|_| "fixture_root".to_owned())
}

fn select_interface() -> Result<NetworkInterfaceCandidate, String> {
    let candidates = SystemInterfaceProvider
        .eligible_interfaces()
        .map_err(|_| "eligible_interface")?;
    candidates
        .iter()
        .find(|candidate| {
            candidate.address_family == AddressFamily::Ipv4
                && candidate.kind == NetworkInterfaceKind::Lan
                && candidate.label.starts_with("en")
        })
        .or_else(|| {
            candidates.iter().find(|candidate| {
                candidate.address_family == AddressFamily::Ipv4
                    && candidate.kind == NetworkInterfaceKind::Lan
            })
        })
        .cloned()
        .ok_or_else(|| "eligible_interface".to_owned())
}

fn initialize_authority(root: &Path, host_private: &StaticPrivateKey) -> Result<(), String> {
    let repository = ControllerDeviceRepository::open(root).map_err(|_| "controller_store")?;
    let snapshot = repository.load().map_err(|_| "controller_store")?;
    repository
        .save(
            snapshot.revision,
            ControllerDeviceAuthority {
                identity: Some(HostIdentityPublic::new(
                    HostIdentityGeneration::INITIAL,
                    HostPublicKey(host_public_key_from_private(host_private).0),
                )),
                secret_ref: Some(
                    HostIdentitySecretRef::new("identity:mobile-controller-fixture")
                        .map_err(|_| "controller_identity")?,
                ),
                state: HostIdentityState::Ready,
                revocation_epoch: REVOCATION_EPOCH,
                session_generation: SESSION_GENERATION,
                ..ControllerDeviceAuthority::default()
            },
        )
        .map_err(|_| "controller_store")?;
    Ok(())
}

fn host_descriptor(
    root: &Path,
    runtime_parent: &Path,
    sessions: &SessionRepository,
    session_id: HostedSessionId,
) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: runtime_parent.join(session_id.to_string()),
        session_dir: sessions.session_data_path(session_id),
        executable: PathBuf::from("/bin/sh"),
        runtime_detection: None,
        arguments: vec![
            "-c".to_owned(),
            "trap 'exit 0' INT TERM; printf 'MOBILE-CONTROLLER-READY\\r\\n'; while IFS= read -r line; do printf 'MOBILE-CONTROLLER-OUT:%s\\r\\n' \"$line\"; done".to_owned(),
        ],
        environment: BTreeMap::from([
            ("HOME".to_owned(), root.to_string_lossy().into_owned()),
            ("LANG".to_owned(), "C".to_owned()),
            ("LC_ALL".to_owned(), "C".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("TERM".to_owned(), "xterm-256color".to_owned()),
        ]),
        cwd: Some(root.to_path_buf()),
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
) -> Result<(), String> {
    let expected = sessions.load().map_err(|_| "session_store")?.revision;
    sessions
        .create_session(
            HostedSession {
                id: session_id,
                project_id,
                group_id: None,
                preset_id: None,
                title: SessionTitle::new("Mobile Controller terminal")
                    .map_err(|_| "session_title")?,
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
        .map_err(|_| "session_store")?;
    Ok(())
}

fn write_config(path: &Path, config: &FixtureConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| "config_directory")?;
    }
    let bytes = serde_json::to_vec(config).map_err(|_| "config_encode")?;
    fs::write(path, bytes).map_err(|_| "config_write")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| "config_write".to_owned())
}

fn serve_control(
    listener: TcpListener,
    token: String,
    repository: ControllerDeviceRepository,
    mut listener_control: UnixStream,
    status: Arc<Mutex<FixtureStatus>>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(code) = handle_control(
                    stream,
                    &token,
                    &repository,
                    &mut listener_control,
                    &status,
                    &shutdown,
                ) {
                    eprintln!("fixture control request failed: {code}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return,
        }
    }
}

fn handle_control(
    mut stream: TcpStream,
    token: &str,
    repository: &ControllerDeviceRepository,
    listener_control: &mut UnixStream,
    status: &Arc<Mutex<FixtureStatus>>,
    shutdown: &Arc<AtomicBool>,
) -> Result<(), String> {
    stream
        .set_nonblocking(false)
        .map_err(|_| "control_blocking")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| "control_timeout")?;
    let mut bytes = Vec::new();
    BufReader::new(&stream)
        .take(MAX_CONTROL_BYTES + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| format!("control_read:{:?}", error.kind()))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_CONTROL_BYTES {
        return Err("control_frame".to_owned());
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    let request: ControlRequest = serde_json::from_slice(&bytes).map_err(|_| "control_frame")?;
    if request.token != token {
        write_control_response(&mut stream, false, Some("unauthorized"));
        return Ok(());
    }

    let mut locked = status.lock().map_err(|_| "fixture_status")?;
    match request.command.as_str() {
        "sas" => write_control_response(&mut stream, locked.sas.is_some(), locked.sas.as_deref()),
        "status" => {
            let value = if locked.revoked {
                "revoked"
            } else if locked.device_id.is_some() {
                "paired"
            } else if locked.sas.is_some() {
                "sas_ready"
            } else {
                "waiting"
            };
            write_control_response(&mut stream, true, Some(value));
        }
        "confirm" => {
            let Some(offer_id) = locked.offer_id else {
                write_control_response(&mut stream, false, Some("sas_not_ready"));
                return Ok(());
            };
            let ok =
                ListenerControlCommand::decide_pairing(offer_id, ProcessPairingDecision::Confirm)
                    .write(listener_control)
                    .is_ok();
            write_control_response(
                &mut stream,
                ok,
                Some(if ok { "confirmed" } else { "failed" }),
            );
        }
        "grant_input" => {
            let Some(device_id) = locked.device_id else {
                write_control_response(&mut stream, false, Some("not_paired"));
                return Ok(());
            };
            let capabilities = ControllerCapabilities::default()
                .with(ControllerCapability::ObserveSessions)
                .with(ControllerCapability::AttachOutput)
                .with(ControllerCapability::SendInput)
                .with(ControllerCapability::Resize)
                .with(ControllerCapability::RespondToApproval);
            let service =
                ControllerDeviceService::new(repository.clone(), Arc::new(NoControllerChannels));
            let ok = service.set_capabilities(device_id, capabilities).is_ok();
            write_control_response(&mut stream, ok, Some(if ok { "granted" } else { "failed" }));
        }
        "revoke" => {
            let Some(device_id) = locked.device_id else {
                write_control_response(&mut stream, false, Some("not_paired"));
                return Ok(());
            };
            let service =
                ControllerDeviceService::new(repository.clone(), Arc::new(NoControllerChannels));
            let ok = service.revoke(device_id).is_ok();
            if ok {
                locked.revoked = true;
            }
            write_control_response(&mut stream, ok, Some(if ok { "revoked" } else { "failed" }));
        }
        "shutdown" => {
            shutdown.store(true, Ordering::Release);
            write_control_response(&mut stream, true, Some("stopping"));
        }
        _ => write_control_response(&mut stream, false, Some("unknown_command")),
    }
    Ok(())
}

fn write_control_response(stream: &mut TcpStream, ok: bool, value: Option<&str>) {
    if let Ok(bytes) = serde_json::to_vec(&ControlResponse { ok, value }) {
        let _ = stream.write_all(&bytes);
        let _ = stream.flush();
    }
}

trait HostedSessionIdUuid {
    fn into_uuid(self) -> Uuid;
}

impl HostedSessionIdUuid for HostedSessionId {
    fn into_uuid(self) -> Uuid {
        Uuid::parse_str(&self.to_string()).expect("Hosted Session IDs are UUIDs")
    }
}
