use std::sync::Arc;
use std::sync::mpsc::Sender;

use crate::local::spawn_local_session;
use crate::models::{ConnectRequest, ConnectionKind};
use crate::ssh::{SessionRuntimeHandle, SshEvent, spawn_session};
use crate::storage::KnownHostStore;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionWorkerKind {
    Local,
    Ssh,
}

impl ConnectionWorkerKind {
    fn for_request(request: &ConnectRequest) -> Self {
        match request.kind {
            ConnectionKind::LocalShell => Self::Local,
            ConnectionKind::Ssh => Self::Ssh,
        }
    }
}

trait ConnectionWorkerSpawner: Send + Sync {
    fn spawn_local(
        &self,
        request: ConnectRequest,
        event_tx: Sender<SshEvent>,
    ) -> SessionRuntimeHandle;

    fn spawn_ssh(
        &self,
        request: ConnectRequest,
        known_hosts: Arc<KnownHostStore>,
        event_tx: Sender<SshEvent>,
        keepalive_secs: u16,
    ) -> SessionRuntimeHandle;
}

struct SystemConnectionWorkerSpawner;

impl ConnectionWorkerSpawner for SystemConnectionWorkerSpawner {
    fn spawn_local(
        &self,
        request: ConnectRequest,
        event_tx: Sender<SshEvent>,
    ) -> SessionRuntimeHandle {
        spawn_local_session(request, event_tx)
    }

    fn spawn_ssh(
        &self,
        request: ConnectRequest,
        known_hosts: Arc<KnownHostStore>,
        event_tx: Sender<SshEvent>,
        keepalive_secs: u16,
    ) -> SessionRuntimeHandle {
        spawn_session(request, known_hosts, event_tx, keepalive_secs)
    }
}

pub(super) struct ConnectionCoordinator {
    event_tx: Sender<SshEvent>,
    known_hosts: Arc<KnownHostStore>,
    ssh_keepalive_secs: u16,
    worker_spawner: Box<dyn ConnectionWorkerSpawner>,
}

impl ConnectionCoordinator {
    pub fn new(
        event_tx: Sender<SshEvent>,
        known_hosts: Arc<KnownHostStore>,
        ssh_keepalive_secs: u16,
    ) -> Self {
        Self {
            event_tx,
            known_hosts,
            ssh_keepalive_secs,
            worker_spawner: Box::new(SystemConnectionWorkerSpawner),
        }
    }

    pub fn set_ssh_keepalive_secs(&mut self, ssh_keepalive_secs: u16) {
        self.ssh_keepalive_secs = ssh_keepalive_secs;
    }

