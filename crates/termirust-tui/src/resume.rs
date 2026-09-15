use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termirust_cli::{
    Cancellation, CliCommand, CliData, CliPaths, CommandService, ErrorCode, LocalCommandService,
};
use termirust_domain::{HostedSessionId, Revision};

use crate::FleetSession;

const MAX_RESUME_TEXT_SCALARS: usize = 256;
const EXACT_PROVIDER: &str = "codex";
const EXACT_PROVIDER_VERSION: &str = "0.150.1";
const SAFE_PERMISSION_POLICY: &str = "read_only";

#[derive(Clone, Eq, PartialEq)]
pub struct ResumeReview {
    pub source_session_id: String,
    pub source_title: String,
    pub source_revision: u64,
    pub provider: String,
    pub provider_version: String,
    pub permission_policy: String,
    pub replacement_generation: u64,
}

impl fmt::Debug for ResumeReview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResumeReview")
            .field("source_revision", &self.source_revision)
            .field("provider", &self.provider)
            .field("provider_version", &self.provider_version)
            .field("permission_policy", &self.permission_policy)
            .field("replacement_generation", &self.replacement_generation)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ResumeResult {
    pub source_session_id: String,
    pub successor_session_id: String,
    pub source_revision: u64,
    pub successor_revision: u64,
    pub provider: String,
    pub provider_version: String,
    pub permission_policy: String,
    pub replacement_generation: u64,
    pub lifecycle: String,
    pub continuity_committed: bool,
}

impl fmt::Debug for ResumeResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResumeResult")
            .field("source_revision", &self.source_revision)
            .field("successor_revision", &self.successor_revision)
            .field("provider", &self.provider)
            .field("provider_version", &self.provider_version)
            .field("permission_policy", &self.permission_policy)
            .field("replacement_generation", &self.replacement_generation)
            .field("lifecycle", &self.lifecycle)
            .field("continuity_committed", &self.continuity_committed)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResumeFailure {
    pub code: &'static str,
    pub summary: String,
    pub recovery: String,
    pub conflict_revision: Option<u64>,
}

