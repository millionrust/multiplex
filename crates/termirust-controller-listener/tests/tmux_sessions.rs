#![cfg(unix)]
//! tmux sessions the app did not create, listed and attached through an authenticated
//! Controller connection. Every test runs its own tmux server under a private socket
//! directory and kills it on drop.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use termirust_controller_listener::{
    AuthoritySnapshot, ControllerAuthorityProvider, ControllerClientChannel, ControllerCommand,
    ControllerResponse, ControllerSessionCapability, ControllerSessionOrigin,
    ControllerSessionSummary, HostBackendFactory, ListenerError, ListenerErrorCode,
    SystemHandshakeEntropy, TMUX_RUNTIME_ID, TmuxSessionSource, serve_authenticated_stdio_stream,
};
use termirust_controller_security::{
    CapabilitySet, ControllerCapability as SecurityCapability, HostStaticPublicKey,
    StaticPrivateKey, device_public_key_from_private, host_public_key_from_private,
};
use termirust_domain::{
    ControllerCapabilities, ControllerCapability as DomainCapability, ControllerDeviceAuthority,
    ControllerDeviceId, ControllerProtocolRange, DevicePublicKey, HostIdentityGeneration,
    HostIdentityPublic, HostIdentitySecretRef, HostIdentityState, HostInstanceId, HostPublicKey,
    HostedSessionId, OccupantGeneration, OutputSequence, PairedDeviceRecord, PairedDeviceStatus,
    PairingOfferId,
};
use termirust_session_host::{LaunchDescriptor, SessionHostHandle, StopDeadlines};
use termirust_store::{JournalLimits, ProjectRepository, SessionRepository};
use termirust_tmux::Tmux;
use tokio::io::DuplexStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const SESSION_GENERATION: u64 = 3;
const REVOCATION_EPOCH: u64 = 1;
const DESKTOP_COLUMNS: u16 = 120;
const DESKTOP_ROWS: u16 = 40;
const WAIT: Duration = Duration::from_secs(10);

struct Fixture {
    tmux: Tmux,
    root: tempfile::TempDir,
    desktop_clients: Vec<SessionHostHandle>,
}

impl Fixture {
    fn start() -> Option<Self> {
        let Ok(tmux) = Tmux::discover() else {
            eprintln!("skipping tmux integration test: tmux is unavailable");
            return None;
        };
        if !tmux.supports_ignore_size() {
            eprintln!("skipping tmux integration test: tmux is older than 3.2");
            return None;
        }
        // Unix socket paths are short; stay out of the long per-user temp directory.
        let root = tempfile::Builder::new()
            .prefix("tr-tmux-")
            .tempdir_in("/tmp")
            .unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        std::fs::create_dir(root_path.join("sockets")).unwrap();
        Some(Self {
            tmux: tmux.with_socket_directory(root_path.join("sockets")),
            root,
            desktop_clients: Vec::new(),
        })
    }

    fn path(&self) -> PathBuf {
        std::fs::canonicalize(self.root.path()).unwrap()
    }

    fn runtime_parent(&self) -> PathBuf {
        self.path().join("runtime")
    }

