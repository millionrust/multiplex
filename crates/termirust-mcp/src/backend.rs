use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use termirust_browser::{
    BrowserArtifactKind, BrowserCancellation, BrowserError, BrowserRequest, BrowserRuntime,
    BrowserRuntimeConfig,
};
use termirust_cli::{
    AutomationCommand, Cancellation, CliCommand, CliData, CliPaths, CommandService,
    LocalCommandService, ManagementCommand, SessionInput, SessionListFilter, SessionWaitCondition,
};
use termirust_domain::{
    ActivityState, ArtifactCancellation, ArtifactId, ArtifactScope, CommandId, GroupId,
    HostedSessionId, HostedSessionState, OutputSequence, PresetId, ProjectId, Revision,
    TranscriptCancellation, TranscriptKind, normalize_transcript_content,
};
use termirust_store::{ArtifactIngestRequest, ArtifactRepository};

use crate::actions::ActionPolicyStore;

const MAX_TRANSCRIPT_SOURCE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_TRANSCRIPT_RECORD_BYTES: usize = 64 * 1024;
const MAX_TRANSCRIPT_SCAN_RECORDS: usize = 100_000;
const MAX_TRANSCRIPT_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InspectionRequest {
    Status,
    Projects,
    Connections {
        project_id: String,
    },
    Sessions {
        project_id: Option<String>,
        state: Option<String>,
        include_archived: bool,
    },
    Session {
        session_id: String,
    },
    RuntimeStatus {
        session_id: String,
    },
    Artifacts {
        session_id: String,
    },
    Transcript {
        session_id: String,
    },
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ActionRequest {
    Launch {
        command_id: String,
        project_id: String,
        preset_id: String,
        group_id: Option<String>,
    },
    Wait {
        session_id: String,
        state: Option<String>,
        activity: Option<String>,
        timeout_ms: u64,
    },
    Attach {
        session_id: String,
        from_sequence: u64,
        columns: u16,
        rows: u16,
    },
    Cancel {
        command_id: String,
        session_id: String,
        expected_revision: u64,
    },
    Input {
        command_id: String,
        session_id: String,
        #[serde(skip_serializing)]
        input: String,
    },
    ResumeReview {
        session_id: String,
    },
    Resume {
        command_id: String,
        session_id: String,
        expected_revision: u64,
    },
    CreateArtifact {
        command_id: String,
        session_id: String,
        display_name: String,
        #[serde(skip_serializing)]
        content: String,
    },
    BrowserText {
        command_id: String,
        session_id: String,
        display_name: String,
        #[serde(skip_serializing)]
        url: String,
    },
    BrowserScreenshot {
        command_id: String,
        session_id: String,
        display_name: String,
        #[serde(skip_serializing)]
        url: String,
    },
    BrowserDownload {
        command_id: String,
        session_id: String,
        display_name: String,
        #[serde(skip_serializing)]
        url: String,
    },
}

impl fmt::Debug for ActionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActionRequest")
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}

