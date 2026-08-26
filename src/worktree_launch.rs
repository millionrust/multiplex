use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use termirust_domain::{
    BaseCandidate, BaseSource, CanonicalPath, CommitOid, GitReference, ManagedPath,
    ManagedWorktreeId, ProjectId, WorktreeError, WorktreeLaunchDraft, WorktreePlan,
};

const MAX_GIT_OUTPUT_BYTES: usize = 256 * 1024;
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Default)]
pub struct WorktreeCancellation(Arc<AtomicBool>);

impl WorktreeCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl fmt::Debug for WorktreeCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorktreeCancellation")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[derive(Clone)]
pub struct GitRunner {
    executable: OsString,
    inspect_timeout: Duration,
    fetch_timeout: Duration,
    output_limit: usize,
}

impl Default for GitRunner {
    fn default() -> Self {
        Self {
            executable: OsString::from("git"),
            inspect_timeout: INSPECT_TIMEOUT,
            fetch_timeout: FETCH_TIMEOUT,
            output_limit: MAX_GIT_OUTPUT_BYTES,
        }
    }
}

impl fmt::Debug for GitRunner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitRunner")
            .field("executable", &"<redacted>")
            .field("inspect_timeout", &self.inspect_timeout)
            .field("fetch_timeout", &self.fetch_timeout)
            .field("output_limit", &self.output_limit)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeInspection {
    pub plan: WorktreePlan,
    pub repository_basename: String,
    pub fetched: bool,
    pub current_branch_fallback: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitWorktreeRecord {
    path: PathBuf,
    head: CommitOid,
    branch: Option<GitReference>,
    detached: bool,
}

#[derive(Debug)]
struct GitOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
}

