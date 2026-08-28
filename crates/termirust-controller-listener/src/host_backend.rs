use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use rand::RngCore as _;
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{
    ActivityState, AuthenticatedPeer, HostedSessionId, HostedSessionState, OccupantGeneration,
};
use termirust_store::{ProjectRepository, SessionRepository, read_host_metadata};
use tokio_util::sync::CancellationToken;

use crate::{
    ControllerBackendFactory, ControllerCommand, ControllerCommandEnvelope,
    ControllerConnectionBackend, ControllerResponse, ControllerSessionSummary, HostCommandContext,
    ListenerError, ListenerErrorCode, MAX_SESSION_PAGE_BYTES,
};

#[derive(Clone)]
pub struct HostBackendFactory {
    sessions: SessionRepository,
    projects: ProjectRepository,
    runtime_parent: PathBuf,
}

impl std::fmt::Debug for HostBackendFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostBackendFactory")
            .field("sessions", &"[REDACTED]")
            .field("projects", &"[REDACTED]")
            .field("runtime_parent", &"[REDACTED]")
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
        }
    }
}

impl ControllerBackendFactory for HostBackendFactory {
    fn open(
        &self,
        _: &AuthenticatedPeer,
    ) -> Result<Box<dyn ControllerConnectionBackend>, ListenerError> {
        Ok(Box::new(HostConnectionBackend {
            sessions: self.sessions.clone(),
            projects: self.projects.clone(),
            runtime_parent: self.runtime_parent.clone(),
            clients: HashMap::new(),
        }))
    }
}

struct HostConnectionBackend {
    sessions: SessionRepository,
    projects: ProjectRepository,
    runtime_parent: PathBuf,
    clients: HashMap<HostedSessionId, HostClient>,
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
        match command.command {
            ControllerCommand::ListSessions {
                offset,
                limit,
                expected_revision,
            } => self.list_sessions(command_id, offset, limit, expected_revision),
            ControllerCommand::Attach {
                session_id,
                from_sequence,
                columns,
                rows,
                ..
            } => {
                if !self.clients.contains_key(&session_id) {
                    let endpoint = LocalEndpoint::new(
                        self.runtime_parent.join(session_id.to_string()),
                        session_id,
                    );
                    let mut nonce = [0; 32];
                    rand::rngs::OsRng.fill_bytes(&mut nonce);
                    let client = HostClient::connect(
                        endpoint,
                        ConnectOptions::local(session_id, nonce),
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
                let mut responses = Vec::with_capacity(outputs.len() + 1);
                responses.push(ControllerResponse::Attached {
                    command_id,
                    session_id,
                    occupant_generation,
                    replay_through_sequence: termirust_domain::OutputSequence::new(
                        state.output_sequence,
                    ),
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
                    client
                        .detach(cancel)
                        .await
                        .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
                }
                Ok(vec![ControllerResponse::Detached { command_id }])
            }
        }
    }
}

impl HostConnectionBackend {
    fn list_sessions(
        &self,
        command_id: termirust_domain::CommandId,
        offset: u32,
        limit: u16,
        expected_revision: Option<u64>,
    ) -> Result<Vec<ControllerResponse>, ListenerError> {
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
        if expected_revision.is_some_and(|expected| expected != revision) {
            return Ok(vec![ControllerResponse::Error {
                command_id,
                code: "snapshot_changed".into(),
                completion_unknown: false,
            }]);
        }
        let start = usize::try_from(offset)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        if start > session_snapshot.sessions.len() {
            return Ok(vec![ControllerResponse::Error {
                command_id,
                code: "page_out_of_range".into(),
                completion_unknown: false,
            }]);
        }
        let requested_end = start
            .saturating_add(usize::from(limit))
            .min(session_snapshot.sessions.len());
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
        let mut encoded_bytes = 512usize;
        let mut sessions = Vec::new();
        for session in &session_snapshot.sessions[start..requested_end] {
            let summary = ControllerSessionSummary {
                session_id: session.id,
                title: session.title.to_string(),
                project: project_names.get(&session.project_id).cloned(),
                group: session
                    .group_id
                    .and_then(|group_id| group_names.get(&group_id).cloned()),
                lifecycle: lifecycle_code(session.lifecycle).to_owned(),
                activity: activity_code(session.activity.state).to_owned(),
                occupant_generation: self.occupant_generation(session.id).ok(),
                last_output_sequence: session.last_output_sequence,
                has_writer: false,
                unread: session.unread(),
            };
            let summary_bytes = serde_json::to_vec(&summary)
                .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?
                .len()
                .saturating_add(1);
            if !sessions.is_empty()
                && encoded_bytes.saturating_add(summary_bytes) > MAX_SESSION_PAGE_BYTES
            {
                break;
            }
            encoded_bytes = encoded_bytes.saturating_add(summary_bytes);
            sessions.push(summary);
        }
        let end = start.saturating_add(sessions.len());
        let next_offset =
            (end < session_snapshot.sessions.len()).then(|| u32::try_from(end).unwrap_or(u32::MAX));
        Ok(vec![ControllerResponse::Sessions {
            command_id,
            revision,
            update_sequence: revision,
            sessions,
            next_offset,
        }])
    }

    fn occupant_generation(
        &self,
        session_id: HostedSessionId,
    ) -> Result<OccupantGeneration, ListenerError> {
        read_host_metadata(&self.sessions.session_data_path(session_id))
            .ok()
            .and_then(|metadata| metadata.runtime_recognition)
            .and_then(|recognition| recognition.occupant)
            .map(|occupant| occupant.generation)
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))
    }

    fn client(&mut self, session_id: HostedSessionId) -> Result<&mut HostClient, ListenerError> {
        self.clients
            .get_mut(&session_id)
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::HostUnavailable))
    }
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
