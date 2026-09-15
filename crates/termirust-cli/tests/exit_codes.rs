use std::io::{self, Write};
use std::process::Command;

use termirust_cli::{
    Cancellation, CliCommand, CliData, CliError, CommandService, ErrorCode, run, write_output,
};

struct FailingService {
    error: CliError,
    calls: usize,
}

impl CommandService for FailingService {
    fn execute(
        &mut self,
        _command: CliCommand,
        _cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        self.calls += 1;
        Err(self.error.clone())
    }
}

#[test]
fn exit_codes_are_stable_and_json_errors_are_complete() {
    for (code, expected) in [
        (ErrorCode::Usage, 2),
        (ErrorCode::Validation, 2),
        (ErrorCode::Unavailable, 3),
        (ErrorCode::Incompatible, 3),
        (ErrorCode::PermissionDenied, 4),
        (ErrorCode::InteractionRequired, 4),
        (ErrorCode::HostKeyUnknown, 4),
        (ErrorCode::HostKeyChanged, 4),
        (ErrorCode::AuthenticationDenied, 4),
        (ErrorCode::BridgeUnavailable, 3),
        (ErrorCode::Conflict, 5),
        (ErrorCode::ResourceLimit, 6),
        (ErrorCode::Timeout, 7),
        (ErrorCode::OperationFailed, 7),
        (ErrorCode::UnknownCompletion, 7),
        (ErrorCode::Cancelled, 130),
    ] {
        let mut service = FailingService {
            error: CliError::new(code, "safe message", "safe hint"),
            calls: 0,
        };
        let output = run(
            &mut service,
            vec!["--json".into(), "status".into()],
            80,
            &Cancellation::default(),
        );
        assert_eq!(output.exit_code, expected);
        assert!(output.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], code.as_str());
        assert_eq!(service.calls, 1);
    }
}

#[test]
fn invalid_arguments_fail_before_service_io() {
    let mut service = FailingService {
        error: CliError::new(ErrorCode::OperationFailed, "called", "called"),
        calls: 0,
    };
    let output = run(
        &mut service,
        vec!["session".into(), "show".into(), "not-a-uuid".into()],
        80,
        &Cancellation::default(),
    );
    assert_eq!(output.exit_code, 2);
    assert_eq!(service.calls, 0);
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("error[usage]")
    );
}

struct BrokenWriter;

impl Write for BrokenWriter {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn broken_stdout_is_reported_after_the_response_is_fully_buffered() {
    let output = termirust_cli::RunOutput {
        stdout: b"complete response\n".to_vec(),
        stderr: Vec::new(),
        exit_code: 0,
    };
    assert_eq!(
        write_output(&output, &mut BrokenWriter, &mut Vec::new())
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[test]
fn executable_help_and_invalid_input_do_not_require_or_create_a_store() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("does-not-exist");
    let binary = env!("CARGO_BIN_EXE_termirust-cli");

    let help = Command::new(binary)
        .args(["--help", "--json"])
        .env("TERMIRUST_CONFIG_DIR", &missing)
        .output()
        .unwrap();
    assert!(help.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&help.stdout).unwrap()["ok"],
        true
    );
    assert!(!missing.exists());

    let invalid = Command::new(binary)
        .args(["session", "show", "invalid", "--json"])
        .env("TERMIRUST_CONFIG_DIR", &missing)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&invalid.stdout).unwrap()["error"]["code"],
        "usage"
    );
    assert!(!missing.exists());
}
