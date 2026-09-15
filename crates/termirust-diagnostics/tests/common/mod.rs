use std::path::Path;

use termirust_diagnostics::{
    Component, Diagnostic, DiagnosticCode, DiagnosticMessageId, DiagnosticRuntime, Operation,
    SafeField, SafeValue, Severity,
};

pub fn runtime(root: &Path, policy: termirust_diagnostics::DiagnosticPolicy) -> DiagnosticRuntime {
    DiagnosticRuntime::start(root, policy).expect("start diagnostic runtime")
}

pub fn safe_diagnostic(timestamp: u64, code: DiagnosticCode) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        timestamp,
        code,
        Severity::Info,
        DiagnosticMessageId::AppLifecycle,
    );
    diagnostic
        .insert(
            SafeField::Component,
            SafeValue::Component(Component::Application),
        )
        .unwrap();
    diagnostic
        .insert(SafeField::Operation, SafeValue::Operation(Operation::Start))
        .unwrap();
    diagnostic
}