impl GitRunner {
    pub fn inspect(
        &self,
        project_root: &Path,
        managed_root: &Path,
        worktree_id: ManagedWorktreeId,
        child_project_id: ProjectId,
        draft: &WorktreeLaunchDraft,
        cancellation: &WorktreeCancellation,
    ) -> Result<WorktreeInspection, WorktreeError> {
        cancellation_check(cancellation)?;
        let repository_output = self.required(
            project_root,
            ["rev-parse", "--path-format=absolute", "--show-toplevel"],
            self.inspect_timeout,
            cancellation,
            "repository-root",
        )?;
        let repository_path = parse_single_path(&repository_output.stdout)?;
        let repository_root = CanonicalPath::resolve(&repository_path)
            .map_err(|_| WorktreeError::InvalidRepository)?;
        if repository_root.as_path().join(".gitmodules").exists() {
            return Err(WorktreeError::SubmodulesUnsupported);
        }

        let status = self.required(
            repository_root.as_path(),
            ["status", "--porcelain=v2", "-z"],
            self.inspect_timeout,
            cancellation,
            "status",
        )?;
        if !status.stdout.is_empty() {
            return Err(WorktreeError::DirtySource);
        }

        fs::create_dir_all(managed_root)
            .map_err(|error| filesystem_error(error, WorktreeError::InvalidPath))?;
        let managed_root =
            CanonicalPath::resolve(managed_root).map_err(|_| WorktreeError::InvalidPath)?;
        if managed_root
            .as_path()
            .starts_with(repository_root.as_path())
        {
            return Err(WorktreeError::Containment);
        }

        if draft.fetch {
            self.required(
                repository_root.as_path(),
                ["fetch", "--prune", "--no-tags"],
                self.fetch_timeout,
                cancellation,
                "fetch",
            )?;
        }

        self.required_os(
            repository_root.as_path(),
            vec![
                OsString::from("check-ref-format"),
                OsString::from("--branch"),
                OsString::from(draft.branch.as_str()),
            ],
            self.inspect_timeout,
            cancellation,
            "invalid-branch",
        )?;

        let (selected_base, current_branch_fallback) = self.select_base(
            repository_root.as_path(),
            draft.requested_base.as_ref(),
            draft.fetch,
            draft.confirm_current_branch,
            cancellation,
        )?;
        let repository_basename = repository_root
            .as_path()
            .file_name()
            .and_then(OsStr::to_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("repository")
            .chars()
            .take(256)
            .collect::<String>();
        let directory_name = format!("wt-{}", short_id(worktree_id));
        let parent = managed_root.as_path().join(path_slug(&repository_basename));
        fs::create_dir_all(&parent)
            .map_err(|error| filesystem_error(error, WorktreeError::InvalidPath))?;
        let canonical_parent = fs::canonicalize(&parent)
            .map_err(|error| filesystem_error(error, WorktreeError::InvalidPath))?;
        if !canonical_parent.starts_with(managed_root.as_path()) {
            return Err(WorktreeError::Containment);
        }
        let managed_path = ManagedPath::new(canonical_parent.join(directory_name))?;
        if fs::symlink_metadata(managed_path.as_path()).is_ok() {
            return Err(WorktreeError::PathCollision);
        }

        let branch_ref = format!("refs/heads/{}", draft.branch.as_str());
        if self
            .optional_os(
                repository_root.as_path(),
                vec![
                    OsString::from("show-ref"),
                    OsString::from("--verify"),
                    OsString::from("--quiet"),
                    OsString::from(branch_ref),
                ],
                self.inspect_timeout,
                cancellation,
            )?
            .status
            .success()
        {
            return Err(WorktreeError::BranchCollision);
        }

        if self
            .list_worktrees(repository_root.as_path(), cancellation)?
            .iter()
            .any(|record| record.path == managed_path.as_path())
        {
            return Err(WorktreeError::PathCollision);
        }

        let plan = WorktreePlan::new(
            worktree_id,
            draft.source_project_id,
            child_project_id,
            repository_root,
            managed_root,
            selected_base,
            draft.branch.clone(),
            managed_path,
        )?;
        Ok(WorktreeInspection {
            plan,
            repository_basename,
            fetched: draft.fetch,
            current_branch_fallback,
        })
    }

    pub fn create(
        &self,
        plan: &WorktreePlan,
        cancellation: &WorktreeCancellation,
    ) -> Result<(), WorktreeError> {
        plan.validate()?;
        cancellation_check(cancellation)?;
        if plan.repository_root.status() != termirust_domain::ProjectStatus::Available
            || plan.managed_root.status() != termirust_domain::ProjectStatus::Available
        {
            return Err(WorktreeError::SymlinkSwap);
        }
        if fs::symlink_metadata(plan.managed_path.as_path()).is_ok() {
            return Err(WorktreeError::PathCollision);
        }
        self.required_os(
            plan.repository_root.as_path(),
            vec![
                OsString::from("worktree"),
                OsString::from("add"),
                OsString::from("-b"),
                OsString::from(plan.generated_branch.as_str()),
                plan.managed_path.as_path().as_os_str().to_os_string(),
                OsString::from(plan.selected_base.commit_oid.as_str()),
            ],
            self.inspect_timeout,
            cancellation,
            "worktree-add",
        )?;
        Ok(())
    }

    pub fn verify(
        &self,
        plan: &WorktreePlan,
        cancellation: &WorktreeCancellation,
    ) -> Result<(), WorktreeError> {
        plan.validate()?;
        cancellation_check(cancellation)?;
        let canonical = fs::canonicalize(plan.managed_path.as_path())
            .map_err(|_| WorktreeError::VerificationMismatch)?;
        if canonical != plan.managed_path.as_path()
            || !canonical.starts_with(plan.managed_root.as_path())
        {
            return Err(WorktreeError::SymlinkSwap);
        }
        let expected_branch = format!("refs/heads/{}", plan.generated_branch.as_str());
        let record = self
            .list_worktrees(plan.repository_root.as_path(), cancellation)?
            .into_iter()
            .find(|record| record.path == canonical)
            .ok_or(WorktreeError::VerificationMismatch)?;
        if record.detached
            || record.head != plan.selected_base.commit_oid
            || record.branch.as_ref().map(GitReference::as_str) != Some(expected_branch.as_str())
        {
            return Err(WorktreeError::VerificationMismatch);
        }
        let head = self.required(
            &canonical,
            ["rev-parse", "--verify", "HEAD"],
            self.inspect_timeout,
            cancellation,
            "verify-head",
        )?;
        if parse_oid(&head.stdout)? != plan.selected_base.commit_oid {
            return Err(WorktreeError::VerificationMismatch);
        }
        Ok(())
    }

    fn list_worktrees(
        &self,
        repository_root: &Path,
        cancellation: &WorktreeCancellation,
    ) -> Result<Vec<GitWorktreeRecord>, WorktreeError> {
        let nul = self.optional(
            repository_root,
            ["worktree", "list", "--porcelain", "-z"],
            self.inspect_timeout,
            cancellation,
        )?;
        if nul.status.success() {
            return parse_worktree_porcelain_z(&nul.stdout);
        }
        let legacy = self.required(
            repository_root,
            ["worktree", "list", "--porcelain"],
            self.inspect_timeout,
            cancellation,
            "worktree-list",
        )?;
        parse_worktree_porcelain(&legacy.stdout)
    }

    fn select_base(
        &self,
        repository_root: &Path,
        requested: Option<&GitReference>,
        fetched: bool,
        confirm_current: bool,
        cancellation: &WorktreeCancellation,
    ) -> Result<(BaseCandidate, bool), WorktreeError> {
        if let Some(reference) = requested {
            return self
                .resolve_base(
                    repository_root,
                    reference.clone(),
                    BaseSource::UserSelected,
                    cancellation,
                )
                .map(|base| (base, false));
        }

        let configured = self.optional(
            repository_root,
            ["config", "--get", "termirust.mainline"],
            self.inspect_timeout,
            cancellation,
        )?;
        let configured = configured
            .status
            .success()
            .then(|| parse_single_line(&configured.stdout))
            .transpose()?
            .and_then(|value| GitReference::new(&value).ok());
        let mut local_candidates = configured.into_iter().collect::<Vec<_>>();
        for fallback in ["main", "master"] {
            let candidate = GitReference::new(fallback)?;
            if !local_candidates.contains(&candidate) {
                local_candidates.push(candidate);
            }
        }
        for candidate in local_candidates {
            if let Ok(base) = self.resolve_base(
                repository_root,
                candidate,
                BaseSource::ConfiguredMainline,
                cancellation,
            ) {
                return Ok((base, false));
            }
        }

        if fetched {
            let remote = self.optional(
                repository_root,
                ["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
                self.inspect_timeout,
                cancellation,
            )?;
            if remote.status.success() {
                let reference = GitReference::new(&parse_single_line(&remote.stdout)?)?;
                let base = self.resolve_base(
                    repository_root,
                    reference,
                    BaseSource::FetchedRemoteMainline,
                    cancellation,
                )?;
                return Ok((base, false));
            }
        }

        if confirm_current {
            let current = self.optional(
                repository_root,
                ["symbolic-ref", "--short", "HEAD"],
                self.inspect_timeout,
                cancellation,
            )?;
            if !current.status.success() {
                return Err(WorktreeError::DetachedHead);
            }
            let reference = GitReference::new(&parse_single_line(&current.stdout)?)?;
            let base = self.resolve_base(
                repository_root,
                reference,
                BaseSource::CurrentBranchConfirmed,
                cancellation,
            )?;
            return Ok((base, true));
        }
        Err(WorktreeError::NoBase)
    }

    fn resolve_base(
        &self,
        repository_root: &Path,
        reference: GitReference,
        source: BaseSource,
        cancellation: &WorktreeCancellation,
    ) -> Result<BaseCandidate, WorktreeError> {
        let expression = format!("{}^{{commit}}", reference.as_str());
        let output = self.required_os(
            repository_root,
            vec![
                OsString::from("rev-parse"),
                OsString::from("--verify"),
                OsString::from("--end-of-options"),
                OsString::from(expression),
            ],
            self.inspect_timeout,
            cancellation,
            "invalid-base",
        )?;
        Ok(BaseCandidate {
            ref_name: reference,
            commit_oid: parse_oid(&output.stdout)?,
            source,
        })
    }

    fn required<const N: usize>(
        &self,
        cwd: &Path,
        args: [&str; N],
        timeout: Duration,
        cancellation: &WorktreeCancellation,
        code: &'static str,
    ) -> Result<GitOutput, WorktreeError> {
        self.required_os(
            cwd,
            args.into_iter().map(OsString::from).collect(),
            timeout,
            cancellation,
            code,
        )
    }

    fn optional<const N: usize>(
        &self,
        cwd: &Path,
        args: [&str; N],
        timeout: Duration,
        cancellation: &WorktreeCancellation,
    ) -> Result<GitOutput, WorktreeError> {
        self.optional_os(
            cwd,
            args.into_iter().map(OsString::from).collect(),
            timeout,
            cancellation,
        )
    }

    fn required_os(
        &self,
        cwd: &Path,
        args: Vec<OsString>,
        timeout: Duration,
        cancellation: &WorktreeCancellation,
        code: &'static str,
    ) -> Result<GitOutput, WorktreeError> {
        let output = self.optional_os(cwd, args, timeout, cancellation)?;
        if output.status.success() {
            Ok(output)
        } else if code == "fetch" {
            Err(WorktreeError::FetchFailed)
        } else {
            Err(WorktreeError::GitFailed { code })
        }
    }

    fn optional_os(
        &self,
        cwd: &Path,
        args: Vec<OsString>,
        timeout: Duration,
        cancellation: &WorktreeCancellation,
    ) -> Result<GitOutput, WorktreeError> {
        cancellation_check(cancellation)?;
        let mut command = Command::new(&self.executable);
        command
            .current_dir(cwd)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("LC_ALL", "C");
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        let mut child = command.spawn().map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => WorktreeError::GitUnavailable,
            std::io::ErrorKind::PermissionDenied => WorktreeError::PermissionDenied,
            std::io::ErrorKind::StorageFull => WorktreeError::StorageFull,
            _ => WorktreeError::GitFailed { code: "spawn" },
        })?;
        let output = Arc::new(Mutex::new(Vec::new()));
        let exceeded = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicUsize::new(0));
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(read_output(
                stdout,
                output.clone(),
                exceeded.clone(),
                observed.clone(),
                self.output_limit,
                true,
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(read_output(
                stderr,
                output.clone(),
                exceeded.clone(),
                observed,
                self.output_limit,
                false,
            ));
        }
        let started = Instant::now();
        let status = loop {
            if cancellation.is_cancelled() {
                terminate_owned(&mut child);
                join_readers(readers);
                return Err(WorktreeError::Cancelled);
            }
            if exceeded.load(Ordering::Acquire) {
                terminate_owned(&mut child);
                join_readers(readers);
                return Err(WorktreeError::OutputLimit);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= timeout => {
                    terminate_owned(&mut child);
                    join_readers(readers);
                    return Err(WorktreeError::Timeout);
                }
                Ok(None) => thread::sleep(POLL_INTERVAL),
                Err(_) => {
                    terminate_owned(&mut child);
                    join_readers(readers);
                    return Err(WorktreeError::GitFailed { code: "wait" });
                }
            }
        };
        join_readers(readers);
        if exceeded.load(Ordering::Acquire) {
            return Err(WorktreeError::OutputLimit);
        }
        let stdout = output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Ok(GitOutput { status, stdout })
    }
}

