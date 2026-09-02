use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use async_trait::async_trait;
use rand::RngCore as _;
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{
    ActivityState, AuthenticatedPeer, ControllerCapabilities, ControllerCapability,
    HostedSessionId, HostedSessionState, OccupantGeneration, OccupantOwnership,
};
use termirust_store::{ProjectRepository, SessionRepository, read_host_metadata};
use tokio_util::sync::CancellationToken;

use crate::DesktopPaneBridgeEndpoint;
use crate::desktop_pane_bridge::DesktopPaneBridgeClient;
use crate::{
    ControllerBackendFactory, ControllerCommand, ControllerCommandEnvelope,
    ControllerConnectionBackend, ControllerResponse, ControllerSessionCapability,
    ControllerSessionOrigin, ControllerSessionSummary, HostCommandContext, ListenerError,
    ListenerErrorCode, MAX_SESSION_PAGE_BYTES, MAX_SNAPSHOT_CHUNK_BYTES,
};

#[derive(Clone)]
pub struct HostBackendFactory {
    sessions: SessionRepository,
    projects: ProjectRepository,
    runtime_parent: PathBuf,
    desktop_pane_bridge: Option<DesktopPaneBridgeEndpoint>,
}

impl std::fmt::Debug for HostBackendFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostBackendFactory")
            .field("sessions", &"[REDACTED]")
            .field("projects", &"[REDACTED]")
            .field("runtime_parent", &"[REDACTED]")
            .field("desktop_pane_bridge", &self.desktop_pane_bridge.is_some())
            .finish()
    }
}

impl HostBackendFactory {
    pub fn new(
        sessions: SessionRepository,
        projects: ProjectRepository,
        runtime_parent: impl Into<PathBuf>,
    ) -> Self {
        Self {
            sessions,
            projects,
            runtime_parent: runtime_parent.into(),
            desktop_pane_bridge: None,
        }
    }

    pub fn with_desktop_pane_bridge(mut self, endpoint: Option<DesktopPaneBridgeEndpoint>) -> Self {
        self.desktop_pane_bridge = endpoint;
        self
    }
}

impl ControllerBackendFactory for HostBackendFactory {
    fn open(
        &self,
        peer: &AuthenticatedPeer,
    ) -> Result<Box<dyn ControllerConnectionBackend>, ListenerError> {
        Ok(Box::new(HostConnectionBackend {
            sessions: self.sessions.clone(),
            projects: self.projects.clone(),
            runtime_parent: self.runtime_parent.clone(),
            capabilities: peer.capabilities,
            clients: HashMap::new(),
            active_attach: None,
            pending_output: VecDeque::new(),
            desktop_pane_bridge_endpoint: self.desktop_pane_bridge.clone(),
            desktop_pane_bridge: None,
            live_commands: HashSet::new(),
            live_attach: false,
        }))
    }
}

struct HostConnectionBackend {
    sessions: SessionRepository,
    projects: ProjectRepository,
    runtime_parent: PathBuf,
    capabilities: ControllerCapabilities,
    clients: HashMap<HostedSessionId, HostClient>,
    active_attach: Option<ActiveAttach>,
    pending_output: VecDeque<ControllerResponse>,
    desktop_pane_bridge_endpoint: Option<DesktopPaneBridgeEndpoint>,
    desktop_pane_bridge: Option<DesktopPaneBridgeClient>,
    live_commands: HashSet<termirust_domain::CommandId>,
    live_attach: bool,
}

#[derive(Clone, Copy)]
struct ActiveAttach {
    session_id: HostedSessionId,
    occupant_generation: OccupantGeneration,
    cursor: termirust_domain::OutputSequence,
    columns: u32,
    rows: u32,
}

