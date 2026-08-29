use std::fs::{self, File};
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde::Deserialize;
use termirust_domain::{
    ConversationHandle, HostedSessionId, PermissionPolicy, ProjectId, ResumeCandidate, ResumeError,
    ResumePlan,
};
use termirust_session_host::process_observation::fingerprint_executable;

const CODEX_VERSION: &str = "0.150.1";
const MAX_METADATA_LINE_BYTES: usize = 64 * 1024;
const MAX_SCANNED_METADATA_BYTES: usize = 8 * 1024 * 1024;
const MAX_SESSION_ROOT_ENTRIES: usize = 4096;
const MAX_SESSION_ROOT_DEPTH: usize = 4;
const DEFAULT_VALIDATION_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct ResumeValidationCancellation(Arc<AtomicBool>);

impl ResumeValidationCancellation {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl Default for ResumeValidationCancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CodexResumeLimits {
    pub max_entries: usize,
    pub max_metadata_bytes: usize,
    pub max_depth: usize,
    pub deadline: Duration,
}

impl Default for CodexResumeLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_SESSION_ROOT_ENTRIES,
            max_metadata_bytes: MAX_SCANNED_METADATA_BYTES,
            max_depth: MAX_SESSION_ROOT_DEPTH,
            deadline: DEFAULT_VALIDATION_DEADLINE,
        }
    }
}

#[derive(Deserialize)]
struct CodexMetadataRecord {
    #[serde(rename = "type")]
    kind: String,
    payload: CodexMetadataPayload,
}

#[derive(Deserialize)]
struct CodexMetadataPayload {
    id: Option<String>,
    session_id: Option<String>,
    cli_version: String,
    cwd: PathBuf,
}

struct ScanState<'a> {
    handle: &'a ConversationHandle,
    expected_working_directory: &'a Path,
    limits: CodexResumeLimits,
    cancel: &'a ResumeValidationCancellation,
    started: Instant,
    entries: usize,
    metadata_bytes: usize,
    match_count: usize,
}