impl ResumeFailure {
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
            summary: "Exact Session resume is unavailable.".into(),
            recovery: "Check the local Session Host installation, then refresh the fleet.".into(),
            conflict_revision: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumeProgress {
    Idle,
    LoadingReview,
    Reviewing,
    Resuming,
    Succeeded,
    Failed(ResumeFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResumeEffect {
    None,
    Close,
    Quit,
    Preview {
        session_id: String,
    },
    Commit {
        session_id: String,
        expected_revision: u64,
    },
}

#[derive(Clone, Eq, PartialEq)]
struct ResumeTarget {
    id: String,
}

pub struct ResumeModel {
    active: bool,
    generation: u64,
    target: Option<ResumeTarget>,
    review: Option<ResumeReview>,
    result: Option<ResumeResult>,
    progress: ResumeProgress,
    cancellation: Cancellation,
}

impl Default for ResumeModel {
    fn default() -> Self {
        Self {
            active: false,
            generation: 0,
            target: None,
            review: None,
            result: None,
            progress: ResumeProgress::Idle,
            cancellation: Cancellation::default(),
        }
    }
}

impl ResumeModel {
    pub fn active(&self) -> bool {
        self.active
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn review(&self) -> Option<&ResumeReview> {
        self.review.as_ref()
    }

    pub fn result(&self) -> Option<&ResumeResult> {
        self.result.as_ref()
    }

    pub fn progress(&self) -> &ResumeProgress {
        &self.progress
    }

    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    pub fn irreversible_active(&self) -> bool {
        matches!(self.progress, ResumeProgress::Resuming)
    }

    pub fn open(&mut self, session: &FleetSession) -> ResumeEffect {
        self.close_internal();
        self.active = true;
        self.target = Some(ResumeTarget {
            id: session.id.clone(),
        });
        if session.archived {
            self.progress = ResumeProgress::Failed(ResumeFailure::validation(
                "Archived Sessions cannot be resumed.",
                "Restore the source Session, refresh the fleet, then review resume.",
            ));
            return ResumeEffect::None;
        }
        if session.state != "exited" {
            self.progress = ResumeProgress::Failed(ResumeFailure::validation(
                "Only an exited exact Codex Session can be resumed.",
                "Wait for the source Session to exit, then refresh the fleet.",
            ));
            return ResumeEffect::None;
        }
        let session_id = session.id.clone();
        self.begin_operation(ResumeProgress::LoadingReview);
        ResumeEffect::Preview { session_id }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ResumeEffect {
        if !self.active {
            return ResumeEffect::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return if self.irreversible_active() {
                ResumeEffect::None
            } else {
                self.close_internal();
                ResumeEffect::Quit
            };
        }
        if !key.modifiers.is_empty() {
            return ResumeEffect::None;
        }
        match key.code {
            KeyCode::Esc if matches!(self.progress, ResumeProgress::LoadingReview) => {
                self.close_internal();
                ResumeEffect::Close
            }
            KeyCode::Esc if !self.irreversible_active() => {
                self.close_internal();
                ResumeEffect::Close
            }
            KeyCode::Char('q') if !self.irreversible_active() => {
                self.close_internal();
                ResumeEffect::Quit
            }
            KeyCode::Enter if matches!(self.progress, ResumeProgress::Reviewing) => {
                let Some(review) = self.review.as_ref() else {
                    self.progress = ResumeProgress::Failed(ResumeFailure::unavailable());
                    return ResumeEffect::None;
                };
                let session_id = review.source_session_id.clone();
                let expected_revision = review.source_revision;
                self.begin_operation(ResumeProgress::Resuming);
                ResumeEffect::Commit {
                    session_id,
                    expected_revision,
                }
            }
            _ => ResumeEffect::None,
        }
    }

    pub fn reviewed(&mut self, generation: u64, result: Result<ResumeReview, ResumeFailure>) {
        if !self.accepts(generation, ResumeProgress::LoadingReview) {
            return;
        }
        match result {
            Ok(review)
                if self
                    .target
                    .as_ref()
                    .is_some_and(|target| target.id == review.source_session_id)
                    && valid_review(&review) =>
            {
                self.review = Some(review);
                self.progress = ResumeProgress::Reviewing;
            }
            Ok(_) => {
                self.review = None;
                self.progress = ResumeProgress::Failed(ResumeFailure::validation(
                    "The resume review did not match the selected exact Session.",
                    "Refresh the fleet before taking another action.",
                ));
            }
            Err(error) => {
                self.review = None;
                self.progress = ResumeProgress::Failed(error);
            }
        }
    }

    pub fn completed(&mut self, generation: u64, result: Result<ResumeResult, ResumeFailure>) {
        if !self.accepts(generation, ResumeProgress::Resuming) {
            return;
        }
        match result {
            Ok(result)
                if self.review.as_ref().is_some_and(|review| {
                    result.source_session_id == review.source_session_id
                        && result.source_revision == review.source_revision
                        && result.provider == review.provider
                        && result.provider_version == review.provider_version
                        && result.permission_policy == review.permission_policy
                        && result.replacement_generation == review.replacement_generation
                }) && valid_result(&result) =>
            {
                self.result = Some(result);
                self.progress = ResumeProgress::Succeeded;
            }
            Ok(_) => {
                self.result = None;
                self.progress = ResumeProgress::Failed(ResumeFailure::validation(
                    "The resume result did not match the exact reviewed Session.",
                    "Refresh the fleet and inspect authoritative Session state.",
                ));
            }
            Err(error) => {
                self.result = None;
                self.progress = ResumeProgress::Failed(error);
            }
        }
    }

    pub fn close(&mut self) {
        if !self.irreversible_active() {
            self.close_internal();
        }
    }

    fn accepts(&self, generation: u64, expected: ResumeProgress) -> bool {
        self.active && generation == self.generation && self.progress == expected
    }

    fn begin_operation(&mut self, progress: ResumeProgress) {
        self.cancellation.cancel();
        self.cancellation = Cancellation::default();
        self.generation = self.generation.saturating_add(1);
        self.progress = progress;
    }

    fn close_internal(&mut self) {
        self.cancellation.cancel();
        self.generation = self.generation.saturating_add(1);
        self.active = false;
        self.target = None;
        self.review = None;
        self.result = None;
        self.progress = ResumeProgress::Idle;
    }
}

impl fmt::Debug for ResumeModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResumeModel")
            .field("active", &self.active)
            .field("generation", &self.generation)
            .field("progress", &self.progress)
            .field("has_target", &self.target.is_some())
            .field("has_review", &self.review.is_some())
            .field("has_result", &self.result.is_some())
            .finish_non_exhaustive()
    }
}

pub trait ResumeExecutor: Send + Sync {
    fn preview(
        &self,
        session_id: &str,
        cancellation: &Cancellation,
    ) -> Result<ResumeReview, ResumeFailure>;

    fn commit(
        &self,
        session_id: &str,
        expected_revision: u64,
        cancellation: &Cancellation,
    ) -> Result<ResumeResult, ResumeFailure>;
}

pub struct LocalResumeExecutor {
    service: Mutex<LocalCommandService>,
}

impl LocalResumeExecutor {
    pub fn new(config_root: PathBuf) -> Result<Self, ResumeFailure> {
        let executable = std::env::current_exe().map_err(|_| ResumeFailure::unavailable())?;
        let host_executable = std::env::var_os("TERMIRUST_SESSION_HOST_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| sibling_binary(&executable, "termirust-session-host"));
        Ok(Self::with_service(LocalCommandService::open(
            CliPaths::new(config_root, host_executable),
        )))
    }

    pub fn with_service(service: LocalCommandService) -> Self {
        Self {
            service: Mutex::new(service),
        }
    }

    fn service(&self) -> Result<std::sync::MutexGuard<'_, LocalCommandService>, ResumeFailure> {
        self.service
            .lock()
            .map_err(|_| ResumeFailure::unavailable())
    }
}

impl fmt::Debug for LocalResumeExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalResumeExecutor")
            .finish_non_exhaustive()
    }
}

impl ResumeExecutor for LocalResumeExecutor {
    fn preview(
        &self,
        session_id: &str,
        cancellation: &Cancellation,
    ) -> Result<ResumeReview, ResumeFailure> {
        let session_id = parse_session_id(session_id)?;
        let mut service = self.service()?;
        let preview = service
            .execute(
                CliCommand::SessionResume {
                    session_id,
                    expected_revision: None,
                    confirmed: false,
                },
                cancellation,
            )
            .map_err(map_cli_error)?;
        let CliData::ResumePreview(preview) = preview else {
            return Err(ResumeFailure::unavailable());
        };
        if cancellation.is_cancelled() {
            return Err(cancelled_failure());
        }
        let source = service
            .execute(CliCommand::SessionShow { session_id }, cancellation)
            .map_err(map_cli_error)?;
        let CliData::Session(source) = source else {
            return Err(ResumeFailure::unavailable());
        };
        let review = ResumeReview {
            source_session_id: bounded_text(preview.source_session_id),
            source_title: bounded_text(source.session.title),
            source_revision: preview.source_revision,
            provider: bounded_text(preview.provider),
            provider_version: bounded_text(preview.provider_version),
            permission_policy: preview.permission_policy,
            replacement_generation: preview.replacement_generation,
        };
        if !preview.confirmation_required
            || source.session.id != review.source_session_id
            || source.session.revision != review.source_revision
            || source.session.state != "exited"
            || source.session.archived
            || !valid_review(&review)
        {
            return Err(ResumeFailure::validation(
                "The authoritative resume review was inconsistent.",
                "Refresh the fleet before taking another action.",
            ));
        }
        Ok(review)
    }