#[async_trait]
impl ControllerConnectionBackend for HostConnectionBackend {
    async fn command_context(
        &mut self,
        command: &ControllerCommandEnvelope,
        cancel: &CancellationToken,
    ) -> Result<HostCommandContext, ListenerError> {
        let Some(session_id) = command.command.session_id() else {
            return Ok(HostCommandContext::default());
        };
        if self.ensure_desktop_bridge().await {
            let context = self
                .desktop_pane_bridge
                .as_mut()
                .expect("desktop bridge was connected")
                .context(session_id)
                .await;
            match context {
                Ok(Some(context)) => {
                    self.live_commands.insert(command.command_id);
                    return Ok(context);
                }
                Ok(None) => {}
                Err(_) => self.desktop_pane_bridge = None,
            }
        }
        let occupant_generation = self.occupant_generation(session_id)?;
        let has_writer_lease = if command.command.kind().requires_writer_lease() {
            let client = self
                .clients
                .get_mut(&session_id)
                .ok_or_else(|| ListenerError::new(ListenerErrorCode::WriterLeaseRequired))?;
            client
                .get_state(cancel)
                .await
                .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?
                .has_writer_lease
        } else {
            false
        };
        Ok(HostCommandContext {
            occupant_generation: Some(occupant_generation),
            has_writer_lease,
        })
    }