fn read_output(
    mut stream: impl Read + Send + 'static,
    output: Arc<Mutex<Vec<u8>>>,
    exceeded: Arc<AtomicBool>,
    observed: Arc<AtomicUsize>,
    limit: usize,
    retain: bool,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let total = observed
                .fetch_add(count, Ordering::AcqRel)
                .saturating_add(count);
            if total > limit {
                exceeded.store(true, Ordering::Release);
            }
            let mut bytes = output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if retain {
                let remaining = limit.saturating_sub(bytes.len());
                bytes.extend_from_slice(&buffer[..count.min(remaining)]);
            }
        }
    })
}

fn join_readers(readers: Vec<thread::JoinHandle<()>>) {
    for reader in readers {
        let _ = reader.join();
    }
}

fn terminate_owned(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn cancellation_check(cancellation: &WorktreeCancellation) -> Result<(), WorktreeError> {
    if cancellation.is_cancelled() {
        Err(WorktreeError::Cancelled)
    } else {
        Ok(())
    }
}

fn filesystem_error(error: std::io::Error, fallback: WorktreeError) -> WorktreeError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => WorktreeError::PermissionDenied,
        std::io::ErrorKind::StorageFull => WorktreeError::StorageFull,
        _ => fallback,
    }
}

fn parse_single_line(bytes: &[u8]) -> Result<String, WorktreeError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| WorktreeError::GitFailed { code: "non-utf8" })?
        .trim();
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return Err(WorktreeError::GitFailed {
            code: "malformed-output",
        });
    }
    Ok(value.to_string())
}