    fn tmux_output(&self, arguments: &[&str]) -> String {
        let output = self.tmux.command().args(arguments).output().unwrap();
        assert!(output.status.success(), "tmux {arguments:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn tmux_succeeds(&self, arguments: &[&str]) -> bool {
        self.tmux
            .command()
            .args(arguments)
            .output()
            .unwrap()
            .status
            .success()
    }

    /// Creates a detached session and returns its tmux id.
    fn new_session(&self, name: &str) -> String {
        self.tmux_output(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_id}",
            "-s",
            name,
            "-x",
            &DESKTOP_COLUMNS.to_string(),
            "-y",
            &DESKTOP_ROWS.to_string(),
            "/bin/sh",
        ])
    }

    fn window_size(&self, tmux_id: &str) -> String {
        self.tmux_output(&[
            "display-message",
            "-p",
            "-t",
            &format!("{tmux_id}:"),
            "#{window_width}x#{window_height}",
        ])
    }

    /// Attaches an ordinary client, the way a desktop terminal would, at the desktop size.
    async fn attach_desktop_client(&mut self, tmux_id: &str) {
        let session_id = HostedSessionId::new();
        let directory = self.path().join(format!("desktop-{session_id}"));
        std::fs::create_dir(&directory).unwrap();
        let descriptor = LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id,
            host_instance_id: HostInstanceId::new(),
            expected_occupant_generation: None,
            runtime_root: directory.join("run"),
            session_dir: directory.join("data"),
            executable: self.tmux.canonical_executable().unwrap(),
            runtime_detection: None,
            arguments: vec![
                "-u".to_owned(),
                "attach-session".to_owned(),
                "-t".to_owned(),
                tmux_id.to_owned(),
            ],
            environment: self.tmux.client_environment().into_iter().collect(),
            cwd: Some(directory.clone()),
            columns: DESKTOP_COLUMNS,
            // One row for the status line.
            rows: DESKTOP_ROWS + 1,
            journal_limits: JournalLimits::default(),
            stop_deadlines: StopDeadlines::default(),
        };
        self.desktop_clients
            .push(termirust_session_host::start(descriptor).await.unwrap());
        let expected = format!("{DESKTOP_COLUMNS}x{DESKTOP_ROWS}");
        wait_until(|| self.window_size(tmux_id) == expected).await;
        let clients = self.tmux_output(&["list-clients", "-t", tmux_id, "-F", "#{client_name}"]);
        assert_eq!(
            clients.lines().count(),
            1,
            "desktop client should be attached"
        );
    }

    fn backends(&self, source: Option<TmuxSessionSource>) -> Arc<HostBackendFactory> {
        let metadata = self.path().join("metadata");
        let sessions =
            SessionRepository::open(&metadata, self.path().join("session-data")).unwrap();
        let projects = ProjectRepository::open(&metadata).unwrap();
        Arc::new(
            HostBackendFactory::new(sessions, projects, self.runtime_parent())
                .with_tmux_sessions(source),
        )
    }

    fn source(&self) -> TmuxSessionSource {
        TmuxSessionSource::with_tmux(self.tmux.clone(), self.runtime_parent()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.desktop_clients.clear();
        let _ = self.tmux.command().arg("kill-server").output();
    }
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within {WAIT:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[derive(Clone)]
struct FixedAuthority {
    authority: Arc<Mutex<ControllerDeviceAuthority>>,
    host_private: StaticPrivateKey,
}

impl ControllerAuthorityProvider for FixedAuthority {
    fn snapshot(&self) -> Result<AuthoritySnapshot, ListenerError> {
        Ok(AuthoritySnapshot {
            authority: self.authority.lock().unwrap().clone(),
            host_private: self.host_private.clone(),
        })
    }
}

struct Controller {
    channel: ControllerClientChannel<DuplexStream>,
    server: JoinHandle<Result<(), ListenerError>>,
}

fn deadline() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        + 10_000
}

async fn connect(backends: Arc<HostBackendFactory>, device_seed: u8) -> Controller {
    let host_private = StaticPrivateKey::from_fixture_bytes([61; 32]);
    let device_private = StaticPrivateKey::from_fixture_bytes([device_seed; 32]);
    let capabilities = ControllerCapabilities::default()
        .with(DomainCapability::ObserveSessions)
        .with(DomainCapability::AttachOutput)
        .with(DomainCapability::SendInput)
        .with(DomainCapability::Resize);
    let authority = Arc::new(FixedAuthority {
        authority: Arc::new(Mutex::new(ControllerDeviceAuthority {
            identity: Some(HostIdentityPublic::new(
                HostIdentityGeneration::INITIAL,
                HostPublicKey(host_public_key_from_private(&host_private).0),
            )),
            secret_ref: Some(HostIdentitySecretRef::new("identity:tmux-test").unwrap()),
            state: HostIdentityState::Ready,
            revocation_epoch: REVOCATION_EPOCH,
            session_generation: SESSION_GENERATION,
            devices: vec![PairedDeviceRecord {
                device_id: ControllerDeviceId::new(),
                public_key: DevicePublicKey(device_public_key_from_private(&device_private).0),
                display_name: "Phone".to_owned(),
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
        })),
        host_private: host_private.clone(),
    });
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
        .with(SecurityCapability::Resize);
    let channel = ControllerClientChannel::connect(
        client,
        1,
        REVOCATION_EPOCH,
        SESSION_GENERATION,
        HostStaticPublicKey(host_public_key_from_private(&host_private).0),
        device_private,
        requested,
        &mut SystemHandshakeEntropy,
    )
    .await
    .unwrap();
    Controller { channel, server }
}

impl Controller {
    async fn list(&mut self) -> (u64, Vec<ControllerSessionSummary>) {
        let command_id = self
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
        loop {
            match self.channel.read_response().await.unwrap() {
                ControllerResponse::Sessions {
                    command_id: actual,
                    revision,
                    sessions,
                    ..
                } if actual == command_id => return (revision, sessions),
                ControllerResponse::Output { .. } => {}
                response => panic!("unexpected list response: {response:?}"),
            }
        }
    }

    async fn only_tmux_row(&mut self) -> ControllerSessionSummary {
        let (_, sessions) = self.list().await;
        let rows = sessions
            .into_iter()
            .filter(|session| session.runtime.as_deref() == Some(TMUX_RUNTIME_ID))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1, "expected exactly one tmux row");
        rows.into_iter().next().unwrap()
    }

    /// Attaches and returns once `Attached` arrives, keeping any output that came with it.
    async fn attach(
        &mut self,
        session_id: HostedSessionId,
        generation: OccupantGeneration,
    ) -> Vec<u8> {
        let command_id = self
            .channel
            .send(
                ControllerCommand::Attach {
                    session_id,
                    occupant_generation: generation,
                    from_sequence: OutputSequence::ZERO,
                    columns: 80,
                    rows: 24,
                },
                deadline(),
            )
            .await
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            match self.channel.read_response().await.unwrap() {
                ControllerResponse::Attached {
                    command_id: actual,
                    occupant_generation,
                    has_writer_lease,
                    ..
                } if actual == command_id => {
                    assert_eq!(occupant_generation, generation);
                    assert!(!has_writer_lease);
                    return bytes;
                }
                ControllerResponse::Snapshot { bytes: chunk, .. }
                | ControllerResponse::Output { bytes: chunk, .. } => bytes.extend(chunk),
                response => panic!("unexpected attach response: {response:?}"),
            }
        }
    }

    async fn complete(&mut self, command: ControllerCommand) {
        let command_id = self.channel.send(command, deadline()).await.unwrap();
        loop {
            match self.channel.read_response().await.unwrap() {
                ControllerResponse::Completed {
                    command_id: actual,
                    applied,
                } if actual == command_id => {
                    assert!(applied);
                    return;
                }
                ControllerResponse::Detached { command_id: actual } if actual == command_id => {
                    return;
                }
                ControllerResponse::Output { .. } => {}
                response => panic!("unexpected response: {response:?}"),
            }
        }
    }

    async fn type_and_expect(
        &mut self,
        session_id: HostedSessionId,
        generation: OccupantGeneration,
        input: &[u8],
        expected: &[u8],
    ) {
        let command_id = self
            .channel
            .send(
                ControllerCommand::Input {
                    session_id,
                    occupant_generation: generation,
                    bytes: input.to_vec(),
                },
                deadline(),
            )
            .await
            .unwrap();
        self.expect_output(session_id, expected, Some(command_id))
            .await;
    }

    async fn expect_output(
        &mut self,
        session_id: HostedSessionId,
        expected: &[u8],
        completion: Option<termirust_domain::CommandId>,
    ) {
        let mut completed = completion.is_none();
        let mut bytes = Vec::new();
        let read = async {
            loop {
                match self.channel.read_response().await.unwrap() {
                    ControllerResponse::Completed {
                        command_id,
                        applied,
                    } if Some(command_id) == completion => {
                        assert!(applied);
                        completed = true;
                    }
                    ControllerResponse::Output {
                        session_id: actual,
                        bytes: chunk,
                        ..
                    } if actual == session_id => bytes.extend(chunk),
                    response => panic!("unexpected response while reading output: {response:?}"),
                }
                if completed && contains(&bytes, expected) {
                    return;
                }
            }
        };
        tokio::time::timeout(WAIT, read).await.unwrap_or_else(|_| {
            panic!(
                "expected {:?} in tmux output",
                String::from_utf8_lossy(expected)
            )
        });
    }

    async fn close(self) {
        let Controller { channel, server } = self;
        drop(channel);
        let result = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server should notice the client closing")
            .unwrap();
        assert!(
            result.is_ok()
                || result
                    .as_ref()
                    .is_err_and(|error| error.code == ListenerErrorCode::Io)
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

async fn wait_for_hosts(source: &TmuxSessionSource, expected: usize) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while source.running_hosts().await != expected {
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {expected} running attach hosts"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn marker(tag: &str) -> (Vec<u8>, Vec<u8>) {
    // The shell computes the number, so the echo of the typed command cannot match.
    (
        format!("echo {tag}-$((40+2))\n").into_bytes(),
        format!("{tag}-42").into_bytes(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tmux_session_lists_attaches_streams_and_accepts_input_without_resizing() {
    let Some(mut fixture) = Fixture::start() else {
        return;
    };
    let tmux_id = fixture.new_session("phone target: a.b | c");
    fixture.attach_desktop_client(&tmux_id).await;
    let source = fixture.source();
    let mut controller = connect(fixture.backends(Some(source.clone())), 71).await;

    let row = controller.only_tmux_row().await;
    assert_eq!(row.title, "phone target: a.b | c");
    assert_eq!(row.origin, ControllerSessionOrigin::Terminal);
    assert_eq!(row.lifecycle, "live");
    assert_eq!(row.occupant_generation, Some(OccupantGeneration::new(1)));
    assert!(
        row.capabilities
            .contains(&ControllerSessionCapability::SendInput)
    );
    assert!(
        !row.capabilities
            .contains(&ControllerSessionCapability::Resize),
        "a tmux row must not offer Resize"
    );
    let generation = row.occupant_generation.unwrap();

    controller.attach(row.session_id, generation).await;
    assert_eq!(source.running_hosts().await, 1);
    controller
        .complete(ControllerCommand::AcquireWriter {
            session_id: row.session_id,
            occupant_generation: generation,
        })
        .await;
    let (input, expected) = marker("PHONE");
    controller
        .type_and_expect(row.session_id, generation, &input, &expected)
        .await;

    // Output typed on the desktop side reaches the phone too.
    fixture.tmux_output(&[
        "send-keys",
        "-t",
        &format!("{tmux_id}:"),
        "-l",
        "echo DESKTOP-$((40+2))\n",
    ]);
    controller
        .expect_output(row.session_id, b"DESKTOP-42", None)
        .await;

    assert_eq!(
        fixture.window_size(&tmux_id),
        format!("{DESKTOP_COLUMNS}x{DESKTOP_ROWS}"),
        "the phone must not resize the desktop window"
    );

    controller
        .complete(ControllerCommand::Detach {
            session_id: row.session_id,
            occupant_generation: generation,
        })
        .await;
    assert_eq!(source.running_hosts().await, 0);
    assert!(
        fixture.tmux_succeeds(&["has-session", "-t", &tmux_id]),
        "detaching must never end the user's tmux session"
    );

    let row = controller.only_tmux_row().await;
    assert_eq!(
        row.occupant_generation,
        Some(OccupantGeneration::new(2)),
        "a new host is a new occupant generation"
    );
    controller.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_share_one_host_and_the_last_one_tears_it_down() {
    let Some(mut fixture) = Fixture::start() else {
        return;
    };
    let tmux_id = fixture.new_session("shared");
    fixture.attach_desktop_client(&tmux_id).await;
    let source = fixture.source();
    let backends = fixture.backends(Some(source.clone()));
    let mut first = connect(backends.clone(), 72).await;
    let mut second = connect(backends, 72).await;

    let row = first.only_tmux_row().await;
    let generation = row.occupant_generation.unwrap();
    assert_eq!(second.only_tmux_row().await.session_id, row.session_id);
    first.attach(row.session_id, generation).await;
    second.attach(row.session_id, generation).await;
    assert_eq!(source.running_hosts().await, 1);

    first
        .complete(ControllerCommand::Detach {
            session_id: row.session_id,
            occupant_generation: generation,
        })
        .await;
    assert_eq!(
        source.running_hosts().await,
        1,
        "the second phone still watches"
    );

    second
        .complete(ControllerCommand::AcquireWriter {
            session_id: row.session_id,
            occupant_generation: generation,
        })
        .await;
    let (input, expected) = marker("SECOND");
    second
        .type_and_expect(row.session_id, generation, &input, &expected)
        .await;

    // Closing the connection, rather than detaching, must release the host too.
    second.close().await;
    wait_for_hosts(&source, 0).await;
    assert!(fixture.tmux_succeeds(&["has-session", "-t", &tmux_id]));
    first.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attach_with_a_generation_from_an_earlier_host_is_stale() {
    let Some(mut fixture) = Fixture::start() else {
        return;
    };
    let tmux_id = fixture.new_session("stale");
    fixture.attach_desktop_client(&tmux_id).await;
    let source = fixture.source();
    let mut controller = connect(fixture.backends(Some(source.clone())), 73).await;
    let row = controller.only_tmux_row().await;
    let old = row.occupant_generation.unwrap();
    controller.attach(row.session_id, old).await;
    controller
        .complete(ControllerCommand::Detach {
            session_id: row.session_id,
            occupant_generation: old,
        })
        .await;

    controller
        .channel
        .send(
            ControllerCommand::Attach {
                session_id: row.session_id,
                occupant_generation: old,
                from_sequence: OutputSequence::new(500),
                columns: 80,
                rows: 24,
            },
            deadline(),
        )
        .await
        .unwrap();
    let result = tokio::time::timeout(WAIT, controller.server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().code, ListenerErrorCode::StaleGeneration);
    assert_eq!(source.running_hosts().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_generation_replays_the_screen_even_from_an_old_watermark() {
    let Some(mut fixture) = Fixture::start() else {
        return;
    };
    let tmux_id = fixture.new_session("rewatch");
    fixture.attach_desktop_client(&tmux_id).await;
    fixture.tmux_output(&[
        "send-keys",
        "-t",
        &format!("{tmux_id}:"),
        "-l",
        "echo SCREEN-$((40+2))\n",
    ]);
    let source = fixture.source();
    let mut controller = connect(fixture.backends(Some(source.clone())), 74).await;
    let row = controller.only_tmux_row().await;
    let generation = row.occupant_generation.unwrap();
    let command_id = controller
        .channel
        .send(
            ControllerCommand::Attach {
                session_id: row.session_id,
                occupant_generation: generation,
                // A watermark this host never produced.
                from_sequence: OutputSequence::new(10_000),
                columns: 80,
                rows: 24,
            },
            deadline(),
        )
        .await
        .unwrap();
    let mut attached = false;
    let mut bytes = Vec::new();
    let read = async {
        loop {
            match controller.channel.read_response().await.unwrap() {
                ControllerResponse::Attached {
                    command_id: actual, ..
                } if actual == command_id => attached = true,
                ControllerResponse::Snapshot { bytes: chunk, .. }
                | ControllerResponse::Output { bytes: chunk, .. } => bytes.extend(chunk),
                response => panic!("unexpected response: {response:?}"),
            }
            if attached && contains(&bytes, b"SCREEN-42") {
                return;
            }
        }
    };
    tokio::time::timeout(WAIT, read)
        .await
        .expect("a new tmux client redraws the existing screen");
    controller.close().await;
    wait_for_hosts(&source, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sessions_that_end_disappear_and_cannot_be_attached() {
    let Some(fixture) = Fixture::start() else {
        return;
    };
    let kept = fixture.new_session("kept");
    let ending = fixture.new_session("ending");
    let source = fixture.source();
    let mut controller = connect(fixture.backends(Some(source.clone())), 75).await;
    let (first_revision, sessions) = controller.list().await;
    let ending_row = sessions
        .iter()
        .find(|session| session.title == "ending")
        .unwrap()
        .clone();
    assert_eq!(sessions.len(), 2);

    fixture.tmux_output(&["kill-session", "-t", &ending]);
    let (second_revision, sessions) = controller.list().await;
    assert_eq!(
        sessions
            .iter()
            .map(|session| session.title.as_str())
            .collect::<Vec<_>>(),
        ["kept"]
    );
    assert_ne!(first_revision, second_revision);

    controller
        .channel
        .send(
            ControllerCommand::Attach {
                session_id: ending_row.session_id,
                occupant_generation: ending_row.occupant_generation.unwrap(),
                from_sequence: OutputSequence::ZERO,
                columns: 80,
                rows: 24,
            },
            deadline(),
        )
        .await
        .unwrap();
    let result = tokio::time::timeout(WAIT, controller.server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().code, ListenerErrorCode::HostUnavailable);
    assert_eq!(source.running_hosts().await, 0);
    assert!(fixture.tmux_succeeds(&["has-session", "-t", &kept]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_is_off_without_a_source_and_empty_without_a_server() {
    let Some(fixture) = Fixture::start() else {
        return;
    };
    let source = fixture.source();
    let mut controller = connect(fixture.backends(Some(source)), 76).await;
    let (_, sessions) = controller.list().await;
    assert!(sessions.is_empty(), "no tmux server means no rows");
    controller.close().await;

    fixture.new_session("present");
    let mut controller = connect(fixture.backends(None), 77).await;
    let (_, sessions) = controller.list().await;
    assert!(sessions.is_empty(), "discovery is opt-in");
    controller.close().await;

    let missing = TmuxSessionSource::with_tmux(
        Tmux::select(
            [PathBuf::from("/fixture/tmux")],
            |_: &Path| true,
            |_: &Path| Ok::<_, std::convert::Infallible>("tmux 3.7".to_owned()),
        )
        .unwrap(),
        fixture.runtime_parent(),
    )
    .unwrap();
    let mut controller = connect(fixture.backends(Some(missing)), 78).await;
    let (_, sessions) = controller.list().await;
    assert!(sessions.is_empty(), "a broken tmux must not break listing");
    controller.close().await;
}

/// The SSH and relay routes serve through `serve_repository_stdio_bridge` in their own
/// process. They must offer the same live sessions as the LAN listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repository_bridge_route_offers_live_panes_and_tmux_sessions() {
    use termirust_controller_listener::{
        DesktopPaneBridgeEndpoint, DesktopPaneBridgeServer, DesktopPaneRegistration,
        DesktopPaneRegistry, DesktopPaneTransport, RepositoryBridgeSources,
        serve_repository_stdio_bridge,
    };
    use termirust_store::ControllerDeviceRepository;

    let Some(mut fixture) = Fixture::start() else {
        return;
    };
    let tmux_id = fixture.new_session("over ssh");
    fixture.attach_desktop_client(&tmux_id).await;
    let runtime_parent = fixture.runtime_parent();
    std::fs::create_dir_all(&runtime_parent).unwrap();

    let registry = DesktopPaneRegistry::default();
    let pane_id = HostedSessionId::new();
    registry.register(DesktopPaneRegistration {
        session_id: pane_id,
        title: "Desktop pane".to_owned(),
        runtime: "local_shell".to_owned(),
        columns: 100,
        rows: 30,
        transport: DesktopPaneTransport::new(|_| true),
    });
    let bridge_root = runtime_parent.join("desktop-pane-bridge");
    let mut pane_bridge = DesktopPaneBridgeServer::start(&bridge_root, registry).unwrap();
    pane_bridge.publish().unwrap();

    let host_private = StaticPrivateKey::from_fixture_bytes([81; 32]);
    let device_private = StaticPrivateKey::from_fixture_bytes([82; 32]);
    let controller_root = fixture.path().join("controller");
    let devices = ControllerDeviceRepository::open(&controller_root).unwrap();
    let snapshot = devices.load().unwrap();
    devices
        .update(snapshot.revision, |authority| {
            authority.identity = Some(HostIdentityPublic::new(
                HostIdentityGeneration::INITIAL,
                HostPublicKey(host_public_key_from_private(&host_private).0),
            ));
            authority.secret_ref =
                Some(HostIdentitySecretRef::new("identity:bridge-test").unwrap());
            authority.state = HostIdentityState::Ready;
            authority.revocation_epoch = REVOCATION_EPOCH;
            authority.session_generation = SESSION_GENERATION;
            authority.devices.push(PairedDeviceRecord {
                device_id: ControllerDeviceId::new(),
                public_key: DevicePublicKey(device_public_key_from_private(&device_private).0),
                display_name: "Phone over SSH".to_owned(),
                capabilities: ControllerCapabilities::default()
                    .with(DomainCapability::ObserveSessions)
                    .with(DomainCapability::AttachOutput)
                    .with(DomainCapability::SendInput),
                protocol_range: ControllerProtocolRange::V1,
                created_at: 1,
                last_seen_at: None,
                revocation_epoch: authority.revocation_epoch,
                identity_generation: HostIdentityGeneration::INITIAL,
                status: PairedDeviceStatus::Online,
                source_offer_id: PairingOfferId::new(),
            });
            Ok(())
        })
        .unwrap();
    let saved = devices.load().unwrap();

    let metadata = fixture.path().join("metadata");
    let (client, server_stream) = tokio::io::duplex(512 * 1024);
    let (reader, writer) = tokio::io::split(server_stream);
    let sources = RepositoryBridgeSources {
        desktop_pane_bridge: DesktopPaneBridgeEndpoint::discover(&bridge_root),
        tmux_sessions: Some(fixture.source()),
    };
    assert!(sources.desktop_pane_bridge.is_some());
    let server = tokio::spawn(serve_repository_stdio_bridge(
        reader,
        writer,
        controller_root,
        metadata,
        fixture.path().join("session-data"),
        runtime_parent.clone(),
        runtime_parent.join("controller-pairing.sock"),
        host_private.clone(),
        sources,
        CancellationToken::new(),
    ));
    let channel = ControllerClientChannel::connect(
        client,
        1,
        saved.authority.revocation_epoch,
        saved.authority.session_generation,
        HostStaticPublicKey(host_public_key_from_private(&host_private).0),
        device_private,
        CapabilitySet::default()
            .with(SecurityCapability::ObserveSessions)
            .with(SecurityCapability::AttachOutput)
            .with(SecurityCapability::SendInput),
        &mut SystemHandshakeEntropy,
    )
    .await
    .unwrap();
    let mut controller = Controller { channel, server };

    let (_, sessions) = controller.list().await;
    assert!(
        sessions.iter().any(|session| session.session_id == pane_id),
        "the desktop's live pane is offered over this route"
    );
    let row = sessions
        .iter()
        .find(|session| session.runtime.as_deref() == Some(TMUX_RUNTIME_ID))
        .expect("the tmux session is offered over this route")
        .clone();
    assert_eq!(row.title, "over ssh");
    let generation = row.occupant_generation.unwrap();
    controller.attach(row.session_id, generation).await;
    controller
        .complete(ControllerCommand::AcquireWriter {
            session_id: row.session_id,
            occupant_generation: generation,
        })
        .await;
    let (input, expected) = marker("SSH");
    controller
        .type_and_expect(row.session_id, generation, &input, &expected)
        .await;
    controller.close().await;
    drop(pane_bridge);
}
