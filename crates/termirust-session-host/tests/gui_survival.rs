#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{
    CommandId, HostInstanceId, HostLifecycle, HostedSessionId, OccupantOwnership, OutputSequence,
    RecognitionConfidence, RuntimeCapability, RuntimeCapabilitySet, RuntimeDetectionResult,
    RuntimeDetectionStatus, RuntimeId,
};
use termirust_host_protocol::wire;
use termirust_session_host::{
    LaunchDescriptor, StopDeadlines, process_observation::fingerprint_executable,
};
use termirust_store::{JournalLimits, read_host_metadata};
use tokio_util::sync::CancellationToken;

fn nonce(index: u64) -> [u8; 32] {
    let mut value = [0_u8; 32];
    value[..8].copy_from_slice(&index.to_le_bytes());
    value[8..16].copy_from_slice(&index.wrapping_mul(17).to_le_bytes());
    value
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_survives_one_thousand_gui_drops_during_ordered_replay() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        runtime_root: fixture.path().join("runtime"),
        session_dir: fixture.path().join("session"),
        executable: "/bin/sh".into(),
        runtime_detection: None,
        arguments: vec![
            "-c".to_string(),
            "i=1; while [ $i -le 1000 ]; do printf 'COUNT:%04d\\n' \"$i\"; i=$((i+1)); sleep 0.001; done; while IFS= read -r line; do printf 'INPUT:%s\\n' \"$line\"; done".to_string(),
        ],
        environment: BTreeMap::from([
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
        ]),
        cwd: Some(fixture.path().to_path_buf()),
        columns: 100,
        rows: 30,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    };
    let mut process = Command::new(env!("CARGO_BIN_EXE_termirust-session-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    serde_json::to_writer(process.stdin.as_mut().unwrap(), &descriptor).unwrap();
    process.stdin.take().unwrap().flush().unwrap();
    let mut ready = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("host_ready"));

    let endpoint = LocalEndpoint::new(&descriptor.runtime_root, session_id);
    let cancel = CancellationToken::new();
    let mut watermark = OutputSequence::ZERO;
    let mut terminal_bytes = Vec::new();
    for cycle in 1..=1_000_u64 {
        let mut client = HostClient::connect(
            endpoint.clone(),
            ConnectOptions::local(session_id, nonce(cycle)),
            &cancel,
        )
        .await
        .unwrap();
        for output in client.attach(watermark, 100, 30, &cancel).await.unwrap() {
            assert_eq!(output.sequence, watermark.checked_next().unwrap());
            watermark = output.sequence;
            terminal_bytes.extend_from_slice(&output.bytes);
        }
        if cycle % 2 == 0 {
            client.detach(&cancel).await.unwrap();
        } else {
            client.disconnect();
        }
        assert!(
            process.try_wait().unwrap().is_none(),
            "Host died at cycle {cycle}"
        );
    }

    let mut final_client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, nonce(1_001)),
        &cancel,
    )
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !String::from_utf8_lossy(&terminal_bytes).contains("COUNT:1000")
        && Instant::now() < deadline
    {
        for output in final_client
            .attach(watermark, 100, 30, &cancel)
            .await
            .unwrap()
        {
            assert_eq!(output.sequence, watermark.checked_next().unwrap());
            watermark = output.sequence;
            terminal_bytes.extend_from_slice(&output.bytes);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(String::from_utf8_lossy(&terminal_bytes).contains("COUNT:1000"));
    final_client
        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(6);
    while process.try_wait().unwrap().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(process.try_wait().unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires TERMIRUST_REAL_AGENT_EXECUTABLE and TERMIRUST_REAL_AGENT_VERSION"]
async fn real_managed_agent_survives_controller_restart() {
    let executable = std::env::var_os("TERMIRUST_REAL_AGENT_EXECUTABLE")
        .map(std::path::PathBuf::from)
        .expect("set TERMIRUST_REAL_AGENT_EXECUTABLE to the installed agent executable")
        .canonicalize()
        .expect("the installed agent executable must resolve to a regular file");
    let cwd = std::env::var_os("TERMIRUST_REAL_AGENT_CWD")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let runtime_id = std::env::var("TERMIRUST_REAL_AGENT_ID").unwrap_or_else(|_| "codex".into());
    let runtime_version = std::env::var("TERMIRUST_REAL_AGENT_VERSION")
        .expect("set TERMIRUST_REAL_AGENT_VERSION to the installed agent's numeric version");
    let fingerprint = fingerprint_executable(&executable).unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let host_instance_id = HostInstanceId::new();
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id,
        runtime_root: fixture.path().join("runtime"),
        session_dir: fixture.path().join("session"),
        executable,
        runtime_detection: Some(RuntimeDetectionResult {
            runtime_id: RuntimeId::new(&runtime_id).unwrap(),
            descriptor_version: 1,
            status: RuntimeDetectionStatus::Available,
            fingerprint: Some(fingerprint),
            safe_version: Some(runtime_version),
            capabilities: RuntimeCapabilitySet::new([
                RuntimeCapability::InteractivePty,
                RuntimeCapability::Cancellation,
            ]),
            diagnostic_code: None,
        }),
        arguments: Vec::new(),
        environment: ["HOME", "LANG", "LC_ALL", "PATH", "SHELL", "USER"]
            .into_iter()
            .filter_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| (name.to_string(), value))
            })
            .collect(),
        cwd: Some(cwd),
        columns: 100,
        rows: 30,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    };
    let mut process = Command::new(env!("CARGO_BIN_EXE_termirust-session-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(process.stdin.as_mut().unwrap(), &descriptor).unwrap();
    process.stdin.take().unwrap().flush().unwrap();
    let mut ready = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    if !ready.contains("host_ready") {
        let mut diagnostic = String::new();
        process
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut diagnostic)
            .unwrap();
        panic!("Host did not become ready: {}", diagnostic.trim());
    }

    let metadata_before = read_host_metadata(&descriptor.session_dir).unwrap();
    assert_eq!(metadata_before.lifecycle, HostLifecycle::Ready);
    let recognition_before = metadata_before
        .runtime_recognition
        .expect("the exact executable fingerprint should produce managed recognition");
    assert_eq!(
        recognition_before.confidence,
        RecognitionConfidence::Verified
    );
    assert!(matches!(
        recognition_before
            .occupant
            .as_ref()
            .map(|occupant| &occupant.ownership),
        Some(OccupantOwnership::Managed { host_instance, .. })
            if *host_instance == host_instance_id
    ));

    let endpoint = LocalEndpoint::new(&descriptor.runtime_root, session_id);
    let cancel = CancellationToken::new();
    let mut first_controller = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(session_id, nonce(2_001)),
        &cancel,
    )
    .await
    .unwrap();
    let first_output = first_controller
        .attach(OutputSequence::ZERO, 100, 30, &cancel)
        .await
        .unwrap();
    let watermark = first_output
        .last()
        .map_or(OutputSequence::ZERO, |output| output.sequence);
    first_controller.disconnect();
    drop(first_controller);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        process.try_wait().unwrap().is_none(),
        "closing the first Controller terminated the managed agent"
    );

    let mut restarted_controller = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, nonce(2_002)),
        &cancel,
    )
    .await
    .unwrap();
    let replay = restarted_controller
        .attach(watermark, 100, 30, &cancel)
        .await
        .unwrap();
    let mut sequence = watermark;
    for output in replay {
        assert_eq!(output.sequence, sequence.checked_next().unwrap());
        sequence = output.sequence;
    }
    let metadata_after = read_host_metadata(&descriptor.session_dir).unwrap();
    assert_eq!(metadata_after.lifecycle, HostLifecycle::Ready);
    let recognition_after = metadata_after
        .runtime_recognition
        .expect("managed recognition must survive Controller restart");
    assert_eq!(recognition_after, recognition_before);

    restarted_controller
        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    while process.try_wait().unwrap().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(process.try_wait().unwrap().is_some());
}