fn parse_single_path(bytes: &[u8]) -> Result<PathBuf, WorktreeError> {
    Ok(PathBuf::from(parse_single_line(bytes)?))
}

fn parse_oid(bytes: &[u8]) -> Result<CommitOid, WorktreeError> {
    CommitOid::new(&parse_single_line(bytes)?)
}

fn parse_worktree_porcelain_z(bytes: &[u8]) -> Result<Vec<GitWorktreeRecord>, WorktreeError> {
    if bytes.len() > MAX_GIT_OUTPUT_BYTES {
        return Err(WorktreeError::OutputLimit);
    }
    let mut records = Vec::new();
    let mut path = None;
    let mut head = None;
    let mut branch = None;
    let mut detached = false;
    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            if let (Some(path), Some(head)) = (path.take(), head.take()) {
                records.push(GitWorktreeRecord {
                    path,
                    head,
                    branch: branch.take(),
                    detached,
                });
            } else if path.is_some() || head.is_some() || branch.is_some() || detached {
                return Err(WorktreeError::GitFailed {
                    code: "malformed-worktree-list",
                });
            }
            detached = false;
            continue;
        }
        let text = std::str::from_utf8(field).map_err(|_| WorktreeError::GitFailed {
            code: "non-utf8-worktree-list",
        })?;
        if let Some(value) = text.strip_prefix("worktree ") {
            if path.replace(PathBuf::from(value)).is_some() {
                return Err(WorktreeError::GitFailed {
                    code: "duplicate-worktree-path",
                });
            }
        } else if let Some(value) = text.strip_prefix("HEAD ") {
            if head.replace(CommitOid::new(value)?).is_some() {
                return Err(WorktreeError::GitFailed {
                    code: "duplicate-worktree-head",
                });
            }
        } else if let Some(value) = text.strip_prefix("branch ") {
            branch = Some(GitReference::new(value)?);
        } else if text == "detached" {
            detached = true;
        }
    }
    if let (Some(path), Some(head)) = (path, head) {
        records.push(GitWorktreeRecord {
            path,
            head,
            branch,
            detached,
        });
    }
    if records.len() > termirust_domain::MAX_WORKTREE_REGISTRATIONS {
        return Err(WorktreeError::ResourceLimit {
            limit: termirust_domain::MAX_WORKTREE_REGISTRATIONS,
        });
    }
    Ok(records)
}