    async fn execute(
        &mut self,
        command: ControllerCommandEnvelope,
        cancel: &CancellationToken,
    ) -> Result<Vec<ControllerResponse>, ListenerError> {
        let command_id = command.command_id;
        if self.live_commands.remove(&command_id) {
            if matches!(command.command, ControllerCommand::Attach { .. }) {
                if let Some(active) = self.active_attach.take()
                    && let Some(mut client) = self.clients.remove(&active.session_id)
                {
                    client.disconnect();
                }
                self.pending_output.clear();
                self.live_attach = true;
            }
            let is_detach = matches!(command.command, ControllerCommand::Detach { .. });
            let responses = self
                .desktop_pane_bridge
                .as_mut()
                .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))?
                .execute(command_id, command.command)
                .await?;
            if is_detach {
                self.live_attach = false;
            }
            return Ok(responses);
        }
        if matches!(command.command, ControllerCommand::Attach { .. }) && self.live_attach {
            if let Some(bridge) = self.desktop_pane_bridge.as_mut() {
                bridge.reset().await?;
            }
            self.live_attach = false;
        }
        match command.command {
            ControllerCommand::ListSessions {
                offset,
                limit,
                expected_revision,
            } => {
                self.list_sessions(command_id, offset, limit, expected_revision)
                    .await
            }
            ControllerCommand::Attach {
                session_id,
                occupant_generation,
                from_sequence,
                columns,
                rows,
            } => {
                self.replace_active_session(session_id);
                if !self.clients.contains_key(&session_id) {
                    let endpoint = LocalEndpoint::new(
                        self.runtime_parent.join(session_id.to_string()),
                        session_id,
                    );
                    let mut nonce = [0; 32];
                    rand::rngs::OsRng.fill_bytes(&mut nonce);
                    let client = HostClient::connect(
                        endpoint,
                        ConnectOptions::local_read_only(session_id, nonce),
                        cancel,
                    )
                    .await
                    .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
                    self.clients.insert(session_id, client);
                }
                let client = self
                    .clients
                    .get_mut(&session_id)
                    .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
                let outputs = client
                    .attach(from_sequence, columns, rows, cancel)
                    .await
                    .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
                let state = client
                    .take_last_state()
                    .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
                let snapshot = client.take_last_snapshot();
                let replay_through_sequence = outputs
                    .last()
                    .map(|output| output.sequence)
                    .or_else(|| {
                        snapshot.as_ref().map(|snapshot| {
                            termirust_domain::OutputSequence::new(snapshot.boundary_sequence)
                        })
                    })
                    .unwrap_or(from_sequence);
                self.active_attach = Some(ActiveAttach {
                    session_id,
                    occupant_generation,
                    cursor: replay_through_sequence,
                    columns,
                    rows,
                });
                let snapshot_chunks = snapshot
                    .as_ref()
                    .map(|snapshot| {
                        snapshot
                            .terminal_bytes
                            .len()
                            .max(1)
                            .div_ceil(MAX_SNAPSHOT_CHUNK_BYTES)
                    })
                    .unwrap_or(0);
                let mut responses = Vec::with_capacity(outputs.len() + snapshot_chunks + 1);
                if let Some(snapshot) = snapshot {
                    let chunk_count = u32::try_from(snapshot_chunks)
                        .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
                    if snapshot.terminal_bytes.is_empty() {
                        responses.push(ControllerResponse::Snapshot {
                            command_id,
                            session_id,
                            boundary_sequence: termirust_domain::OutputSequence::new(
                                snapshot.boundary_sequence,
                            ),
                            columns,
                            rows,
                            chunk_index: 0,
                            chunk_count: 1,
                            bytes: Vec::new(),
                        });
                    } else {
                        for (index, bytes) in snapshot
                            .terminal_bytes
                            .chunks(MAX_SNAPSHOT_CHUNK_BYTES)
                            .enumerate()
                        {
                            responses.push(ControllerResponse::Snapshot {
                                command_id,
                                session_id,
                                boundary_sequence: termirust_domain::OutputSequence::new(
                                    snapshot.boundary_sequence,
                                ),
                                columns,
                                rows,
                                chunk_index: u32::try_from(index).map_err(|_| {
                                    ListenerError::new(ListenerErrorCode::HostUnavailable)
                                })?,
                                chunk_count,
                                bytes: bytes.to_vec(),
                            });
                        }
                    }
                }
                responses.push(ControllerResponse::Attached {
                    command_id,
                    session_id,
                    occupant_generation,
                    replay_through_sequence,
                    has_writer_lease: state.has_writer_lease,
                });
                responses.extend(
                    outputs
                        .into_iter()
                        .map(|output| ControllerResponse::Output {
                            session_id,
                            sequence: output.sequence,
                            bytes: output.bytes,
                        }),
                );
                Ok(responses)
            }
            ControllerCommand::Input {
                session_id, bytes, ..
            } => {
                let result = self
                    .client(session_id)?
                    .input(command_id, bytes, cancel)
                    .await;
                Ok(vec![mutation_response(command_id, result)])
            }
            ControllerCommand::AcquireWriter { session_id, .. } => {
                let result = self
                    .client(session_id)?
                    .set_writer_lease(command_id, true, cancel)
                    .await;
                Ok(vec![mutation_response(command_id, result)])
            }
            ControllerCommand::ReleaseWriter { session_id, .. } => {
                let response = match self
                    .client(session_id)?
                    .set_writer_lease(command_id, false, cancel)
                    .await
                {
                    Ok(false) => ControllerResponse::Completed {
                        command_id,
                        applied: true,
                    },
                    Ok(true) | Err(_) => ControllerResponse::Error {
                        command_id,
                        code: "completion_unknown".to_owned(),
                        completion_unknown: true,
                    },
                };
                Ok(vec![response])
            }
            ControllerCommand::Resize {
                session_id,
                columns,
                rows,
                ..
            } => {
                let result = self
                    .client(session_id)?
                    .resize(command_id, columns, rows, cancel)
                    .await;
                Ok(vec![mutation_response(command_id, result)])
            }
            ControllerCommand::Approval { .. } => Ok(vec![ControllerResponse::Error {
                command_id,
                code: "approval_unavailable".to_owned(),
                completion_unknown: false,
            }]),
            ControllerCommand::Detach { session_id, .. } => {
                if let Some(mut client) = self.clients.remove(&session_id) {
                    client.disconnect();
                }
                if self
                    .active_attach
                    .is_some_and(|active| active.session_id == session_id)
                {
                    self.active_attach = None;
                    self.pending_output.clear();
                }
                Ok(vec![ControllerResponse::Detached { command_id }])
            }
        }
    }

    async fn next_response(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<Option<ControllerResponse>, ListenerError> {
        if self.live_attach {
            return self
                .desktop_pane_bridge
                .as_mut()
                .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))?
                .next()
                .await;
        }
        if let Some(response) = self.pending_output.pop_front() {
            return Ok(Some(response));
        }
        let Some(active) = self.active_attach else {
            return Ok(None);
        };
        if self.occupant_generation(active.session_id)? != active.occupant_generation {
            return Err(ListenerError::new(ListenerErrorCode::HostUnavailable));
        }
        let client = self
            .clients
            .get_mut(&active.session_id)
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        let outputs = client
            .attach(active.cursor, active.columns, active.rows, cancel)
            .await
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        if client.take_last_snapshot().is_some() {
            return Err(ListenerError::new(ListenerErrorCode::HostUnavailable));
        }
        if let Some(last) = outputs.last() {
            self.active_attach = Some(ActiveAttach {
                cursor: last.sequence,
                ..active
            });
        }
        self.pending_output.extend(
            outputs
                .into_iter()
                .map(|output| ControllerResponse::Output {
                    session_id: active.session_id,
                    sequence: output.sequence,
                    bytes: output.bytes,
                }),
        );
        Ok(self.pending_output.pop_front())
    }
}

