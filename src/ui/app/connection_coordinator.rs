use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use crate::local::spawn_local_session;
use crate::models::{ConnectRequest, ConnectionKind};
use crate::sftp::{
    SftpEvent, spawn_delete_path, spawn_download_file, spawn_list_directory, spawn_upload_file,
};
use crate::ssh::{SessionRuntimeHandle, SshEvent, spawn_session};
use crate::storage::KnownHostStore;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionWorkerKind {
    Local,
    Ssh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReconnectScheduleInput {
    pub user_closed: bool,
    pub local_shell: bool,
    pub attempts: u8,
    pub now_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ReconnectScheduleDecision {
    Disabled,
    UserClosed,
    LocalShell,
    Exhausted,
    Schedule {
        attempts: u8,
        at_millis: u64,
        status: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ReconnectTickInput<'a> {
    pub target_millis: Option<u64>,
    pub user_closed: bool,
    pub attempts: u8,
    pub now_millis: u64,
    pub current_status: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ReconnectTickDecision {
    Ignore,
    Waiting { status: String, changed: bool },
    Due,
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

// Transfer handlers are retained for the existing Files workflows and focused tests,
// even where the current production UI does not yet expose their commands.
#[allow(dead_code)]
pub(super) enum SftpOperationRequest {
    List {
        workspace_id: u64,
        operation_id: u64,
        request: ConnectRequest,
        path: String,
    },
    Upload {
        workspace_id: u64,
        operation_id: u64,
        request: ConnectRequest,
        remote_dir: String,
        local_path: PathBuf,
    },
    Download {
        workspace_id: u64,
        operation_id: u64,
        request: ConnectRequest,
        remote_path: String,
        local_path: PathBuf,
    },
    Delete {
        workspace_id: u64,
        operation_id: u64,
        request: ConnectRequest,
        remote_path: String,
        is_dir: bool,
    },
}

trait SftpWorkerSpawner: Send + Sync {
    fn spawn(
        &self,
        operation: SftpOperationRequest,
        known_hosts: Arc<KnownHostStore>,
        event_tx: Sender<SftpEvent>,
    );
}

struct SystemConnectionWorkerSpawner;
struct SystemSftpWorkerSpawner;

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

impl SftpWorkerSpawner for SystemSftpWorkerSpawner {
    fn spawn(
        &self,
        operation: SftpOperationRequest,
        known_hosts: Arc<KnownHostStore>,
        event_tx: Sender<SftpEvent>,
    ) {
        match operation {
            SftpOperationRequest::List {
                workspace_id,
                operation_id,
                request,
                path,
            } => spawn_list_directory(
                workspace_id,
                operation_id,
                request,
                known_hosts,
                path,
                event_tx,
            ),
            SftpOperationRequest::Upload {
                workspace_id,
                operation_id,
                request,
                remote_dir,
                local_path,
            } => spawn_upload_file(
                workspace_id,
                operation_id,
                request,
                known_hosts,
                remote_dir,
                local_path,
                event_tx,
            ),
            SftpOperationRequest::Download {
                workspace_id,
                operation_id,
                request,
                remote_path,
                local_path,
            } => spawn_download_file(
                workspace_id,
                operation_id,
                request,
                known_hosts,
                remote_path,
                local_path,
                event_tx,
            ),
            SftpOperationRequest::Delete {
                workspace_id,
                operation_id,
                request,
                remote_path,
                is_dir,
            } => spawn_delete_path(
                workspace_id,
                operation_id,
                request,
                known_hosts,
                remote_path,
                is_dir,
                event_tx,
            ),
        }
    }
}

pub(super) struct ConnectionCoordinator {
    event_tx: Sender<SshEvent>,
    sftp_event_tx: Sender<SftpEvent>,
    known_hosts: Arc<KnownHostStore>,
    ssh_keepalive_secs: u16,
    auto_reconnect_attempts: u8,
    auto_reconnect_delay_secs: u8,
    worker_spawner: Box<dyn ConnectionWorkerSpawner>,
    sftp_worker_spawner: Box<dyn SftpWorkerSpawner>,
}

impl ConnectionCoordinator {
    pub fn new(
        event_tx: Sender<SshEvent>,
        sftp_event_tx: Sender<SftpEvent>,
        known_hosts: Arc<KnownHostStore>,
        ssh_keepalive_secs: u16,
        auto_reconnect_attempts: u8,
        auto_reconnect_delay_secs: u8,
    ) -> Self {
        Self {
            event_tx,
            sftp_event_tx,
            known_hosts,
            ssh_keepalive_secs,
            auto_reconnect_attempts,
            auto_reconnect_delay_secs,
            worker_spawner: Box::new(SystemConnectionWorkerSpawner),
            sftp_worker_spawner: Box::new(SystemSftpWorkerSpawner),
        }
    }

    pub fn set_ssh_keepalive_secs(&mut self, ssh_keepalive_secs: u16) {
        self.ssh_keepalive_secs = ssh_keepalive_secs;
    }

    pub fn set_auto_reconnect_policy(&mut self, attempts: u8, delay_secs: u8) {
        self.auto_reconnect_attempts = attempts;
        self.auto_reconnect_delay_secs = delay_secs;
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

    pub fn start_sftp(&self, operation: SftpOperationRequest) {
        self.sftp_worker_spawner.spawn(
            operation,
            self.known_hosts.clone(),
            self.sftp_event_tx.clone(),
        );
    }

    pub fn schedule_reconnect(&self, input: ReconnectScheduleInput) -> ReconnectScheduleDecision {
        if self.auto_reconnect_attempts == 0 {
            return ReconnectScheduleDecision::Disabled;
        }
        if input.user_closed {
            return ReconnectScheduleDecision::UserClosed;
        }
        if input.local_shell {
            return ReconnectScheduleDecision::LocalShell;
        }
        if input.attempts >= self.auto_reconnect_attempts {
            return ReconnectScheduleDecision::Exhausted;
        }

        let attempts = input.attempts.saturating_add(1);
        let at_millis = input
            .now_millis
            .saturating_add(u64::from(self.auto_reconnect_delay_secs).saturating_mul(1000));
        ReconnectScheduleDecision::Schedule {
            attempts,
            at_millis,
            status: reconnect_status(
                u64::from(self.auto_reconnect_delay_secs),
                attempts,
                self.auto_reconnect_attempts,
            ),
        }
    }

    pub fn project_reconnect_tick(&self, input: ReconnectTickInput<'_>) -> ReconnectTickDecision {
        let Some(target_millis) = input.target_millis else {
            return ReconnectTickDecision::Ignore;
        };
        if input.user_closed {
            return ReconnectTickDecision::Ignore;
        }
        if input.now_millis >= target_millis {
            return ReconnectTickDecision::Due;
        }

        let remaining_secs = target_millis
            .saturating_sub(input.now_millis)
            .saturating_add(999)
            / 1000;
        let status = reconnect_status(remaining_secs, input.attempts, self.auto_reconnect_attempts);
        let changed = input.current_status != status;
        ReconnectTickDecision::Waiting { status, changed }
    }
}

fn reconnect_status(remaining_secs: u64, attempts: u8, max_attempts: u8) -> String {
    format!("Reconnecting in {remaining_secs}s ({attempts}/{max_attempts})")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc as tokio_mpsc;

    use super::{
        ConnectionCoordinator, ConnectionWorkerSpawner, ReconnectScheduleDecision,
        ReconnectScheduleInput, ReconnectTickDecision, ReconnectTickInput, SftpOperationRequest,
        SftpWorkerSpawner,
    };
    use crate::models::{ConnectRequest, ConnectionKind};
    use crate::sftp::SftpEvent;
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

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum SftpCall {
        List {
            workspace_id: u64,
            operation_id: u64,
            request: String,
            path: String,
            shared_known_hosts: bool,
        },
        Upload {
            workspace_id: u64,
            operation_id: u64,
            request: String,
            remote_dir: String,
            local_path: PathBuf,
            shared_known_hosts: bool,
        },
        Download {
            workspace_id: u64,
            operation_id: u64,
            request: String,
            remote_path: String,
            local_path: PathBuf,
            shared_known_hosts: bool,
        },
        Delete {
            workspace_id: u64,
            operation_id: u64,
            request: String,
            remote_path: String,
            is_dir: bool,
            shared_known_hosts: bool,
        },
    }

    struct RecordingSftpSpawner {
        calls: Arc<Mutex<Vec<SftpCall>>>,
        expected_known_hosts: Arc<KnownHostStore>,
    }

    impl SftpWorkerSpawner for RecordingSftpSpawner {
        fn spawn(
            &self,
            operation: SftpOperationRequest,
            known_hosts: Arc<KnownHostStore>,
            event_tx: Sender<SftpEvent>,
        ) {
            let shared_known_hosts = Arc::ptr_eq(&known_hosts, &self.expected_known_hosts);
            let (workspace_id, operation_id, call) = match operation {
                SftpOperationRequest::List {
                    workspace_id,
                    operation_id,
                    request,
                    path,
                } => (
                    workspace_id,
                    operation_id,
                    SftpCall::List {
                        workspace_id,
                        operation_id,
                        request: format!("{request:?}"),
                        path,
                        shared_known_hosts,
                    },
                ),
                SftpOperationRequest::Upload {
                    workspace_id,
                    operation_id,
                    request,
                    remote_dir,
                    local_path,
                } => (
                    workspace_id,
                    operation_id,
                    SftpCall::Upload {
                        workspace_id,
                        operation_id,
                        request: format!("{request:?}"),
                        remote_dir,
                        local_path,
                        shared_known_hosts,
                    },
                ),
                SftpOperationRequest::Download {
                    workspace_id,
                    operation_id,
                    request,
                    remote_path,
                    local_path,
                } => (
                    workspace_id,
                    operation_id,
                    SftpCall::Download {
                        workspace_id,
                        operation_id,
                        request: format!("{request:?}"),
                        remote_path,
                        local_path,
                        shared_known_hosts,
                    },
                ),
                SftpOperationRequest::Delete {
                    workspace_id,
                    operation_id,
                    request,
                    remote_path,
                    is_dir,
                } => (
                    workspace_id,
                    operation_id,
                    SftpCall::Delete {
                        workspace_id,
                        operation_id,
                        request: format!("{request:?}"),
                        remote_path,
                        is_dir,
                        shared_known_hosts,
                    },
                ),
            };
            self.calls.lock().unwrap().push(call);
            event_tx
                .send(SftpEvent::Error {
                    workspace_id,
                    operation_id,
                    message: "synthetic SFTP event".to_string(),
                })
                .unwrap();
        }
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
        let (sftp_event_tx, _sftp_event_rx) = mpsc::channel();
        let sftp_worker_spawner = RecordingSftpSpawner {
            calls: Arc::new(Mutex::new(Vec::new())),
            expected_known_hosts: known_hosts.clone(),
        };
        let coordinator = ConnectionCoordinator {
            event_tx,
            sftp_event_tx,
            known_hosts,
            ssh_keepalive_secs: 12,
            auto_reconnect_attempts: 3,
            auto_reconnect_delay_secs: 4,
            worker_spawner: Box::new(worker_spawner),
            sftp_worker_spawner: Box::new(sftp_worker_spawner),
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
    fn reconnect_schedule_classifies_every_ineligible_case() {
        let (mut coordinator, _event_rx, _calls) = coordinator_fixture();
        let input = |user_closed, local_shell, attempts| ReconnectScheduleInput {
            user_closed,
            local_shell,
            attempts,
            now_millis: 1_000,
        };

        coordinator.set_auto_reconnect_policy(0, 4);
        assert_eq!(
            coordinator.schedule_reconnect(input(false, false, 0)),
            ReconnectScheduleDecision::Disabled
        );
        coordinator.set_auto_reconnect_policy(3, 4);
        assert_eq!(
            coordinator.schedule_reconnect(input(true, false, 0)),
            ReconnectScheduleDecision::UserClosed
        );
        assert_eq!(
            coordinator.schedule_reconnect(input(false, true, 0)),
            ReconnectScheduleDecision::LocalShell
        );
        assert_eq!(
            coordinator.schedule_reconnect(input(false, false, 3)),
            ReconnectScheduleDecision::Exhausted
        );
    }

    #[test]
    fn reconnect_schedule_advances_attempt_deadline_and_exact_status() {
        let (coordinator, _event_rx, _calls) = coordinator_fixture();

        assert_eq!(
            coordinator.schedule_reconnect(ReconnectScheduleInput {
                user_closed: false,
                local_shell: false,
                attempts: 1,
                now_millis: 5_000,
            }),
            ReconnectScheduleDecision::Schedule {
                attempts: 2,
                at_millis: 9_000,
                status: "Reconnecting in 4s (2/3)".to_string(),
            }
        );
    }

    #[test]
    fn reconnect_schedule_saturates_deadline_at_clock_limit() {
        let (coordinator, _event_rx, _calls) = coordinator_fixture();

        assert_eq!(
            coordinator.schedule_reconnect(ReconnectScheduleInput {
                user_closed: false,
                local_shell: false,
                attempts: 0,
                now_millis: u64::MAX - 1,
            }),
            ReconnectScheduleDecision::Schedule {
                attempts: 1,
                at_millis: u64::MAX,
                status: "Reconnecting in 4s (1/3)".to_string(),
            }
        );
    }

    #[test]
    fn reconnect_tick_ignores_unscheduled_and_user_closed_panes() {
        let (coordinator, _event_rx, _calls) = coordinator_fixture();
        assert_eq!(
            coordinator.project_reconnect_tick(ReconnectTickInput {
                target_millis: None,
                user_closed: false,
                attempts: 1,
                now_millis: 1_000,
                current_status: "Closed",
            }),
            ReconnectTickDecision::Ignore
        );
        assert_eq!(
            coordinator.project_reconnect_tick(ReconnectTickInput {
                target_millis: Some(2_000),
                user_closed: true,
                attempts: 1,
                now_millis: 1_000,
                current_status: "Closed",
            }),
            ReconnectTickDecision::Ignore
        );
    }

    #[test]
    fn reconnect_tick_uses_ceiling_seconds_changed_state_and_due_boundary() {
        let (coordinator, _event_rx, _calls) = coordinator_fixture();
        assert_eq!(
            coordinator.project_reconnect_tick(ReconnectTickInput {
                target_millis: Some(2_001),
                user_closed: false,
                attempts: 2,
                now_millis: 1_000,
                current_status: "Closed",
            }),
            ReconnectTickDecision::Waiting {
                status: "Reconnecting in 2s (2/3)".to_string(),
                changed: true,
            }
        );
        assert_eq!(
            coordinator.project_reconnect_tick(ReconnectTickInput {
                target_millis: Some(2_001),
                user_closed: false,
                attempts: 2,
                now_millis: 1_001,
                current_status: "Reconnecting in 1s (2/3)",
            }),
            ReconnectTickDecision::Waiting {
                status: "Reconnecting in 1s (2/3)".to_string(),
                changed: false,
            }
        );
        assert_eq!(
            coordinator.project_reconnect_tick(ReconnectTickInput {
                target_millis: Some(2_001),
                user_closed: false,
                attempts: 2,
                now_millis: 2_001,
                current_status: "Reconnecting in 1s (2/3)",
            }),
            ReconnectTickDecision::Due
        );
    }

    #[test]
    fn sftp_operations_preserve_requests_paths_shared_store_and_event_sink() {
        let (event_tx, _event_rx) = mpsc::channel();
        let (sftp_event_tx, sftp_event_rx) = mpsc::channel();
        let known_hosts = Arc::new(KnownHostStore::load().unwrap());
        let connection_calls = Arc::new(Mutex::new(Vec::new()));
        let sftp_calls = Arc::new(Mutex::new(Vec::new()));
        let mut request = ConnectRequest::local_shell(77);
        request.title = "Exact request".to_string();
        request.environment = vec![("TERMIRUST_TEST".to_string(), "exact".to_string())];
        let request_debug = format!("{request:?}");
        let coordinator = ConnectionCoordinator {
            event_tx,
            sftp_event_tx,
            known_hosts: known_hosts.clone(),
            ssh_keepalive_secs: 12,
            auto_reconnect_attempts: 3,
            auto_reconnect_delay_secs: 4,
            worker_spawner: Box::new(RecordingSpawner {
                calls: connection_calls,
                expected_known_hosts: known_hosts.clone(),
            }),
            sftp_worker_spawner: Box::new(RecordingSftpSpawner {
                calls: sftp_calls.clone(),
                expected_known_hosts: known_hosts,
            }),
        };

        coordinator.start_sftp(SftpOperationRequest::List {
            workspace_id: 1,
            operation_id: 11,
            request: request.clone(),
            path: "/srv/project".to_string(),
        });
        coordinator.start_sftp(SftpOperationRequest::Upload {
            workspace_id: 2,
            operation_id: 12,
            request: request.clone(),
            remote_dir: "/srv/upload".to_string(),
            local_path: PathBuf::from("/synthetic/local/upload.txt"),
        });
        coordinator.start_sftp(SftpOperationRequest::Download {
            workspace_id: 3,
            operation_id: 13,
            request: request.clone(),
            remote_path: "/srv/download.txt".to_string(),
            local_path: PathBuf::from("/synthetic/local/download.txt"),
        });
        coordinator.start_sftp(SftpOperationRequest::Delete {
            workspace_id: 4,
            operation_id: 14,
            request,
            remote_path: "/srv/old".to_string(),
            is_dir: true,
        });

        assert_eq!(
            *sftp_calls.lock().unwrap(),
            vec![
                SftpCall::List {
                    workspace_id: 1,
                    operation_id: 11,
                    request: request_debug.clone(),
                    path: "/srv/project".to_string(),
                    shared_known_hosts: true,
                },
                SftpCall::Upload {
                    workspace_id: 2,
                    operation_id: 12,
                    request: request_debug.clone(),
                    remote_dir: "/srv/upload".to_string(),
                    local_path: PathBuf::from("/synthetic/local/upload.txt"),
                    shared_known_hosts: true,
                },
                SftpCall::Download {
                    workspace_id: 3,
                    operation_id: 13,
                    request: request_debug.clone(),
                    remote_path: "/srv/download.txt".to_string(),
                    local_path: PathBuf::from("/synthetic/local/download.txt"),
                    shared_known_hosts: true,
                },
                SftpCall::Delete {
                    workspace_id: 4,
                    operation_id: 14,
                    request: request_debug,
                    remote_path: "/srv/old".to_string(),
                    is_dir: true,
                    shared_known_hosts: true,
                },
            ]
        );
        for (workspace_id, operation_id) in [(1, 11), (2, 12), (3, 13), (4, 14)] {
            assert!(matches!(
                sftp_event_rx.recv().unwrap(),
                SftpEvent::Error {
                    workspace_id: actual_workspace_id,
                    operation_id: actual_operation_id,
                    ..
                } if actual_workspace_id == workspace_id && actual_operation_id == operation_id
            ));
        }
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
            for worker in [
                "spawn_list_directory(",
                "spawn_upload_file(",
                "spawn_download_file(",
                "spawn_delete_path(",
            ] {
                assert!(
                    !source.contains(worker),
                    "{} bypasses ConnectionCoordinator via {worker}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn reconnect_policy_arithmetic_does_not_drift_back_into_app_module() {
        let app_source = include_str!("mod.rs");
        let schedule_start = app_source.find("fn maybe_schedule_auto_reconnect").unwrap();
        let reconnect_start = app_source.find("fn reconnect_pane").unwrap();
        let policy_adapter = &app_source[schedule_start..reconnect_start];
        assert!(policy_adapter.contains("schedule_reconnect"));
        assert!(policy_adapter.contains("project_reconnect_tick"));
        assert!(!policy_adapter.contains("remaining_secs"));
        assert!(!policy_adapter.contains("saturating_mul(1000)"));
        assert!(!policy_adapter.contains("format!(\"Reconnecting in"));
    }

    #[test]
    fn coordinator_module_has_no_ui_framework_dependency() {
        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("connection_coordinator.rs").contains(&forbidden_crate));
    }
}
