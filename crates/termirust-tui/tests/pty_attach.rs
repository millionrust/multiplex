#![cfg(unix)]

mod support;

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use termirust_client::LocalEndpoint;
use termirust_domain::HostedSessionId;
use termirust_session_host::start;
use termirust_tui::{
    AttachCommand, AttachedTerminal, TuiAttachState, Viewport, spawn_attach_worker,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_pty_replays_accepts_input_resizes_and_survives_detach() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let descriptor = support::descriptor(&fixture, session_id);
    let host = start(descriptor).await.unwrap();
    let endpoint = LocalEndpoint::new(host.runtime_root(), session_id);
    let (event_tx, event_rx) = mpsc::sync_channel(8);
    let sink = Arc::new(move |event| event_tx.send(event).is_ok());
    let viewport = Viewport::new(80, 24);
    let mut model = AttachedTerminal::new(1, session_id, "PTY fixture".into(), viewport);
    let worker = spawn_attach_worker(1, endpoint.clone(), session_id, viewport, sink).unwrap();

    support::wait_for_output(&event_rx, &mut model, "TUI-HOST-READY").await;
    assert_eq!(model.state(), TuiAttachState::LiveInteractive);
    let input_started = Instant::now();
    assert!(worker.try_send(AttachCommand::Input(b"hello-tui\n".to_vec())));
    support::wait_for_output(&event_rx, &mut model, "TUI-ECHO:hello-tui").await;
    assert!(
        input_started.elapsed() < Duration::from_secs(2),
        "local input-to-render latency exceeded the bounded integration deadline"
    );

    assert!(worker.try_send(AttachCommand::Resize(Viewport::new(100, 30))));
    assert!(worker.try_send(AttachCommand::Input(b"resize-ok\n".to_vec())));
    support::wait_for_output(&event_rx, &mut model, "TUI-ECHO:resize-ok").await;
    assert!(worker.try_send(AttachCommand::Detach));
    loop {
        let event = event_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        model.apply(event);
        if model.state() == TuiAttachState::Detached {
            break;
        }
    }
    drop(worker);
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready,
        "TUI detach must not stop the durable Host"
    );

    let (reconnect_tx, reconnect_rx) = mpsc::sync_channel(8);
    let reconnect_sink = Arc::new(move |event| reconnect_tx.send(event).is_ok());
    let mut reconnected = AttachedTerminal::new(2, session_id, "PTY fixture".into(), viewport);
    let reconnect_worker =
        spawn_attach_worker(2, endpoint, session_id, viewport, reconnect_sink).unwrap();
    support::wait_for_output(&reconnect_rx, &mut reconnected, "TUI-ECHO:resize-ok").await;
    assert_eq!(reconnected.state(), TuiAttachState::LiveInteractive);
    assert!(reconnect_worker.try_send(AttachCommand::Input(b"after-reconnect\n".to_vec())));
    support::wait_for_output(&reconnect_rx, &mut reconnected, "TUI-ECHO:after-reconnect").await;
    assert_eq!(
        reconnected
            .terminal()
            .contents()
            .matches("TUI-ECHO:after-reconnect")
            .count(),
        1,
        "reconnect must not duplicate acknowledged input"
    );
    assert!(reconnect_worker.try_send(AttachCommand::Detach));
    loop {
        let event = reconnect_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        reconnected.apply(event);
        if reconnected.state() == TuiAttachState::Detached {
            break;
        }
    }
    drop(reconnect_worker);
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready,
        "reconnected TUI detach must leave the durable Host running"
    );
    host.shutdown().await.unwrap();
}