impl HostConnectionBackend {
    async fn ensure_desktop_bridge(&mut self) -> bool {
        if self.desktop_pane_bridge.is_some() {
            return true;
        }
        let Some(endpoint) = self.desktop_pane_bridge_endpoint.clone() else {
            return false;
        };
        match DesktopPaneBridgeClient::connect(&endpoint).await {
            Ok(client) => {
                self.desktop_pane_bridge = Some(client);
                true
            }
            Err(_) => false,
        }
    }

    fn replace_active_session(&mut self, session_id: HostedSessionId) {
        if let Some(active) = self.active_attach
            && active.session_id != session_id
            && let Some(mut client) = self.clients.remove(&active.session_id)
        {
            client.disconnect();
        }
        self.pending_output.clear();
        self.active_attach = None;
    }

    async fn list_sessions(
        &mut self,
        command_id: termirust_domain::CommandId,
        offset: u32,
        limit: u16,
        expected_revision: Option<u64>,
    ) -> Result<Vec<ControllerResponse>, ListenerError> {
        let (durable_revision, mut sessions) = self.durable_session_summaries()?;
        let (live_revision, mut live_sessions) = if self.ensure_desktop_bridge().await {
            match self
                .desktop_pane_bridge
                .as_mut()
                .expect("desktop bridge was connected")
                .list()
                .await
            {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    self.desktop_pane_bridge = None;
                    (1, Vec::new())
                }
            }
        } else {
            (1, Vec::new())
        };
        let live_capabilities = controller_session_capabilities(self.capabilities)
            .into_iter()
            .filter(|capability| *capability != ControllerSessionCapability::Resize)
            .collect::<Vec<_>>();
        for session in &mut live_sessions {
            session.capabilities = live_capabilities.clone();
        }
        let live_ids = live_sessions
            .iter()
            .map(|session| session.session_id)
            .collect::<HashSet<_>>();
        sessions.retain(|session| !live_ids.contains(&session.session_id));
        live_sessions.append(&mut sessions);
        let sessions = live_sessions;
        let revision = durable_revision
            .wrapping_mul(31)
            .wrapping_add(live_revision)
            .max(1);
        if expected_revision.is_some_and(|expected| expected != revision) {
            return Ok(vec![ControllerResponse::Error {
                command_id,
                code: "snapshot_changed".into(),
                completion_unknown: false,
            }]);
        }
        let start = usize::try_from(offset)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        if start > sessions.len() {
            return Ok(vec![ControllerResponse::Error {
                command_id,
                code: "page_out_of_range".into(),
                completion_unknown: false,
            }]);
        }
        let requested_end = start.saturating_add(usize::from(limit)).min(sessions.len());
        let mut encoded_bytes = 512usize;
        let mut page = Vec::new();
        for summary in &sessions[start..requested_end] {
            let summary_bytes = serde_json::to_vec(summary)
                .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?
                .len()
                .saturating_add(1);
            if !page.is_empty()
                && encoded_bytes.saturating_add(summary_bytes) > MAX_SESSION_PAGE_BYTES
            {
                break;
            }
            encoded_bytes = encoded_bytes.saturating_add(summary_bytes);
            page.push(summary.clone());
        }
        let end = start.saturating_add(page.len());
        let next_offset = (end < sessions.len()).then(|| u32::try_from(end).unwrap_or(u32::MAX));
        Ok(vec![ControllerResponse::Sessions {
            command_id,
            revision,
            update_sequence: revision,
            sessions: page,
            next_offset,
        }])
    }

    fn durable_session_summaries(
        &self,
    ) -> Result<(u64, Vec<ControllerSessionSummary>), ListenerError> {
        let session_snapshot = self
            .sessions
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        let project_snapshot = self
            .projects
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        let revision = session_snapshot
            .revision
            .get()
            .saturating_add(project_snapshot.revision.get())
            .saturating_add(1);
        let project_names = project_snapshot
            .projects
            .iter()
            .map(|summary| {
                (
                    summary.project.id,
                    summary.project.display_name.as_str().to_owned(),
                )
            })
            .collect::<HashMap<_, _>>();
        let group_names = project_snapshot
            .groups
            .iter()
            .map(|group| (group.id, group.name.to_string()))
            .collect::<HashMap<_, _>>();
        let mut sessions = Vec::new();
        for session in &session_snapshot.sessions {
            let metadata = read_host_metadata(&self.sessions.session_data_path(session.id)).ok();
            let occupant = metadata
                .as_ref()
                .and_then(|metadata| metadata.runtime_recognition.as_ref())
                .and_then(|recognition| recognition.occupant.as_ref());
            let origin = match occupant.map(|occupant| &occupant.ownership) {
                Some(OccupantOwnership::Managed { .. }) => ControllerSessionOrigin::ManagedAgent,
                Some(OccupantOwnership::Observed { .. }) => ControllerSessionOrigin::ObservedAgent,
                Some(OccupantOwnership::Ambiguous) => ControllerSessionOrigin::Unknown,
                None if metadata.is_some() => ControllerSessionOrigin::Terminal,
                None => ControllerSessionOrigin::Unknown,
            };
            let summary = ControllerSessionSummary {
                session_id: session.id,
                host_instance_id: metadata.as_ref().map(|metadata| metadata.host_instance_id),
                origin,
                runtime: occupant.map(|occupant| occupant.runtime_id.as_str().to_owned()),
                capabilities: controller_session_capabilities(self.capabilities),
                title: session.title.to_string(),
                project: project_names.get(&session.project_id).cloned(),
                group: session
                    .group_id
                    .and_then(|group_id| group_names.get(&group_id).cloned()),
                lifecycle: lifecycle_code(session.lifecycle).to_owned(),
                activity: activity_code(session.activity.state).to_owned(),
                occupant_generation: metadata
                    .as_ref()
                    .map(|metadata| metadata.activity.generation),
                last_output_sequence: session.last_output_sequence,
                has_writer: false,
                unread: session.unread(),
            };
            sessions.push(summary);
        }
        Ok((revision, sessions))
    }

    fn occupant_generation(
        &self,
        session_id: HostedSessionId,
    ) -> Result<OccupantGeneration, ListenerError> {
        read_host_metadata(&self.sessions.session_data_path(session_id))
            .ok()
            .map(|metadata| metadata.activity.generation)
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))
    }

    fn client(&mut self, session_id: HostedSessionId) -> Result<&mut HostClient, ListenerError> {
        self.clients
            .get_mut(&session_id)
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))
    }
}

