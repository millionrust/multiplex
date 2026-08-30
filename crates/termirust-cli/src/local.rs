use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::RngCore as _;
use termirust_client::{ClientError, ClientErrorCode, ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{
    ActivityAggregate, CommandId, GroupId, HostInstanceId, HostedSession, HostedSessionId,
    HostedSessionState, OutputSequence, PositionKey, PresetId, PresetRisk, ProjectId, Revision,
    SessionMutation, SessionStateError, SessionTitle, TitleSource, resolve_launch,
};
use termirust_host_protocol::{CURRENT_PROTOCOL, wire};
use termirust_session_host::{LaunchDescriptor, StopDeadlines};
use termirust_store::{
    JournalLimits, PresetRepository, PresetSnapshot, ProjectRepository, ProjectSnapshot,
    SessionRemovalManifest, SessionRepository, SessionSnapshot, StoreError, StoreHealth,
    read_host_metadata,
};
use tokio::runtime::Builder;
use tokio_util::sync::CancellationToken;

use crate::{
    CLI_JSON_SCHEMA_VERSION, Cancellation, CliCommand, CliData, CliError, CommandService,
    ControllerSshCommand, ErrorCode, MAX_RESPONSE_RECORDS, MAX_SESSION_WAIT_TIMEOUT_MS,
    PresetListData, PresetView, ProjectListData, ProjectView, RemovalConfirmationKind, SessionData,
    SessionListData, SessionListFilter, SessionMutationData, SessionRemovalPreviewData,
    SessionView, SessionWaitCondition, SessionWaitConditionData, SessionWaitData, StatusData,
};

const STORE_DIR_NAME: &str = "agent-workspace";
const SESSION_DATA_DIR_NAME: &str = "durable-sessions";
const FORMAT_FILE_NAME: &str = "format.json";
const HOST_READY_DEADLINE: Duration = Duration::from_secs(5);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(25);
const SESSION_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SESSION_WAIT_CANCELLATION_SLICE: Duration = Duration::from_millis(10);

#[derive(Clone)]
pub struct CliPaths {
    config_root: PathBuf,
    metadata_root: PathBuf,
    session_data_root: PathBuf,
    runtime_parent: PathBuf,
    host_executable: PathBuf,
}

impl fmt::Debug for CliPaths {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CliPaths")
            .field("config_root", &"<redacted>")
            .field("metadata_root", &"<redacted>")
            .field("session_data_root", &"<redacted>")
            .field("runtime_parent", &"<redacted>")
            .field("host_executable", &"<redacted>")
            .finish()
    }
}

