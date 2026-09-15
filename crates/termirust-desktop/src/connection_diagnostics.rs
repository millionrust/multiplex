use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tokio::runtime::Builder;
use tokio_util::sync::CancellationToken;

use crate::models::{ConnectRequest, JumpHostConnection};
use crate::ssh::{SshDiagnosticFailure, SshDiagnosticStage, SshDiagnosticStageState};
use crate::storage::KnownHostStore;

pub const MAX_ACTIVE_DIAGNOSTICS: usize = 4;
pub const MAX_QUEUED_DIAGNOSTICS: usize = 64;
pub const MAX_DIAGNOSTIC_BATCH: usize = 64;
const TOTAL_DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticStage {
    Configuration,
    RouteAndAuthentication,
    SessionChannel,
    Sftp,
}

impl DiagnosticStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Configuration => "Configuration",
            Self::RouteAndAuthentication => "Route and authentication",
            Self::SessionChannel => "SSH channel",
            Self::Sftp => "SFTP",
        }
    }
}

impl From<SshDiagnosticStage> for DiagnosticStage {
    fn from(value: SshDiagnosticStage) -> Self {
        match value {
            SshDiagnosticStage::RouteAndAuthenticate => Self::RouteAndAuthentication,
            SshDiagnosticStage::SessionChannel => Self::SessionChannel,
            SshDiagnosticStage::Sftp => Self::Sftp,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticFailureKind {
    UnknownHostKey,
    HostKeyMismatch,
    CredentialDenied,
    RouteUnavailable,
    Timeout,
    SessionChannelUnavailable,
    SftpUnavailable,
    Cancelled,
    Internal,
}

impl DiagnosticFailureKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::UnknownHostKey => "Host key not trusted",
            Self::HostKeyMismatch => "Host key mismatch",
            Self::CredentialDenied => "Authentication denied",
            Self::RouteUnavailable => "Route unavailable",
            Self::Timeout => "Timed out",
            Self::SessionChannelUnavailable => "SSH channel unavailable",
            Self::SftpUnavailable => "SFTP unavailable",
            Self::Cancelled => "Cancelled",
            Self::Internal => "Diagnostic failed",
        }
    }

    pub fn recovery(self) -> &'static str {
        match self {
            Self::UnknownHostKey => {
                "Connect normally once to review and pin this host key, then retry."
            }
            Self::HostKeyMismatch => {
                "Verify the server key changed intentionally before removing the saved key."
            }
            Self::CredentialDenied => {
                "Review the saved username and authentication settings, then retry."
            }
            Self::RouteUnavailable => {
                "Check the host, network, proxy, and jump-host route, then retry."
            }
            Self::Timeout => "Check reachability and route latency, then retry.",
            Self::SessionChannelUnavailable => {
                "The server denied SSH session channels. Review its SSH policy."
            }
            Self::SftpUnavailable => {
                "The SSH route works, but this server does not provide a usable SFTP subsystem."
            }
            Self::Cancelled => "Run Diagnose again when ready.",
            Self::Internal => "Review the application log and retry the diagnostic.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticEvent {
    Queued {
        operation_id: u64,
        profile_id: String,
        title: String,
        address: String,
        route: String,
    },
    StageStarted {
        operation_id: u64,
        stage: DiagnosticStage,
    },
    StagePassed {
        operation_id: u64,
        stage: DiagnosticStage,
        elapsed: Duration,
    },
    Completed {
        operation_id: u64,
        elapsed: Duration,
    },
    Failed {
        operation_id: u64,
        stage: DiagnosticStage,
        kind: DiagnosticFailureKind,
        message: String,
        recovery: String,
        elapsed: Duration,
    },
    Cancelled {
        operation_id: u64,
        stage: DiagnosticStage,
        elapsed: Duration,
    },
}

#[derive(Clone)]
pub struct DiagnosticControl {
    pub operation_id: u64,
    cancellation: CancellationToken,
}

impl DiagnosticControl {
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiagnosticSubmitError {
    AlreadyRunning,
    QueueFull,
    ManagerStopped,
    InvalidConfiguration(String),
}

struct DiagnosticJob {
    operation_id: u64,
    profile_id: String,
    request: ConnectRequest,
    cancellation: CancellationToken,
}

pub struct ConnectionDiagnosticManager {
    job_tx: SyncSender<DiagnosticJob>,
    event_tx: Sender<DiagnosticEvent>,
    in_flight_profiles: Arc<Mutex<HashSet<String>>>,
    next_operation_id: AtomicU64,
}

impl ConnectionDiagnosticManager {
    pub fn new(known_hosts: Arc<KnownHostStore>) -> (Self, Receiver<DiagnosticEvent>) {
        let (job_tx, job_rx) = sync_channel(MAX_QUEUED_DIAGNOSTICS);
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let shared_rx = Arc::new(Mutex::new(job_rx));
        let in_flight_profiles = Arc::new(Mutex::new(HashSet::new()));

        for worker_index in 0..MAX_ACTIVE_DIAGNOSTICS {
            let worker_rx = shared_rx.clone();
            let worker_events = event_tx.clone();
            let worker_known_hosts = known_hosts.clone();
            let worker_in_flight = in_flight_profiles.clone();
            let _ = thread::Builder::new()
                .name(format!("connection-diagnostic-{worker_index}"))
                .spawn(move || {
                    let runtime = match Builder::new_current_thread()
                        .enable_io()
                        .enable_time()
                        .build()
                    {
                        Ok(runtime) => runtime,
                        Err(_) => return,
                    };
                    loop {
                        let job = match worker_rx.lock() {
                            Ok(rx) => rx.recv(),
                            Err(_) => return,
                        };
                        let Ok(job) = job else { return };
                        run_job(
                            &runtime,
                            job,
                            worker_known_hosts.clone(),
                            &worker_events,
                            &worker_in_flight,
                        );
                    }
                });
        }

        (
            Self {
                job_tx,
                event_tx,
                in_flight_profiles,
                next_operation_id: AtomicU64::new(1),
            },
            event_rx,
        )
    }

