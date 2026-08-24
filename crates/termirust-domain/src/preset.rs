use std::fmt;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::{LocalizedUserText, PositionKey, PresetId, Revision};

pub const MAX_PRESETS: usize = 512;
pub const MAX_ARGUMENTS: usize = 256;
pub const MAX_EXECUTABLE_BYTES: usize = 4 * 1024;
pub const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
pub const MAX_RESOLVED_LAUNCH_BYTES: usize = 1024 * 1024;
pub const MAX_RUNTIME_ID_BYTES: usize = 128;
pub const MAX_SUBDIRECTORY_BYTES: usize = 32 * 1024;
pub const MAX_DETECTION_CANDIDATES: usize = 64;

#[derive(Clone, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RuntimeId(String);

impl RuntimeId {
    pub fn new(value: &str) -> Result<Self, PresetError> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > MAX_RUNTIME_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(PresetError::InvalidRuntime);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RuntimeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ExecutableSpec {
    SearchPath(String),
    Absolute(String),
}

impl ExecutableSpec {
    pub fn parse(value: &str) -> Result<Self, PresetError> {
        if value.is_empty() {
            return Err(PresetError::EmptyExecutable);
        }
        if value.contains('\0') {
            return Err(PresetError::ExecutableContainsNul);
        }
        if value.len() > MAX_EXECUTABLE_BYTES {
            return Err(PresetError::ExecutableTooLong);
        }
        let path = Path::new(value);
        if path.is_absolute() {
            return Ok(Self::Absolute(value.to_string()));
        }
        if path.components().count() != 1
            || !matches!(path.components().next(), Some(Component::Normal(_)))
        {
            return Err(PresetError::RelativeExecutablePath);
        }
        Ok(Self::SearchPath(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::SearchPath(value) | Self::Absolute(value) => value,
        }
    }
}

impl fmt::Debug for ExecutableSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SearchPath(value) => formatter.debug_tuple("SearchPath").field(value).finish(),
            Self::Absolute(_) => formatter
                .debug_tuple("Absolute")
                .field(&"<redacted>")
                .finish(),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct OsStringValue(String);

impl OsStringValue {
    pub fn new(value: impl Into<String>) -> Result<Self, PresetError> {
        let value = value.into();
        if value.contains('\0') {
            return Err(PresetError::ArgumentContainsNul);
        }
        if value.len() > MAX_ARGUMENT_BYTES {
            return Err(PresetError::ArgumentTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OsStringValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OsStringValue(<redacted>)")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WorkingDirectoryRule {
    ProjectRoot,
    ContainedSubdirectory(String),
    PlatformHome,
}

impl WorkingDirectoryRule {
    pub fn contained(value: &str) -> Result<Self, PresetError> {
        if value.is_empty() || value.len() > MAX_SUBDIRECTORY_BYTES || value.contains('\0') {
            return Err(PresetError::InvalidWorkingDirectory);
        }
        let path = Path::new(value);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(PresetError::InvalidWorkingDirectory);
        }
        Ok(Self::ContainedSubdirectory(value.to_string()))
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    #[default]
    AskAsNeeded,
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetOrigin {
    BuiltIn,
    Detected,
    #[default]
    User,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "option", rename_all = "snake_case")]
pub enum PresetRisk {
    Safe,
    Risky(String),
}

impl PresetRisk {
    pub fn is_risky(&self) -> bool {
        matches!(self, Self::Risky(_))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LaunchPreset {
    pub id: PresetId,
    pub label: LocalizedUserText,
    pub executable: ExecutableSpec,
    pub args: Vec<OsStringValue>,
    pub working_directory: WorkingDirectoryRule,
    pub runtime: Option<RuntimeId>,
    pub enabled: bool,
    pub favorite: bool,
    pub position: PositionKey,
    pub permission_policy: PermissionPolicy,
    pub origin: PresetOrigin,
    pub risk: PresetRisk,
    pub revision: Revision,
}

impl LaunchPreset {
    pub fn to_draft(&self) -> PresetDraft {
        PresetDraft {
            id: self.id,
            label: self.label.as_str().to_string(),
            executable: self.executable.as_str().to_string(),
            args: self
                .args
                .iter()
                .map(|argument| argument.as_str().to_string())
                .collect(),
            working_directory: self.working_directory.clone(),
            runtime: self
                .runtime
                .as_ref()
                .map(|runtime| runtime.as_str().to_string()),
            enabled: self.enabled,
            favorite: self.favorite,
            permission_policy: self.permission_policy,
            origin: self.origin,
            confirm_risky_favorite: self.risk.is_risky(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PresetDraft {
    pub id: PresetId,
    pub label: String,
    pub executable: String,
    pub args: Vec<String>,
    pub working_directory: WorkingDirectoryRule,
    pub runtime: Option<String>,
    pub enabled: bool,
    pub favorite: bool,
    pub permission_policy: PermissionPolicy,
    pub origin: PresetOrigin,
    pub confirm_risky_favorite: bool,
}

impl PresetDraft {
    pub fn validate(
        self,
        position: PositionKey,
        revision: Revision,
    ) -> Result<LaunchPreset, PresetError> {
        let label = LocalizedUserText::new(&self.label).map_err(|_| PresetError::InvalidLabel)?;
        let executable = ExecutableSpec::parse(&self.executable)?;
        if self.args.len() > MAX_ARGUMENTS {
            return Err(PresetError::TooManyArguments);
        }
        let args = self
            .args
            .into_iter()
            .map(OsStringValue::new)
            .collect::<Result<Vec<_>, _>>()?;
        validate_working_directory(&self.working_directory)?;
        let runtime = self.runtime.as_deref().map(RuntimeId::new).transpose()?;
        let risk = classify_arguments(runtime.as_ref().map(RuntimeId::as_str), &args);
        if self.origin == PresetOrigin::BuiltIn && risk.is_risky() {
            return Err(PresetError::BuiltInBypassOption);
        }
        if self.favorite && risk.is_risky() && !self.confirm_risky_favorite {
            return Err(PresetError::RiskConfirmationRequired);
        }
        let total = executable.as_str().len()
            + args
                .iter()
                .map(|argument| argument.as_str().len())
                .sum::<usize>()
            + match &self.working_directory {
                WorkingDirectoryRule::ContainedSubdirectory(value) => value.len(),
                WorkingDirectoryRule::ProjectRoot | WorkingDirectoryRule::PlatformHome => 0,
            };
        if total > MAX_RESOLVED_LAUNCH_BYTES {
            return Err(PresetError::LaunchTooLarge);
        }
        Ok(LaunchPreset {
            id: self.id,
            label,
            executable,
            args,
            working_directory: self.working_directory,
            runtime,
            enabled: self.enabled,
            favorite: self.favorite,
            position,
            permission_policy: self.permission_policy,
            origin: self.origin,
            risk,
            revision,
        })
    }
}

fn validate_working_directory(rule: &WorkingDirectoryRule) -> Result<(), PresetError> {
    if let WorkingDirectoryRule::ContainedSubdirectory(value) = rule {
        WorkingDirectoryRule::contained(value)?;
    }
    Ok(())
}

pub fn classify_arguments(runtime: Option<&str>, args: &[OsStringValue]) -> PresetRisk {
    classify_argument_values(runtime, args.iter().map(OsStringValue::as_str))
}

pub fn classify_argument_strings(runtime: Option<&str>, args: &[String]) -> PresetRisk {
    classify_argument_values(runtime, args.iter().map(String::as_str))
}

fn classify_argument_values<'a>(
    runtime: Option<&str>,
    args: impl Iterator<Item = &'a str>,
) -> PresetRisk {
    let forbidden: &[&str] = match runtime.unwrap_or_default() {
        "codex" => &[
            "--dangerously-bypass-approvals-and-sandbox",
            "danger-full-access",
        ],
        "claude" | "claude-code" => &["--dangerously-skip-permissions"],
        "gemini" | "gemini-cli" => &["--yolo", "yolo"],
        _ => &[
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-skip-permissions",
            "--yolo",
        ],
    };
    args.map(str::to_ascii_lowercase)
        .find_map(|argument| {
            forbidden
                .iter()
                .find(|candidate| argument.contains(**candidate))
                .map(|candidate| PresetRisk::Risky((*candidate).to_string()))
        })
        .unwrap_or(PresetRisk::Safe)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionStatus {
    Supported,
    DetectedUnknownVersion,
    UnsupportedVersion,
    Missing,
    PermissionDenied,
    TimedOut,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DetectionCandidate {
    pub runtime: RuntimeId,
    pub executable: ExecutableSpec,
    pub version: Option<String>,
    pub status: DetectionStatus,
    pub diagnostic_code: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DetectionReport {
    pub candidates: Vec<DetectionCandidate>,
    pub partial: bool,
    pub cancelled: bool,
}

impl DetectionReport {
    pub fn validate(&self) -> Result<(), PresetError> {
        if self.candidates.len() > MAX_DETECTION_CANDIDATES {
            return Err(PresetError::DetectionLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresetError {
    EmptyExecutable,
    ExecutableContainsNul,
    ExecutableTooLong,
    RelativeExecutablePath,
    ArgumentContainsNul,
    ArgumentTooLong,
    TooManyArguments,
    LaunchTooLarge,
    InvalidLabel,
    InvalidRuntime,
    InvalidWorkingDirectory,
    BuiltInBypassOption,
    RiskConfirmationRequired,
    AlreadyPresent,
    Unavailable,
    StaleRevision {
        expected: Revision,
        actual: Revision,
    },
    ResourceLimit {
        limit: usize,
    },
    RevisionOverflow,
    DetectionLimit,
    Store {
        code: &'static str,
    },
}

impl fmt::Display for PresetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyExecutable => "preset executable is empty",
            Self::ExecutableContainsNul => "preset executable contains NUL",
            Self::ExecutableTooLong => "preset executable is too long",
            Self::RelativeExecutablePath => {
                "preset executable must be a bare name or absolute path"
            }
            Self::ArgumentContainsNul => "preset argument contains NUL",
            Self::ArgumentTooLong => "preset argument is too long",
            Self::TooManyArguments => "preset has too many arguments",
            Self::LaunchTooLarge => "preset launch exceeds one MiB",
            Self::InvalidLabel => "preset label is invalid",
            Self::InvalidRuntime => "preset runtime identifier is invalid",
            Self::InvalidWorkingDirectory => "preset working directory rule is invalid",
            Self::BuiltInBypassOption => "built-in preset contains a permission-bypass option",
            Self::RiskConfirmationRequired => {
                "risky preset requires explicit confirmation before becoming favorite"
            }
            Self::AlreadyPresent => "preset already exists",
            Self::Unavailable => "preset is unavailable",
            Self::StaleRevision { .. } => "preset library changed; reload required",
            Self::ResourceLimit { .. } => "preset library limit reached",
            Self::RevisionOverflow => "preset revision exhausted",
            Self::DetectionLimit => "preset discovery result limit exceeded",
            Self::Store { .. } => "preset store failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PresetError {}

pub trait PresetService {
    fn list(&self) -> Result<Vec<LaunchPreset>, PresetError>;
    fn save(&self, draft: PresetDraft, expected: Revision) -> Result<LaunchPreset, PresetError>;
    fn remove(&self, id: PresetId, expected: Revision) -> Result<(), PresetError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn draft(args: Vec<String>) -> PresetDraft {
        PresetDraft {
            id: PresetId::from_uuid(Uuid::from_u128(1)),
            label: "  Literal CLI  ".to_string(),
            executable: "codex".to_string(),
            args,
            working_directory: WorkingDirectoryRule::ProjectRoot,
            runtime: Some("codex".to_string()),
            enabled: true,
            favorite: false,
            permission_policy: PermissionPolicy::AskAsNeeded,
            origin: PresetOrigin::User,
            confirm_risky_favorite: false,
        }
    }

    #[test]
    fn preset_literal_argv_round_trips_without_shell_parsing() {
        let values = vec![
            "argument with spaces".to_string(),
            "$(touch should-not-run)".to_string(),
            "single'quote".to_string(),
            "日本語".to_string(),
            "--leading-dash".to_string(),
        ];
        let preset = draft(values.clone())
            .validate(PositionKey::FIRST, Revision::new(1))
            .unwrap();
        assert_eq!(preset.label.as_str(), "Literal CLI");
        assert_eq!(
            preset
                .args
                .iter()
                .map(OsStringValue::as_str)
                .collect::<Vec<_>>(),
            values.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn preset_limits_and_nul_are_rejected() {
        assert_eq!(
            draft(vec!["bad\0arg".to_string()]).validate(PositionKey::FIRST, Revision::new(1)),
            Err(PresetError::ArgumentContainsNul)
        );
        assert_eq!(
            draft(vec![String::new(); MAX_ARGUMENTS + 1])
                .validate(PositionKey::FIRST, Revision::new(1)),
            Err(PresetError::TooManyArguments)
        );
        let mut invalid = draft(Vec::new());
        invalid.executable = "relative/tool".to_string();
        assert_eq!(
            invalid.validate(PositionKey::FIRST, Revision::new(1)),
            Err(PresetError::RelativeExecutablePath)
        );
    }

    #[test]
    fn dangerous_options_are_rejected_for_builtins_and_confirmed_for_favorites() {
        let mut risky = draft(vec![
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
        ]);
        risky.origin = PresetOrigin::BuiltIn;
        assert_eq!(
            risky.clone().validate(PositionKey::FIRST, Revision::new(1)),
            Err(PresetError::BuiltInBypassOption)
        );
        risky.origin = PresetOrigin::User;
        risky.favorite = true;
        assert_eq!(
            risky.clone().validate(PositionKey::FIRST, Revision::new(1)),
            Err(PresetError::RiskConfirmationRequired)
        );
        risky.confirm_risky_favorite = true;
        assert!(
            risky
                .validate(PositionKey::FIRST, Revision::new(1))
                .unwrap()
                .risk
                .is_risky()
        );
    }

    #[test]
    fn contained_working_directory_cannot_escape_project() {
        assert!(WorkingDirectoryRule::contained("src/tools").is_ok());
        assert_eq!(
            WorkingDirectoryRule::contained("../outside"),
            Err(PresetError::InvalidWorkingDirectory)
        );
    }

    #[test]
    fn sensitive_values_are_redacted_from_debug() {
        let executable = ExecutableSpec::parse("/Users/private/customer-cli").unwrap();
        let argument = OsStringValue::new("--token=secret").unwrap();
        assert!(!format!("{executable:?}").contains("customer-cli"));
        assert!(!format!("{argument:?}").contains("secret"));
    }
}