fn controller_session_capabilities(
    capabilities: ControllerCapabilities,
) -> Vec<ControllerSessionCapability> {
    [
        (
            ControllerCapability::ObserveSessions,
            ControllerSessionCapability::ObserveSessions,
        ),
        (
            ControllerCapability::AttachOutput,
            ControllerSessionCapability::AttachOutput,
        ),
        (
            ControllerCapability::SendInput,
            ControllerSessionCapability::SendInput,
        ),
        (
            ControllerCapability::Resize,
            ControllerSessionCapability::Resize,
        ),
        (
            ControllerCapability::RespondToApproval,
            ControllerSessionCapability::RespondToApproval,
        ),
    ]
    .into_iter()
    .filter_map(|(permission, summary)| capabilities.contains(permission).then_some(summary))
    .collect()
}

fn mutation_response(
    command_id: termirust_domain::CommandId,
    result: Result<bool, termirust_client::ClientError>,
) -> ControllerResponse {
    match result {
        Ok(applied) => ControllerResponse::Completed {
            command_id,
            applied,
        },
        Err(_) => ControllerResponse::Error {
            command_id,
            code: "completion_unknown".to_owned(),
            completion_unknown: true,
        },
    }
}

fn lifecycle_code(state: HostedSessionState) -> &'static str {
    match state {
        HostedSessionState::Draft => "draft",
        HostedSessionState::Validating => "validating",
        HostedSessionState::Starting => "starting",
        HostedSessionState::Provisioning => "provisioning",
        HostedSessionState::Attaching => "attaching",
        HostedSessionState::Replaying => "replaying",
        HostedSessionState::Live => "live",
        HostedSessionState::RecordingPaused => "recording_paused",
        HostedSessionState::Stopping => "stopping",
        HostedSessionState::Offline => "offline",
        HostedSessionState::Orphaned => "orphaned",
        HostedSessionState::Gap => "gap",
        HostedSessionState::PermissionDenied => "permission_denied",
        HostedSessionState::Incompatible => "incompatible",
        HostedSessionState::RunningAppAttached => "running_app_attached",
        HostedSessionState::Failed => "failed",
        HostedSessionState::Cancelled => "cancelled",
        HostedSessionState::Exited => "exited",
    }
}

