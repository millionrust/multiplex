#![cfg(unix)]

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use termirust_client::{ClientErrorCode, ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use termirust_host_protocol::wire;
use termirust_session_host::{LaunchDescriptor, StopDeadlines, start};
use termirust_store::JournalLimits;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Deserialize)]
struct GoldenFixture {
    schema_version: u16,
    session: GoldenSession,
    controller: GoldenController,
    commands: GoldenCommands,
    input_bytes: Vec<u8>,
    viewport: GoldenViewport,
    scenarios: Vec<String>,
}

#[derive(Deserialize)]
struct GoldenSession {
    session_id: Uuid,
    host_instance_id: Uuid,
    occupant_generation: u64,
    session_generation: u64,
    origin: String,
    runtime: String,
    capabilities: Vec<String>,
    last_output_sequence: u64,
}

#[derive(Deserialize)]
struct GoldenCommands {
    first_writer: Uuid,
    second_writer: Uuid,
    input: Uuid,
    release: Uuid,
    second_after_release: Uuid,
    denied_input: Uuid,
    reconnect_writer: Uuid,
    resize: Uuid,
    stop: Uuid,
}

#[derive(Deserialize)]
struct GoldenController {
    device_id: Uuid,
    identity_generation: u64,
    revocation_epoch: u64,
    capability_bits: u16,
}

#[derive(Deserialize)]
struct GoldenViewport {
    columns: u16,
    rows: u16,
}

fn fixture() -> GoldenFixture {
    serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/universal-session-v1/golden.json"
    ))
    .unwrap()
}

fn command(value: Uuid) -> CommandId {
    CommandId::from_uuid(value)
}

async fn wait_for_output(client: &mut HostClient, cancel: &CancellationToken) {
    for _ in 0..100 {
        if client
            .attach(OutputSequence::ZERO, 120, 40, cancel)
            .await
            .is_ok_and(|frames| {
                String::from_utf8_lossy(
                    &frames
                        .iter()
                        .flat_map(|frame| frame.bytes.iter().copied())
                        .collect::<Vec<_>>(),
                )
                .contains("HOST-OUT:pwd")
            })
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("golden input did not reach Host output before timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn universal_session_fixture_proves_identity_writer_and_reconnect_contract() {
    let fixture = fixture();
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(fixture.session.occupant_generation, 7);
    assert_eq!(fixture.session.session_generation, 11);
    assert_eq!(fixture.session.origin, "managed_agent");
    assert_eq!(fixture.session.runtime, "codex");
    assert_eq!(fixture.session.last_output_sequence, 40);
    assert_eq!(fixture.session.capabilities.len(), 5);
    assert_ne!(fixture.controller.device_id, Uuid::nil());
    assert_eq!(fixture.controller.identity_generation, 1);
    assert_eq!(fixture.controller.revocation_epoch, 3);
    assert_eq!(fixture.controller.capability_bits, 0x1f);
    assert_eq!(fixture.scenarios.len(), 5);

    let temp = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::from_uuid(fixture.session.session_id);
    let host_instance_id = HostInstanceId::from_uuid(fixture.session.host_instance_id);
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id,
        expected_occupant_generation: None,
        runtime_root: temp.path().join("runtime"),
        session_dir: temp.path().join("session"),
        executable: "/bin/sh".into(),
        runtime_detection: None,
        arguments: vec![
            "-c".into(),
            "trap '' INT TERM; printf 'HOST-READY\\n'; while IFS= read -r line; do printf 'HOST-OUT:%s\\n' \"$line\"; done".into(),
        ],
        environment: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        cwd: Some(temp.path().to_path_buf()),
        columns: fixture.viewport.columns,
        rows: fixture.viewport.rows,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines {
            interrupt_millis: 50,
            terminate_millis: 100,
            total_millis: 500,
        },
    };
    let host = start(descriptor).await.unwrap();
    let endpoint = LocalEndpoint::new(host.runtime_root(), session_id);
    let cancel = CancellationToken::new();
    let mut first = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local_read_only(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let mut second = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local_read_only(session_id, [2; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(first.host_instance_id(), Some(host_instance_id));
    assert_eq!(second.host_instance_id(), Some(host_instance_id));

    assert!(
        first
            .set_writer_lease(command(fixture.commands.first_writer), true, &cancel)
            .await
            .unwrap()
    );
    assert!(
        !second
            .set_writer_lease(command(fixture.commands.second_writer), true, &cancel)
            .await
            .unwrap()
    );

    let input_id = command(fixture.commands.input);
    assert!(
        first
            .input(input_id, fixture.input_bytes.clone(), &cancel)
            .await
            .unwrap()
    );
    assert!(
        first
            .input(input_id, fixture.input_bytes.clone(), &cancel)
            .await
            .unwrap()
    );
    wait_for_output(&mut first, &cancel).await;
    let replay = first
        .attach(
            OutputSequence::ZERO,
            u32::from(fixture.viewport.columns),
            u32::from(fixture.viewport.rows),
            &cancel,
        )
        .await
        .unwrap();
    let replay_text = String::from_utf8_lossy(
        &replay
            .iter()
            .flat_map(|frame| frame.bytes.iter().copied())
            .collect::<Vec<_>>(),
    )
    .into_owned();
    assert_eq!(replay_text.matches("HOST-OUT:pwd").count(), 1);
    let acknowledged = replay.last().unwrap().sequence;

    assert!(
        !first
            .set_writer_lease(command(fixture.commands.release), false, &cancel)
            .await
            .unwrap()
    );
    assert!(
        second
            .set_writer_lease(
                command(fixture.commands.second_after_release),
                true,
                &cancel,
            )
            .await
            .unwrap()
    );
    assert_eq!(
        first
            .input(
                command(fixture.commands.denied_input),
                b"denied\n".to_vec(),
                &cancel,
            )
            .await
            .unwrap_err()
            .code,
        ClientErrorCode::PermissionDenied
    );
    second.disconnect();
    first.disconnect();

    let mut reconnected = HostClient::connect(
        endpoint,
        ConnectOptions::local_read_only(session_id, [3; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(reconnected.host_instance_id(), Some(host_instance_id));
    assert!(
        reconnected
            .attach(
                acknowledged,
                u32::from(fixture.viewport.columns),
                u32::from(fixture.viewport.rows),
                &cancel,
            )
            .await
            .unwrap()
            .is_empty(),
        "reconnect from the acknowledged watermark must not duplicate output"
    );
    assert!(
        reconnected
            .set_writer_lease(command(fixture.commands.reconnect_writer), true, &cancel,)
            .await
            .unwrap()
    );
    assert!(
        reconnected
            .resize(
                command(fixture.commands.resize),
                u32::from(fixture.viewport.columns),
                u32::from(fixture.viewport.rows),
                &cancel,
            )
            .await
            .unwrap()
    );
    assert!(
        reconnected
            .stop(
                command(fixture.commands.stop),
                wire::StopMode::Graceful,
                &cancel,
            )
            .await
            .unwrap()
    );
    host.wait().await.unwrap();
}
