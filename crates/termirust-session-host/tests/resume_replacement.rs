#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{
    CommandId, ContinuityLink, HostInstanceId, HostLifecycle, HostedSessionId, OccupantGeneration,
    OutputSequence, RuntimeCapability, RuntimeCapabilitySet, RuntimeDetectionResult,
    RuntimeDetectionStatus, RuntimeId,
};
use termirust_host_protocol::wire;
use termirust_session_host::{
    HostErrorCode, LaunchDescriptor, StopDeadlines, process_observation::fingerprint_executable,
    start, start_with_cancel,
};
use termirust_store::{ContinuityRepository, JournalLimits, read_host_metadata};
use tokio_util::sync::CancellationToken;

const CONVERSATION_HANDLE: &str = "019cf76d-0493-77d1-8572-3fb4ac801ac8";

fn nonce(index: u64) -> [u8; 32] {
    let mut value = [0_u8; 32];
    value[..8].copy_from_slice(&index.to_le_bytes());
    value[8..16].copy_from_slice(&index.wrapping_mul(29).to_le_bytes());
    value
}

fn fake_codex(root: &Path) -> PathBuf {
    let executable = root.join("codex");
    fs::write(
        &executable,
        "#!/bin/sh\nif [ \"$1\" = resume ]; then\n  trap 'exit 0' INT TERM\n  printf 'RESUME-HOST-READY\\n'\n  while IFS= read -r line; do printf 'RESUME:%s\\n' \"$line\"; done\nelse\n  printf 'SOURCE-EXITED\\n'\n  sleep 0.05\nfi\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    executable
}

fn detection(executable: &Path) -> RuntimeDetectionResult {
    RuntimeDetectionResult {
        runtime_id: RuntimeId::new("codex").unwrap(),
        descriptor_version: 1,
        status: RuntimeDetectionStatus::Available,
        fingerprint: Some(fingerprint_executable(executable).unwrap()),
        safe_version: Some("0.150.1".to_string()),
        capabilities: RuntimeCapabilitySet::new([
            RuntimeCapability::InteractivePty,
            RuntimeCapability::Cancellation,
            RuntimeCapability::Resume,
        ]),
        diagnostic_code: None,
    }
}

fn descriptor(
    root: &Path,
    executable: &Path,
    session_id: HostedSessionId,
    generation: OccupantGeneration,
    arguments: Vec<String>,
) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: Some(generation),
        runtime_root: root.join("runtime"),
        session_dir: root.join(format!("session-{session_id}")),
        executable: executable.to_path_buf(),
        runtime_detection: Some(detection(executable)),
        arguments,
        environment: BTreeMap::from([
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
        ]),
        cwd: Some(root.to_path_buf()),
        columns: 100,
        rows: 30,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    }
}