pub fn build_codex_resume_plan(
    candidate: ResumeCandidate,
    conversation_root: &Path,
    canonical_project: ProjectId,
    expected_working_directory: &Path,
    permission_policy: PermissionPolicy,
    executable: &Path,
    replacement_session_id: HostedSessionId,
    cancel: &ResumeValidationCancellation,
) -> Result<ResumePlan, ResumeError> {
    build_codex_resume_plan_with_limits(
        candidate,
        conversation_root,
        canonical_project,
        expected_working_directory,
        permission_policy,
        executable,
        replacement_session_id,
        cancel,
        CodexResumeLimits::default(),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_codex_resume_plan_with_limits(
    candidate: ResumeCandidate,
    conversation_root: &Path,
    canonical_project: ProjectId,
    expected_working_directory: &Path,
    permission_policy: PermissionPolicy,
    executable: &Path,
    replacement_session_id: HostedSessionId,
    cancel: &ResumeValidationCancellation,
    limits: CodexResumeLimits,
) -> Result<ResumePlan, ResumeError> {
    check_cancel(cancel)?;
    let root = canonical_regular_directory(conversation_root)?;
    let working_directory = canonical_regular_directory(expected_working_directory)?;
    let executable = canonical_regular_executable(executable)?;
    let fingerprint = fingerprint_executable(&executable).map_err(map_io)?;
    if fingerprint != candidate.expected_executable_fingerprint {
        return Err(ResumeError::ProviderUnavailable);
    }
    let mut scan = ScanState {
        handle: &candidate.handle,
        expected_working_directory: &working_directory,
        limits,
        cancel,
        started: Instant::now(),
        entries: 0,
        metadata_bytes: 0,
        match_count: 0,
    };
    scan_directory(&root, 0, &mut scan)?;
    match scan.match_count {
        0 => return Err(ResumeError::ConversationMissing),
        1 => {}
        _ => return Err(ResumeError::ConversationMalformed),
    }
    let mut arguments = vec![
        "resume".to_string(),
        "--cd".to_string(),
        working_directory.to_string_lossy().into_owned(),
    ];
    match permission_policy {
        PermissionPolicy::AskAsNeeded => {}
        PermissionPolicy::ReadOnly => {
            arguments.extend(["--sandbox".to_string(), "read-only".to_string()]);
        }
        PermissionPolicy::WorkspaceWrite => {
            arguments.extend(["--sandbox".to_string(), "workspace-write".to_string()]);
        }
    }
    arguments.push(candidate.handle.expose_to_provider().to_string());
    if arguments.len() > termirust_domain::MAX_RESUME_ARGUMENTS
        || arguments.iter().any(|argument| {
            argument.len() > termirust_domain::MAX_RESUME_ARGUMENT_BYTES || argument.contains('\0')
        })
    {
        return Err(ResumeError::ResourceLimit);
    }
    Ok(ResumePlan {
        candidate,
        replacement_session_id,
        canonical_project,
        working_directory,
        permission_policy,
        executable,
        arguments,
        safe_conversation_label: "Codex conversation".to_string(),
    })
}

fn scan_directory(path: &Path, depth: usize, state: &mut ScanState<'_>) -> Result<(), ResumeError> {
    check_budget(state)?;
    if depth > state.limits.max_depth {
        return Err(ResumeError::ResourceLimit);
    }
    let entries = fs::read_dir(path).map_err(map_io)?;
    for entry in entries {
        check_budget(state)?;
        state.entries = state.entries.saturating_add(1);
        if state.entries > state.limits.max_entries {
            return Err(ResumeError::ResourceLimit);
        }
        let entry = entry.map_err(map_io)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(map_io)?;
        if metadata.file_type().is_symlink() {
            return Err(ResumeError::ConversationMalformed);
        }
        if metadata.is_dir() {
            scan_directory(&entry.path(), depth.saturating_add(1), state)?;
        } else if metadata.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            inspect_metadata(&entry.path(), state)?;
        }
    }
    Ok(())
}

fn inspect_metadata(path: &Path, state: &mut ScanState<'_>) -> Result<(), ResumeError> {
    let file = File::open(path).map_err(map_io)?;
    let mut line = Vec::new();
    BufReader::new(file)
        .take((MAX_METADATA_LINE_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)
        .map_err(map_io)?;
    if line.len() > MAX_METADATA_LINE_BYTES {
        return Err(ResumeError::ResourceLimit);
    }
    state.metadata_bytes = state.metadata_bytes.saturating_add(line.len());
    if state.metadata_bytes > state.limits.max_metadata_bytes {
        return Err(ResumeError::ResourceLimit);
    }
    let Ok(record) = serde_json::from_slice::<CodexMetadataRecord>(&line) else {
        return Ok(());
    };
    if record.kind != "session_meta" {
        return Ok(());
    }
    let handle = state.handle.expose_to_provider();
    let matches = record.payload.id.as_deref() == Some(handle)
        || record.payload.session_id.as_deref() == Some(handle);
    if !matches {
        return Ok(());
    }
    if record.payload.cli_version != CODEX_VERSION {
        return Err(ResumeError::UnsupportedVersion);
    }
    let cwd = canonical_regular_directory(&record.payload.cwd)?;
    if cwd != state.expected_working_directory {
        return Err(ResumeError::ConversationMalformed);
    }
    state.match_count = state.match_count.saturating_add(1);
    Ok(())
}

fn canonical_regular_directory(path: &Path) -> Result<PathBuf, ResumeError> {
    let metadata = fs::symlink_metadata(path).map_err(map_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ResumeError::ConversationMalformed);
    }
    fs::canonicalize(path).map_err(map_io)
}

fn canonical_regular_executable(path: &Path) -> Result<PathBuf, ResumeError> {
    let canonical = fs::canonicalize(path).map_err(map_io)?;
    let metadata = fs::symlink_metadata(&canonical).map_err(map_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ResumeError::ProviderUnavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(ResumeError::PermissionDenied);
        }
    }
    Ok(canonical)
}

fn check_budget(state: &ScanState<'_>) -> Result<(), ResumeError> {
    check_cancel(state.cancel)?;
    if state.started.elapsed() > state.limits.deadline {
        return Err(ResumeError::ResourceLimit);
    }
    Ok(())
}

fn check_cancel(cancel: &ResumeValidationCancellation) -> Result<(), ResumeError> {
    if cancel.is_cancelled() {
        Err(ResumeError::Cancelled)
    } else {
        Ok(())
    }
}