    pub fn start(&self, request: ConnectRequest) -> SessionRuntimeHandle {
        match ConnectionWorkerKind::for_request(&request) {
            ConnectionWorkerKind::Local => self
                .worker_spawner
                .spawn_local(request, self.event_tx.clone()),
            ConnectionWorkerKind::Ssh => self.worker_spawner.spawn_ssh(
                request,
                self.known_hosts.clone(),
                self.event_tx.clone(),
                self.ssh_keepalive_secs,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc as tokio_mpsc;

    use super::{ConnectionCoordinator, ConnectionWorkerSpawner};
    use crate::models::{ConnectRequest, ConnectionKind};
    use crate::ssh::{SessionRuntimeHandle, SshEvent};
    use crate::storage::KnownHostStore;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum WorkerCall {
        Local {
            session_id: u64,
        },
        Ssh {
            session_id: u64,
            keepalive_secs: u16,
            shared_known_hosts: bool,
        },
    }

    struct RecordingSpawner {
        calls: Arc<Mutex<Vec<WorkerCall>>>,
        expected_known_hosts: Arc<KnownHostStore>,
    }

    impl RecordingSpawner {
        fn runtime() -> SessionRuntimeHandle {
            let (command_tx, _command_rx) = tokio_mpsc::unbounded_channel();
            SessionRuntimeHandle { command_tx }
        }

        fn report_event(event_tx: Sender<SshEvent>, session_id: u64) {
            event_tx
                .send(SshEvent::Error {
                    session_id,
                    message: "synthetic worker event".to_string(),
                })
                .unwrap();
        }
    }

    impl ConnectionWorkerSpawner for RecordingSpawner {
        fn spawn_local(
            &self,
            request: ConnectRequest,
            event_tx: Sender<SshEvent>,
        ) -> SessionRuntimeHandle {
            self.calls.lock().unwrap().push(WorkerCall::Local {
                session_id: request.session_id,
            });
            Self::report_event(event_tx, request.session_id);
            Self::runtime()
        }

        fn spawn_ssh(
            &self,
            request: ConnectRequest,
            known_hosts: Arc<KnownHostStore>,
            event_tx: Sender<SshEvent>,
            keepalive_secs: u16,
        ) -> SessionRuntimeHandle {
            self.calls.lock().unwrap().push(WorkerCall::Ssh {
                session_id: request.session_id,
                keepalive_secs,
                shared_known_hosts: Arc::ptr_eq(&known_hosts, &self.expected_known_hosts),
            });
            Self::report_event(event_tx, request.session_id);
            Self::runtime()
        }
    }

    fn coordinator_fixture() -> (
        ConnectionCoordinator,
        mpsc::Receiver<SshEvent>,
        Arc<Mutex<Vec<WorkerCall>>>,
    ) {
        let (event_tx, event_rx) = mpsc::channel();
        let known_hosts = Arc::new(KnownHostStore::load().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let worker_spawner = RecordingSpawner {
            calls: calls.clone(),
            expected_known_hosts: known_hosts.clone(),
        };
        let coordinator = ConnectionCoordinator {
            event_tx,
            known_hosts,
            ssh_keepalive_secs: 12,
            worker_spawner: Box::new(worker_spawner),
        };
        (coordinator, event_rx, calls)
    }

    #[test]
    fn local_request_uses_local_worker_and_shared_event_sink() {
        let (coordinator, event_rx, calls) = coordinator_fixture();
        let request = ConnectRequest::local_shell(41);

        let _runtime = coordinator.start(request);

        assert_eq!(
            *calls.lock().unwrap(),
            vec![WorkerCall::Local { session_id: 41 }]
        );
        assert!(matches!(
            event_rx.recv().unwrap(),
            SshEvent::Error { session_id: 41, .. }
        ));
    }

    #[test]
    fn ssh_request_uses_shared_known_hosts_event_sink_and_current_keepalive() {
        let (mut coordinator, event_rx, calls) = coordinator_fixture();
        coordinator.set_ssh_keepalive_secs(27);
        let mut request = ConnectRequest::local_shell(42);
        request.kind = ConnectionKind::Ssh;

        let _runtime = coordinator.start(request);

        assert_eq!(
            *calls.lock().unwrap(),
            vec![WorkerCall::Ssh {
                session_id: 42,
                keepalive_secs: 27,
                shared_known_hosts: true,
            }]
        );
        assert!(matches!(
            event_rx.recv().unwrap(),
            SshEvent::Error { session_id: 42, .. }
        ));
    }

    #[test]
    fn connection_coordinator_is_the_only_ui_worker_start_boundary() {
        let app_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/app");
        for entry in fs::read_dir(app_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|value| value.to_str()) != Some("rs")
                || path.file_name().and_then(|value| value.to_str())
                    == Some("connection_coordinator.rs")
            {
                continue;
            }
            let source = fs::read_to_string(&path).unwrap();
            assert!(
                !source.contains("spawn_local_session("),
                "{} bypasses ConnectionCoordinator",
                path.display()
            );
            assert!(
                !source.contains("spawn_session("),
                "{} bypasses ConnectionCoordinator",
                path.display()
            );
        }
    }

    #[test]
    fn coordinator_module_has_no_ui_framework_dependency() {
        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("connection_coordinator.rs").contains(&forbidden_crate));
    }
}
