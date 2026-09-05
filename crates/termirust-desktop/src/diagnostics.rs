use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use termirust_diagnostics::{
    Component, Diagnostic, DiagnosticCode, DiagnosticHandle, DiagnosticMessageId, DiagnosticPolicy,
    DiagnosticRuntime, DiagnosticState, DiagnosticStatus, DiagnosticUsage, ExportCancellation,
    ExportError, Operation, PreparedExport, SafeField, SafeValue, Severity,
};

use crate::models::AppSettings;

static HANDLE: OnceLock<DiagnosticHandle> = OnceLock::new();
static INITIALIZATION_FAILED: AtomicBool = AtomicBool::new(false);

pub fn policy_from_settings(settings: &AppSettings) -> DiagnosticPolicy {
    DiagnosticPolicy {
        enabled: settings.diagnostics_enabled,
        max_file_bytes: u64::from(settings.diagnostics_max_file_mib) * 1024 * 1024,
        max_files: termirust_diagnostics::DEFAULT_MAX_FILES,
        retention_days: settings.diagnostics_retention_days,
        ..DiagnosticPolicy::default()
    }
    .normalized()
}

pub fn initialize(root: PathBuf, settings: &AppSettings) -> Option<DiagnosticRuntime> {
    match DiagnosticRuntime::start(root, policy_from_settings(settings)) {
        Ok(runtime) => {
            if HANDLE.set(runtime.handle()).is_err() {
                INITIALIZATION_FAILED.store(true, Ordering::Release);
                return None;
            }
            Some(runtime)
        }
        Err(_) => {
            INITIALIZATION_FAILED.store(true, Ordering::Release);
            None
        }
    }
}

pub fn record(
    code: DiagnosticCode,
    severity: Severity,
    message: DiagnosticMessageId,
    component: Component,
    operation: Operation,
) {
    let Some(handle) = HANDLE.get() else {
        return;
    };
    let mut diagnostic = Diagnostic::new(now_ms(), code, severity, message);
    let _ = diagnostic.insert(SafeField::Component, SafeValue::Component(component));
    let _ = diagnostic.insert(SafeField::Operation, SafeValue::Operation(operation));
    let _ = handle.record(diagnostic);
}

pub fn record_state(
    code: DiagnosticCode,
    severity: Severity,
    message: DiagnosticMessageId,
    component: Component,
    operation: Operation,
    state: DiagnosticState,
) {
    let Some(handle) = HANDLE.get() else {
        return;
    };
    let mut diagnostic = Diagnostic::new(now_ms(), code, severity, message);
    let _ = diagnostic.insert(SafeField::Component, SafeValue::Component(component));
    let _ = diagnostic.insert(SafeField::Operation, SafeValue::Operation(operation));
    let _ = diagnostic.insert(SafeField::State, SafeValue::State(state));
    let _ = handle.record(diagnostic);
}

pub fn status() -> DiagnosticStatus {
    HANDLE.get().map_or_else(
        || {
            if INITIALIZATION_FAILED.load(Ordering::Acquire) {
                DiagnosticStatus::DiskError
            } else {
                DiagnosticStatus::Disabled
            }
        },
        DiagnosticHandle::status,
    )
}

pub fn usage() -> Result<DiagnosticUsage, ExportError> {
    HANDLE
        .get()
        .ok_or_else(ExportError::runtime_unavailable_for_app)?
        .usage()
}

pub fn apply_settings(settings: &AppSettings) -> Result<(), ExportError> {
    HANDLE
        .get()
        .ok_or_else(ExportError::runtime_unavailable_for_app)?
        .set_policy(policy_from_settings(settings))
}

pub fn clear() -> Result<(), ExportError> {
    HANDLE
        .get()
        .ok_or_else(ExportError::runtime_unavailable_for_app)?
        .clear()
}

pub fn prepare_export_with_cancellation(
    cancellation: &ExportCancellation,
) -> Result<PreparedExport, ExportError> {
    HANDLE
        .get()
        .ok_or_else(ExportError::runtime_unavailable_for_app)?
        .prepare_export_with_cancellation(cancellation)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}