fn activity_code(state: ActivityState) -> &'static str {
    match state {
        ActivityState::Unknown => "unknown",
        ActivityState::Idle => "idle",
        ActivityState::Busy => "busy",
        ActivityState::NeedsInput => "needs_input",
        ActivityState::Done => "done",
        ActivityState::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use termirust_domain::CommandId;

    use super::*;
    use crate::{
        DesktopPaneBridgeServer, DesktopPaneRegistration, DesktopPaneRegistry, DesktopPaneTransport,
    };

    #[tokio::test]
    async fn session_list_puts_live_desktop_panes_before_durable_history() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("projects");
        let sessions =
            SessionRepository::open(project_root.clone(), fixture.path().join("session-data"))
                .unwrap();
        let projects = ProjectRepository::open(project_root).unwrap();
        let registry = DesktopPaneRegistry::default();
        let session_id = HostedSessionId::new();
        let writes = Arc::new(AtomicUsize::new(0));
        registry.register(DesktopPaneRegistration {
            session_id,
            title: "Current SSH terminal".to_owned(),
            runtime: "ssh".to_owned(),
            columns: 120,
            rows: 36,
            transport: DesktopPaneTransport::new(move |bytes| {
                writes.fetch_add(bytes.len(), std::sync::atomic::Ordering::Relaxed);
                true
            }),
        });
        let server =
            DesktopPaneBridgeServer::start(fixture.path().join("bridge"), registry).unwrap();
        let capabilities = ControllerCapabilities::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput)
            .with(ControllerCapability::Resize);
        let mut backend = HostConnectionBackend {
            sessions,
            projects,
            runtime_parent: fixture.path().join("runtime"),
            capabilities,
            clients: HashMap::new(),
            active_attach: None,
            pending_output: VecDeque::new(),
            desktop_pane_bridge_endpoint: Some(server.endpoint()),
            desktop_pane_bridge: None,
            live_commands: HashSet::new(),
            live_attach: false,
        };

        let responses = backend
            .list_sessions(CommandId::new(), 0, 100, None)
            .await
            .unwrap();
        let [ControllerResponse::Sessions { sessions, .. }] = responses.as_slice() else {
            panic!("expected one session page");
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session_id);
        assert_eq!(sessions[0].title, "Current SSH terminal");
        assert_eq!(sessions[0].runtime.as_deref(), Some("ssh"));
        assert!(
            sessions[0]
                .capabilities
                .contains(&ControllerSessionCapability::SendInput)
        );
        assert!(
            !sessions[0]
                .capabilities
                .contains(&ControllerSessionCapability::Resize)
        );

        drop(backend);
        drop(server);
    }

    #[tokio::test]
    async fn live_desktop_pane_attach_replays_output_and_accepts_input() {
        let fixture = tempfile::tempdir().unwrap();
        let project_root = fixture.path().join("projects");
        let sessions =
            SessionRepository::open(project_root.clone(), fixture.path().join("session-data"))
                .unwrap();
        let projects = ProjectRepository::open(project_root).unwrap();
        let registry = DesktopPaneRegistry::default();
        let session_id = HostedSessionId::new();
        let written = Arc::new(std::sync::Mutex::new(Vec::new()));
        let written_for_transport = written.clone();
        registry.register(DesktopPaneRegistration {
            session_id,
            title: "Current local terminal".to_owned(),
            runtime: "local_shell".to_owned(),
            columns: 120,
            rows: 36,
            transport: DesktopPaneTransport::new(move |bytes| {
                written_for_transport.lock().unwrap().extend(bytes);
                true
            }),
        });
        registry.append_output(session_id, b"desktop history\r\n", Vec::new);
        let server =
            DesktopPaneBridgeServer::start(fixture.path().join("bridge"), registry).unwrap();
        let capabilities = ControllerCapabilities::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput);
        let mut backend = HostConnectionBackend {
            sessions,
            projects,
            runtime_parent: fixture.path().join("runtime"),
            capabilities,
            clients: HashMap::new(),
            active_attach: None,
            pending_output: VecDeque::new(),
            desktop_pane_bridge_endpoint: Some(server.endpoint()),
            desktop_pane_bridge: None,
            live_commands: HashSet::new(),
            live_attach: false,
        };
        let cancel = CancellationToken::new();
        let generation = OccupantGeneration::new(1);

        let attach_id = CommandId::new();
        let attach = ControllerCommandEnvelope::new(
            attach_id,
            1,
            1,
            ControllerCommand::Attach {
                session_id,
                occupant_generation: generation,
                from_sequence: termirust_domain::OutputSequence::ZERO,
                columns: 80,
                rows: 24,
            },
        );
        let context = backend.command_context(&attach, &cancel).await.unwrap();
        assert_eq!(context.occupant_generation, Some(generation));
        let responses = backend.execute(attach, &cancel).await.unwrap();
        assert!(responses.iter().any(|response| matches!(
            response,
            ControllerResponse::Attached { command_id, .. } if *command_id == attach_id
        )));
        assert!(responses.iter().any(|response| matches!(
            response,
            ControllerResponse::Output { session_id: actual, bytes, .. }
                if *actual == session_id && bytes == b"desktop history\r\n"
        )));

        let acquire_id = CommandId::new();
        let acquire = ControllerCommandEnvelope::new(
            acquire_id,
            1,
            1,
            ControllerCommand::AcquireWriter {
                session_id,
                occupant_generation: generation,
            },
        );
        backend.command_context(&acquire, &cancel).await.unwrap();
        assert_eq!(
            backend.execute(acquire, &cancel).await.unwrap(),
            vec![ControllerResponse::Completed {
                command_id: acquire_id,
                applied: true,
            }]
        );

        let input_id = CommandId::new();
        let input = ControllerCommandEnvelope::new(
            input_id,
            1,
            1,
            ControllerCommand::Input {
                session_id,
                occupant_generation: generation,
                bytes: b"from phone\n".to_vec(),
            },
        );
        let context = backend.command_context(&input, &cancel).await.unwrap();
        assert!(context.has_writer_lease);
        assert_eq!(
            backend.execute(input, &cancel).await.unwrap(),
            vec![ControllerResponse::Completed {
                command_id: input_id,
                applied: true,
            }]
        );
        assert_eq!(*written.lock().unwrap(), b"from phone\n");
    }
}