impl CliPaths {
    pub fn discover() -> Result<Self, CliError> {
        let config_root = std::env::var_os("TERMIRUST_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|root| root.join("termirust")))
            .ok_or_else(|| {
                CliError::new(
                    ErrorCode::Unavailable,
                    "TermiRust configuration directory is unavailable",
                    "Set TERMIRUST_CONFIG_DIR to the existing TermiRust data directory.",
                )
            })?;
        let current = std::env::current_exe().map_err(|_| {
            CliError::new(
                ErrorCode::Unavailable,
                "CLI installation path is unavailable",
                "Reinstall TermiRust and try again.",
            )
        })?;
        let host_executable = std::env::var_os("TERMIRUST_SESSION_HOST_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| sibling_binary(&current, "termirust-session-host"));
        Ok(Self::new(config_root, host_executable))
    }

    pub fn new(config_root: impl Into<PathBuf>, host_executable: impl Into<PathBuf>) -> Self {
        let config_root = config_root.into();
        Self {
            metadata_root: config_root.join(STORE_DIR_NAME),
            session_data_root: config_root.join(SESSION_DATA_DIR_NAME),
            runtime_parent: durable_runtime_parent(&config_root),
            config_root,
            host_executable: host_executable.into(),
        }
    }

    pub fn config_root(&self) -> &Path {
        &self.config_root
    }

    pub fn metadata_root(&self) -> &Path {
        &self.metadata_root
    }

    pub fn session_data_root(&self) -> &Path {
        &self.session_data_root
    }

    fn runtime_root(&self, session_id: HostedSessionId) -> PathBuf {
        self.runtime_parent.join(session_id.to_string())
    }

    fn session_dir(&self, session_id: HostedSessionId) -> PathBuf {
        self.session_data_root.join(session_id.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliInstallationStatus {
    pub path: PathBuf,
    pub available: bool,
    pub host_available: bool,
    pub json_schema_version: u16,
    pub protocol_version: String,
}

pub fn cli_installation_status(current_executable: &Path) -> CliInstallationStatus {
    let path = sibling_binary(current_executable, "termirust-cli");
    let host_path = sibling_binary(current_executable, "termirust-session-host");
    CliInstallationStatus {
        available: path.is_file(),
        host_available: host_path.is_file(),
        path,
        json_schema_version: CLI_JSON_SCHEMA_VERSION,
        protocol_version: format!(
            "{}.{}",
            CURRENT_PROTOCOL.maximum.major, CURRENT_PROTOCOL.maximum.minor
        ),
    }
}

pub trait HostLauncher: Send + Sync {
    fn launch(
        &self,
        descriptor: &LaunchDescriptor,
        host_executable: &Path,
        cancellation: &Cancellation,
    ) -> Result<HostLaunchOutcome, CliError>;
}

pub trait HostController: Send + Sync {
    fn stop(
        &self,
        runtime_root: &Path,
        session_id: HostedSessionId,
        expected_host_instance_id: Option<HostInstanceId>,
        command_id: CommandId,
        cancellation: &Cancellation,
    ) -> Result<(), CliError>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ManagementRemovalManifest {
    pub metadata_bytes: u64,
    pub journal_bytes: u64,
    pub transcript_bytes: u64,
    pub artifact_bytes: u64,
    pub file_count: usize,
}

impl ManagementRemovalManifest {
    pub fn total_bytes(self) -> u64 {
        self.metadata_bytes
            .saturating_add(self.journal_bytes)
            .saturating_add(self.transcript_bytes)
            .saturating_add(self.artifact_bytes)
    }

    pub fn requires_title_confirmation(self) -> bool {
        self.transcript_bytes > 0 || self.artifact_bytes > 0
    }
}

impl From<SessionRemovalManifest> for ManagementRemovalManifest {
    fn from(value: SessionRemovalManifest) -> Self {
        Self {
            metadata_bytes: value.metadata_bytes,
            journal_bytes: value.journal_bytes,
            transcript_bytes: value.transcript_bytes,
            artifact_bytes: value.artifact_bytes,
            file_count: value.file_count,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CliRemovalPreviewToken {
    expected_revision: Revision,
    manifest: ManagementRemovalManifest,
}

impl CliRemovalPreviewToken {
    const PREFIX: &'static str = "tr-remove-v1";

    fn encode(self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            Self::PREFIX,
            self.expected_revision.get(),
            self.manifest.metadata_bytes,
            self.manifest.journal_bytes,
            self.manifest.transcript_bytes,
            self.manifest.artifact_bytes,
            self.manifest.file_count,
        )
    }

    fn parse(value: &str) -> Result<Self, CliError> {
        if value.len() > 256 {
            return Err(invalid_removal_preview_token());
        }
        let mut parts = value.split(':');
        if parts.next() != Some(Self::PREFIX) {
            return Err(invalid_removal_preview_token());
        }
        let expected_revision = parse_token_u64(parts.next())?;
        let token = Self {
            expected_revision: Revision::new(expected_revision),
            manifest: ManagementRemovalManifest {
                metadata_bytes: parse_token_u64(parts.next())?,
                journal_bytes: parse_token_u64(parts.next())?,
                transcript_bytes: parse_token_u64(parts.next())?,
                artifact_bytes: parse_token_u64(parts.next())?,
                file_count: parts
                    .next()
                    .ok_or_else(invalid_removal_preview_token)?
                    .parse::<usize>()
                    .map_err(|_| invalid_removal_preview_token())?,
            },
        };
        if parts.next().is_some() || token.encode() != value {
            return Err(invalid_removal_preview_token());
        }
        Ok(token)
    }
}

fn parse_token_u64(value: Option<&str>) -> Result<u64, CliError> {
    value
        .ok_or_else(invalid_removal_preview_token)?
        .parse::<u64>()
        .map_err(|_| invalid_removal_preview_token())
}

fn invalid_removal_preview_token() -> CliError {
    validation(
        "session removal preview token is invalid",
        "Run session remove <id> again and use the exact new preview token.",
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagementRemovalPreview {
    pub session_id: HostedSessionId,
    pub expected_revision: Revision,
    pub manifest: ManagementRemovalManifest,
}

#[derive(Clone, Eq, PartialEq)]
pub enum ManagementCommand {
    Launch {
        command_id: CommandId,
        project_id: ProjectId,
        preset_id: PresetId,
        group_id: Option<GroupId>,
    },
    Rename {
        command_id: CommandId,
        session_id: HostedSessionId,
        expected_revision: Revision,
        title: String,
    },
    SetPinned {
        command_id: CommandId,
        session_id: HostedSessionId,
        expected_revision: Revision,
        pinned: bool,
    },
    MarkRead {
        command_id: CommandId,
        session_id: HostedSessionId,
        expected_revision: Revision,
    },
    Stop {
        command_id: CommandId,
        session_id: HostedSessionId,
        expected_revision: Revision,
    },
    Archive {
        command_id: CommandId,
        session_id: HostedSessionId,
        expected_revision: Revision,
    },
    Restore {
        command_id: CommandId,
        session_id: HostedSessionId,
        expected_revision: Revision,
    },
    StopAndArchive {
        command_id: CommandId,
        session_id: HostedSessionId,
        expected_revision: Revision,
    },
    Remove {
        command_id: CommandId,
        session_id: HostedSessionId,
        expected_revision: Revision,
        expected_manifest: ManagementRemovalManifest,
        title_confirmation: Option<String>,
    },
}

impl fmt::Debug for ManagementCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Launch { .. } => "launch",
            Self::Rename { .. } => "rename",
            Self::SetPinned { .. } => "set-pinned",
            Self::MarkRead { .. } => "mark-read",
            Self::Stop { .. } => "stop",
            Self::Archive { .. } => "archive",
            Self::Restore { .. } => "restore",
            Self::StopAndArchive { .. } => "stop-and-archive",
            Self::Remove { .. } => "remove",
        };
        formatter
            .debug_struct("ManagementCommand")
            .field("kind", &kind)
            .finish_non_exhaustive()
    }
}

pub trait CliClock: Send + Sync {
    fn now_millis(&self) -> u64;
}

pub trait CliWaiter: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep_interruptibly(&self, duration: Duration, cancellation: &Cancellation) -> bool;
}

pub trait CliIds: Send + Sync {
    fn session_id(&self) -> HostedSessionId;
    fn command_id(&self) -> CommandId;
    fn host_instance_id(&self) -> HostInstanceId;
}

pub trait SshControllerCommandExecutor: Send + Sync {
    fn execute(
        &self,
        command: ControllerSshCommand,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostLaunchOutcome {
    Ready,
    ReadyAfterPreReadyCancellation,
}

pub struct LocalCommandService {
    paths: CliPaths,
    launcher: Arc<dyn HostLauncher>,
    controller: Arc<dyn HostController>,
    clock: Arc<dyn CliClock>,
    waiter: Arc<dyn CliWaiter>,
    ids: Arc<dyn CliIds>,
    ssh_controller: Arc<dyn SshControllerCommandExecutor>,
}

impl LocalCommandService {
    pub fn open(paths: CliPaths) -> Self {
        let ssh_controller = Arc::new(crate::SystemSshControllerExecutor::new(
            paths.config_root.clone(),
        ));
        Self::with_adapters(
            paths,
            Arc::new(ProcessHostLauncher),
            Arc::new(LocalHostController),
            Arc::new(SystemClock),
            Arc::new(RandomIds),
        )
        .with_ssh_controller(ssh_controller)
    }

    pub fn with_adapters(
        paths: CliPaths,
        launcher: Arc<dyn HostLauncher>,
        controller: Arc<dyn HostController>,
        clock: Arc<dyn CliClock>,
        ids: Arc<dyn CliIds>,
    ) -> Self {
        Self {
            paths,
            launcher,
            controller,
            clock,
            waiter: Arc::new(SystemWaiter::new()),
            ids,
            ssh_controller: Arc::new(UnavailableSshController),
        }
    }

    pub fn with_ssh_controller(
        mut self,
        ssh_controller: Arc<dyn SshControllerCommandExecutor>,
    ) -> Self {
        self.ssh_controller = ssh_controller;
        self
    }

    pub fn with_waiter(mut self, waiter: Arc<dyn CliWaiter>) -> Self {
        self.waiter = waiter;
        self
    }

    pub fn execute_management(
        &self,
        command: ManagementCommand,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        match command {
            ManagementCommand::Launch {
                command_id,
                project_id,
                preset_id,
                group_id,
            } => self.session_launch(
                project_id,
                preset_id,
                group_id,
                Some(HostedSessionId::from_uuid(command_id.as_uuid())),
                Some(command_id),
                cancellation,
            ),
            ManagementCommand::Rename {
                command_id: _,
                session_id,
                expected_revision,
                title,
            } => {
                let title = SessionTitle::new(&title).map_err(|_| {
                    validation(
                        "session title is invalid",
                        "Use a non-empty title without control characters.",
                    )
                })?;
                self.session_metadata_mutation(
                    session_id,
                    expected_revision,
                    SessionMutation::Rename(title),
                    "renamed",
                )
            }
            ManagementCommand::SetPinned {
                command_id: _,
                session_id,
                expected_revision,
                pinned,
            } => self.session_metadata_mutation(
                session_id,
                expected_revision,
                SessionMutation::SetPinned(pinned),
                if pinned { "pinned" } else { "unpinned" },
            ),
            ManagementCommand::MarkRead {
                command_id: _,
                session_id,
                expected_revision,
            } => {
                let repository = self.sessions()?;
                let snapshot = repository.load().map_err(map_store)?;
                let session = require_session(&snapshot.sessions, session_id)?;
                if expected_revision != session.revision && !session.unread() {
                    return Ok(mutation("marked_read", session));
                }
                let revision = mutation_revision(&snapshot, session, Some(expected_revision))?;
                let through = session.last_output_sequence;
                let session = repository
                    .mutate_session(
                        session_id,
                        revision,
                        SessionMutation::MarkRead { through },
                        self.clock.now_millis(),
                    )
                    .map_err(map_store)?;
                Ok(mutation("marked_read", &session))
            }
            ManagementCommand::Stop {
                command_id,
                session_id,
                expected_revision,
            } => self.session_stop(
                session_id,
                Some(expected_revision),
                true,
                true,
                Some(command_id),
                cancellation,
            ),
            ManagementCommand::Archive {
                command_id: _,
                session_id,
                expected_revision,
            } => self.session_archive(session_id, Some(expected_revision), true),
            ManagementCommand::Restore {
                command_id: _,
                session_id,
                expected_revision,
            } => self.session_restore(session_id, Some(expected_revision), true),
            ManagementCommand::StopAndArchive {
                command_id,
                session_id,
                expected_revision,
            } => {
                let stopped = self.session_stop(
                    session_id,
                    Some(expected_revision),
                    true,
                    true,
                    Some(command_id),
                    cancellation,
                )?;
                let CliData::Mutation(stopped) = stopped else {
                    return Err(operation("stop returned an inconsistent result"));
                };
                let archived = self.session_archive(
                    session_id,
                    Some(Revision::new(stopped.session.revision)),
                    true,
                )?;
                let CliData::Mutation(mut archived) = archived else {
                    return Err(operation("archive returned an inconsistent result"));
                };
                archived.outcome = "stopped_and_archived".into();
                Ok(CliData::Mutation(archived))
            }
            ManagementCommand::Remove {
                command_id: _,
                session_id,
                expected_revision,
                expected_manifest,
                title_confirmation,
            } => self.remove_management_session(
                session_id,
                expected_revision,
                expected_manifest,
                title_confirmation.as_deref(),
                cancellation,
            ),
        }
    }

    pub fn prepare_management_removal(
        &self,
        session_id: HostedSessionId,
        expected_session_revision: Revision,
        cancellation: &Cancellation,
    ) -> Result<ManagementRemovalPreview, CliError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let repository = self.sessions()?;
        let snapshot = repository.load().map_err(map_store)?;
        let session = require_session(&snapshot.sessions, session_id)?;
        mutation_revision(&snapshot, session, Some(expected_session_revision))?;
        let plan = repository.removal_plan(session_id).map_err(map_store)?;
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        Ok(ManagementRemovalPreview {
            session_id,
            expected_revision: plan.expected_revision,
            manifest: plan.manifest.into(),
        })
    }

    fn remove_management_session(
        &self,
        session_id: HostedSessionId,
        expected_revision: Revision,
        expected_manifest: ManagementRemovalManifest,
        title_confirmation: Option<&str>,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        if title_confirmation
            .is_some_and(|value| value.chars().count() > 256 || value.chars().any(char::is_control))
        {
            return Err(validation(
                "session removal confirmation is invalid",
                "Use the bounded confirmation requested by the removal preview.",
            ));
        }
        let repository = self.sessions()?;
        let plan = repository.removal_plan(session_id).map_err(map_store)?;
        if plan.expected_revision != expected_revision {
            return Err(CliError::new(
                ErrorCode::Conflict,
                "session metadata changed after the removal preview",
                "Refresh the fleet and review a new removal preview.",
            )
            .with_revision(plan.expected_revision));
        }
        if ManagementRemovalManifest::from(plan.manifest) != expected_manifest {
            return Err(CliError::new(
                ErrorCode::Conflict,
                "session data changed after the removal preview",
                "Refresh the fleet and review the updated removal manifest.",
            )
            .with_revision(plan.expected_revision));
        }
        let expected_confirmation = if plan.manifest.requires_title_confirmation() {
            plan.title.as_str()
        } else {
            "REMOVE"
        };
        if title_confirmation != Some(expected_confirmation) {
            return Err(validation(
                "session removal confirmation did not match",
                "Type the exact confirmation shown in the reviewed removal preview.",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let removed = repository
            .remove_session(&plan, expected_revision)
            .map_err(map_store)?;
        Ok(mutation("removed", &removed.session))
    }

    fn session_removal_preview(
        &self,
        session_id: HostedSessionId,
        expected_session_revision: Option<Revision>,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let repository = self.sessions()?;
        let snapshot = repository.load().map_err(map_store)?;
        let session = require_session(&snapshot.sessions, session_id)?.clone();
        mutation_revision(&snapshot, &session, expected_session_revision)?;
        let plan = repository.removal_plan(session_id).map_err(map_store)?;
        if plan.expected_revision != snapshot.revision {
            return Err(CliError::new(
                ErrorCode::Conflict,
                "session metadata changed while the removal preview was prepared",
                "Run session remove <id> again and review the new preview.",
            )
            .with_revision(plan.expected_revision));
        }
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let manifest = ManagementRemovalManifest::from(plan.manifest);
        let token = CliRemovalPreviewToken {
            expected_revision: plan.expected_revision,
            manifest,
        };
        Ok(CliData::RemovalPreview(SessionRemovalPreviewData {
            session: SessionView::from(&session),
            preview_token: token.encode(),
            repository_revision: plan.expected_revision.get(),
            metadata_bytes: manifest.metadata_bytes,
            journal_bytes: manifest.journal_bytes,
            transcript_bytes: manifest.transcript_bytes,
            artifact_bytes: manifest.artifact_bytes,
            total_bytes: manifest.total_bytes(),
            file_count: manifest.file_count,
            confirmation: if manifest.requires_title_confirmation() {
                RemovalConfirmationKind::SessionTitle
            } else {
                RemovalConfirmationKind::Remove
            },
        }))
    }

    fn commit_cli_session_removal(
        &self,
        session_id: HostedSessionId,
        expected_session_revision: Option<Revision>,
        preview_token: &str,
        confirmation: &str,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        let token = CliRemovalPreviewToken::parse(preview_token)?;
        let current =
            self.session_removal_preview(session_id, expected_session_revision, cancellation)?;
        let CliData::RemovalPreview(current) = current else {
            return Err(operation("session removal preview was inconsistent"));
        };
        if current.preview_token != preview_token {
            return Err(CliError::new(
                ErrorCode::Conflict,
                "session removal preview is stale",
                "Run session remove <id> again and review the new preview.",
            )
            .with_revision(Revision::new(current.repository_revision)));
        }
        self.execute_management(
            ManagementCommand::Remove {
                command_id: self.ids.command_id(),
                session_id,
                expected_revision: token.expected_revision,
                expected_manifest: token.manifest,
                title_confirmation: Some(confirmation.into()),
            },
            cancellation,
        )
    }

    fn session_metadata_mutation(
        &self,
        session_id: HostedSessionId,
        expected_revision: Revision,
        session_mutation: SessionMutation,
        outcome: &'static str,
    ) -> Result<CliData, CliError> {
        let repository = self.sessions()?;
        let snapshot = repository.load().map_err(map_store)?;
        let session = require_session(&snapshot.sessions, session_id)?;
        if expected_revision != session.revision
            && management_mutation_already_applied(session, &session_mutation)
        {
            return Ok(mutation(outcome, session));
        }
        let revision = mutation_revision(&snapshot, session, Some(expected_revision))?;
        let session = repository
            .mutate_session(
                session_id,
                revision,
                session_mutation,
                self.clock.now_millis(),
            )
            .map_err(map_store)?;
        Ok(mutation(outcome, &session))
    }

    fn status(&self) -> Result<CliData, CliError> {
        self.require_existing_store()?;
        let projects =
            ProjectRepository::open(self.paths.metadata_root.clone()).map_err(map_store)?;
        let project_snapshot = projects.load().map_err(map_store)?;
        Ok(CliData::Status(StatusData {
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            json_schema_version: CLI_JSON_SCHEMA_VERSION,
            protocol_minimum: format!(
                "{}.{}",
                CURRENT_PROTOCOL.minimum.major, CURRENT_PROTOCOL.minimum.minor
            ),
            protocol_maximum: format!(
                "{}.{}",
                CURRENT_PROTOCOL.maximum.major, CURRENT_PROTOCOL.maximum.minor
            ),
            store: if project_snapshot.health == StoreHealth::Healthy {
                "available"
            } else {
                "recovered_read_only"
            }
            .to_string(),
            host_control: if self.paths.host_executable.is_file() {
                "available"
            } else {
                "host_unavailable"
            }
            .to_string(),
        }))
    }

    fn project_list(&self) -> Result<CliData, CliError> {
        let snapshot = self.projects()?.load().map_err(map_store)?;
        bounded_records(snapshot.projects.len())?;
        Ok(CliData::Projects(ProjectListData {
            projects: snapshot.projects.iter().map(ProjectView::from).collect(),
        }))
    }

    fn preset_list(&self, project_id: ProjectId) -> Result<CliData, CliError> {
        let (projects, snapshot) = self.consistent_project_preset_snapshot()?;
        require_project(&projects.projects, project_id)?;
        bounded_records(snapshot.presets.len())?;
        Ok(CliData::Presets(PresetListData {
            project_id: project_id.to_string(),
            presets: snapshot.presets.iter().map(PresetView::from).collect(),
        }))
    }

    fn session_list(&self, filter: SessionListFilter) -> Result<CliData, CliError> {
        let (projects, snapshot) = self.consistent_project_session_snapshot()?;
        if let Some(project_id) = filter.project_id {
            require_project(&projects.projects, project_id)?;
        }
        if let Some(group_id) = filter.group_id {
            let group = projects
                .groups
                .iter()
                .find(|group| group.id == group_id)
                .ok_or_else(|| unavailable("group is unavailable"))?;
            if filter
                .project_id
                .is_some_and(|project_id| group.project_id != project_id)
            {
                return Err(validation(
                    "group does not belong to the selected project",
                    "Choose a group from the same project.",
                ));
            }
        }
        let sessions = snapshot
            .sessions
            .iter()
            .filter(|session| {
                filter
                    .project_id
                    .is_none_or(|project_id| session.project_id == project_id)
                    && filter
                        .group_id
                        .is_none_or(|group_id| session.group_id == Some(group_id))
                    && filter.state.is_none_or(|state| session.lifecycle == state)
                    && (!filter.archived_only || session.archived_at.is_some())
            })
            .collect::<Vec<_>>();
        bounded_records(sessions.len())?;
        Ok(CliData::Sessions(SessionListData {
            sessions: sessions.into_iter().map(SessionView::from).collect(),
        }))
    }

    fn session_show(&self, session_id: HostedSessionId) -> Result<CliData, CliError> {
        let snapshot = self.sessions()?.load().map_err(map_store)?;
        let session = require_session(&snapshot.sessions, session_id)?;
        Ok(CliData::Session(SessionData {
            session: SessionView::from(session),
        }))
    }

    fn session_wait(
        &self,
        session_id: HostedSessionId,
        condition: SessionWaitCondition,
        timeout_ms: u64,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        if !(1..=MAX_SESSION_WAIT_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(validation(
                "session wait timeout is outside the supported range",
                "Use a timeout from 1 through 300000 milliseconds.",
            ));
        }
        let repository = self.sessions()?;
        let timeout = Duration::from_millis(timeout_ms);
        let deadline = self.waiter.now().saturating_add(timeout);
        let mut first_observation = true;
        let mut last_revision = None;

        loop {
            if cancellation.is_cancelled() {
                return Err(cancelled());
            }
            let now = self.waiter.now();
            if !first_observation && now >= deadline {
                let mut error = CliError::new(
                    ErrorCode::Timeout,
                    "session wait timed out before the requested state was observed",
                    "Inspect the current Session state and retry with a suitable bounded timeout.",
                );
                if let Some(revision) = last_revision {
                    error = error.with_revision(revision);
                }
                return Err(error);
            }

            let snapshot = repository.load().map_err(map_store)?;
            let session = require_session(&snapshot.sessions, session_id)?;
            last_revision = Some(session.revision);
            if wait_condition_matches(condition, session) {
                return Ok(CliData::Wait(SessionWaitData {
                    session: SessionView::from(session),
                    condition: SessionWaitConditionData::from(condition),
                }));
            }
            first_observation = false;

            let now = self.waiter.now();
            if now >= deadline {
                continue;
            }
            let wait_for = SESSION_WAIT_POLL_INTERVAL.min(deadline.saturating_sub(now));
            if !self.waiter.sleep_interruptibly(wait_for, cancellation)
                || cancellation.is_cancelled()
            {
                return Err(cancelled());
            }
        }
    }

    fn session_archive(
        &self,
        session_id: HostedSessionId,
        expected: Option<Revision>,
        allow_idempotent_replay: bool,
    ) -> Result<CliData, CliError> {
        let repository = self.sessions()?;
        let snapshot = repository.load().map_err(map_store)?;
        let session = require_session(&snapshot.sessions, session_id)?;
        if allow_idempotent_replay && session.archived_at.is_some() {
            return Ok(mutation("archived", session));
        }
        if !session.lifecycle.is_exited() {
            return Err(validation(
                "only an exited session can be archived",
                "Run session stop <id> --yes first, wait for Exited, then archive.",
            ));
        }
        let revision = mutation_revision(&snapshot, session, expected)?;
        let session = repository
            .mutate_session(
                session_id,
                revision,
                SessionMutation::Archive {
                    at: self.clock.now_millis(),
                },
                self.clock.now_millis(),
            )
            .map_err(map_store)?;
        Ok(mutation("archived", &session))
    }

    fn session_restore(
        &self,
        session_id: HostedSessionId,
        expected: Option<Revision>,
        allow_idempotent_replay: bool,
    ) -> Result<CliData, CliError> {
        let repository = self.sessions()?;
        let snapshot = repository.load().map_err(map_store)?;
        let session = require_session(&snapshot.sessions, session_id)?;
        if allow_idempotent_replay && session.archived_at.is_none() {
            return Ok(mutation("restored", session));
        }
        let revision = mutation_revision(&snapshot, session, expected)?;
        let session = repository
            .mutate_session(
                session_id,
                revision,
                SessionMutation::Restore,
                self.clock.now_millis(),
            )
            .map_err(map_store)?;
        Ok(mutation("restored", &session))
    }

    fn session_stop(
        &self,
        session_id: HostedSessionId,
        expected: Option<Revision>,
        confirmed: bool,
        require_exact_host: bool,
        command_id: Option<CommandId>,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        if !confirmed {
            return Err(validation(
                "session stop requires --yes",
                "Review the session ID, then rerun with --yes.",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let repository = self.sessions()?;
        let snapshot = repository.load().map_err(map_store)?;
        let session = require_session(&snapshot.sessions, session_id)?;
        if require_exact_host && session.lifecycle.is_exited() {
            return Ok(mutation("stopped", session));
        }
        if !session.lifecycle.can_stop() {
            return Err(validation(
                "session is not in a stoppable state",
                "Inspect the session and wait for a live or provisioning state.",
            ));
        }
        let revision = mutation_revision(&snapshot, session, expected)?;
        let runtime_root = self.paths.runtime_root(session_id);
        let expected_host_instance_id = require_exact_host
            .then(|| read_host_metadata(&self.paths.session_dir(session_id)))
            .transpose()
            .map_err(|_| unavailable("durable Host ownership metadata is unavailable or unsafe"))?
            .map(|metadata| metadata.host_instance_id);
        let stopping = repository
            .mutate_session(
                session_id,
                revision,
                SessionMutation::SetLifecycle(HostedSessionState::Stopping),
                self.clock.now_millis(),
            )
            .map_err(map_store)?;
        let command_id = command_id.unwrap_or_else(|| self.ids.command_id());
        match self.controller.stop(
            &runtime_root,
            session_id,
            expected_host_instance_id,
            command_id,
            cancellation,
        ) {
            Ok(()) => {
                let exited = repository
                    .mutate_session(
                        session_id,
                        stopping.revision,
                        SessionMutation::SetLifecycle(HostedSessionState::Exited),
                        self.clock.now_millis(),
                    )
                    .map_err(map_store)?;
                Ok(mutation("stopped", &exited))
            }
            Err(error) if error.code == ErrorCode::Cancelled => Err(CliError::new(
                ErrorCode::OperationFailed,
                "stop outcome is not yet confirmed",
                "Inspect the session status. The stop command may still complete.",
            )
            .with_revision(stopping.revision)),
            Err(error) => Err(error.with_revision(stopping.revision)),
        }
    }

    fn session_launch(
        &self,
        project_id: ProjectId,
        preset_id: PresetId,
        group_id: Option<GroupId>,
        requested_session_id: Option<HostedSessionId>,
        command_id: Option<CommandId>,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let project_repository = self.projects()?;
        let preset_repository = self.presets()?;
        let session_repository = self.sessions()?;
        let projects = project_repository.load().map_err(map_store)?;
        let presets = preset_repository.load().map_err(map_store)?;
        let sessions = session_repository.load().map_err(map_store)?;
        if let Some(session_id) = requested_session_id
            && let Some(existing) = sessions
                .sessions
                .iter()
                .find(|session| session.id == session_id)
        {
            if existing.project_id != project_id
                || existing.preset_id != Some(preset_id)
                || existing.group_id != group_id
            {
                return Err(CliError::new(
                    ErrorCode::Conflict,
                    "management command identity was already used for another launch",
                    "Refresh Sessions and submit a new launch command.",
                )
                .with_revision(existing.revision));
            }
            return Ok(mutation("launched", existing));
        }
        let project = require_project(&projects.projects, project_id)?
            .project
            .clone();
        let preset = presets
            .presets
            .iter()
            .find(|preset| preset.id == preset_id)
            .cloned()
            .ok_or_else(|| unavailable("preset is unavailable"))?;
        if matches!(preset.risk, PresetRisk::Risky(_)) {
            return Err(validation(
                "risky presets cannot be launched implicitly from CLI v1",
                "Review and launch this preset from the desktop application.",
            ));
        }
        if let Some(group_id) = group_id {
            let group = projects
                .groups
                .iter()
                .find(|group| group.id == group_id)
                .ok_or_else(|| unavailable("group is unavailable"))?;
            if group.project_id != project_id {
                return Err(validation(
                    "group does not belong to the selected project",
                    "Choose a group from the same project.",
                ));
            }
        }
        let session_id = requested_session_id.unwrap_or_else(|| self.ids.session_id());
        let path_snapshot = explicit_path_snapshot();
        let home = dirs::home_dir();
        let resolved = resolve_launch(
            session_id,
            &project,
            &preset,
            &path_snapshot,
            home.as_deref(),
        )
        .map_err(|_| {
            validation(
                "session launch validation failed",
                "Review project availability and the preset executable in the desktop application.",
            )
        })?;
        resolved.revalidate().map_err(|_| {
            CliError::new(
                ErrorCode::Conflict,
                "project or executable changed during launch validation",
                "Reload projects and presets, then run the command again.",
            )
        })?;
        if project_repository.load().map_err(map_store)?.revision != projects.revision
            || preset_repository.load().map_err(map_store)?.revision != presets.revision
        {
            return Err(CliError::new(
                ErrorCode::Conflict,
                "project or preset metadata changed during launch validation",
                "Reload projects and presets, then run the command again.",
            ));
        }
        create_user_only_directory(&self.paths.runtime_root(session_id))?;
        create_user_only_directory(&self.paths.session_dir(session_id))?;
        let now = self.clock.now_millis();
        let session = HostedSession {
            id: session_id,
            project_id,
            group_id,
            preset_id: Some(preset_id),
            title: SessionTitle::new(preset.label.as_str()).map_err(|_| {
                validation(
                    "preset label cannot be used as a session title",
                    "Rename the preset in the desktop application.",
                )
            })?,
            title_source: TitleSource::Default,
            lifecycle: HostedSessionState::Provisioning,
            activity: ActivityAggregate::default(),
            pinned: false,
            position: PositionKey::FIRST,
            last_output_sequence: OutputSequence::ZERO,
            read_through_sequence: OutputSequence::ZERO,
            unread_sequence: None,
            archived_at: None,
            created_at: now,
            updated_at: now,
            revision: Revision::ZERO,
        };
        let created = session_repository
            .create_session(session, sessions.revision)
            .map_err(map_store)?;
        let descriptor = self.launch_descriptor(&resolved);
        let outcome = self
            .launcher
            .launch(&descriptor, &self.paths.host_executable, cancellation);
        match outcome {
            Ok(HostLaunchOutcome::Ready) => {
                let attaching = session_repository.mutate_session(
                    session_id,
                    created.revision,
                    SessionMutation::SetLifecycle(HostedSessionState::Attaching),
                    self.clock.now_millis(),
                );
                let live = match attaching.and_then(|attaching| {
                    session_repository.mutate_session(
                        session_id,
                        attaching.revision,
                        SessionMutation::SetLifecycle(HostedSessionState::Live),
                        self.clock.now_millis(),
                    )
                }) {
                    Ok(session) => return Ok(mutation("launched", &session)),
                    Err(_) => session_repository.load().ok().and_then(|snapshot| {
                        snapshot
                            .sessions
                            .into_iter()
                            .find(|session| session.id == session_id)
                    }),
                };
                live.map(|session| mutation("running_reconciliation_pending", &session))
                    .ok_or_else(|| {
                        CliError::new(
                            ErrorCode::OperationFailed,
                            "Host is ready but session metadata requires reconciliation",
                            "Run session list and inspect the session before taking another action.",
                        )
                    })
            }
            Ok(HostLaunchOutcome::ReadyAfterPreReadyCancellation) => {
                let stop = self.controller.stop(
                    &self.paths.runtime_root(session_id),
                    session_id,
                    Some(descriptor.host_instance_id),
                    command_id.unwrap_or_else(|| self.ids.command_id()),
                    &Cancellation::default(),
                );
                let lifecycle = if stop.is_ok() {
                    HostedSessionState::Cancelled
                } else {
                    HostedSessionState::Offline
                };
                let _ = session_repository.mutate_session(
                    session_id,
                    created.revision,
                    SessionMutation::SetLifecycle(lifecycle),
                    self.clock.now_millis(),
                );
                if stop.is_ok() {
                    Err(cancelled())
                } else {
                    Err(CliError::new(
                        ErrorCode::OperationFailed,
                        "launch cancellation could not confirm Host shutdown",
                        "Inspect the session before retrying or stopping it.",
                    ))
                }
            }
            Err(error) => {
                let lifecycle = if error.code == ErrorCode::Cancelled {
                    HostedSessionState::Cancelled
                } else {
                    HostedSessionState::Failed
                };
                let _ = session_repository.mutate_session(
                    session_id,
                    created.revision,
                    SessionMutation::SetLifecycle(lifecycle),
                    self.clock.now_millis(),
                );
                Err(error)
            }
        }
    }

    fn launch_descriptor(&self, resolved: &termirust_domain::ResolvedLaunch) -> LaunchDescriptor {
        let mut environment = BTreeMap::new();
        for name in ["HOME", "LANG", "LC_ALL", "PATH", "SHELL", "TERM"] {
            if let Ok(value) = std::env::var(name) {
                environment.insert(name.to_string(), value);
            }
        }
        environment
            .entry("TERM".to_string())
            .or_insert_with(|| "xterm-256color".to_string());
        LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id: resolved.session_id,
            host_instance_id: self.ids.host_instance_id(),
            expected_occupant_generation: None,
            runtime_root: self.paths.runtime_root(resolved.session_id),
            session_dir: self.paths.session_dir(resolved.session_id),
            executable: resolved.executable().to_path_buf(),
            runtime_detection: None,
            arguments: resolved.arguments().to_vec(),
            environment,
            cwd: Some(resolved.working_directory().to_path_buf()),
            columns: 160,
            rows: 48,
            journal_limits: JournalLimits::default(),
            stop_deadlines: StopDeadlines::default(),
        }
    }

    fn require_existing_store(&self) -> Result<(), CliError> {
        if self.paths.metadata_root.join(FORMAT_FILE_NAME).is_file() {
            Ok(())
        } else {
            Err(unavailable("TermiRust metadata store is unavailable"))
        }
    }

    fn projects(&self) -> Result<ProjectRepository, CliError> {
        self.require_existing_store()?;
        ProjectRepository::open(self.paths.metadata_root.clone()).map_err(map_store)
    }

    fn presets(&self) -> Result<PresetRepository, CliError> {
        self.require_existing_store()?;
        PresetRepository::open(self.paths.metadata_root.clone()).map_err(map_store)
    }

    fn sessions(&self) -> Result<SessionRepository, CliError> {
        self.require_existing_store()?;
        SessionRepository::open(
            self.paths.metadata_root.clone(),
            self.paths.session_data_root.clone(),
        )
        .map_err(map_store)
    }

    fn consistent_project_preset_snapshot(
        &self,
    ) -> Result<(ProjectSnapshot, PresetSnapshot), CliError> {
        let projects = self.projects()?;
        let presets = self.presets()?;
        for _ in 0..2 {
            let first_projects = projects.load().map_err(map_store)?;
            let first_presets = presets.load().map_err(map_store)?;
            let second_projects = projects.load().map_err(map_store)?;
            let second_presets = presets.load().map_err(map_store)?;
            if first_projects.revision == second_projects.revision
                && first_presets.revision == second_presets.revision
            {
                return Ok((second_projects, second_presets));
            }
        }
        Err(temporarily_inconsistent())
    }

    fn consistent_project_session_snapshot(
        &self,
    ) -> Result<(ProjectSnapshot, SessionSnapshot), CliError> {
        let projects = self.projects()?;
        let sessions = self.sessions()?;
        for _ in 0..2 {
            let first_projects = projects.load().map_err(map_store)?;
            let first_sessions = sessions.load().map_err(map_store)?;
            let second_projects = projects.load().map_err(map_store)?;
            let second_sessions = sessions.load().map_err(map_store)?;
            if first_projects.revision == second_projects.revision
                && first_sessions.revision == second_sessions.revision
            {
                return Ok((second_projects, second_sessions));
            }
        }
        Err(temporarily_inconsistent())
    }
}

impl CommandService for LocalCommandService {
    fn execute(
        &mut self,
        command: CliCommand,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        match command {
            CliCommand::Help => Ok(crate::help_data()),
            CliCommand::Status => self.status(),
            CliCommand::ProjectList => self.project_list(),
            CliCommand::PresetList { project_id } => self.preset_list(project_id),
            CliCommand::SessionList(filter) => self.session_list(filter),
            CliCommand::SessionShow { session_id } => self.session_show(session_id),
            CliCommand::SessionWait {
                session_id,
                condition,
                timeout_ms,
            } => self.session_wait(session_id, condition, timeout_ms, cancellation),
            CliCommand::SessionLaunch {
                project_id,
                preset_id,
                group_id,
            } => self.session_launch(project_id, preset_id, group_id, None, None, cancellation),
            CliCommand::SessionStop {
                session_id,
                expected_revision,
                confirmed,
            } => self.session_stop(
                session_id,
                expected_revision,
                confirmed,
                false,
                None,
                cancellation,
            ),
            CliCommand::SessionArchive {
                session_id,
                expected_revision,
            } => self.session_archive(session_id, expected_revision, false),
            CliCommand::SessionRestore {
                session_id,
                expected_revision,
            } => self.session_restore(session_id, expected_revision, false),
            CliCommand::SessionRemove {
                session_id,
                expected_revision,
                preview_token,
                confirmed,
                confirmation_stdin,
                confirmation,
            } => match (
                preview_token.as_deref(),
                confirmed,
                confirmation_stdin,
                confirmation.as_ref(),
            ) {
                (None, false, false, None) => {
                    self.session_removal_preview(session_id, expected_revision, cancellation)
                }
                (Some(token), true, true, Some(confirmation)) => self.commit_cli_session_removal(
                    session_id,
                    expected_revision,
                    token,
                    confirmation.expose(),
                    cancellation,
                ),
                _ => Err(CliError::new(
                    ErrorCode::InteractionRequired,
                    "session removal confirmation from stdin is required",
                    "Pipe the requested confirmation to stdin and use the exact preview token, --yes, and --confirmation-stdin.",
                )),
            },
            CliCommand::ControllerSsh(command) => {
                self.ssh_controller.execute(command, cancellation)
            }
        }
    }
}

struct UnavailableSshController;

impl SshControllerCommandExecutor for UnavailableSshController {
    fn execute(
        &self,
        _command: ControllerSshCommand,
        _cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        Err(CliError::new(
            ErrorCode::Unavailable,
            "remote SSH Controller service is unavailable",
            "Install a compatible TermiRust CLI and session Host, then retry the same route.",
        ))
    }
}

struct ProcessHostLauncher;

impl HostLauncher for ProcessHostLauncher {
    fn launch(
        &self,
        descriptor: &LaunchDescriptor,
        host_executable: &Path,
        cancellation: &Cancellation,
    ) -> Result<HostLaunchOutcome, CliError> {
        let host_executable = fs::canonicalize(host_executable)
            .map_err(|_| unavailable("TermiRust session Host companion is unavailable"))?;
        let mut child = Command::new(host_executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(map_process)?;
        let write_result = child.stdin.as_mut().map_or_else(
            || Err(()),
            |stdin| {
                serde_json::to_writer(&mut *stdin, descriptor)
                    .and_then(|()| stdin.flush().map_err(serde_json::Error::io))
                    .map_err(|_| ())
            },
        );
        child.stdin.take();
        if write_result.is_err() {
            terminate_host(&mut child);
            return Err(operation(
                "unable to send the bounded Host launch descriptor",
            ));
        }
        let stdout = child.stdout.take().ok_or_else(|| {
            terminate_host(&mut child);
            operation("session Host readiness pipe is unavailable")
        })?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = ready_tx.send(result);
        });
        let deadline = Instant::now() + HOST_READY_DEADLINE;
        let mut cancelled_before_ready = false;
        loop {
            cancelled_before_ready |= cancellation.is_cancelled();
            match ready_rx.recv_timeout(READY_POLL_INTERVAL) {
                Ok(Ok(line)) if line.contains("\"code\":\"host_ready\"") => {
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Ok(if cancelled_before_ready {
                        HostLaunchOutcome::ReadyAfterPreReadyCancellation
                    } else {
                        HostLaunchOutcome::Ready
                    });
                }
                Ok(_) => {
                    terminate_host(&mut child);
                    return Err(if cancelled_before_ready {
                        cancelled()
                    } else {
                        operation("session Host failed before becoming ready")
                    });
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    terminate_host(&mut child);
                    return Err(operation("session Host exited before becoming ready"));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    terminate_host(&mut child);
                    if cancelled_before_ready {
                        return Err(cancelled());
                    }
                    return Err(CliError::new(
                        ErrorCode::Timeout,
                        "session Host did not become ready within five seconds",
                        "Inspect local Host availability, then retry once.",
                    ));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

struct LocalHostController;

impl HostController for LocalHostController {
    fn stop(
        &self,
        runtime_root: &Path,
        session_id: HostedSessionId,
        expected_host_instance_id: Option<HostInstanceId>,
        command_id: CommandId,
        cancellation: &Cancellation,
    ) -> Result<(), CliError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| operation("unable to initialize local Host control"))?;
        let async_cancel = CancellationToken::new();
        let done = Arc::new(AtomicBool::new(false));
        let monitor_done = done.clone();
        let monitor_cancel = async_cancel.clone();
        let source = cancellation.clone();
        let monitor = std::thread::spawn(move || {
            while !monitor_done.load(Ordering::Acquire) {
                if source.is_cancelled() {
                    monitor_cancel.cancel();
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let endpoint = LocalEndpoint::new(runtime_root, session_id);
        let mut nonce = [0_u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let result = runtime.block_on(async {
            let mut client = HostClient::connect(
                endpoint,
                ConnectOptions::local(session_id, nonce),
                &async_cancel,
            )
            .await?;
            if expected_host_instance_id
                .is_some_and(|expected| client.host_instance_id() != Some(expected))
            {
                client.disconnect();
                return Err(ClientError::new(ClientErrorCode::InvalidIdentity));
            }
            client
                .stop(command_id, wire::StopMode::Graceful, &async_cancel)
                .await?;
            client.disconnect();
            Ok::<(), ClientError>(())
        });
        done.store(true, Ordering::Release);
        let _ = monitor.join();
        result.map_err(map_client)
    }
}

struct SystemClock;

impl CliClock for SystemClock {
    fn now_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

struct SystemWaiter {
    origin: Instant,
}

impl SystemWaiter {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl CliWaiter for SystemWaiter {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep_interruptibly(&self, duration: Duration, cancellation: &Cancellation) -> bool {
        let deadline = Instant::now() + duration;
        loop {
            if cancellation.is_cancelled() {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            std::thread::sleep(SESSION_WAIT_CANCELLATION_SLICE.min(deadline - now));
        }
    }
}

struct RandomIds;

impl CliIds for RandomIds {
    fn session_id(&self) -> HostedSessionId {
        HostedSessionId::new()
    }

    fn command_id(&self) -> CommandId {
        CommandId::new()
    }

    fn host_instance_id(&self) -> HostInstanceId {
        HostInstanceId::new()
    }
}

fn mutation(outcome: &str, session: &HostedSession) -> CliData {
    CliData::Mutation(SessionMutationData {
        outcome: outcome.to_string(),
        session: SessionView::from(session),
    })
}

fn require_project(
    projects: &[termirust_domain::ProjectSummary],
    id: ProjectId,
) -> Result<&termirust_domain::ProjectSummary, CliError> {
    projects
        .iter()
        .find(|summary| summary.project.id == id)
        .ok_or_else(|| unavailable("project is unavailable"))
}

fn require_session(
    sessions: &[HostedSession],
    id: HostedSessionId,
) -> Result<&HostedSession, CliError> {
    sessions
        .iter()
        .find(|session| session.id == id)
        .ok_or_else(|| unavailable("session is unavailable"))
}

fn wait_condition_matches(condition: SessionWaitCondition, session: &HostedSession) -> bool {
    match condition {
        SessionWaitCondition::Lifecycle(state) => session.lifecycle == state,
        SessionWaitCondition::Activity(state) => session.activity.state == state,
    }
}

fn mutation_revision(
    snapshot: &SessionSnapshot,
    session: &HostedSession,
    expected: Option<Revision>,
) -> Result<Revision, CliError> {
    if let Some(expected) = expected
        && expected != session.revision
    {
        return Err(CliError::new(
            ErrorCode::Conflict,
            "session changed after the expected revision was captured",
            "Inspect the session and retry with its current revision.",
        )
        .with_revision(session.revision));
    }
    Ok(snapshot.revision)
}

fn management_mutation_already_applied(
    session: &HostedSession,
    mutation: &SessionMutation,
) -> bool {
    match mutation {
        SessionMutation::Rename(title) => session.title == *title,
        SessionMutation::SetPinned(pinned) => session.pinned == *pinned,
        _ => false,
    }
}

fn bounded_records(count: usize) -> Result<(), CliError> {
    if count > MAX_RESPONSE_RECORDS {
        Err(CliError::new(
            ErrorCode::ResourceLimit,
            "command result exceeds 1,000 records",
            "Narrow the query with project, group, state, or archived filters.",
        ))
    } else {
        Ok(())
    }
}

fn map_store(error: StoreError) -> CliError {
    match error {
        StoreError::StoreNewer { .. } => CliError::new(
            ErrorCode::Incompatible,
            "TermiRust metadata was written by a newer version",
            "Upgrade the CLI. The newer metadata was not modified.",
        ),
        StoreError::Io {
            kind: std::io::ErrorKind::PermissionDenied,
            ..
        }
        | StoreError::Domain(termirust_domain::ProjectError::PermissionDenied) => CliError::new(
            ErrorCode::PermissionDenied,
            "permission to access local TermiRust metadata was denied",
            "Check ownership and user-only permissions, then retry.",
        ),
        StoreError::Io {
            operation,
            kind: std::io::ErrorKind::WouldBlock,
        } if operation.starts_with("lock ") => CliError::new(
            ErrorCode::Timeout,
            "local metadata remained busy for two seconds",
            "Wait for the current TermiRust operation to finish, then retry once.",
        ),
        StoreError::TooLarge { .. }
        | StoreError::Domain(termirust_domain::ProjectError::ResourceLimit { .. })
        | StoreError::PresetDomain(termirust_domain::PresetError::ResourceLimit { .. })
        | StoreError::SessionDomain(SessionStateError::ResourceLimit { .. }) => CliError::new(
            ErrorCode::ResourceLimit,
            "a local TermiRust resource limit was reached",
            "Reduce retained records or use a narrower command.",
        ),
        StoreError::SessionDomain(SessionStateError::StaleRevision { actual, .. }) => {
            CliError::new(
                ErrorCode::Conflict,
                "session metadata changed before the command committed",
                "Reload the session and retry with the current revision.",
            )
            .with_revision(actual)
        }
        StoreError::GroupDomain(termirust_domain::GroupError::StaleRevision { actual, .. }) => {
            CliError::new(
                ErrorCode::Conflict,
                "group metadata changed before the command committed",
                "Reload the group and retry with the current revision.",
            )
            .with_revision(actual)
        }
        StoreError::Corrupt { .. } | StoreError::UnsafeEntry { .. } => CliError::new(
            ErrorCode::Unavailable,
            "local TermiRust metadata is unsafe or corrupt",
            "Open Storage and recovery in the desktop application.",
        ),
        StoreError::SessionDomain(SessionStateError::StopRequiredBeforeArchive) => validation(
            "only an exited session can be archived",
            "Run session stop <id> --yes first, wait for Exited, then archive.",
        ),
        StoreError::SessionDomain(SessionStateError::RemoveRequiresExitedArchive) => validation(
            "only an exited archived session can be removed",
            "Stop and archive the Session before requesting removal.",
        ),
        StoreError::SessionDomain(SessionStateError::Store {
            code: "removal-plan-changed" | "quarantine-conflict",
        }) => CliError::new(
            ErrorCode::Conflict,
            "session removal state changed before commit",
            "Refresh the fleet and review a new removal preview.",
        ),
        StoreError::SessionDomain(SessionStateError::Unavailable)
        | StoreError::PresetDomain(termirust_domain::PresetError::Unavailable)
        | StoreError::Domain(termirust_domain::ProjectError::Unavailable) => {
            unavailable("requested local record is unavailable")
        }
        _ => operation("local metadata operation failed"),
    }
}

fn map_client(error: ClientError) -> CliError {
    match error.code {
        ClientErrorCode::ProtocolIncompatible => CliError::new(
            ErrorCode::Incompatible,
            "local session Host protocol is incompatible",
            "Upgrade TermiRust and inspect the session again.",
        ),
        ClientErrorCode::PermissionDenied | ClientErrorCode::InvalidIdentity => CliError::new(
            ErrorCode::PermissionDenied,
            "local session Host rejected this user or identity",
            "Run the CLI as the same local user that owns the session.",
        ),
        ClientErrorCode::ConflictingDuplicate => CliError::new(
            ErrorCode::Conflict,
            "local Host command conflicts with an earlier command",
            "Inspect current session state before issuing another mutation.",
        ),
        ClientErrorCode::ResourceLimit | ClientErrorCode::FrameTooLarge => CliError::new(
            ErrorCode::ResourceLimit,
            "local session Host resource limit was reached",
            "Wait for current work to finish, then retry once.",
        ),
        ClientErrorCode::Cancelled => cancelled(),
        _ => operation("local session Host operation failed"),
    }
}

fn map_process(error: std::io::Error) -> CliError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        CliError::new(
            ErrorCode::PermissionDenied,
            "permission to start the local session Host was denied",
            "Check the TermiRust installation permissions.",
        )
    } else {
        unavailable("TermiRust session Host companion is unavailable")
    }
}

fn validation(message: &str, hint: &str) -> CliError {
    CliError::new(ErrorCode::Validation, message, hint)
}

fn unavailable(message: &str) -> CliError {
    CliError::new(
        ErrorCode::Unavailable,
        message,
        "Open TermiRust desktop and inspect local status, then retry.",
    )
}

fn operation(message: &str) -> CliError {
    CliError::new(
        ErrorCode::OperationFailed,
        message,
        "Inspect current session status before retrying.",
    )
}

fn temporarily_inconsistent() -> CliError {
    CliError::new(
        ErrorCode::Conflict,
        "local metadata changed throughout two snapshot attempts",
        "Wait for the current metadata operation to finish, then retry once.",
    )
}

fn cancelled() -> CliError {
    CliError::new(
        ErrorCode::Cancelled,
        "operation was cancelled",
        "Inspect current state before running another mutation.",
    )
}

fn explicit_path_snapshot() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default()
}

fn sibling_binary(current: &Path, name: &str) -> PathBuf {
    current
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

#[cfg(target_os = "macos")]
fn durable_runtime_parent(_: &Path) -> PathBuf {
    PathBuf::from(format!("/private/tmp/termirust-{}", unsafe {
        libc::geteuid()
    }))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn durable_runtime_parent(_: &Path) -> PathBuf {
    PathBuf::from(format!("/tmp/termirust-{}", unsafe { libc::geteuid() }))
}

#[cfg(not(unix))]
fn durable_runtime_parent(config_root: &Path) -> PathBuf {
    config_root.join("session-host-runtime")
}

#[cfg(unix)]
fn create_user_only_directory(path: &Path) -> Result<(), CliError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(CliError::new(
                ErrorCode::PermissionDenied,
                "durable session directory is not a trusted directory",
                "Inspect local data ownership in the desktop application.",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(map_directory_io)?;
        }
        Err(error) => return Err(map_directory_io(error)),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(map_directory_io)?;
    let metadata = fs::symlink_metadata(path).map_err(map_directory_io)?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(CliError::new(
            ErrorCode::PermissionDenied,
            "durable session directory has unsafe ownership or permissions",
            "Inspect local data ownership in the desktop application.",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_user_only_directory(path: &Path) -> Result<(), CliError> {
    fs::create_dir_all(path).map_err(map_directory_io)
}

fn map_directory_io(error: std::io::Error) -> CliError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        CliError::new(
            ErrorCode::PermissionDenied,
            "permission to prepare durable session storage was denied",
            "Check local TermiRust data ownership and permissions.",
        )
    } else {
        operation("unable to prepare durable session storage")
    }
}

fn terminate_host(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_faults_map_to_the_frozen_exit_classes_without_internal_context() {
        let permission = map_store(StoreError::Io {
            operation: "read secret path",
            kind: std::io::ErrorKind::PermissionDenied,
        });
        assert_eq!(permission.code, ErrorCode::PermissionDenied);
        assert!(!permission.message.contains("secret"));

        let resource = map_store(StoreError::TooLarge {
            name: "private.json",
            limit: 1,
        });
        assert_eq!(resource.code, ErrorCode::ResourceLimit);
        assert!(!resource.message.contains("private.json"));

        let timeout = map_store(StoreError::Io {
            operation: "lock project metadata",
            kind: std::io::ErrorKind::WouldBlock,
        });
        assert_eq!(timeout.code, ErrorCode::Timeout);
        assert!(!timeout.message.contains("project"));

        assert_eq!(
            bounded_records(MAX_RESPONSE_RECORDS + 1).unwrap_err().code,
            ErrorCode::ResourceLimit
        );
    }
}
