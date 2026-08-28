#![forbid(unsafe_code)]

mod export;
mod runtime;
mod schema;
mod storage;

pub use export::{
    DiagnosticBundle, DiagnosticBundleFile, DiagnosticExportManifest, ExportCancellation,
    ExportError, ExportErrorCode, ExportFileManifest, PreparedExport,
};
pub use runtime::{DiagnosticHandle, DiagnosticRuntime, DiagnosticStatus};
pub use schema::{
    Component, CorrelationId, Diagnostic, DiagnosticCode, DiagnosticMessageId, DiagnosticState,
    DurationBucket, IoErrorClass, Operation, RecoveryAction, SafeField, SafeValue, Severity,
};
pub use storage::{DiagnosticPolicy, DiagnosticUsage};

pub const SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const DEFAULT_MAX_FILES: u8 = 5;
pub const DEFAULT_RETENTION_DAYS: u8 = 14;
pub const MAX_BUNDLE_BYTES: u64 = 50 * 1024 * 1024;