fn parse_worktree_porcelain(bytes: &[u8]) -> Result<Vec<GitWorktreeRecord>, WorktreeError> {
    if bytes.len() > MAX_GIT_OUTPUT_BYTES {
        return Err(WorktreeError::OutputLimit);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| WorktreeError::GitFailed {
        code: "non-utf8-worktree-list",
    })?;
    let mut records = Vec::new();
    for block in text.split("\n\n").filter(|block| !block.trim().is_empty()) {
        let mut path = None;
        let mut head = None;
        let mut branch = None;
        let mut detached = false;
        for field in block.lines() {
            if let Some(value) = field.strip_prefix("worktree ") {
                path = Some(PathBuf::from(value));
            } else if let Some(value) = field.strip_prefix("HEAD ") {
                head = Some(CommitOid::new(value)?);
            } else if let Some(value) = field.strip_prefix("branch ") {
                branch = Some(GitReference::new(value)?);
            } else if field == "detached" {
                detached = true;
            }
        }
        records.push(GitWorktreeRecord {
            path: path.ok_or(WorktreeError::GitFailed {
                code: "malformed-worktree-list",
            })?,
            head: head.ok_or(WorktreeError::GitFailed {
                code: "malformed-worktree-list",
            })?,
            branch,
            detached,
        });
    }
    if records.len() > termirust_domain::MAX_WORKTREE_REGISTRATIONS {
        return Err(WorktreeError::ResourceLimit {
            limit: termirust_domain::MAX_WORKTREE_REGISTRATIONS,
        });
    }
    Ok(records)
}

