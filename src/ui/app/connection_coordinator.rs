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
    auto_reconnect_attempts: u8,
    auto_reconnect_delay_secs: u8,
    worker_spawner: Box<dyn ConnectionWorkerSpawner>,
}

impl ConnectionCoordinator {
    pub fn new(
        event_tx: Sender<SshEvent>,
        known_hosts: Arc<KnownHostStore>,
        ssh_keepalive_secs: u16,
        auto_reconnect_attempts: u8,
        auto_reconnect_delay_secs: u8,
    ) -> Self {
        Self {
            event_tx,
            known_hosts,
            ssh_keepalive_secs,
            auto_reconnect_attempts,
            auto_reconnect_delay_secs,
            worker_spawner: Box::new(SystemConnectionWorkerSpawner),
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
    use std::path::Path;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc as tokio_mpsc;

    use super::{
        ConnectionCoordinator, ConnectionWorkerSpawner, ReconnectScheduleDecision,
        ReconnectScheduleInput, ReconnectTickDecision, ReconnectTickInput,
    };
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
            auto_reconnect_attempts: 3,
            auto_reconnect_delay_secs: 4,
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