impl ActionRequest {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Launch { .. } => "sessions.launch",
            Self::Wait { .. } => "sessions.wait",
            Self::Attach { .. } => "sessions.attach",
            Self::Cancel { .. } => "sessions.cancel",
            Self::Input { .. } => "sessions.input",
            Self::ResumeReview { .. } => "sessions.resume.review",
            Self::Resume { .. } => "sessions.resume",
            Self::CreateArtifact { .. } => "artifacts.create",
            Self::BrowserText { .. } => "browser.text",
            Self::BrowserScreenshot { .. } => "browser.screenshot",
            Self::BrowserDownload { .. } => "browser.download",
        }
    }

    pub(crate) fn command_id(&self) -> Option<&str> {
        match self {
            Self::Launch { command_id, .. }
            | Self::Cancel { command_id, .. }
            | Self::Input { command_id, .. }
            | Self::Resume { command_id, .. }
            | Self::CreateArtifact { command_id, .. }
            | Self::BrowserText { command_id, .. }
            | Self::BrowserScreenshot { command_id, .. }
            | Self::BrowserDownload { command_id, .. } => Some(command_id),
            Self::Wait { .. } | Self::Attach { .. } | Self::ResumeReview { .. } => None,
        }
    }

    pub(crate) fn project_scope(&self) -> Option<&str> {
        match self {
            Self::Launch { project_id, .. } => Some(project_id),
            _ => None,
        }
    }

    pub(crate) fn session_scope(&self) -> Option<&str> {
        match self {
            Self::Wait { session_id, .. }
            | Self::Attach { session_id, .. }
            | Self::Cancel { session_id, .. }
            | Self::Input { session_id, .. }
            | Self::ResumeReview { session_id }
            | Self::Resume { session_id, .. }
            | Self::CreateArtifact { session_id, .. }
            | Self::BrowserText { session_id, .. }
            | Self::BrowserScreenshot { session_id, .. }
            | Self::BrowserDownload { session_id, .. } => Some(session_id),
            Self::Launch { .. } => None,
        }
    }

    pub(crate) fn fingerprint(&self) -> String {
        let mut canonical = serde_json::to_value(self).unwrap_or_else(|_| json!({}));
        let payload = match self {
            Self::Input { input, .. } => Some(input.as_bytes()),
            Self::CreateArtifact { content, .. } => Some(content.as_bytes()),
            Self::BrowserText { url, .. }
            | Self::BrowserScreenshot { url, .. }
            | Self::BrowserDownload { url, .. } => Some(url.as_bytes()),
            _ => None,
        };
        if let (Some(object), Some(payload)) = (canonical.as_object_mut(), payload) {
            object.insert(
                "payload_sha256".to_string(),
                Value::String(format!("{:x}", Sha256::digest(payload))),
            );
        }
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
    }

    pub(crate) fn browser_url(&self) -> Option<&str> {
        match self {
            Self::BrowserText { url, .. }
            | Self::BrowserScreenshot { url, .. }
            | Self::BrowserDownload { url, .. } => Some(url),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectionPage {
    pub data: Value,
    pub next_offset: Option<usize>,
}

pub trait InspectionSource: Send + Sync {
    fn inspect(
        &self,
        request: InspectionRequest,
        offset: usize,
        page_size: usize,
        cancellation: &Cancellation,
    ) -> Result<InspectionPage, SourceError>;

    fn act(
        &self,
        _request: ActionRequest,
        _cancellation: &Cancellation,
    ) -> Result<Value, SourceError> {
        Err(SourceError::PermissionDenied)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceError {
    Cancelled,
    InvalidInput,
    PermissionDenied,
    ResourceLimit,
    Unavailable,
    Inconsistent,
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "operation was cancelled",
            Self::InvalidInput => "operation input is invalid",
            Self::PermissionDenied => "operation is not permitted",
            Self::ResourceLimit => "operation exceeded a resource limit",
            Self::Unavailable => "operation is unavailable",
            Self::Inconsistent => "operation state is inconsistent",
        })
    }
}

impl std::error::Error for SourceError {}

pub struct LocalInspectionSource {
    paths: CliPaths,
    service: Mutex<LocalCommandService>,
    actions: ActionPolicyStore,
}

impl fmt::Debug for LocalInspectionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalInspectionSource")
            .field("paths", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl LocalInspectionSource {
    pub fn discover() -> Result<Self, SourceError> {
        let paths = CliPaths::discover().map_err(map_cli_error)?;
        Ok(Self::new(paths))
    }

    pub fn new(paths: CliPaths) -> Self {
        let service = LocalCommandService::open(paths.clone());
        let actions = ActionPolicyStore::new(paths.config_root().join("mcp"));
        Self {
            paths,
            service: Mutex::new(service),
            actions,
        }
    }

    fn execute(
        &self,
        command: CliCommand,
        cancellation: &Cancellation,
    ) -> Result<CliData, SourceError> {
        cancellation_check(cancellation)?;
        let mut service = LocalCommandService::open(self.paths.clone());
        let result = service
            .execute(command, cancellation)
            .map_err(map_cli_error)?;
        cancellation_check(cancellation)?;
        Ok(result)
    }

    fn transcript_page(
        &self,
        session_id: HostedSessionId,
        offset: usize,
        page_size: usize,
        cancellation: &Cancellation,
    ) -> Result<InspectionPage, SourceError> {
        let _ = self.execute(CliCommand::SessionShow { session_id }, cancellation)?;
        if offset >= MAX_TRANSCRIPT_SCAN_RECORDS {
            return Err(SourceError::ResourceLimit);
        }
        let transcript_root = self
            .paths
            .session_data_root()
            .join(session_id.to_string())
            .join("transcripts");
        let source = open_contained_transcript(&transcript_root)?;
        let mut reader = BufReader::new(source);
        let mut ordinal = 0usize;
        let mut records = Vec::with_capacity(page_size);
        let mut response_bytes = 0usize;
        let mut skipped = 0u64;
        let mut redactions = 0u64;
        let mut eof = false;
        while ordinal < MAX_TRANSCRIPT_SCAN_RECORDS && records.len() < page_size {
            cancellation_check(cancellation)?;
            let Some(record) = read_bounded_record(&mut reader, cancellation)? else {
                eof = true;
                break;
            };
            ordinal = ordinal.saturating_add(1);
            if ordinal <= offset {
                continue;
            }
            if record.oversize {
                skipped = skipped.saturating_add(1);
                continue;
            }
            let Ok(raw) = serde_json::from_slice::<RawTranscriptRecord>(&record.bytes) else {
                skipped = skipped.saturating_add(1);
                continue;
            };
            if !matches!(raw.kind, TranscriptKind::User | TranscriptKind::Assistant) {
                skipped = skipped.saturating_add(1);
                continue;
            }
            let normalized =
                normalize_transcript_content(&raw.content, &TranscriptCancellation::default())
                    .map_err(|_| SourceError::InvalidInput)?;
            let content = normalized.content.expose();
            let next_size = response_bytes.saturating_add(content.len());
            if next_size > MAX_TRANSCRIPT_RESPONSE_BYTES {
                if records.is_empty() {
                    records.push(json!({
                        "sequence": ordinal,
                        "kind": raw.kind,
                        "occurred_at": raw.occurred_at,
                        "content": "[content omitted: MCP response limit]",
                        "redacted": true,
                    }));
                    redactions = redactions.saturating_add(1);
                }
                break;
            }
            response_bytes = next_size;
            redactions = redactions.saturating_add(normalized.redaction_count);
            records.push(json!({
                "sequence": ordinal,
                "kind": raw.kind,
                "occurred_at": raw.occurred_at,
                "content": content,
                "redacted": normalized.redaction_count > 0,
            }));
        }
        if ordinal >= MAX_TRANSCRIPT_SCAN_RECORDS && !eof {
            return Err(SourceError::ResourceLimit);
        }
        cancellation_check(cancellation)?;
        Ok(InspectionPage {
            data: json!({
                "session_id": session_id.to_string(),
                "entries": records,
                "skipped_count": skipped,
                "redaction_count": redactions,
                "categories": ["user", "assistant"],
                "terminal_output_included": false,
            }),
            next_offset: (!eof).then_some(ordinal),
        })
    }
}

impl InspectionSource for LocalInspectionSource {
    fn inspect(
        &self,
        request: InspectionRequest,
        offset: usize,
        page_size: usize,
        cancellation: &Cancellation,
    ) -> Result<InspectionPage, SourceError> {
        match request {
            InspectionRequest::Status => {
                let CliData::Status(data) = self.execute(CliCommand::Status, cancellation)? else {
                    return Err(SourceError::Inconsistent);
                };
                singleton(data, "status")
            }
            InspectionRequest::Projects => {
                let CliData::Projects(data) =
                    self.execute(CliCommand::ProjectList, cancellation)?
                else {
                    return Err(SourceError::Inconsistent);
                };
                paginated("projects", data.projects, offset, page_size)
            }
            InspectionRequest::Connections { project_id } => {
                let project_id = parse_project_id(&project_id)?;
                let CliData::Presets(data) =
                    self.execute(CliCommand::PresetList { project_id }, cancellation)?
                else {
                    return Err(SourceError::Inconsistent);
                };
                paginated("connections", data.presets, offset, page_size)
            }
            InspectionRequest::Sessions {
                project_id,
                state,
                include_archived,
            } => {
                let project_id = project_id.as_deref().map(parse_project_id).transpose()?;
                let state = state.as_deref().map(parse_session_state).transpose()?;
                let CliData::Sessions(data) = self.execute(
                    CliCommand::SessionList(SessionListFilter {
                        project_id,
                        group_id: None,
                        state,
                        archived_only: false,
                    }),
                    cancellation,
                )?
                else {
                    return Err(SourceError::Inconsistent);
                };
                let sessions = data
                    .sessions
                    .into_iter()
                    .filter(|session| include_archived || !session.archived)
                    .collect::<Vec<_>>();
                paginated("sessions", sessions, offset, page_size)
            }
            InspectionRequest::Session { session_id } => {
                let session_id = parse_session_id(&session_id)?;
                let CliData::Session(data) =
                    self.execute(CliCommand::SessionShow { session_id }, cancellation)?
                else {
                    return Err(SourceError::Inconsistent);
                };
                singleton(data.session, "session")
            }
            InspectionRequest::RuntimeStatus { session_id } => {
                let session_id = parse_session_id(&session_id)?;
                let CliData::Session(data) =
                    self.execute(CliCommand::SessionShow { session_id }, cancellation)?
                else {
                    return Err(SourceError::Inconsistent);
                };
                Ok(InspectionPage {
                    data: json!({
                        "session_id": data.session.id,
                        "lifecycle": data.session.state,
                        "activity": data.session.activity,
                        "unread": data.session.unread,
                        "revision": data.session.revision,
                        "authority": "termirust_host_projection",
                    }),
                    next_offset: None,
                })
            }
            InspectionRequest::Artifacts { session_id } => {
                let session_id = parse_session_id(&session_id)?;
                let _ = self.execute(CliCommand::SessionShow { session_id }, cancellation)?;
                let repository = ArtifactRepository::open(self.paths.session_data_root())
                    .map_err(map_artifact_error)?;
                let snapshot = repository
                    .list(ArtifactScope { session_id })
                    .map_err(map_artifact_error)?;
                paginated("artifacts", snapshot.artifacts, offset, page_size)
            }
            InspectionRequest::Transcript { session_id } => self.transcript_page(
                parse_session_id(&session_id)?,
                offset,
                page_size,
                cancellation,
            ),
        }
    }

    fn act(
        &self,
        request: ActionRequest,
        cancellation: &Cancellation,
    ) -> Result<Value, SourceError> {
        self.actions
            .run_with_policy(&request, cancellation, |authorized, policy| {
                self.execute_action(&request, &policy.browser_origins, authorized)
            })
    }
}

impl LocalInspectionSource {
    fn execute_action(
        &self,
        request: &ActionRequest,
        browser_origins: &[String],
        cancellation: &Cancellation,
    ) -> Result<Value, SourceError> {
        if matches!(
            request,
            ActionRequest::BrowserText { .. }
                | ActionRequest::BrowserScreenshot { .. }
                | ActionRequest::BrowserDownload { .. }
        ) {
            return self.execute_browser_action(request, browser_origins, cancellation);
        }
        let mut service = self.service.lock().map_err(|_| SourceError::Unavailable)?;
        let data = match request {
            ActionRequest::Launch {
                command_id,
                project_id,
                preset_id,
                group_id,
            } => service.execute_management(
                ManagementCommand::Launch {
                    command_id: parse_command_id(command_id)?,
                    project_id: parse_project_id(project_id)?,
                    preset_id: parse_preset_id(preset_id)?,
                    group_id: group_id.as_deref().map(parse_group_id).transpose()?,
                },
                cancellation,
            ),
            ActionRequest::Wait {
                session_id,
                state,
                activity,
                timeout_ms,
            } => service.execute_automation(
                AutomationCommand::Wait {
                    session_id: parse_session_id(session_id)?,
                    condition: parse_wait_condition(state.as_deref(), activity.as_deref())?,
                    timeout_ms: *timeout_ms,
                },
                cancellation,
            ),
            ActionRequest::Attach {
                session_id,
                from_sequence,
                columns,
                rows,
            } => service.execute_automation(
                AutomationCommand::Attach {
                    session_id: parse_session_id(session_id)?,
                    from_sequence: OutputSequence::new(*from_sequence),
                    columns: *columns,
                    rows: *rows,
                },
                cancellation,
            ),
            ActionRequest::Cancel {
                command_id,
                session_id,
                expected_revision,
            } => service.execute_management(
                ManagementCommand::Stop {
                    command_id: parse_command_id(command_id)?,
                    session_id: parse_session_id(session_id)?,
                    expected_revision: Revision::new(*expected_revision),
                },
                cancellation,
            ),
            ActionRequest::Input {
                command_id,
                session_id,
                input,
            } => service.execute_automation(
                AutomationCommand::Input {
                    command_id: parse_command_id(command_id)?,
                    session_id: parse_session_id(session_id)?,
                    input: SessionInput::new(input.as_bytes().to_vec()).map_err(map_cli_error)?,
                },
                cancellation,
            ),
            ActionRequest::ResumeReview { session_id } => service.execute_automation(
                AutomationCommand::Resume {
                    command_id: CommandId::new(),
                    session_id: parse_session_id(session_id)?,
                    expected_revision: None,
                    confirmed: false,
                },
                cancellation,
            ),
            ActionRequest::Resume {
                command_id,
                session_id,
                expected_revision,
            } => service.execute_automation(
                AutomationCommand::Resume {
                    command_id: parse_command_id(command_id)?,
                    session_id: parse_session_id(session_id)?,
                    expected_revision: Some(Revision::new(*expected_revision)),
                    confirmed: true,
                },
                cancellation,
            ),
            ActionRequest::CreateArtifact {
                command_id,
                session_id,
                display_name,
                content,
            } => {
                let session_id = parse_session_id(session_id)?;
                let _ = service
                    .execute(CliCommand::SessionShow { session_id }, cancellation)
                    .map_err(map_cli_error)?;
                return self.create_artifact(
                    parse_command_id(command_id)?,
                    session_id,
                    display_name,
                    content,
                    cancellation,
                );
            }
            ActionRequest::BrowserText { .. }
            | ActionRequest::BrowserScreenshot { .. }
            | ActionRequest::BrowserDownload { .. } => {
                return Err(SourceError::Inconsistent);
            }
        }
        .map_err(map_cli_error)?;
        serde_json::to_value(data).map_err(|_| SourceError::Inconsistent)
    }

    fn create_artifact(
        &self,
        command_id: CommandId,
        session_id: HostedSessionId,
        display_name: &str,
        content: &str,
        cancellation: &Cancellation,
    ) -> Result<Value, SourceError> {
        const MAX_CONTENT_BYTES: usize = 64 * 1024;
        if content.is_empty() || content.len() > MAX_CONTENT_BYTES || cancellation.is_cancelled() {
            return Err(if cancellation.is_cancelled() {
                SourceError::Cancelled
            } else {
                SourceError::InvalidInput
            });
        }
        self.ingest_artifact_bytes(
            command_id,
            session_id,
            display_name,
            content.as_bytes(),
            "txt",
            cancellation,
        )
    }

    fn execute_browser_action(
        &self,
        request: &ActionRequest,
        approved_origins: &[String],
        cancellation: &Cancellation,
    ) -> Result<Value, SourceError> {
        let (command_id, session_id, display_name, url, kind, extension) = match request {
            ActionRequest::BrowserText {
                command_id,
                session_id,
                display_name,
                url,
            } => (
                parse_command_id(command_id)?,
                parse_session_id(session_id)?,
                display_name.as_str(),
                url.as_str(),
                BrowserArtifactKind::SemanticText,
                "txt",
            ),
            ActionRequest::BrowserScreenshot {
                command_id,
                session_id,
                display_name,
                url,
            } => (
                parse_command_id(command_id)?,
                parse_session_id(session_id)?,
                display_name.as_str(),
                url.as_str(),
                BrowserArtifactKind::ScreenshotPng,
                "png",
            ),
            ActionRequest::BrowserDownload {
                command_id,
                session_id,
                display_name,
                url,
            } => (
                parse_command_id(command_id)?,
                parse_session_id(session_id)?,
                display_name.as_str(),
                url.as_str(),
                BrowserArtifactKind::Download,
                "bin",
            ),
            _ => return Err(SourceError::Inconsistent),
        };
        let _ = self.execute(CliCommand::SessionShow { session_id }, cancellation)?;
        let browser_cancellation = BrowserCancellation::default();
        let monitor_cancellation = browser_cancellation.clone();
        let source_cancellation = cancellation.clone();
        let done = Arc::new(AtomicBool::new(false));
        let monitor_done = done.clone();
        let monitor = thread::spawn(move || {
            while !monitor_done.load(Ordering::Acquire) {
                if source_cancellation.is_cancelled() {
                    monitor_cancellation.cancel();
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
        let runtime = BrowserRuntime::new(BrowserRuntimeConfig {
            profile_parent: self
                .paths
                .config_root()
                .join("mcp")
                .join("browser-profiles"),
            executable: std::env::var_os("TERMIRUST_BROWSER_EXECUTABLE").map(Into::into),
        });
        let browser_request = BrowserRequest {
            url: url.to_string(),
            approved_origins: approved_origins.to_vec(),
            kind,
        };
        let artifact = if kind == BrowserArtifactKind::Download {
            runtime.download(browser_request, &browser_cancellation)
        } else {
            runtime.capture(browser_request, &browser_cancellation)
        };
        done.store(true, Ordering::Release);
        let _ = monitor.join();
        let artifact = artifact.map_err(map_browser_error)?;
        self.ingest_artifact_bytes(
            command_id,
            session_id,
            display_name,
            &artifact.bytes,
            extension,
            cancellation,
        )
    }

    fn ingest_artifact_bytes(
        &self,
        command_id: CommandId,
        session_id: HostedSessionId,
        display_name: &str,
        content: &[u8],
        extension: &str,
        cancellation: &Cancellation,
    ) -> Result<Value, SourceError> {
        if content.is_empty()
            || content.len() as u64 > termirust_domain::MAX_ARTIFACT_BYTES
            || cancellation.is_cancelled()
        {
            return Err(if cancellation.is_cancelled() {
                SourceError::Cancelled
            } else {
                SourceError::ResourceLimit
            });
        }
        let staging_root = self.paths.config_root().join("mcp").join("staging");
        fs::create_dir_all(&staging_root).map_err(map_io_error)?;
        #[cfg(unix)]
        fs::set_permissions(&staging_root, fs::Permissions::from_mode(0o700))
            .map_err(map_io_error)?;
        let source = staging_root.join(format!("{}.{}", command_id, extension));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&source).map_err(map_io_error)?;
        let write_result = file
            .write_all(content)
            .and_then(|()| file.sync_all())
            .map_err(map_io_error);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&source);
            return Err(error);
        }
        let result = ArtifactRepository::open(self.paths.session_data_root())
            .map_err(map_artifact_error)?
            .ingest(
                ArtifactIngestRequest {
                    id: ArtifactId::from_uuid(command_id.as_uuid()),
                    scope: ArtifactScope { session_id },
                    source: source.clone(),
                    display_name: Some(display_name.to_string()),
                    created_at: now_millis(),
                },
                &ArtifactCancellation::default(),
                |_| {},
            )
            .map_err(map_artifact_error);
        let _ = fs::remove_file(source);
        cancellation_check(cancellation)?;
        serde_json::to_value(result?).map_err(|_| SourceError::Inconsistent)
    }
}

fn singleton(
    data: impl serde::Serialize,
    key: &'static str,
) -> Result<InspectionPage, SourceError> {
    let value = serde_json::to_value(data).map_err(|_| SourceError::Inconsistent)?;
    let mut object = serde_json::Map::new();
    object.insert(key.to_string(), value);
    Ok(InspectionPage {
        data: Value::Object(object),
        next_offset: None,
    })
}

fn paginated<T: serde::Serialize>(
    key: &'static str,
    values: Vec<T>,
    offset: usize,
    page_size: usize,
) -> Result<InspectionPage, SourceError> {
    if offset > values.len() {
        return Err(SourceError::InvalidInput);
    }
    let total = values.len();
    let end = offset.saturating_add(page_size).min(total);
    let records = values.get(offset..end).ok_or(SourceError::InvalidInput)?;
    let records = serde_json::to_value(records).map_err(|_| SourceError::Inconsistent)?;
    Ok(InspectionPage {
        data: json!({ key: records, "total": total }),
        next_offset: (end < total).then_some(end),
    })
}

fn parse_project_id(value: &str) -> Result<ProjectId, SourceError> {
    value.parse().map_err(|_| SourceError::InvalidInput)
}

fn parse_command_id(value: &str) -> Result<CommandId, SourceError> {
    value.parse().map_err(|_| SourceError::InvalidInput)
}

fn parse_preset_id(value: &str) -> Result<PresetId, SourceError> {
    value.parse().map_err(|_| SourceError::InvalidInput)
}

fn parse_group_id(value: &str) -> Result<GroupId, SourceError> {
    value.parse().map_err(|_| SourceError::InvalidInput)
}

fn parse_session_id(value: &str) -> Result<HostedSessionId, SourceError> {
    value.parse().map_err(|_| SourceError::InvalidInput)
}

fn parse_session_state(value: &str) -> Result<HostedSessionState, SourceError> {
    use HostedSessionState as State;
    match value {
        "draft" => Ok(State::Draft),
        "validating" => Ok(State::Validating),
        "starting" => Ok(State::Starting),
        "provisioning" => Ok(State::Provisioning),
        "attaching" => Ok(State::Attaching),
        "replaying" => Ok(State::Replaying),
        "live" => Ok(State::Live),
        "recording_paused" => Ok(State::RecordingPaused),
        "stopping" => Ok(State::Stopping),
        "offline" => Ok(State::Offline),
        "orphaned" => Ok(State::Orphaned),
        "gap" => Ok(State::Gap),
        "permission_denied" => Ok(State::PermissionDenied),
        "incompatible" => Ok(State::Incompatible),
        "running_app_attached" => Ok(State::RunningAppAttached),
        "failed" => Ok(State::Failed),
        "cancelled" => Ok(State::Cancelled),
        "exited" => Ok(State::Exited),
        _ => Err(SourceError::InvalidInput),
    }
}

fn parse_wait_condition(
    state: Option<&str>,
    activity: Option<&str>,
) -> Result<SessionWaitCondition, SourceError> {
    match (state, activity) {
        (Some(value), None) => Ok(SessionWaitCondition::Lifecycle(parse_session_state(value)?)),
        (None, Some("idle")) => Ok(SessionWaitCondition::Activity(ActivityState::Idle)),
        (None, Some("busy")) => Ok(SessionWaitCondition::Activity(ActivityState::Busy)),
        (None, Some("needs_input")) => {
            Ok(SessionWaitCondition::Activity(ActivityState::NeedsInput))
        }
        (None, Some("done")) => Ok(SessionWaitCondition::Activity(ActivityState::Done)),
        _ => Err(SourceError::InvalidInput),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn cancellation_check(cancellation: &Cancellation) -> Result<(), SourceError> {
    if cancellation.is_cancelled() {
        Err(SourceError::Cancelled)
    } else {
        Ok(())
    }
}

fn map_cli_error(error: termirust_cli::CliError) -> SourceError {
    use termirust_cli::ErrorCode;
    match error.code {
        ErrorCode::Cancelled => SourceError::Cancelled,
        ErrorCode::Usage | ErrorCode::Validation => SourceError::InvalidInput,
        ErrorCode::PermissionDenied
        | ErrorCode::InteractionRequired
        | ErrorCode::HostKeyUnknown
        | ErrorCode::HostKeyChanged
        | ErrorCode::AuthenticationDenied => SourceError::PermissionDenied,
        ErrorCode::ResourceLimit => SourceError::ResourceLimit,
        ErrorCode::Conflict | ErrorCode::UnknownCompletion => SourceError::Inconsistent,
        ErrorCode::Unavailable
        | ErrorCode::Incompatible
        | ErrorCode::BridgeUnavailable
        | ErrorCode::Timeout
        | ErrorCode::OperationFailed => SourceError::Unavailable,
    }
}

fn map_artifact_error(error: termirust_store::ArtifactStoreError) -> SourceError {
    use termirust_store::ArtifactStoreError;
    match error {
        ArtifactStoreError::Domain(termirust_domain::ArtifactError::Cancelled) => {
            SourceError::Cancelled
        }
        ArtifactStoreError::Domain(termirust_domain::ArtifactError::PermissionDenied) => {
            SourceError::PermissionDenied
        }
        ArtifactStoreError::Domain(
            termirust_domain::ArtifactError::ItemQuotaExceeded
            | termirust_domain::ArtifactError::SessionQuotaExceeded
            | termirust_domain::ArtifactError::GlobalQuotaExceeded
            | termirust_domain::ArtifactError::CountQuotaExceeded,
        )
        | ArtifactStoreError::TooLarge { .. } => SourceError::ResourceLimit,
        ArtifactStoreError::UnsafeEntry { .. } | ArtifactStoreError::Corrupt { .. } => {
            SourceError::Inconsistent
        }
        ArtifactStoreError::Domain(_) | ArtifactStoreError::Io { .. } => SourceError::Unavailable,
    }
}

fn map_browser_error(error: BrowserError) -> SourceError {
    match error {
        BrowserError::Cancelled => SourceError::Cancelled,
        BrowserError::InvalidPolicy | BrowserError::NetworkDenied => SourceError::PermissionDenied,
        BrowserError::ResourceLimit => SourceError::ResourceLimit,
        BrowserError::BrowserMissing | BrowserError::Timeout | BrowserError::Unavailable => {
            SourceError::Unavailable
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTranscriptRecord {
    kind: TranscriptKind,
    #[serde(default)]
    occurred_at: Option<i64>,
    content: String,
}

struct BoundedRecord {
    bytes: Vec<u8>,
    oversize: bool,
}

fn open_contained_transcript(root: &Path) -> Result<File, SourceError> {
    let root_metadata = fs::symlink_metadata(root).map_err(map_io_error)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(SourceError::PermissionDenied);
    }
    let canonical_root = root.canonicalize().map_err(map_io_error)?;
    let source = canonical_root.join("records.jsonl");
    let metadata = fs::symlink_metadata(&source).map_err(map_io_error)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_TRANSCRIPT_SOURCE_BYTES
    {
        return Err(SourceError::PermissionDenied);
    }
    let canonical_source = source.canonicalize().map_err(map_io_error)?;
    if canonical_source.parent() != Some(canonical_root.as_path()) {
        return Err(SourceError::PermissionDenied);
    }
    File::open(canonical_source).map_err(map_io_error)
}

fn read_bounded_record(
    reader: &mut BufReader<File>,
    cancellation: &Cancellation,
) -> Result<Option<BoundedRecord>, SourceError> {
    let mut bytes = Vec::new();
    let mut oversize = false;
    let mut consumed_any = false;
    loop {
        cancellation_check(cancellation)?;
        let available = reader.fill_buf().map_err(map_io_error)?;
        if available.is_empty() {
            return if consumed_any {
                Ok(Some(BoundedRecord { bytes, oversize }))
            } else {
                Ok(None)
            };
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        let chunk = &available[..consumed];
        consumed_any = true;
        if !oversize {
            let remaining = MAX_TRANSCRIPT_RECORD_BYTES.saturating_sub(bytes.len());
            if chunk.len() <= remaining {
                bytes.extend_from_slice(chunk);
            } else {
                bytes.extend_from_slice(&chunk[..remaining]);
                oversize = true;
            }
        }
        let ended = chunk.last() == Some(&b'\n');
        reader.consume(consumed);
        if ended {
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
                if bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
            }
            return Ok(Some(BoundedRecord { bytes, oversize }));
        }
    }
}

fn map_io_error(error: io::Error) -> SourceError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => SourceError::PermissionDenied,
        io::ErrorKind::NotFound => SourceError::Unavailable,
        _ => SourceError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::{ActionPolicy, ApprovedAction};
    use termirust_domain::{
        ActivityAggregate, AddProject, ArtifactCancellation, ArtifactId, HostedSession,
        OutputSequence, PositionKey, Revision, SessionTitle, TitleSource,
    };
    use termirust_store::{ArtifactIngestRequest, ProjectRepository, SessionRepository};

    #[test]
    fn invalid_identifiers_never_become_paths() {
        assert_eq!(
            parse_session_id("../../etc/passwd"),
            Err(SourceError::InvalidInput)
        );
        assert_eq!(
            parse_project_id("not-a-uuid"),
            Err(SourceError::InvalidInput)
        );
    }

    #[test]
    fn state_parser_is_an_exact_allowlist() {
        assert_eq!(parse_session_state("live"), Ok(HostedSessionState::Live));
        assert_eq!(parse_session_state("LIVE"), Err(SourceError::InvalidInput));
    }

    #[cfg(unix)]
    #[test]
    fn transcript_source_rejects_symlinked_files_and_roots() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary fixture");
        let transcript_root = temp.path().join("transcripts");
        let outside = temp.path().join("outside.jsonl");
        fs::create_dir_all(&transcript_root).expect("transcript directory");
        fs::write(&outside, b"{\"kind\":\"user\",\"content\":\"outside\"}\n")
            .expect("outside transcript");
        symlink(&outside, transcript_root.join("records.jsonl")).expect("file symlink");
        assert!(matches!(
            open_contained_transcript(&transcript_root),
            Err(SourceError::PermissionDenied)
        ));

        let real_root = temp.path().join("real-transcripts");
        let linked_root = temp.path().join("linked-transcripts");
        fs::create_dir_all(&real_root).expect("real transcript directory");
        fs::write(real_root.join("records.jsonl"), b"").expect("real transcript");
        symlink(&real_root, &linked_root).expect("directory symlink");
        assert!(matches!(
            open_contained_transcript(&linked_root),
            Err(SourceError::PermissionDenied)
        ));
    }

    #[test]
    fn local_source_reads_bounded_authoritative_records_and_redacts_transcripts() {
        let temp = tempfile::tempdir().expect("temporary fixture");
        let config_root = temp.path().join("config");
        let metadata_root = config_root.join("agent-workspace");
        let data_root = config_root.join("durable-sessions");
        let project_root = temp.path().join("project");
        fs::create_dir_all(&project_root).expect("project fixture");
        let project_id = "00000000-0000-0000-0000-000000000001"
            .parse::<ProjectId>()
            .expect("project ID");
        let session_id = "00000000-0000-0000-0000-000000000003"
            .parse::<HostedSessionId>()
            .expect("session ID");
        let projects = ProjectRepository::open(&metadata_root).expect("project repository");
        projects
            .add_project(AddProject {
                id: project_id,
                root: project_root,
                display_name: Some("MCP Fixture".to_string()),
                expected: Revision::ZERO,
            })
            .expect("seed project");
        let sessions =
            SessionRepository::open(&metadata_root, &data_root).expect("session repository");
        sessions
            .create_session(
                HostedSession {
                    id: session_id,
                    project_id,
                    group_id: None,
                    preset_id: None,
                    title: SessionTitle::new("MCP Session").expect("session title"),
                    title_source: TitleSource::Manual,
                    lifecycle: HostedSessionState::Live,
                    activity: ActivityAggregate::default(),
                    pinned: false,
                    position: PositionKey::FIRST,
                    last_output_sequence: OutputSequence::ZERO,
                    read_through_sequence: OutputSequence::ZERO,
                    unread_sequence: None,
                    archived_at: None,
                    created_at: 1,
                    updated_at: 1,
                    revision: Revision::ZERO,
                },
                Revision::ZERO,
            )
            .expect("seed session");
        let import = temp.path().join("artifact.txt");
        fs::write(&import, b"inert artifact payload").expect("artifact source");
        ArtifactRepository::open(&data_root)
            .expect("artifact repository")
            .ingest(
                ArtifactIngestRequest {
                    id: "00000000-0000-0000-0000-000000000004"
                        .parse::<ArtifactId>()
                        .expect("artifact ID"),
                    scope: ArtifactScope { session_id },
                    source: import,
                    display_name: Some("report.txt".to_string()),
                    created_at: 2,
                },
                &ArtifactCancellation::default(),
                |_| {},
            )
            .expect("seed artifact");
        let transcript_root = data_root.join(session_id.to_string()).join("transcripts");
        fs::create_dir_all(&transcript_root).expect("transcript directory");
        fs::write(
            transcript_root.join("records.jsonl"),
            concat!(
                "{\"kind\":\"user\",\"content\":\"API_KEY=secret-canary\"}\n",
                "{\"kind\":\"reasoning\",\"content\":\"private reasoning\"}\n",
                "{\"kind\":\"assistant\",\"content\":\"safe answer\"}\n"
            ),
        )
        .expect("semantic transcript");

        let source = LocalInspectionSource::new(CliPaths::new(
            config_root,
            temp.path().join("missing-host"),
        ));
        let cancellation = Cancellation::default();
        let projects = source
            .inspect(InspectionRequest::Projects, 0, 10, &cancellation)
            .expect("projects inspect");
        assert_eq!(projects.data["projects"][0]["name"], "MCP Fixture");
        let sessions = source
            .inspect(
                InspectionRequest::Sessions {
                    project_id: None,
                    state: None,
                    include_archived: false,
                },
                0,
                10,
                &cancellation,
            )
            .expect("sessions inspect");
        assert_eq!(sessions.data["sessions"][0]["state"], "live");
        let artifacts = source
            .inspect(
                InspectionRequest::Artifacts {
                    session_id: session_id.to_string(),
                },
                0,
                10,
                &cancellation,
            )
            .expect("artifacts inspect");
        assert_eq!(artifacts.data["artifacts"][0]["display_name"], "report.txt");
        assert!(
            !artifacts
                .data
                .to_string()
                .contains("inert artifact payload")
        );
        let transcript = source
            .inspect(
                InspectionRequest::Transcript {
                    session_id: session_id.to_string(),
                },
                0,
                10,
                &cancellation,
            )
            .expect("transcript inspect");
        let rendered = transcript.data.to_string();
        assert!(rendered.contains("API_KEY=[REDACTED]"));
        assert!(rendered.contains("safe answer"));
        assert!(!rendered.contains("secret-canary"));
        assert!(!rendered.contains("private reasoning"));
        assert_eq!(transcript.data["terminal_output_included"], false);

        source
            .actions
            .write_policy(&ActionPolicy {
                schema_version: 1,
                grant_id: "00000000-0000-0000-0000-000000000006".to_string(),
                expires_at_unix_ms: now_millis().saturating_add(60_000),
                actions: vec![ApprovedAction::CreateArtifact],
                project_ids: Vec::new(),
                session_ids: vec![session_id.to_string()],
                browser_origins: Vec::new(),
            })
            .expect("artifact approval");
        let create = ActionRequest::CreateArtifact {
            command_id: "00000000-0000-0000-0000-000000000005".to_string(),
            session_id: session_id.to_string(),
            display_name: "agent-note.txt".to_string(),
            content: "semantic artifact content".to_string(),
        };
        let first = source
            .act(create.clone(), &Cancellation::default())
            .expect("create artifact");
        let replay = source
            .act(create, &Cancellation::default())
            .expect("replay artifact command");
        assert_eq!(first, replay);
        let artifacts = source
            .inspect(
                InspectionRequest::Artifacts {
                    session_id: session_id.to_string(),
                },
                0,
                10,
                &Cancellation::default(),
            )
            .expect("artifacts after action");
        assert_eq!(artifacts.data["total"], 2);
        assert!(
            !artifacts
                .data
                .to_string()
                .contains("semantic artifact content")
        );
    }
}