fn short_id(id: ManagedWorktreeId) -> String {
    id.as_uuid().simple().to_string().chars().take(12).collect()
}

fn path_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            dash = false;
        } else if !slug.is_empty() && !dash {
            slug.push('-');
            dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "repository".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

pub fn generated_worktree_branch(id: ManagedWorktreeId) -> GitReference {
    GitReference::new(&format!("termirust/worktree/{}", short_id(id)))
        .expect("generated worktree branch is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed without exposing fixture content"
        );
    }

    fn repository() -> tempfile::TempDir {
        let fixture = tempfile::tempdir().unwrap();
        git(fixture.path(), &["init", "-q", "-b", "main"]);
        git(fixture.path(), &["config", "user.name", "TermiRust Test"]);
        git(
            fixture.path(),
            &["config", "user.email", "test@termirust.invalid"],
        );
        fs::write(fixture.path().join("README.md"), "base\n").unwrap();
        git(fixture.path(), &["add", "README.md"]);
        git(fixture.path(), &["commit", "-q", "-m", "base"]);
        fixture
    }

    fn launch_draft(branch: &GitReference) -> WorktreeLaunchDraft {
        WorktreeLaunchDraft {
            source_project_id: ProjectId::new(),
            requested_base: None,
            fetch: false,
            confirm_current_branch: false,
            branch: branch.clone(),
            preset_id: None,
        }
    }

    #[test]
    fn parses_bounded_porcelain_z_without_reordering() {
        let records = parse_worktree_porcelain_z(
            b"worktree /tmp/main\0HEAD aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\0branch refs/heads/main\0\0worktree /tmp/child\0HEAD bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\0detached\0\0",
        )
        .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].path, PathBuf::from("/tmp/main"));
        assert!(records[1].detached);
    }

    #[test]
    fn inspect_create_verify_uses_literal_git_arguments() {
        let repository = repository();
        let managed_fixture = tempfile::tempdir().unwrap();
        let managed_root = managed_fixture.path().join("managed $(touch no)");
        let id = ManagedWorktreeId::new();
        let branch = GitReference::new("termirust/worktree/literal;echo-no").unwrap();
        let draft = launch_draft(&branch);
        let runner = GitRunner::default();
        let inspection = runner
            .inspect(
                repository.path(),
                &managed_root,
                id,
                ProjectId::new(),
                &draft,
                &WorktreeCancellation::default(),
            )
            .unwrap();
        assert_eq!(inspection.plan.selected_base.ref_name.as_str(), "main");
        runner
            .create(&inspection.plan, &WorktreeCancellation::default())
            .unwrap();
        runner
            .verify(&inspection.plan, &WorktreeCancellation::default())
            .unwrap();
        assert!(inspection.plan.managed_path.as_path().is_dir());
        assert!(!managed_fixture.path().join("no").exists());
    }

    #[test]
    fn dirty_detached_collision_and_cancel_fail_closed() {
        let repository = repository();
        let managed = tempfile::tempdir().unwrap();
        fs::write(repository.path().join("dirty"), "keep").unwrap();
        let runner = GitRunner::default();
        let id = ManagedWorktreeId::new();
        let branch = generated_worktree_branch(id);
        let draft = launch_draft(&branch);
        let result = runner.inspect(
            repository.path(),
            managed.path(),
            id,
            ProjectId::new(),
            &draft,
            &WorktreeCancellation::default(),
        );
        assert_eq!(result, Err(WorktreeError::DirtySource));
        fs::remove_file(repository.path().join("dirty")).unwrap();

        let cancellation = WorktreeCancellation::default();
        cancellation.cancel();
        assert_eq!(
            runner.inspect(
                repository.path(),
                managed.path(),
                id,
                ProjectId::new(),
                &draft,
                &cancellation,
            ),
            Err(WorktreeError::Cancelled)
        );
    }

    #[test]
    fn detached_no_base_branch_collision_and_submodules_have_explicit_errors() {
        let repository = repository();
        let managed = tempfile::tempdir().unwrap();
        let runner = GitRunner::default();
        let id = ManagedWorktreeId::new();
        let branch = generated_worktree_branch(id);
        let draft = launch_draft(&branch);
        let missing_remote = managed.path().join("missing-remote");
        git(
            repository.path(),
            &["remote", "add", "origin", missing_remote.to_str().unwrap()],
        );
        let mut fetch_draft = draft.clone();
        fetch_draft.fetch = true;
        assert_eq!(
            runner.inspect(
                repository.path(),
                managed.path(),
                id,
                ProjectId::new(),
                &fetch_draft,
                &WorktreeCancellation::default(),
            ),
            Err(WorktreeError::FetchFailed)
        );
        git(repository.path(), &["remote", "remove", "origin"]);
        git(repository.path(), &["branch", branch.as_str()]);
        assert_eq!(
            runner.inspect(
                repository.path(),
                managed.path(),
                id,
                ProjectId::new(),
                &draft,
                &WorktreeCancellation::default(),
            ),
            Err(WorktreeError::BranchCollision)
        );
        git(repository.path(), &["branch", "-D", branch.as_str()]);

        git(repository.path(), &["checkout", "-q", "--detach"]);
        git(repository.path(), &["branch", "-D", "main"]);
        assert_eq!(
            runner.inspect(
                repository.path(),
                managed.path(),
                id,
                ProjectId::new(),
                &draft,
                &WorktreeCancellation::default(),
            ),
            Err(WorktreeError::NoBase)
        );
        let mut current_draft = draft.clone();
        current_draft.confirm_current_branch = true;
        assert_eq!(
            runner.inspect(
                repository.path(),
                managed.path(),
                id,
                ProjectId::new(),
                &current_draft,
                &WorktreeCancellation::default(),
            ),
            Err(WorktreeError::DetachedHead)
        );

        fs::write(
            repository.path().join(".gitmodules"),
            "[submodule \"fixture\"]\n",
        )
        .unwrap();
        let mut explicit_draft = draft;
        explicit_draft.requested_base = Some(GitReference::new("HEAD").unwrap());
        assert_eq!(
            runner.inspect(
                repository.path(),
                managed.path(),
                id,
                ProjectId::new(),
                &explicit_draft,
                &WorktreeCancellation::default(),
            ),
            Err(WorktreeError::SubmodulesUnsupported)
        );
    }

    #[cfg(unix)]
    fn executable_script(root: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = root.join("fake-git");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn runner_enforces_combined_output_limit_and_deadline() {
        let fixture = tempfile::tempdir().unwrap();
        let cancellation = WorktreeCancellation::default();
        let output_runner = GitRunner {
            executable: executable_script(
                fixture.path(),
                "while :; do printf '0123456789' >&1; printf 'abcdefghij' >&2; done",
            )
            .into_os_string(),
            inspect_timeout: Duration::from_secs(2),
            fetch_timeout: Duration::from_secs(2),
            output_limit: 128,
        };
        assert!(matches!(
            output_runner.optional_os(
                fixture.path(),
                Vec::new(),
                output_runner.inspect_timeout,
                &cancellation,
            ),
            Err(WorktreeError::OutputLimit)
        ));

        let timeout_root = tempfile::tempdir().unwrap();
        let timeout_runner = GitRunner {
            executable: executable_script(timeout_root.path(), "sleep 30").into_os_string(),
            inspect_timeout: Duration::from_millis(40),
            fetch_timeout: Duration::from_millis(40),
            output_limit: MAX_GIT_OUTPUT_BYTES,
        };
        assert!(matches!(
            timeout_runner.optional_os(
                timeout_root.path(),
                Vec::new(),
                timeout_runner.inspect_timeout,
                &cancellation,
            ),
            Err(WorktreeError::Timeout)
        ));

        let unavailable_runner = GitRunner {
            executable: fixture.path().join("does-not-exist").into_os_string(),
            inspect_timeout: Duration::from_millis(40),
            fetch_timeout: Duration::from_millis(40),
            output_limit: MAX_GIT_OUTPUT_BYTES,
        };
        assert!(matches!(
            unavailable_runner.optional_os(
                fixture.path(),
                Vec::new(),
                unavailable_runner.inspect_timeout,
                &cancellation,
            ),
            Err(WorktreeError::GitUnavailable)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_only_the_owned_process_group() {
        let fixture = tempfile::tempdir().unwrap();
        let runner = GitRunner {
            executable: executable_script(fixture.path(), "sleep 30 &\necho $! > child.pid\nwait")
                .into_os_string(),
            inspect_timeout: Duration::from_secs(30),
            fetch_timeout: Duration::from_secs(30),
            output_limit: MAX_GIT_OUTPUT_BYTES,
        };
        let cancellation = WorktreeCancellation::default();
        let thread_runner = runner.clone();
        let thread_cancellation = cancellation.clone();
        let cwd = fixture.path().to_path_buf();
        let handle = thread::spawn(move || {
            thread_runner.optional_os(
                &cwd,
                Vec::new(),
                thread_runner.inspect_timeout,
                &thread_cancellation,
            )
        });
        let child_path = fixture.path().join("child.pid");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !child_path.exists() && Instant::now() < deadline {
            thread::sleep(POLL_INTERVAL);
        }
        let owned_pid: i32 = fs::read_to_string(&child_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut unrelated = Command::new("sleep").arg("30").spawn().unwrap();

        cancellation.cancel();
        assert!(matches!(
            handle.join().unwrap(),
            Err(WorktreeError::Cancelled)
        ));
        let reap_deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(owned_pid, 0) } == 0 && Instant::now() < reap_deadline {
            thread::sleep(POLL_INTERVAL);
        }
        let owned_stopped = unsafe { libc::kill(owned_pid, 0) } != 0;
        let unrelated_still_running = unrelated.try_wait().unwrap().is_none();
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        assert!(owned_stopped);
        assert!(unrelated_still_running);
    }

    #[test]
    fn runner_debug_and_errors_do_not_expose_paths_or_git_stderr() {
        let runner = GitRunner::default();
        assert!(!format!("{runner:?}").contains('/'));
        let fixture = tempfile::tempdir().unwrap();
        let error = runner
            .required(
                fixture.path(),
                ["rev-parse", "--verify", "definitely-secret-ref"],
                INSPECT_TIMEOUT,
                &WorktreeCancellation::default(),
                "fixture-failure",
            )
            .unwrap_err();
        assert_eq!(
            error,
            WorktreeError::GitFailed {
                code: "fixture-failure"
            }
        );
        assert!(!error.to_string().contains("secret"));
    }
}
