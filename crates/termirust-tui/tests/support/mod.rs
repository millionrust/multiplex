#![cfg(unix)]

use std::collections::BTreeMap;
use std::time::Duration;

use termirust_domain::{HostInstanceId, HostedSessionId};
use termirust_session_host::{LaunchDescriptor, StopDeadlines};
use termirust_store::JournalLimits;

pub fn descriptor(fixture: &tempfile::TempDir, session_id: HostedSessionId) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: fixture.path().join("runtime"),
        session_dir: fixture.path().join("session"),
        executable: "/bin/sh".into(),
        runtime_detection: None,
        arguments: vec![
            "-c".into(),
            "printf 'TUI-HOST-READY\\n'; while IFS= read -r line; do printf 'TUI-ECHO:%s\\n' \"$line\"; done".into(),
        ],
        environment: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        cwd: Some(fixture.path().to_path_buf()),
        columns: 80,
        rows: 24,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines {
            interrupt_millis: 50,
            terminate_millis: 100,
            total_millis: 500,
        },
    }
}

pub async fn wait_for_output(
    receiver: &std::sync::mpsc::Receiver<termirust_tui::AttachEvent>,
    model: &mut termirust_tui::AttachedTerminal,
    needle: &str,
) {
    for _ in 0..100 {
        let event = receiver
            .recv_timeout(Duration::from_millis(100))
            .expect("attach worker stopped before expected output");
        model.apply(event);
        if model.terminal().contents().contains(needle) {
            return;
        }
    }
    panic!("terminal output did not contain the expected marker");
}