fn map_io(error: std::io::Error) -> ResumeError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ResumeError::ConversationMissing,
        std::io::ErrorKind::PermissionDenied => ResumeError::PermissionDenied,
        _ => ResumeError::ConversationMalformed,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use termirust_domain::{
        CommandId, ExecutableFingerprint, OccupantGeneration, ResumeRequest, RuntimeId,
        RuntimeVersion,
    };

    use super::*;

    fn executable(root: &Path) -> (PathBuf, ExecutableFingerprint) {
        let path = root.join("codex");
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let fingerprint = fingerprint_executable(&path).unwrap();
        (path, fingerprint)
    }

    fn candidate(fingerprint: ExecutableFingerprint) -> ResumeCandidate {
        ResumeCandidate {
            request: ResumeRequest {
                command_id: CommandId::new(),
                session_id: HostedSessionId::new(),
                expected_generation: OccupantGeneration::new(1),
                expected_revision: termirust_domain::Revision::new(1),
            },
            runtime_id: RuntimeId::new("codex").unwrap(),
            runtime_version: RuntimeVersion::new(0, 150, 1),
            prior_generation: OccupantGeneration::new(1),
            expected_executable_fingerprint: fingerprint,
            handle: ConversationHandle::codex("019cf76d-0493-77d1-8572-3fb4ac801ac8").unwrap(),
        }
    }

    fn write_metadata(root: &Path, cwd: &Path, handle: &str, version: &str) -> PathBuf {
        let directory = root.join("2026/08/29");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rollout.jsonl");
        let value = serde_json::json!({
            "ordinal": 0,
            "timestamp": "2026-08-29T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": handle,
                "session_id": handle,
                "cli_version": version,
                "cwd": cwd,
                "ignored_content": BTreeMap::<String, String>::new()
            }
        });
        fs::write(&path, format!("{value}\n")).unwrap();
        path
    }

    fn build(
        fixture: &tempfile::TempDir,
        limits: CodexResumeLimits,
        cancel: &ResumeValidationCancellation,
    ) -> Result<ResumePlan, ResumeError> {
        let root = fixture.path().join("sessions");
        let cwd = fixture.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let (executable, fingerprint) = executable(fixture.path());
        write_metadata(
            &root,
            &cwd,
            "019cf76d-0493-77d1-8572-3fb4ac801ac8",
            CODEX_VERSION,
        );
        build_codex_resume_plan_with_limits(
            candidate(fingerprint),
            &root,
            ProjectId::new(),
            &cwd,
            PermissionPolicy::ReadOnly,
            &executable,
            HostedSessionId::new(),
            cancel,
            limits,
        )
    }

    #[test]
    fn exact_contained_metadata_builds_literal_redacted_plan() {
        let fixture = tempfile::tempdir().unwrap();
        let plan = build(
            &fixture,
            CodexResumeLimits::default(),
            &ResumeValidationCancellation::new(),
        )
        .unwrap();
        assert_eq!(plan.arguments[0], "resume");
        assert!(
            plan.arguments
                .iter()
                .any(|argument| argument == "read-only")
        );
        assert_eq!(
            plan.arguments.last().unwrap(),
            "019cf76d-0493-77d1-8572-3fb4ac801ac8"
        );
        let debug = format!("{plan:?}");
        assert!(!debug.contains("019cf76d"));
        assert!(!debug.contains(fixture.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn cancellation_limits_symlink_duplicate_version_cwd_and_fingerprint_fail_closed() {
        let cancelled_fixture = tempfile::tempdir().unwrap();
        let cancel = ResumeValidationCancellation::new();
        cancel.cancel();
        assert_eq!(
            build(&cancelled_fixture, CodexResumeLimits::default(), &cancel),
            Err(ResumeError::Cancelled)
        );

        let limited_fixture = tempfile::tempdir().unwrap();
        assert_eq!(
            build(
                &limited_fixture,
                CodexResumeLimits {
                    max_entries: 1,
                    ..CodexResumeLimits::default()
                },
                &ResumeValidationCancellation::new(),
            ),
            Err(ResumeError::ResourceLimit)
        );

        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("sessions");
        let cwd = fixture.path().join("project");
        fs::create_dir_all(root.join("2026/08/29")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let (executable, fingerprint) = executable(fixture.path());
        let source = write_metadata(
            &root,
            &cwd,
            "019cf76d-0493-77d1-8572-3fb4ac801ac8",
            CODEX_VERSION,
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(&source, root.join("2026/08/29/link.jsonl")).unwrap();
        let result = build_codex_resume_plan(
            candidate(fingerprint),
            &root,
            ProjectId::new(),
            &cwd,
            PermissionPolicy::AskAsNeeded,
            &executable,
            HostedSessionId::new(),
            &ResumeValidationCancellation::new(),
        );
        #[cfg(unix)]
        assert_eq!(result, Err(ResumeError::ConversationMalformed));

        fs::remove_file(root.join("2026/08/29/link.jsonl")).ok();
        fs::copy(&source, root.join("2026/08/29/duplicate.jsonl")).unwrap();
        assert_eq!(
            build_codex_resume_plan(
                candidate(fingerprint),
                &root,
                ProjectId::new(),
                &cwd,
                PermissionPolicy::AskAsNeeded,
                &executable,
                HostedSessionId::new(),
                &ResumeValidationCancellation::new(),
            ),
            Err(ResumeError::ConversationMalformed)
        );

        fs::write(&executable, "#!/bin/sh\nexit 1\n").unwrap();
        assert_eq!(
            build_codex_resume_plan(
                candidate(fingerprint),
                &root,
                ProjectId::new(),
                &cwd,
                PermissionPolicy::AskAsNeeded,
                &executable,
                HostedSessionId::new(),
                &ResumeValidationCancellation::new(),
            ),
            Err(ResumeError::ProviderUnavailable)
        );
    }
}