fn spawn_host(descriptor: &LaunchDescriptor) -> Child {
    let mut process = Command::new(env!("CARGO_BIN_EXE_termirust-session-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(process.stdin.as_mut().unwrap(), descriptor).unwrap();
    process.stdin.take().unwrap().flush().unwrap();
    let mut ready = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    if !ready.contains("host_ready") {
        let diagnostic = process
            .stderr
            .take()
            .map(|stderr| {
                use std::io::Read as _;
                let mut output = String::new();
                let _ = BufReader::new(stderr).read_to_string(&mut output);
                output
            })
            .unwrap_or_default();
        panic!("Host did not become ready: {}", diagnostic.trim());
    }
    process
}

async fn wait_for_exit(process: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while process.try_wait().unwrap().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(process.try_wait().unwrap().is_some(), "Host did not exit");
}

fn directory_snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, path: &Path, snapshot: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            let metadata = fs::symlink_metadata(&entry).unwrap();
            assert!(!metadata.file_type().is_symlink());
            let relative = entry.strip_prefix(root).unwrap().to_path_buf();
            if metadata.is_dir() {
                snapshot.insert(relative, None);
                walk(root, &entry, snapshot);
            } else {
                snapshot.insert(relative, Some(fs::read(&entry).unwrap()));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    walk(root, root, &mut snapshot);
    snapshot
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_replacement_uses_new_host_generation_and_preserves_source_journal() {
    let fixture = tempfile::tempdir().unwrap();
    let executable = fake_codex(fixture.path());
    let source_session_id = HostedSessionId::new();
    let source = descriptor(
        fixture.path(),
        &executable,
        source_session_id,
        OccupantGeneration::new(1),
        Vec::new(),
    );
    let mut source_process = spawn_host(&source);
    wait_for_exit(&mut source_process).await;
    let source_metadata = read_host_metadata(&source.session_dir).unwrap();
    assert_eq!(source_metadata.lifecycle, HostLifecycle::Exited);
    assert_eq!(
        source_metadata.activity.generation,
        OccupantGeneration::new(1)
    );
    let source_before = directory_snapshot(&source.session_dir);

    let continuity = ContinuityRepository::open(fixture.path().join("continuity")).unwrap();
    let initial_continuity = continuity.load().unwrap();
    assert!(initial_continuity.links.is_empty());

    let cancelled_session_id = HostedSessionId::new();
    let cancelled = descriptor(
        fixture.path(),
        &executable,
        cancelled_session_id,
        OccupantGeneration::new(2),
        vec!["resume".to_string(), CONVERSATION_HANDLE.to_string()],
    );
    let cancel_before_ready = CancellationToken::new();
    cancel_before_ready.cancel();
    assert_eq!(
        start_with_cancel(cancelled.clone(), &cancel_before_ready)
            .await
            .unwrap_err()
            .code,
        HostErrorCode::Cancelled
    );
    assert!(!cancelled.session_dir.exists());
    assert!(continuity.load().unwrap().links.is_empty());
    assert_eq!(directory_snapshot(&source.session_dir), source_before);

    let replacement_session_id = HostedSessionId::new();
    let replacement = descriptor(
        fixture.path(),
        &executable,
        replacement_session_id,
        OccupantGeneration::new(2),
        vec![
            "resume".to_string(),
            "--cd".to_string(),
            fixture.path().to_string_lossy().into_owned(),
            CONVERSATION_HANDLE.to_string(),
        ],
    );
    assert_ne!(source.host_instance_id, replacement.host_instance_id);
    assert_ne!(source.session_dir, replacement.session_dir);
    let mut replacement_process = spawn_host(&replacement);
    assert!(continuity.load().unwrap().links.is_empty());
    let replacement_metadata = read_host_metadata(&replacement.session_dir).unwrap();
    assert_eq!(replacement_metadata.lifecycle, HostLifecycle::Ready);
    assert_eq!(
        replacement_metadata.activity.generation,
        OccupantGeneration::new(2)
    );

    let command_id = CommandId::new();
    let committed = continuity
        .record(
            initial_continuity.revision,
            ContinuityLink {
                command_id,
                source_session_id,
                replacement_session_id,
                runtime_id: RuntimeId::new("codex").unwrap(),
                prior_generation: OccupantGeneration::new(1),
                replacement_generation: OccupantGeneration::new(2),
                committed_at: 1,
            },
        )
        .unwrap();
    assert_eq!(committed.links.len(), 1);

    let endpoint = LocalEndpoint::new(&replacement.runtime_root, replacement_session_id);
    let cancel = CancellationToken::new();
    let mut first_controller = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(replacement_session_id, nonce(1)),
        &cancel,
    )
    .await
    .unwrap();
    let first_output = first_controller
        .attach(OutputSequence::ZERO, 100, 30, &cancel)
        .await
        .unwrap();
    let output = first_output
        .iter()
        .flat_map(|event| event.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert!(String::from_utf8_lossy(&output).contains("RESUME-HOST-READY"));
    let watermark = first_output
        .last()
        .map_or(OutputSequence::ZERO, |event| event.sequence);
    first_controller.disconnect();
    drop(first_controller);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        replacement_process.try_wait().unwrap().is_none(),
        "detaching after Ready stopped the replacement Host"
    );

    let mut restarted_controller = HostClient::connect(
        endpoint,
        ConnectOptions::local(replacement_session_id, nonce(2)),
        &cancel,
    )
    .await
    .unwrap();
    let replay = restarted_controller
        .attach(watermark, 100, 30, &cancel)
        .await
        .unwrap();
    let mut sequences = BTreeSet::new();
    for event in replay {
        assert!(event.sequence > watermark);
        assert!(sequences.insert(event.sequence));
    }
    assert_eq!(directory_snapshot(&source.session_dir), source_before);
    assert_eq!(
        continuity
            .record(initial_continuity.revision, committed.links[0].clone(),)
            .unwrap()
            .revision,
        committed.revision
    );

    restarted_controller
        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
        .await
        .unwrap();
    wait_for_exit(&mut replacement_process).await;
    assert_eq!(
        start(replacement).await.unwrap_err().code,
        HostErrorCode::DescriptorInvalid
    );
    assert_eq!(directory_snapshot(&source.session_dir), source_before);
}
