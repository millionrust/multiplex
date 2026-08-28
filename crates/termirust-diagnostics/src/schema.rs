use std::collections::BTreeMap;
use std::fmt;

use rand::RngCore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::SCHEMA_VERSION;

const MAX_SAFE_FIELDS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    AppStarted,
    AppStopped,
    PanicCaptured,
    StorageUnavailable,
    SettingsReadFailed,
    SettingsWriteFailed,
    SessionStateChanged,
    HostOperationFailed,
    AgentOperationFailed,
    ControllerOperationFailed,
    UpdateVerificationFailed,
    EventsDropped,
    DiagnosticsCleared,
    ExportPrepared,
    ExportFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticMessageId {
    AppLifecycle,
    UnexpectedFailure,
    LocalStorageUnavailable,
    OperationUnavailable,
    DiagnosticsDropping,
    DiagnosticsExport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Retry,
    CheckPermissions,
    FreeDiskSpace,
    RestartApplication,
    OpenDiagnosticsSettings,
    None,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeField {
    Component,
    Operation,
    State,
    ErrorClass,
    AttemptCount,
    ItemCount,
    DroppedCount,
    DurationBucket,
    Enabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Application,
    Settings,
    Storage,
    Ssh,
    LocalPty,
    Agent,
    Controller,
    Updater,
    Diagnostics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Start,
    Stop,
    Read,
    Write,
    Connect,
    Disconnect,
    Verify,
    Rotate,
    Clear,
    PreviewExport,
    PublishExport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticState {
    Starting,
    Ready,
    Disconnected,
    Failed,
    Cancelled,
    Completed,
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IoErrorClass {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidData,
    OutOfSpace,
    Interrupted,
    TimedOut,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurationBucket {
    UnderOneSecond,
    UnderTenSeconds,
    UnderOneMinute,
    UnderTenMinutes,
    Longer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SafeValue {
    Component(Component),
    Operation(Operation),
    State(DiagnosticState),
    ErrorClass(IoErrorClass),
    Count(u64),
    DurationBucket(DurationBucket),
    Boolean(bool),
}

impl SafeValue {
    fn matches(self, field: SafeField) -> bool {
        matches!(
            (field, self),
            (SafeField::Component, Self::Component(_))
                | (SafeField::Operation, Self::Operation(_))
                | (SafeField::State, Self::State(_))
                | (SafeField::ErrorClass, Self::ErrorClass(_))
                | (
                    SafeField::AttemptCount | SafeField::ItemCount | SafeField::DroppedCount,
                    Self::Count(_)
                )
                | (SafeField::DurationBucket, Self::DurationBucket(_))
                | (SafeField::Enabled, Self::Boolean(_))
        )
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CorrelationId([u8; 16]);

impl CorrelationId {
    #[must_use]
    pub fn random() -> Self {
        let mut bytes = [0_u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn parse(value: &str) -> Result<Self, SchemaError> {
        if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SchemaError::InvalidCorrelationId);
        }
        let mut bytes = [0_u8; 16];
        for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
            let text = std::str::from_utf8(chunk).map_err(|_| SchemaError::InvalidCorrelationId)?;
            bytes[index] =
                u8::from_str_radix(text, 16).map_err(|_| SchemaError::InvalidCorrelationId)?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Debug for CorrelationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CorrelationId(<opaque>)")
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for CorrelationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CorrelationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub schema_version: u16,
    pub occurred_at_unix_ms: u64,
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub user_message_id: DiagnosticMessageId,
    pub recovery: Vec<RecoveryAction>,
    pub correlation_id: CorrelationId,
    pub safe_context: BTreeMap<SafeField, SafeValue>,
}

impl Diagnostic {
    #[must_use]
    pub fn new(
        occurred_at_unix_ms: u64,
        code: DiagnosticCode,
        severity: Severity,
        user_message_id: DiagnosticMessageId,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            occurred_at_unix_ms,
            code,
            severity,
            user_message_id,
            recovery: Vec::new(),
            correlation_id: CorrelationId::random(),
            safe_context: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_recovery(mut self, recovery: impl IntoIterator<Item = RecoveryAction>) -> Self {
        self.recovery = recovery.into_iter().take(4).collect();
        self
    }

    pub fn insert(&mut self, field: SafeField, value: SafeValue) -> Result<(), SchemaError> {
        if !value.matches(field) {
            return Err(SchemaError::FieldTypeMismatch);
        }
        if self.safe_context.len() >= MAX_SAFE_FIELDS && !self.safe_context.contains_key(&field) {
            return Err(SchemaError::TooManyFields);
        }
        self.safe_context.insert(field, value);
        Ok(())
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(SchemaError::UnsupportedVersion);
        }
        if self.recovery.len() > 4 || self.safe_context.len() > MAX_SAFE_FIELDS {
            return Err(SchemaError::TooManyFields);
        }
        if self
            .safe_context
            .iter()
            .any(|(field, value)| !value.matches(*field))
        {
            return Err(SchemaError::FieldTypeMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaError {
    UnsupportedVersion,
    InvalidCorrelationId,
    FieldTypeMismatch,
    TooManyFields,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("diagnostic did not match the safe schema")
    }
}

impl std::error::Error for SchemaError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_field_types_are_closed_and_checked() {
        let mut diagnostic = Diagnostic::new(
            10,
            DiagnosticCode::AppStarted,
            Severity::Info,
            DiagnosticMessageId::AppLifecycle,
        );
        assert!(
            diagnostic
                .insert(
                    SafeField::Component,
                    SafeValue::Component(Component::Application)
                )
                .is_ok()
        );
        assert_eq!(
            diagnostic.insert(SafeField::Component, SafeValue::Count(1)),
            Err(SchemaError::FieldTypeMismatch)
        );
    }

    #[test]
    fn unknown_json_fields_and_variants_are_rejected() {
        let diagnostic = Diagnostic::new(
            10,
            DiagnosticCode::AppStarted,
            Severity::Info,
            DiagnosticMessageId::AppLifecycle,
        );
        let mut value = serde_json::to_value(diagnostic).unwrap();
        value["terminal_output"] = serde_json::Value::String("secret".into());
        assert!(serde_json::from_value::<Diagnostic>(value).is_err());
    }

    #[test]
    fn correlation_ids_are_opaque_in_debug_output() {
        let id = CorrelationId::random();
        assert_eq!(format!("{id:?}"), "CorrelationId(<opaque>)");
        assert_eq!(CorrelationId::parse(&id.to_string()).unwrap(), id);
    }
}