    fn commit(
        &self,
        session_id: &str,
        expected_revision: u64,
        cancellation: &Cancellation,
    ) -> Result<ResumeResult, ResumeFailure> {
        let source_session_id = parse_session_id(session_id)?;
        let data = self
            .service()?
            .execute(
                CliCommand::SessionResume {
                    session_id: source_session_id,
                    expected_revision: Some(Revision::new(expected_revision)),
                    confirmed: true,
                },
                cancellation,
            )
            .map_err(map_cli_error)?;
        let CliData::Resume(data) = data else {
            return Err(ResumeFailure::unavailable());
        };
        let result = ResumeResult {
            source_session_id: bounded_text(data.source_session_id),
            successor_session_id: bounded_text(data.successor_session_id),
            source_revision: data.source_revision,
            successor_revision: data.successor_revision,
            provider: bounded_text(data.provider),
            provider_version: bounded_text(data.provider_version),
            permission_policy: data.permission_policy,
            replacement_generation: data.replacement_generation,
            lifecycle: data.lifecycle,
            continuity_committed: data.continuity_committed,
        };
        if result.source_session_id != session_id
            || result.source_revision != expected_revision
            || !valid_result(&result)
        {
            return Err(ResumeFailure::validation(
                "The authoritative resume result was inconsistent.",
                "Refresh the fleet and inspect authoritative Session state.",
            ));
        }
        Ok(result)
    }
}

fn valid_review(review: &ResumeReview) -> bool {
    review.provider == EXACT_PROVIDER
        && review.provider_version == EXACT_PROVIDER_VERSION
        && review.permission_policy == SAFE_PERMISSION_POLICY
        && review.replacement_generation > 0
}

fn valid_result(result: &ResumeResult) -> bool {
    result.source_session_id != result.successor_session_id
        && result.provider == EXACT_PROVIDER
        && result.provider_version == EXACT_PROVIDER_VERSION
        && result.permission_policy == SAFE_PERMISSION_POLICY
        && result.replacement_generation > 0
        && result.lifecycle == "live"
        && result.continuity_committed
}

fn parse_session_id(value: &str) -> Result<HostedSessionId, ResumeFailure> {
    value.parse().map_err(|_| {
        ResumeFailure::validation(
            "The selected Session identity is invalid.",
            "Refresh the fleet before taking another action.",
        )
    })
}

fn map_cli_error(error: termirust_cli::CliError) -> ResumeFailure {
    ResumeFailure {
        code: match error.code {
            ErrorCode::Conflict => "conflict",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::PermissionDenied => "permission-denied",
            ErrorCode::ResourceLimit => "resource-limit",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::Incompatible => "incompatible",
            ErrorCode::Validation => "validation",
            ErrorCode::InteractionRequired => "interaction-required",
            _ => "operation-failed",
        },
        summary: bounded_text(error.message),
        recovery: bounded_text(error.hint),
        conflict_revision: error.current_revision,
    }
}

fn cancelled_failure() -> ResumeFailure {
    ResumeFailure {
        code: "cancelled",
        summary: "Resume review was cancelled.".into(),
        recovery: "Select the Session and press c to review it again.".into(),
        conflict_revision: None,
    }
}

fn bounded_text(value: String) -> String {
    value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '\u{202a}'
                        | '\u{202b}'
                        | '\u{202c}'
                        | '\u{202d}'
                        | '\u{202e}'
                        | '\u{2066}'
                        | '\u{2067}'
                        | '\u{2068}'
                        | '\u{2069}'
                )
        })
        .take(MAX_RESUME_TEXT_SCALARS)
        .collect()
}

