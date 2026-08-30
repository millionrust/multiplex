#![cfg(unix)]

mod support;

use std::sync::{Arc, mpsc};
use std::time::Duration;

use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{CommandId, HostedSessionId};
use termirust_session_host::start;
use termirust_tui::{
    AttachCommand, AttachedTerminal, InteractiveLease, Viewport, spawn_attach_worker,
};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_client_cannot_write_until_tui_detaches_and_releases_lease() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let descriptor = support::descriptor(&fixture, session_id);
    let host = start(descriptor).await.unwrap();
    let endpoint = LocalEndpoint::new(host.runtime_root(), session_id);
    let (event_tx, event_rx) = mpsc::sync_channel(8);
    let sink = Arc::new(move |event| event_tx.send(event).is_ok());
    let viewport = Viewport::new(80, 24);
    let mut model = AttachedTerminal::new(7, session_id, "Lease fixture".into(), viewport);
    let worker = spawn_attach_worker(7, endpoint.clone(), session_id, viewport, sink).unwrap();
    support::wait_for_output(&event_rx, &mut model, "TUI-HOST-READY").await;
    assert_eq!(model.input().lease(), InteractiveLease::Interactive);

    let cancel = CancellationToken::new();
    let mut contender = HostClient::connect(
        endpoint,
        ConnectOptions::local_read_only(session_id, [9; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(
        !contender
            .set_writer_lease(CommandId::new(), true, &cancel)
            .await
            .unwrap()
    );
    assert!(worker.try_send(AttachCommand::Detach));
    loop {
        let event = event_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        if matches!(event, termirust_tui::AttachEvent::Detached { .. }) {
            break;
        }
    }
    drop(worker);
    assert!(
        contender
            .set_writer_lease(CommandId::new(), true, &cancel)
            .await
            .unwrap()
    );
    contender.detach(&cancel).await.unwrap();
    host.shutdown().await.unwrap();
}