    pub fn submit(
        &self,
        profile_id: String,
        request: ConnectRequest,
    ) -> Result<DiagnosticControl, DiagnosticSubmitError> {
        {
            let mut in_flight = self
                .in_flight_profiles
                .lock()
                .map_err(|_| DiagnosticSubmitError::ManagerStopped)?;
            if !in_flight.insert(profile_id.clone()) {
                return Err(DiagnosticSubmitError::AlreadyRunning);
            }
        }

        let operation_id = self.next_operation_id.fetch_add(1, Ordering::Relaxed);
        let cancellation = CancellationToken::new();
        let route = route_summary(&request);
        let queued = DiagnosticEvent::Queued {
            operation_id,
            profile_id: profile_id.clone(),
            title: request.title.clone(),
            address: request.address(),
            route,
        };
        let job = DiagnosticJob {
            operation_id,
            profile_id: profile_id.clone(),
            request,
            cancellation: cancellation.clone(),
        };

        if let Err(error) = self.job_tx.try_send(job) {
            if let Ok(mut in_flight) = self.in_flight_profiles.lock() {
                in_flight.remove(&profile_id);
            }
            return Err(match error {
                TrySendError::Full(_) => DiagnosticSubmitError::QueueFull,
                TrySendError::Disconnected(_) => DiagnosticSubmitError::ManagerStopped,
            });
        }
        let _ = self.event_tx.send(queued);
        Ok(DiagnosticControl {
            operation_id,
            cancellation,
        })
    }
}

fn run_job(
    runtime: &tokio::runtime::Runtime,
    job: DiagnosticJob,
    known_hosts: Arc<KnownHostStore>,
    event_tx: &Sender<DiagnosticEvent>,
    in_flight_profiles: &Arc<Mutex<HashSet<String>>>,
) {
    let started = Instant::now();
    let current_stage = Arc::new(Mutex::new(DiagnosticStage::Configuration));
    let _ = event_tx.send(DiagnosticEvent::StageStarted {
        operation_id: job.operation_id,
        stage: DiagnosticStage::Configuration,
    });

    let configuration_elapsed = started.elapsed();
    let _ = event_tx.send(DiagnosticEvent::StagePassed {
        operation_id: job.operation_id,
        stage: DiagnosticStage::Configuration,
        elapsed: configuration_elapsed,
    });

    let operation_id = job.operation_id;
    let stage_for_probe = current_stage.clone();
    let probe_events = event_tx.clone();
    let cancellation = job.cancellation.clone();
    let result = runtime.block_on(async move {
        tokio::time::timeout(
            TOTAL_DIAGNOSTIC_TIMEOUT,
            crate::ssh::diagnose_connection(
                job.request,
                known_hosts,
                cancellation,
                move |stage, state, elapsed| {
                    let stage = DiagnosticStage::from(stage);
                    if let Ok(mut current) = stage_for_probe.lock() {
                        *current = stage;
                    }
                    let event = match state {
                        SshDiagnosticStageState::Started => DiagnosticEvent::StageStarted {
                            operation_id,
                            stage,
                        },
                        SshDiagnosticStageState::Passed => DiagnosticEvent::StagePassed {
                            operation_id,
                            stage,
                            elapsed,
                        },
                    };
                    let _ = probe_events.send(event);
                },
            ),
        )
        .await
    });

    if let Ok(mut in_flight) = in_flight_profiles.lock() {
        in_flight.remove(&job.profile_id);
    }

    let elapsed = started.elapsed();
    let stage = current_stage
        .lock()
        .map(|stage| *stage)
        .unwrap_or(DiagnosticStage::Configuration);
    let event = if job.cancellation.is_cancelled() {
        DiagnosticEvent::Cancelled {
            operation_id,
            stage,
            elapsed,
        }
    } else {
        match result {
            Ok(Ok(())) => DiagnosticEvent::Completed {
                operation_id,
                elapsed,
            },
            Ok(Err(failure)) => failed_event(operation_id, failure, elapsed),
            Err(_) => DiagnosticEvent::Failed {
                operation_id,
                stage,
                kind: DiagnosticFailureKind::Timeout,
                message: "Connection diagnostic exceeded its 45-second limit.".to_string(),
                recovery: DiagnosticFailureKind::Timeout.recovery().to_string(),
                elapsed,
            },
        }
    };
    let _ = event_tx.send(event);
}

fn failed_event(
    operation_id: u64,
    failure: SshDiagnosticFailure,
    elapsed: Duration,
) -> DiagnosticEvent {
    let stage = DiagnosticStage::from(failure.stage);
    let kind = classify_failure(stage, &format!("{:#}", failure.error));
    DiagnosticEvent::Failed {
        operation_id,
        stage,
        kind,
        message: kind.label().to_string(),
        recovery: kind.recovery().to_string(),
        elapsed,
    }
}

fn classify_failure(stage: DiagnosticStage, message: &str) -> DiagnosticFailureKind {
    let message = message.to_ascii_lowercase();
    if message.contains("cancelled") {
        DiagnosticFailureKind::Cancelled
    } else if message.contains("host key is not trusted") {
        DiagnosticFailureKind::UnknownHostKey
    } else if message.contains("host key mismatch") {
        DiagnosticFailureKind::HostKeyMismatch
    } else if message.contains("authentication was rejected")
        || message.contains("authentication failed")
    {
        DiagnosticFailureKind::CredentialDenied
    } else if message.contains("timed out") || message.contains("timeout") {
        DiagnosticFailureKind::Timeout
    } else {
        match stage {
            DiagnosticStage::RouteAndAuthentication => DiagnosticFailureKind::RouteUnavailable,
            DiagnosticStage::SessionChannel => DiagnosticFailureKind::SessionChannelUnavailable,
            DiagnosticStage::Sftp => DiagnosticFailureKind::SftpUnavailable,
            DiagnosticStage::Configuration => DiagnosticFailureKind::Internal,
        }
    }
}

fn route_summary(request: &ConnectRequest) -> String {
    let jumps = request.jump_host.as_ref().map(jump_chain_len).unwrap_or(0);
    let proxy = request
        .outbound_proxy
        .as_ref()
        .map(|proxy| proxy.kind().label());
    match (proxy, jumps) {
        (None, 0) => "Direct SSH".to_string(),
        (Some(proxy), 0) => format!("{proxy} proxy"),
        (None, jumps) => format!("{jumps} jump host(s)"),
        (Some(proxy), jumps) => format!("{proxy} proxy + {jumps} jump host(s)"),
    }
}

fn jump_chain_len(jump: &JumpHostConnection) -> usize {
    1 + jump.jump_host.as_deref().map(jump_chain_len).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestIsolation;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;

    fn test_request(port: u16) -> ConnectRequest {
        ConnectRequest {
            session_id: 1,
            title: "Target".to_string(),
            kind: crate::models::ConnectionKind::Ssh,
            host: "127.0.0.1".to_string(),
            port,
            username: "user".to_string(),
            auth: Some(crate::models::AuthConfig::Password {
                password: "secret".to_string(),
            }),
            jump_host: None,
            outbound_proxy: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 1_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }

    #[test]
    fn failure_classification_is_actionable_and_stage_specific() {
        assert_eq!(
            classify_failure(
                DiagnosticStage::RouteAndAuthentication,
                "Host key is not trusted for example:22"
            ),
            DiagnosticFailureKind::UnknownHostKey
        );
        assert_eq!(
            classify_failure(
                DiagnosticStage::RouteAndAuthentication,
                "Authentication was rejected by the server"
            ),
            DiagnosticFailureKind::CredentialDenied
        );
        assert_eq!(
            classify_failure(DiagnosticStage::Sftp, "subsystem request failed"),
            DiagnosticFailureKind::SftpUnavailable
        );
        assert_eq!(
            classify_failure(
                DiagnosticStage::RouteAndAuthentication,
                "network operation timed out"
            ),
            DiagnosticFailureKind::Timeout
        );
        assert_eq!(
            classify_failure(
                DiagnosticStage::RouteAndAuthentication,
                "Host key mismatch for example:22"
            ),
            DiagnosticFailureKind::HostKeyMismatch
        );
    }

    #[test]
    fn route_summary_reports_proxy_and_nested_jump_count_without_endpoints() {
        let auth = crate::models::AuthConfig::Password {
            password: "secret".to_string(),
        };
        let jump = |name: &str| JumpHostConnection {
            title: name.to_string(),
            host: format!("{name}.example"),
            port: 22,
            username: "user".to_string(),
            auth: auth.clone(),
            outbound_proxy: None,
            jump_host: None,
        };
        let mut request = ConnectRequest {
            session_id: 1,
            title: "Target".to_string(),
            kind: crate::models::ConnectionKind::Ssh,
            host: "target.example".to_string(),
            port: 22,
            username: "user".to_string(),
            auth: Some(auth.clone()),
            jump_host: None,
            outbound_proxy: Some(crate::models::OutboundProxy::Socks5 {
                host: "secret-proxy".to_string(),
                port: 1080,
            }),
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 1_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        };
        request.jump_host = Some(JumpHostConnection {
            jump_host: Some(Box::new(jump("first"))),
            ..jump("second")
        });
        assert_eq!(route_summary(&request), "SOCKS5 proxy + 2 jump host(s)");
    }

    #[test]
    fn manager_bounds_queue_deduplicates_and_cancels_all_work() {
        let _isolation = TestIsolation::acquire();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let server = thread::spawn(move || {
            let mut connections = Vec::new();
            while !server_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((connection, _)) => connections.push(connection),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        let known_hosts = Arc::new(KnownHostStore::load().unwrap());
        let (manager, events) = ConnectionDiagnosticManager::new(known_hosts);
        let first = manager
            .submit("profile-0".to_string(), test_request(port))
            .unwrap();
        assert!(matches!(
            manager.submit("profile-0".to_string(), test_request(port)),
            Err(DiagnosticSubmitError::AlreadyRunning)
        ));

        let mut controls = vec![first];
        let mut saw_full = false;
        for index in 1..=MAX_ACTIVE_DIAGNOSTICS + MAX_QUEUED_DIAGNOSTICS + 4 {
            match manager.submit(format!("profile-{index}"), test_request(port)) {
                Ok(control) => controls.push(control),
                Err(DiagnosticSubmitError::QueueFull) => {
                    saw_full = true;
                    break;
                }
                Err(error) => panic!("unexpected submission error: {error:?}"),
            }
        }
        assert!(saw_full);
        assert!(controls.len() <= MAX_ACTIVE_DIAGNOSTICS + MAX_QUEUED_DIAGNOSTICS);
        assert!(controls.len() >= MAX_QUEUED_DIAGNOSTICS);

        for control in &controls {
            control.cancel();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut terminal = HashSet::new();
        while terminal.len() < controls.len() && Instant::now() < deadline {
            if let Ok(event) = events.recv_timeout(Duration::from_millis(100))
                && let DiagnosticEvent::Cancelled { operation_id, .. } = event
            {
                terminal.insert(operation_id);
            }
        }
        assert_eq!(terminal.len(), controls.len());

        drop(manager);
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