fn sibling_binary(current: &Path, name: &str) -> PathBuf {
    current
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn session(state: &str, archived: bool) -> FleetSession {
        FleetSession {
            id: "00000000-0000-0000-0000-000000000007".into(),
            project_id: "00000000-0000-0000-0000-000000000001".into(),
            group_id: None,
            title: "Private Session".into(),
            state: state.into(),
            activity: "idle".into(),
            unread: false,
            pinned: false,
            archived,
            revision: 5,
        }
    }

    fn review() -> ResumeReview {
        ResumeReview {
            source_session_id: session("exited", false).id,
            source_title: "Private Session".into(),
            source_revision: 7,
            provider: EXACT_PROVIDER.into(),
            provider_version: EXACT_PROVIDER_VERSION.into(),
            permission_policy: SAFE_PERMISSION_POLICY.into(),
            replacement_generation: 3,
        }
    }

    fn result() -> ResumeResult {
        ResumeResult {
            source_session_id: review().source_session_id,
            successor_session_id: "00000000-0000-0000-0000-000000000008".into(),
            source_revision: 7,
            successor_revision: 2,
            provider: EXACT_PROVIDER.into(),
            provider_version: EXACT_PROVIDER_VERSION.into(),
            permission_policy: SAFE_PERMISSION_POLICY.into(),
            replacement_generation: 3,
            lifecycle: "live".into(),
            continuity_committed: true,
        }
    }

    #[test]
    fn resume_requires_fresh_review_and_dispatches_exactly_once() {
        let mut model = ResumeModel::default();
        assert_eq!(
            model.open(&session("exited", false)),
            ResumeEffect::Preview {
                session_id: session("exited", false).id,
            }
        );
        let generation = model.generation();
        model.reviewed(generation, Ok(review()));
        assert_eq!(
            model.handle_key(key(KeyCode::Enter)),
            ResumeEffect::Commit {
                session_id: review().source_session_id,
                expected_revision: 7,
            }
        );
        assert_eq!(model.handle_key(key(KeyCode::Enter)), ResumeEffect::None);
        let generation = model.generation();
        model.completed(generation, Ok(result()));
        assert!(matches!(model.progress(), ResumeProgress::Succeeded));
        assert_eq!(
            model.result().unwrap().successor_session_id,
            result().successor_session_id
        );
    }

    #[test]
    fn resume_cancel_and_stale_completion_are_safe_defaults() {
        let mut model = ResumeModel::default();
        model.open(&session("exited", false));
        let stale = model.generation();
        assert_eq!(model.handle_key(key(KeyCode::Esc)), ResumeEffect::Close);
        model.reviewed(stale, Ok(review()));
        assert!(!model.active());
        assert!(model.review().is_none());

        model.open(&session("exited", false));
        let generation = model.generation();
        model.reviewed(generation, Ok(review()));
        assert_eq!(model.handle_key(key(KeyCode::Esc)), ResumeEffect::Close);
        assert!(!model.active());
    }

    #[test]
    fn resume_rejects_ineligible_and_inconsistent_contracts() {
        let mut model = ResumeModel::default();
        assert_eq!(model.open(&session("live", false)), ResumeEffect::None);
        assert!(matches!(model.progress(), ResumeProgress::Failed(_)));
        assert_eq!(model.open(&session("exited", true)), ResumeEffect::None);
        assert!(matches!(model.progress(), ResumeProgress::Failed(_)));

        model.open(&session("exited", false));
        let generation = model.generation();
        let mut unsafe_review = review();
        unsafe_review.permission_policy = "workspace_write".into();
        model.reviewed(generation, Ok(unsafe_review));
        assert!(matches!(model.progress(), ResumeProgress::Failed(_)));
    }

    #[test]
    fn irreversible_resume_ignores_close_quit_and_control_c() {
        let mut model = ResumeModel::default();
        model.open(&session("exited", false));
        let generation = model.generation();
        model.reviewed(generation, Ok(review()));
        model.handle_key(key(KeyCode::Enter));
        assert_eq!(model.handle_key(key(KeyCode::Esc)), ResumeEffect::None);
        assert_eq!(
            model.handle_key(key(KeyCode::Char('q'))),
            ResumeEffect::None
        );
        assert_eq!(
            model.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ResumeEffect::None
        );
        model.close();
        assert!(model.active());
        assert!(model.irreversible_active());
    }
}
