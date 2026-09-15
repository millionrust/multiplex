use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termirust_cli::{
    Cancellation, CliCommand, CliData, CliPaths, CommandService, ErrorCode, LocalCommandService,
    ManagementCommand as LocalCommand, ManagementRemovalManifest,
};
use termirust_domain::{CommandId, GroupId, ProjectId, Revision};

use crate::FleetSession;

pub const UNDO_WINDOW: Duration = Duration::from_secs(10);
pub const MAX_MANAGEMENT_TEXT_SCALARS: usize = 256;
pub const MAX_LAUNCH_CHOICES: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementIntent {
    Launch,
    Rename,
    TogglePin,
    MarkRead,
    Stop,
    Archive,
    Restore,
    Remove,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationKind {
    Pin,
    Unpin,
    MarkRead,
    Stop,
    StopAndArchive,
    Archive,
    Restore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchChoice {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub safe: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemovalPreview {
    pub expected_revision: u64,
    pub manifest: ManagementRemovalManifest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagementTarget {
    pub id: String,
    pub title: String,
    pub state: String,
    pub revision: u64,
    pub pinned: bool,
    pub unread: bool,
    pub archived: bool,
}

impl From<&FleetSession> for ManagementTarget {
    fn from(value: &FleetSession) -> Self {
        Self {
            id: value.id.clone(),
            title: value.title.clone(),
            state: value.state.clone(),
            revision: value.revision,
            pinned: value.pinned,
            unread: value.unread,
            archived: value.archived,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManagementDraft {
    LoadingLaunch {
        project_id: String,
        project_name: String,
        group_id: Option<String>,
    },
    LoadingRemoval {
        target: ManagementTarget,
    },
    Launch {
        project_id: String,
        project_name: String,
        group_id: Option<String>,
        choices: Vec<LaunchChoice>,
        selected: usize,
    },
    Rename {
        target: ManagementTarget,
        value: String,
    },
    Confirm {
        kind: ConfirmationKind,
        target: ManagementTarget,
    },
    Remove {
        target: ManagementTarget,
        preview: RemovalPreview,
        confirmation: String,
        confirmation_invalid: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandProgress {
    Idle,
    Reviewing,
    Running,
    Succeeded {
        summary: String,
        undo_deadline: Option<Instant>,
    },
    Failed(ManagementFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagementFailure {
    pub code: &'static str,
    pub summary: String,
    pub recovery: String,
    pub conflict_revision: Option<u64>,
}

impl ManagementFailure {
    fn validation(summary: impl Into<String>, recovery: impl Into<String>) -> Self {
        Self {
            code: "validation",
            summary: bounded_text(summary.into()),
            recovery: bounded_text(recovery.into()),
            conflict_revision: None,
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            code: "unavailable",
            summary: "Local Session management is unavailable.".into(),
            recovery: "Install the TermiRust CLI and session Host, then refresh.".into(),
            conflict_revision: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagementResult {
    pub outcome: String,
    pub session_id: String,
    pub title: String,
    pub state: String,
    pub revision: u64,
    pub unread: bool,
    pub archived: bool,
}

#[derive(Clone, Eq, PartialEq)]
pub enum ManagementCommand {
    Launch {
        command_id: CommandId,
        project_id: String,
        preset_id: String,
        group_id: Option<String>,
    },
    Rename {
        command_id: CommandId,
        session_id: String,
        expected_revision: u64,
        title: String,
    },
    SetPinned {
        command_id: CommandId,
        session_id: String,
        expected_revision: u64,
        pinned: bool,
    },
    MarkRead {
        command_id: CommandId,
        session_id: String,
        expected_revision: u64,
    },
    Stop {
        command_id: CommandId,
        session_id: String,
        expected_revision: u64,
    },
    Archive {
        command_id: CommandId,
        session_id: String,
        expected_revision: u64,
    },
    Restore {
        command_id: CommandId,
        session_id: String,
        expected_revision: u64,
    },
    StopAndArchive {
        command_id: CommandId,
        session_id: String,
        expected_revision: u64,
    },
    Remove {
        command_id: CommandId,
        session_id: String,
        expected_revision: u64,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManagementEffect {
    None,
    Close,
    LoadLaunchChoices {
        project_id: String,
    },
    LoadRemovalPreview {
        session_id: String,
        expected_session_revision: u64,
    },
    Execute(ManagementCommand),
}

#[derive(Clone)]
struct PendingUndo {
    command: ManagementCommand,
    deadline: Instant,
}

pub struct ManagementModel {
    intent: Option<ManagementIntent>,
    draft: Option<ManagementDraft>,
    progress: CommandProgress,
    generation: u64,
    cancellation: Cancellation,
    pending_undo: Option<PendingUndo>,
    dispatched: Option<ManagementCommand>,
    executing_undo: bool,
}

impl Default for ManagementModel {
    fn default() -> Self {
        Self {
            intent: None,
            draft: None,
            progress: CommandProgress::Idle,
            generation: 0,
            cancellation: Cancellation::default(),
            pending_undo: None,
            dispatched: None,
            executing_undo: false,
        }
    }
}

impl ManagementModel {
    pub fn active(&self) -> bool {
        self.intent.is_some()
    }

    pub const fn intent(&self) -> Option<ManagementIntent> {
        self.intent
    }

    pub fn draft(&self) -> Option<&ManagementDraft> {
        self.draft.as_ref()
    }

    pub fn progress(&self) -> &CommandProgress {
        &self.progress
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    pub fn deadline(&self) -> Option<Instant> {
        match &self.progress {
            CommandProgress::Succeeded { undo_deadline, .. } => *undo_deadline,
            _ => None,
        }
    }

    pub fn cancellation_available(&self) -> bool {
        matches!(self.progress, CommandProgress::Running)
            && (self.intent == Some(ManagementIntent::Launch)
                || (self.intent == Some(ManagementIntent::Remove) && self.dispatched.is_none()))
    }

    pub fn begin_launch(
        &mut self,
        project_id: String,
        project_name: String,
        group_id: Option<String>,
    ) -> ManagementEffect {
        self.reset_for(ManagementIntent::Launch);
        self.draft = Some(ManagementDraft::LoadingLaunch {
            project_id: project_id.clone(),
            project_name,
            group_id,
        });
        self.progress = CommandProgress::Running;
        ManagementEffect::LoadLaunchChoices { project_id }
    }

    pub fn begin_session(
        &mut self,
        intent: ManagementIntent,
        session: &FleetSession,
    ) -> ManagementEffect {
        let target = ManagementTarget::from(session);
        let validation = match intent {
            ManagementIntent::Stop if !can_stop(&target.state) => Some((
                "This Session is not in a stoppable state.",
                "Refresh and choose a live or starting Session.",
            )),
            ManagementIntent::Archive
                if target.archived || (target.state != "exited" && !can_stop(&target.state)) =>
            {
                Some((
                    "This Session cannot be archived from its current state.",
                    "Refresh and choose an exited or safely stoppable Session.",
                ))
            }
            ManagementIntent::Restore if !target.archived => Some((
                "This Session is not archived.",
                "Choose an archived Session and try again.",
            )),
            ManagementIntent::Remove if !target.archived || target.state != "exited" => Some((
                "Only an exited archived Session can be removed.",
                "Stop and archive the Session, then refresh the fleet.",
            )),
            ManagementIntent::MarkRead if !target.unread => Some((
                "This Session has no unread activity.",
                "Choose a Session with unread activity.",
            )),
            ManagementIntent::Launch => Some((
                "Launch requires a Project selection.",
                "Select a Project and press n.",
            )),
            _ => None,
        };
        self.reset_for(intent);
        if let Some((summary, recovery)) = validation {
            self.progress =
                CommandProgress::Failed(ManagementFailure::validation(summary, recovery));
            return ManagementEffect::None;
        }
        let draft = match intent {
            ManagementIntent::Rename => ManagementDraft::Rename {
                value: target.title.clone(),
                target,
            },
            ManagementIntent::TogglePin => ManagementDraft::Confirm {
                kind: if target.pinned {
                    ConfirmationKind::Unpin
                } else {
                    ConfirmationKind::Pin
                },
                target,
            },
            ManagementIntent::MarkRead => ManagementDraft::Confirm {
                kind: ConfirmationKind::MarkRead,
                target,
            },
            ManagementIntent::Stop => ManagementDraft::Confirm {
                kind: ConfirmationKind::Stop,
                target,
            },
            ManagementIntent::Archive => ManagementDraft::Confirm {
                kind: if target.state == "exited" {
                    ConfirmationKind::Archive
                } else {
                    ConfirmationKind::StopAndArchive
                },
                target,
            },
            ManagementIntent::Restore => ManagementDraft::Confirm {
                kind: ConfirmationKind::Restore,
                target,
            },
            ManagementIntent::Remove => {
                let effect = ManagementEffect::LoadRemovalPreview {
                    session_id: target.id.clone(),
                    expected_session_revision: target.revision,
                };
                self.draft = Some(ManagementDraft::LoadingRemoval { target });
                self.progress = CommandProgress::Running;
                return effect;
            }
            ManagementIntent::Launch => {
                self.progress = CommandProgress::Failed(ManagementFailure::validation(
                    "Launch requires a Project selection.",
                    "Select a Project and press n.",
                ));
                return ManagementEffect::None;
            }
        };
        self.draft = Some(draft);
        self.progress = CommandProgress::Reviewing;
        ManagementEffect::None
    }

    pub fn removal_preview_loaded(
        &mut self,
        generation: u64,
        result: Result<RemovalPreview, ManagementFailure>,
    ) {
        if generation != self.generation || self.intent != Some(ManagementIntent::Remove) {
            return;
        }
        match result {
            Ok(preview) => {
                let Some(ManagementDraft::LoadingRemoval { target }) = self.draft.take() else {
                    return;
                };
                self.draft = Some(ManagementDraft::Remove {
                    target,
                    preview,
                    confirmation: String::new(),
                    confirmation_invalid: false,
                });
                self.progress = CommandProgress::Reviewing;
            }
            Err(error) => self.progress = CommandProgress::Failed(error),
        }
    }

    pub fn launch_choices_loaded(
        &mut self,
        generation: u64,
        result: Result<Vec<LaunchChoice>, ManagementFailure>,
    ) {
        if generation != self.generation || self.intent != Some(ManagementIntent::Launch) {
            return;
        }
        match result {
            Ok(choices) => {
                let Some(ManagementDraft::LoadingLaunch {
                    project_id,
                    project_name,
                    group_id,
                }) = self.draft.take()
                else {
                    return;
                };
                let choices = choices
                    .into_iter()
                    .filter(|choice| choice.enabled && choice.safe)
                    .take(MAX_LAUNCH_CHOICES)
                    .collect::<Vec<_>>();
                if choices.is_empty() {
                    self.progress = CommandProgress::Failed(ManagementFailure::validation(
                        "No enabled safe preset is available for this Project.",
                        "Create or enable a safe preset in the desktop application.",
                    ));
                    return;
                }
                self.draft = Some(ManagementDraft::Launch {
                    project_id,
                    project_name,
                    group_id,
                    choices,
                    selected: 0,
                });
                self.progress = CommandProgress::Reviewing;
            }
            Err(error) => self.progress = CommandProgress::Failed(error),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, now: Instant) -> ManagementEffect {
        if !self.active() {
            return ManagementEffect::None;
        }
        if key.code == KeyCode::Esc {
            return match self.progress {
                CommandProgress::Running if self.cancellation_available() => {
                    self.cancellation.cancel();
                    ManagementEffect::None
                }
                CommandProgress::Running => ManagementEffect::None,
                _ => ManagementEffect::Close,
            };
        }
        if let CommandProgress::Succeeded { undo_deadline, .. } = &self.progress {
            if key.code == KeyCode::Char('u')
                && undo_deadline.is_some_and(|deadline| deadline >= now)
                && let Some(undo) = self.pending_undo.take()
            {
                self.progress = CommandProgress::Running;
                self.dispatched = Some(undo.command.clone());
                self.executing_undo = true;
                return ManagementEffect::Execute(undo.command);
            }
            return ManagementEffect::None;
        }
        if !matches!(self.progress, CommandProgress::Reviewing) {
            return ManagementEffect::None;
        }
        match self.draft.as_mut() {
            Some(ManagementDraft::Rename { value, target }) => match key.code {
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    value.clear();
                    ManagementEffect::None
                }
                KeyCode::Backspace => {
                    value.pop();
                    ManagementEffect::None
                }
                KeyCode::Char(character)
                    if !character.is_control()
                        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                        && value.chars().count() < MAX_MANAGEMENT_TEXT_SCALARS =>
                {
                    value.push(character);
                    ManagementEffect::None
                }
                KeyCode::Enter => {
                    let command = ManagementCommand::Rename {
                        command_id: CommandId::new(),
                        session_id: target.id.clone(),
                        expected_revision: target.revision,
                        title: value.trim().to_string(),
                    };
                    self.dispatch(command)
                }
                _ => ManagementEffect::None,
            },
            Some(ManagementDraft::Launch {
                project_id,
                group_id,
                choices,
                selected,
                ..
            }) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = selected.saturating_sub(1);
                    ManagementEffect::None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = selected
                        .saturating_add(1)
                        .min(choices.len().saturating_sub(1));
                    ManagementEffect::None
                }
                KeyCode::Enter => {
                    let Some(choice) = choices.get(*selected) else {
                        return ManagementEffect::None;
                    };
                    let command = ManagementCommand::Launch {
                        command_id: CommandId::new(),
                        project_id: project_id.clone(),
                        preset_id: choice.id.clone(),
                        group_id: group_id.clone(),
                    };
                    self.dispatch(command)
                }
                _ => ManagementEffect::None,
            },
            Some(ManagementDraft::Confirm { kind, target }) if key.code == KeyCode::Enter => {
                let command = command_for_confirmation(*kind, target);
                self.dispatch(command)
            }
            Some(ManagementDraft::Remove {
                target,
                preview,
                confirmation,
                confirmation_invalid,
            }) => match key.code {
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    confirmation.clear();
                    *confirmation_invalid = false;
                    ManagementEffect::None
                }
                KeyCode::Backspace => {
                    confirmation.pop();
                    *confirmation_invalid = false;
                    ManagementEffect::None
                }
                KeyCode::Char(character)
                    if !character.is_control()
                        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                        && confirmation.chars().count() < MAX_MANAGEMENT_TEXT_SCALARS =>
                {
                    confirmation.push(character);
                    *confirmation_invalid = false;
                    ManagementEffect::None
                }
                KeyCode::Enter => {
                    let expected = if preview.manifest.requires_title_confirmation() {
                        target.title.as_str()
                    } else {
                        "REMOVE"
                    };
                    if confirmation != expected {
                        *confirmation_invalid = true;
                        return ManagementEffect::None;
                    }
                    let command = ManagementCommand::Remove {
                        command_id: CommandId::new(),
                        session_id: target.id.clone(),
                        expected_revision: preview.expected_revision,
                        expected_manifest: preview.manifest,
                        title_confirmation: Some(confirmation.clone()),
                    };
                    self.dispatch(command)
                }
                _ => ManagementEffect::None,
            },
            _ => ManagementEffect::None,
        }
    }

    pub fn append_paste(&mut self, text: &str) {
        if !matches!(self.progress, CommandProgress::Reviewing) {
            return;
        }
        let value = match self.draft.as_mut() {
            Some(ManagementDraft::Rename { value, .. }) => value,
            Some(ManagementDraft::Remove {
                confirmation,
                confirmation_invalid,
                ..
            }) => {
                *confirmation_invalid = false;
                confirmation
            }
            _ => return,
        };
        let remaining = MAX_MANAGEMENT_TEXT_SCALARS.saturating_sub(value.chars().count());
        value.extend(
            text.chars()
                .filter(|character| !character.is_control())
                .take(remaining),
        );
    }

    pub fn completed(
        &mut self,
        generation: u64,
        result: Result<ManagementResult, ManagementFailure>,
        now: Instant,
    ) {
        if generation != self.generation || !matches!(self.progress, CommandProgress::Running) {
            return;
        }
        match result {
            Ok(result) => {
                let undo = if self.executing_undo {
                    None
                } else {
                    self.dispatched
                        .as_ref()
                        .and_then(|command| inverse(command, self.draft.as_ref(), &result))
                };
                let undo_deadline = undo.as_ref().map(|_| now + UNDO_WINDOW);
                self.pending_undo = undo.map(|command| PendingUndo {
                    command,
                    deadline: now + UNDO_WINDOW,
                });
                self.progress = CommandProgress::Succeeded {
                    summary: success_summary(&result.outcome).to_string(),
                    undo_deadline,
                };
            }
            Err(error) => {
                self.pending_undo = None;
                self.progress = CommandProgress::Failed(error);
            }
        }
        self.executing_undo = false;
    }

    pub fn expire_undo(&mut self, now: Instant) {
        if self
            .pending_undo
            .as_ref()
            .is_some_and(|undo| undo.deadline < now)
        {
            self.pending_undo = None;
            if let CommandProgress::Succeeded { undo_deadline, .. } = &mut self.progress {
                *undo_deadline = None;
            }
        }
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    fn reset_for(&mut self, intent: ManagementIntent) {
        self.generation = self.generation.saturating_add(1);
        self.intent = Some(intent);
        self.draft = None;
        self.progress = CommandProgress::Idle;
        self.cancellation = Cancellation::default();
        self.pending_undo = None;
        self.dispatched = None;
        self.executing_undo = false;
    }

    fn dispatch(&mut self, command: ManagementCommand) -> ManagementEffect {
        self.progress = CommandProgress::Running;
        self.dispatched = Some(command.clone());
        ManagementEffect::Execute(command)
    }
}

pub trait ManagementExecutor: Send + Sync {
    fn launch_choices(
        &self,
        project_id: &str,
        cancellation: &Cancellation,
    ) -> Result<Vec<LaunchChoice>, ManagementFailure>;

    fn removal_preview(
        &self,
        session_id: &str,
        expected_session_revision: u64,
        cancellation: &Cancellation,
    ) -> Result<RemovalPreview, ManagementFailure>;

    fn execute(
        &self,
        command: ManagementCommand,
        cancellation: &Cancellation,
    ) -> Result<ManagementResult, ManagementFailure>;
}

#[derive(Clone)]
pub struct LocalManagementExecutor {
    paths: CliPaths,
}

impl LocalManagementExecutor {
    pub fn new(config_root: PathBuf) -> Result<Self, ManagementFailure> {
        let executable = std::env::current_exe().map_err(|_| {
            ManagementFailure::validation(
                "TermiRust installation path is unavailable.",
                "Reinstall TermiRust and retry.",
            )
        })?;
        let host_executable = std::env::var_os("TERMIRUST_SESSION_HOST_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| sibling_binary(&executable, "termirust-session-host"));
        Ok(Self {
            paths: CliPaths::new(config_root, host_executable),
        })
    }

    pub fn with_host_executable(config_root: PathBuf, host_executable: PathBuf) -> Self {
        Self {
            paths: CliPaths::new(config_root, host_executable),
        }
    }

    fn service(&self) -> LocalCommandService {
        LocalCommandService::open(self.paths.clone())
    }
}

impl ManagementExecutor for LocalManagementExecutor {
    fn launch_choices(
        &self,
        project_id: &str,
        cancellation: &Cancellation,
    ) -> Result<Vec<LaunchChoice>, ManagementFailure> {
        let project_id = parse_id::<ProjectId>(project_id, "Project")?;
        let mut service = self.service();
        let data = service
            .execute(CliCommand::PresetList { project_id }, cancellation)
            .map_err(map_cli_error)?;
        let CliData::Presets(data) = data else {
            return Err(ManagementFailure::validation(
                "Preset response was inconsistent.",
                "Refresh the fleet and retry.",
            ));
        };
        Ok(data
            .presets
            .into_iter()
            .take(MAX_LAUNCH_CHOICES)
            .map(|preset| LaunchChoice {
                id: preset.id,
                label: bounded_text(preset.label),
                enabled: preset.enabled,
                safe: preset.risk == "safe",
            })
            .collect())
    }

    fn removal_preview(
        &self,
        session_id: &str,
        expected_session_revision: u64,
        cancellation: &Cancellation,
    ) -> Result<RemovalPreview, ManagementFailure> {
        let session_id = parse_id(session_id, "Session")?;
        let preview = self
            .service()
            .prepare_management_removal(
                session_id,
                Revision::new(expected_session_revision),
                cancellation,
            )
            .map_err(map_cli_error)?;
        Ok(RemovalPreview {
            expected_revision: preview.expected_revision.get(),
            manifest: preview.manifest,
        })
    }

    fn execute(
        &self,
        command: ManagementCommand,
        cancellation: &Cancellation,
    ) -> Result<ManagementResult, ManagementFailure> {
        let data = self
            .service()
            .execute_management(map_command(command)?, cancellation)
            .map_err(map_cli_error)?;
        let CliData::Mutation(data) = data else {
            return Err(ManagementFailure::validation(
                "Management response was inconsistent.",
                "Refresh the fleet before taking another action.",
            ));
        };
        Ok(ManagementResult {
            outcome: data.outcome,
            session_id: data.session.id,
            title: bounded_text(data.session.title),
            state: data.session.state,
            revision: data.session.revision,
            unread: data.session.unread,
            archived: data.session.archived,
        })
    }
}

fn map_command(command: ManagementCommand) -> Result<LocalCommand, ManagementFailure> {
    Ok(match command {
        ManagementCommand::Launch {
            command_id,
            project_id,
            preset_id,
            group_id,
        } => LocalCommand::Launch {
            command_id,
            project_id: parse_id(&project_id, "Project")?,
            preset_id: parse_id(&preset_id, "preset")?,
            group_id: group_id
                .as_deref()
                .map(|id| parse_id::<GroupId>(id, "group"))
                .transpose()?,
        },
        ManagementCommand::Rename {
            command_id,
            session_id,
            expected_revision,
            title,
        } => LocalCommand::Rename {
            command_id,
            session_id: parse_id(&session_id, "Session")?,
            expected_revision: Revision::new(expected_revision),
            title,
        },
        ManagementCommand::SetPinned {
            command_id,
            session_id,
            expected_revision,
            pinned,
        } => LocalCommand::SetPinned {
            command_id,
            session_id: parse_id(&session_id, "Session")?,
            expected_revision: Revision::new(expected_revision),
            pinned,
        },
        ManagementCommand::MarkRead {
            command_id,
            session_id,
            expected_revision,
        } => LocalCommand::MarkRead {
            command_id,
            session_id: parse_id(&session_id, "Session")?,
            expected_revision: Revision::new(expected_revision),
        },
        ManagementCommand::Stop {
            command_id,
            session_id,
            expected_revision,
        } => LocalCommand::Stop {
            command_id,
            session_id: parse_id(&session_id, "Session")?,
            expected_revision: Revision::new(expected_revision),
        },
        ManagementCommand::Archive {
            command_id,
            session_id,
            expected_revision,
        } => LocalCommand::Archive {
            command_id,
            session_id: parse_id(&session_id, "Session")?,
            expected_revision: Revision::new(expected_revision),
        },
        ManagementCommand::Restore {
            command_id,
            session_id,
            expected_revision,
        } => LocalCommand::Restore {
            command_id,
            session_id: parse_id(&session_id, "Session")?,
            expected_revision: Revision::new(expected_revision),
        },
        ManagementCommand::StopAndArchive {
            command_id,
            session_id,
            expected_revision,
        } => LocalCommand::StopAndArchive {
            command_id,
            session_id: parse_id(&session_id, "Session")?,
            expected_revision: Revision::new(expected_revision),
        },
        ManagementCommand::Remove {
            command_id,
            session_id,
            expected_revision,
            expected_manifest,
            title_confirmation,
        } => LocalCommand::Remove {
            command_id,
            session_id: parse_id(&session_id, "Session")?,
            expected_revision: Revision::new(expected_revision),
            expected_manifest,
            title_confirmation,
        },
    })
}

fn parse_id<T>(value: &str, name: &str) -> Result<T, ManagementFailure>
where
    T: std::str::FromStr,
{
    value.parse().map_err(|_| {
        ManagementFailure::validation(
            format!("{name} identity is invalid."),
            "Refresh the fleet before taking another action.",
        )
    })
}

fn map_cli_error(error: termirust_cli::CliError) -> ManagementFailure {
    ManagementFailure {
        code: match error.code {
            ErrorCode::Conflict => "conflict",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::PermissionDenied => "permission-denied",
            ErrorCode::ResourceLimit => "resource-limit",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::Incompatible => "incompatible",
            ErrorCode::Validation => "validation",
            _ => "operation-failed",
        },
        summary: bounded_text(error.message),
        recovery: bounded_text(error.hint),
        conflict_revision: error.current_revision,
    }
}

fn command_for_confirmation(
    kind: ConfirmationKind,
    target: &ManagementTarget,
) -> ManagementCommand {
    match kind {
        ConfirmationKind::Pin => ManagementCommand::SetPinned {
            command_id: CommandId::new(),
            session_id: target.id.clone(),
            expected_revision: target.revision,
            pinned: true,
        },
        ConfirmationKind::Unpin => ManagementCommand::SetPinned {
            command_id: CommandId::new(),
            session_id: target.id.clone(),
            expected_revision: target.revision,
            pinned: false,
        },
        ConfirmationKind::MarkRead => ManagementCommand::MarkRead {
            command_id: CommandId::new(),
            session_id: target.id.clone(),
            expected_revision: target.revision,
        },
        ConfirmationKind::Stop => ManagementCommand::Stop {
            command_id: CommandId::new(),
            session_id: target.id.clone(),
            expected_revision: target.revision,
        },
        ConfirmationKind::Archive => ManagementCommand::Archive {
            command_id: CommandId::new(),
            session_id: target.id.clone(),
            expected_revision: target.revision,
        },
        ConfirmationKind::Restore => ManagementCommand::Restore {
            command_id: CommandId::new(),
            session_id: target.id.clone(),
            expected_revision: target.revision,
        },
        ConfirmationKind::StopAndArchive => ManagementCommand::StopAndArchive {
            command_id: CommandId::new(),
            session_id: target.id.clone(),
            expected_revision: target.revision,
        },
    }
}

fn inverse(
    command: &ManagementCommand,
    draft: Option<&ManagementDraft>,
    result: &ManagementResult,
) -> Option<ManagementCommand> {
    match command {
        ManagementCommand::Rename { session_id, .. } => {
            let ManagementDraft::Rename { target, .. } = draft? else {
                return None;
            };
            Some(ManagementCommand::Rename {
                command_id: CommandId::new(),
                session_id: session_id.clone(),
                expected_revision: result.revision,
                title: target.title.clone(),
            })
        }
        ManagementCommand::SetPinned {
            session_id, pinned, ..
        } => Some(ManagementCommand::SetPinned {
            command_id: CommandId::new(),
            session_id: session_id.clone(),
            expected_revision: result.revision,
            pinned: !pinned,
        }),
        ManagementCommand::Archive { session_id, .. } => Some(ManagementCommand::Restore {
            command_id: CommandId::new(),
            session_id: session_id.clone(),
            expected_revision: result.revision,
        }),
        ManagementCommand::Restore { session_id, .. } => Some(ManagementCommand::Archive {
            command_id: CommandId::new(),
            session_id: session_id.clone(),
            expected_revision: result.revision,
        }),
        _ => None,
    }
}

fn can_stop(state: &str) -> bool {
    matches!(
        state,
        "provisioning"
            | "attaching"
            | "replaying"
            | "live"
            | "recording_paused"
            | "running_app_attached"
    )
}

fn success_summary(outcome: &str) -> &'static str {
    match outcome {
        "launched" => "Session launched.",
        "renamed" => "Session renamed.",
        "pinned" => "Session pinned.",
        "unpinned" => "Session unpinned.",
        "marked_read" => "Session marked read.",
        "stopped" => "Session stopped after confirmed Host exit.",
        "stopped_and_archived" => "Session stopped, confirmed exited, and archived.",
        "archived" => "Exited Session archived.",
        "restored" => "Session restored as metadata only.",
        "removed" => "Session metadata removed and owned data quarantined.",
        _ => "Session command completed.",
    }
}

fn bounded_text(value: String) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_MANAGEMENT_TEXT_SCALARS)
        .collect()
}

fn sibling_binary(current: &Path, name: &str) -> PathBuf {
    let file_name = if current.extension().and_then(|value| value.to_str()) == Some("exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    current
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file_name)
}

impl fmt::Debug for ManagementModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagementModel")
            .field("intent", &self.intent)
            .field("progress", &self.progress)
            .field("generation", &self.generation)
            .field("draft", &self.draft.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;
    use termirust_domain::HostedSessionId;

    use super::*;

    fn session() -> FleetSession {
        FleetSession {
            id: HostedSessionId::new().to_string(),
            project_id: ProjectId::new().to_string(),
            group_id: None,
            title: "Build".into(),
            state: "live".into(),
            activity: "working".into(),
            unread: true,
            pinned: false,
            archived: false,
            revision: 4,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn management_requires_review_and_uses_captured_revision() {
        let mut model = ManagementModel::default();
        let session = session();
        model.begin_session(ManagementIntent::Stop, &session);
        assert_eq!(model.progress(), &CommandProgress::Reviewing);
        assert!(matches!(
            model.handle_key(key(KeyCode::Enter), Instant::now()),
            ManagementEffect::Execute(ManagementCommand::Stop {
                session_id,
                expected_revision: 4,
                ..
            }) if session_id == session.id
        ));
        assert_eq!(model.progress(), &CommandProgress::Running);
        assert_eq!(
            model.handle_key(key(KeyCode::Esc), Instant::now()),
            ManagementEffect::None
        );
    }

    #[test]
    fn rename_is_bounded_and_undo_is_revision_scoped() {
        let mut model = ManagementModel::default();
        let original = session();
        model.begin_session(ManagementIntent::Rename, &original);
        for _ in 0..300 {
            model.handle_key(key(KeyCode::Char('x')), Instant::now());
        }
        let ManagementDraft::Rename { value, .. } = model.draft().unwrap() else {
            panic!("rename draft expected");
        };
        assert_eq!(value.chars().count(), MAX_MANAGEMENT_TEXT_SCALARS);
        assert!(matches!(
            model.handle_key(key(KeyCode::Enter), Instant::now()),
            ManagementEffect::Execute(ManagementCommand::Rename { .. })
        ));
        let now = Instant::now();
        model.completed(
            model.generation(),
            Ok(ManagementResult {
                outcome: "renamed".into(),
                session_id: original.id,
                title: "Changed".into(),
                state: "live".into(),
                revision: 5,
                unread: true,
                archived: false,
            }),
            now,
        );
        assert!(matches!(
            model.handle_key(key(KeyCode::Char('u')), now),
            ManagementEffect::Execute(ManagementCommand::Rename {
                expected_revision: 5,
                ..
            })
        ));
    }

    #[test]
    fn rename_rejects_control_shortcuts_and_bounds_paste() {
        let mut model = ManagementModel::default();
        let original = session();
        model.begin_session(ManagementIntent::Rename, &original);

        model.handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Instant::now(),
        );
        let ManagementDraft::Rename { value, .. } = model.draft().unwrap() else {
            panic!("rename draft expected");
        };
        assert_eq!(value, "Build");

        model.handle_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Instant::now(),
        );
        model.append_paste(&format!("renamed\n{}", "x".repeat(300)));
        let ManagementDraft::Rename { value, .. } = model.draft().unwrap() else {
            panic!("rename draft expected");
        };
        assert!(!value.contains('\n'));
        assert_eq!(value.chars().count(), MAX_MANAGEMENT_TEXT_SCALARS);
    }

    #[test]
    fn launch_filters_disabled_and_risky_presets() {
        let mut model = ManagementModel::default();
        assert_eq!(
            model.begin_launch("project".into(), "Project".into(), None),
            ManagementEffect::LoadLaunchChoices {
                project_id: "project".into()
            }
        );
        model.launch_choices_loaded(
            model.generation(),
            Ok(vec![
                LaunchChoice {
                    id: "disabled".into(),
                    label: "Disabled".into(),
                    enabled: false,
                    safe: true,
                },
                LaunchChoice {
                    id: "risky".into(),
                    label: "Risky".into(),
                    enabled: true,
                    safe: false,
                },
                LaunchChoice {
                    id: "safe".into(),
                    label: "Safe".into(),
                    enabled: true,
                    safe: true,
                },
            ]),
        );
        let ManagementDraft::Launch { choices, .. } = model.draft().unwrap() else {
            panic!("launch draft expected");
        };
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, "safe");
    }

    #[test]
    fn live_archive_requires_reviewed_stop_and_archive() {
        let mut model = ManagementModel::default();
        let session = session();
        model.begin_session(ManagementIntent::Archive, &session);
        assert!(matches!(
            model.draft(),
            Some(ManagementDraft::Confirm {
                kind: ConfirmationKind::StopAndArchive,
                ..
            })
        ));
        assert!(matches!(
            model.handle_key(key(KeyCode::Enter), Instant::now()),
            ManagementEffect::Execute(ManagementCommand::StopAndArchive {
                expected_revision: 4,
                ..
            })
        ));
    }

    #[test]
    fn launch_can_cancel_while_loading_but_stale_results_stay_discarded() {
        let mut model = ManagementModel::default();
        model.begin_launch("project".into(), "Project".into(), None);
        let generation = model.generation();
        assert_eq!(
            model.handle_key(key(KeyCode::Esc), Instant::now()),
            ManagementEffect::None
        );
        assert!(model.cancellation().is_cancelled());

        model.close();
        model.launch_choices_loaded(
            generation,
            Ok(vec![LaunchChoice {
                id: "stale".into(),
                label: "Stale".into(),
                enabled: true,
                safe: true,
            }]),
        );
        assert!(!model.active());
        assert!(model.draft().is_none());
    }

    #[test]
    fn management_command_debug_redacts_rename_text() {
        let command = ManagementCommand::Rename {
            command_id: CommandId::new(),
            session_id: session().id,
            expected_revision: 4,
            title: "PRIVATE-TITLE-CANARY".into(),
        };
        assert!(!format!("{command:?}").contains("PRIVATE-TITLE-CANARY"));
    }

    #[test]
    fn removal_requires_archived_exit_exact_preview_and_typed_title() {
        let mut archived = session();
        archived.state = "exited".into();
        archived.archived = true;
        let mut model = ManagementModel::default();
        assert_eq!(
            model.begin_session(ManagementIntent::Remove, &archived),
            ManagementEffect::LoadRemovalPreview {
                session_id: archived.id.clone(),
                expected_session_revision: 4,
            }
        );
        assert!(model.cancellation_available());
        let generation = model.generation();
        let manifest = ManagementRemovalManifest {
            metadata_bytes: 10,
            journal_bytes: 20,
            transcript_bytes: 30,
            artifact_bytes: 40,
            file_count: 4,
        };
        model.removal_preview_loaded(
            generation,
            Ok(RemovalPreview {
                expected_revision: 9,
                manifest,
            }),
        );
        assert_eq!(model.progress(), &CommandProgress::Reviewing);

        model.append_paste("wrong\n");
        assert_eq!(
            model.handle_key(key(KeyCode::Enter), Instant::now()),
            ManagementEffect::None
        );
        let Some(ManagementDraft::Remove {
            confirmation_invalid,
            ..
        }) = model.draft()
        else {
            panic!("removal draft expected");
        };
        assert!(*confirmation_invalid);

        model.handle_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Instant::now(),
        );
        model.append_paste(&archived.title);
        assert!(matches!(
            model.handle_key(key(KeyCode::Enter), Instant::now()),
            ManagementEffect::Execute(ManagementCommand::Remove {
                expected_revision: 9,
                expected_manifest,
                title_confirmation: Some(ref title),
                ..
            }) if expected_manifest == manifest && title == &archived.title
        ));
        assert!(!model.cancellation_available());
        assert_eq!(
            model.handle_key(key(KeyCode::Esc), Instant::now()),
            ManagementEffect::None
        );
    }

    #[test]
    fn removal_without_content_requires_remove_token_and_bounds_input() {
        let mut archived = session();
        archived.state = "exited".into();
        archived.archived = true;
        let mut model = ManagementModel::default();
        model.begin_session(ManagementIntent::Remove, &archived);
        model.removal_preview_loaded(
            model.generation(),
            Ok(RemovalPreview {
                expected_revision: 6,
                manifest: ManagementRemovalManifest::default(),
            }),
        );
        model.append_paste(&format!("REMOVE\n{}", "x".repeat(300)));
        let Some(ManagementDraft::Remove { confirmation, .. }) = model.draft() else {
            panic!("removal draft expected");
        };
        assert!(!confirmation.contains('\n'));
        assert_eq!(confirmation.chars().count(), MAX_MANAGEMENT_TEXT_SCALARS);
        model.handle_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Instant::now(),
        );
        model.append_paste("REMOVE");
        assert!(matches!(
            model.handle_key(key(KeyCode::Enter), Instant::now()),
            ManagementEffect::Execute(ManagementCommand::Remove {
                title_confirmation: Some(ref confirmation),
                ..
            }) if confirmation == "REMOVE"
        ));
    }

    #[test]
    fn removal_rejects_live_session_and_discards_stale_preview() {
        let live = session();
        let mut model = ManagementModel::default();
        assert_eq!(
            model.begin_session(ManagementIntent::Remove, &live),
            ManagementEffect::None
        );
        assert!(matches!(model.progress(), CommandProgress::Failed(_)));

        let mut archived = live;
        archived.state = "exited".into();
        archived.archived = true;
        model.begin_session(ManagementIntent::Remove, &archived);
        let generation = model.generation();
        assert_eq!(
            model.handle_key(key(KeyCode::Esc), Instant::now()),
            ManagementEffect::None
        );
        assert!(model.cancellation().is_cancelled());
        model.close();
        model.removal_preview_loaded(
            generation,
            Ok(RemovalPreview {
                expected_revision: 5,
                manifest: ManagementRemovalManifest::default(),
            }),
        );
        assert!(!model.active());
    }

    #[test]
    fn removal_command_debug_redacts_confirmation() {
        let command = ManagementCommand::Remove {
            command_id: CommandId::new(),
            session_id: session().id,
            expected_revision: 4,
            expected_manifest: ManagementRemovalManifest::default(),
            title_confirmation: Some("PRIVATE-REMOVAL-CANARY".into()),
        };
        assert!(!format!("{command:?}").contains("PRIVATE-REMOVAL-CANARY"));
    }
}
