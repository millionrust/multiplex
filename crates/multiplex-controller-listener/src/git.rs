//! Git for the folder a terminal is in, so a paired phone can read its history, branches,
//! worktrees and changes, and switch branch.
//!
//! The folder is the shell's own current directory where the platform can tell
//! ([`process_folder`]), so `cd` is followed. Git runs as the person, in that folder, with no
//! prompts, no pager, no external diff or text conversion programs, no optional locks (so the
//! index a person's own `git` is using is never taken) and no file-system monitor; every run is
//! bounded in time and output and is killed with its process group when it overruns.
//!
//! Everything here blocks; callers run it on a blocking thread.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::protocol::{
    GitBranch, GitChange, GitChangeStatus, GitCommit, GitCommitDetail, GitDiff, GitDiffFile,
    GitRepository, GitWorktree, MAX_GIT_DIFF_PATCH_BYTES, MAX_GIT_RESPONSE_BYTES,
};

const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Switching runs checkout hooks, which may take a while on a large tree.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_LISTING_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_STDERR: usize = 8 * 1024;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;
const MAX_SUBJECT_CHARS: usize = 200;
const MAX_BRANCHES_READ: usize = 400;
const MAX_WORKTREES: usize = 64;
const MAX_FILES: usize = 1_000;
/// The tree with nothing in it, which a branch with no commits yet is compared with.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Options every run gets. Configuration that would start programs is turned off here or per
/// command (`--no-ext-diff`, `--no-textconv`).
const GLOBAL_ARGS: [&str; 7] = [
    "--no-pager",
    "-c",
    "core.quotepath=off",
    "-c",
    "color.ui=false",
    "-c",
    "core.fsmonitor=false",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitError {
    /// No `git` could be started.
    Unavailable,
    Timeout,
    /// The worktree asked for is not one of this repository's.
    UnknownWorktree,
    UnknownCommit,
    UnknownBranch,
    Failed,
}

impl GitError {
    /// The code a phone is told.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "git_unavailable",
            Self::Timeout => "git_timeout",
            Self::UnknownWorktree => "git_unknown_worktree",
            Self::UnknownCommit => "git_unknown_commit",
            Self::UnknownBranch => "git_unknown_branch",
            Self::Failed => "git_failed",
        }
    }
}

/// The shell's current directory. `None` where the platform cannot say, or the process is gone.
pub fn process_folder(process_id: u64) -> Option<PathBuf> {
    let process_id = i32::try_from(process_id).ok().filter(|id| *id > 0)?;
    platform_process_folder(process_id).filter(|folder| folder.is_dir())
}

#[cfg(target_os = "macos")]
fn platform_process_folder(process_id: i32) -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt as _;

    // SAFETY: an all-zero `proc_vnodepathinfo` is valid; the kernel fills it in.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: `info` is a writable buffer of exactly `size` bytes for the call's duration.
    let written = unsafe {
        libc::proc_pidinfo(
            process_id,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    let path = &info.pvi_cdir.vip_path;
    // SAFETY: `vip_path` is a fixed `MAXPATHLEN` array the kernel terminates with a NUL.
    let bytes = unsafe {
        CStr::from_bytes_until_nul(std::slice::from_raw_parts(
            path.as_ptr().cast::<u8>(),
            std::mem::size_of_val(path),
        ))
    }
    .ok()?
    .to_bytes();
    (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(target_os = "linux")]
fn platform_process_folder(process_id: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{process_id}/cwd")).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_process_folder(_process_id: i32) -> Option<PathBuf> {
    None
}

/// What one run printed.
struct Output {
    success: bool,
    stdout: Vec<u8>,
    stderr: String,
    /// Stdout reached the limit and the run was stopped there.
    overflowed: bool,
}

/// Runs git in `folder`. A run that prints more than `limit` is stopped and keeps what it had.
fn run(folder: &Path, args: &[&str], limit: usize, timeout: Duration) -> Result<Output, GitError> {
    let mut command = Command::new("git");
    command
        .current_dir(folder)
        .args(GLOBAL_ARGS)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|_| GitError::Unavailable)?;
    let overflowed = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().map(|mut pipe| {
        let overflowed = overflowed.clone();
        thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0u8; 16 * 1024];
            while let Ok(read) = pipe.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let room = limit.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..read.min(room)]);
                if read > room {
                    overflowed.store(true, Ordering::Relaxed);
                    break;
                }
            }
            kept
        })
    });
    let stderr = child.stderr.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut kept = Vec::new();
            let _ = pipe.by_ref().take(MAX_STDERR as u64).read_to_end(&mut kept);
            // Drain the rest so the child never blocks on a full pipe.
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
            String::from_utf8_lossy(&kept).into_owned()
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if overflowed.load(Ordering::Relaxed) || Instant::now() >= deadline => {
                stop(&mut child);
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => {
                stop(&mut child);
                return Err(GitError::Failed);
            }
        }
    };
    let stdout = stdout
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let overflowed = overflowed.load(Ordering::Relaxed);
    match status {
        Some(status) => Ok(Output {
            success: status.success(),
            stdout,
            stderr,
            overflowed,
        }),
        None if overflowed => Ok(Output {
            success: true,
            stdout,
            stderr,
            overflowed,
        }),
        None => Err(GitError::Timeout),
    }
}

/// Ends the run and anything it started.
fn stop(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(group) = i32::try_from(child.id()) {
        // SAFETY: the child leads its own process group, so this reaches only what it started.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A run that must succeed.
fn required(folder: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>, GitError> {
    let output = run(folder, args, limit, READ_TIMEOUT)?;
    if output.success {
        Ok(output.stdout)
    } else {
        Err(GitError::Failed)
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The top of the worktree `folder` is in, or `None` when it is in no repository.
pub fn worktree_root(folder: &Path) -> Result<Option<PathBuf>, GitError> {
    let output = run(
        folder,
        &["rev-parse", "--show-toplevel"],
        64 * 1024,
        READ_TIMEOUT,
    )?;
    if !output.success {
        // Outside a repository, or inside its `.git` folder: either way nothing to show.
        return Ok(None);
    }
    let root = text(&output.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_owned();
    Ok((!root.is_empty()).then(|| PathBuf::from(root)))
}

/// Which worktree to show: the terminal's own, or another of the same repository's by path.
/// `None` when the terminal is not in a repository.
pub fn view(folder: &Path, requested: Option<&str>) -> Result<Option<PathBuf>, GitError> {
    let Some(root) = worktree_root(folder)? else {
        return Ok(None);
    };
    let Some(requested) = requested else {
        return Ok(Some(root));
    };
    let requested = Path::new(requested);
    let found = worktrees(&root, &root, &root)?
        .into_iter()
        .map(|worktree| PathBuf::from(worktree.path))
        .find(|path| same_path(path, requested))
        .ok_or(GitError::UnknownWorktree)?;
    Ok(Some(found))
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (left.canonicalize(), right.canonicalize()),
            (Ok(left), Ok(right)) if left == right
        )
}

/// Branch, upstream, changes, branches and worktrees for the worktree at `root`.
pub fn overview(
    folder: &Path,
    terminal_root: &Path,
    root: &Path,
) -> Result<GitRepository, GitError> {
    let status = required(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=dirty",
        ],
        MAX_LISTING_OUTPUT,
    )?;
    let status = parse_status(&status);
    let mut repository = GitRepository {
        folder: folder.display().to_string(),
        root: root.display().to_string(),
        branch: status.branch,
        head: status.head.map(|oid| short(&oid)),
        upstream: status.upstream,
        ahead: status.ahead,
        behind: status.behind,
        changes: status.changes,
        changes_truncated: false,
        branches: branches(root)?,
        branches_truncated: false,
        worktrees: worktrees(root, terminal_root, root)?,
    };
    fit_overview(&mut repository);
    Ok(repository)
}

/// Halves whichever list is longest until the answer fits one control frame.
fn fit_overview(repository: &mut GitRepository) {
    while serde_json::to_vec(repository).map_or(0, |bytes| bytes.len()) > MAX_GIT_RESPONSE_BYTES {
        if repository.changes.len() >= repository.branches.len() && !repository.changes.is_empty() {
            let keep = repository.changes.len() / 2;
            repository.changes.truncate(keep);
            repository.changes_truncated = true;
        } else if !repository.branches.is_empty() {
            let keep = repository.branches.len() / 2;
            repository.branches.truncate(keep);
            repository.branches_truncated = true;
        } else {
            repository
                .worktrees
                .truncate(repository.worktrees.len() / 2);
        }
    }
}

#[derive(Default)]
struct Status {
    head: Option<String>,
    branch: Option<String>,
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    changes: Vec<GitChange>,
}

fn parse_status(bytes: &[u8]) -> Status {
    let mut status = Status::default();
    let mut records = bytes.split(|byte| *byte == 0).map(text);
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head = Some(value.to_owned()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_owned()),
                "branch.upstream" => status.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(count) = part.strip_prefix('+') {
                            status.ahead = count.parse().unwrap_or(0);
                        } else if let Some(count) = part.strip_prefix('-') {
                            status.behind = count.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if status.changes.len() >= MAX_FILES {
            continue;
        }
        let change = match record.as_bytes().first() {
            Some(b'1') => record
                .splitn(9, ' ')
                .collect::<Vec<_>>()
                .get(1..)
                .and_then(|fields| ordinary_change(fields[0], fields.get(7)?, None)),
            Some(b'2') => {
                let fields: Vec<&str> = record.splitn(10, ' ').collect();
                let original = records.next();
                fields
                    .get(9)
                    .and_then(|path| ordinary_change(fields[1], path, original))
            }
            Some(b'u') => record.splitn(11, ' ').nth(10).map(|path| GitChange {
                path: path.to_owned(),
                original_path: None,
                status: GitChangeStatus::Conflicted,
                staged: false,
                unstaged: true,
            }),
            Some(b'?') => record.get(2..).map(|path| GitChange {
                path: path.to_owned(),
                original_path: None,
                status: GitChangeStatus::Untracked,
                staged: false,
                unstaged: true,
            }),
            _ => None,
        };
        status.changes.extend(change);
    }
    status
}

fn ordinary_change(xy: &str, path: &str, original: Option<String>) -> Option<GitChange> {
    let mut codes = xy.chars();
    let (index, worktree) = (codes.next()?, codes.next()?);
    let letter = if index == '.' { worktree } else { index };
    Some(GitChange {
        path: path.to_owned(),
        original_path: original.filter(|original| !original.is_empty()),
        status: change_status(letter),
        staged: index != '.',
        unstaged: worktree != '.',
    })
}

fn change_status(letter: char) -> GitChangeStatus {
    match letter {
        'A' => GitChangeStatus::Added,
        'D' => GitChangeStatus::Deleted,
        'R' => GitChangeStatus::Renamed,
        'C' => GitChangeStatus::Copied,
        'T' => GitChangeStatus::TypeChanged,
        'U' => GitChangeStatus::Conflicted,
        _ => GitChangeStatus::Modified,
    }
}

fn branches(root: &Path) -> Result<Vec<GitBranch>, GitError> {
    let count = format!("--count={MAX_BRANCHES_READ}");
    let output = required(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            &count,
            "--format=%(refname)%00%(objectname:short)%00%(upstream:short)%00%(committerdate:unix)%00%(HEAD)%00%(contents:subject)",
            "refs/heads",
            "refs/remotes",
        ],
        MAX_LISTING_OUTPUT,
    )?;
    Ok(parse_branches(&output))
}

fn parse_branches(bytes: &[u8]) -> Vec<GitBranch> {
    text(bytes)
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\0').collect();
            let [refname, commit, upstream, time, head, subject] = fields[..] else {
                return None;
            };
            let (name, remote) = if let Some(name) = refname.strip_prefix("refs/heads/") {
                (name, false)
            } else {
                let name = refname.strip_prefix("refs/remotes/")?;
                // `origin/HEAD` names another branch rather than being one.
                if name.ends_with("/HEAD") {
                    return None;
                }
                (name, true)
            };
            Some(GitBranch {
                name: name.to_owned(),
                remote,
                current: head == "*",
                upstream: (!upstream.is_empty()).then(|| upstream.to_owned()),
                commit: commit.to_owned(),
                subject: clip_chars(subject, MAX_SUBJECT_CHARS),
                unix_time: time.parse().unwrap_or(0),
            })
        })
        .collect()
}

/// The repository's worktrees, marking the one shown and the one the terminal is in.
fn worktrees(
    root: &Path,
    terminal_root: &Path,
    shown: &Path,
) -> Result<Vec<GitWorktree>, GitError> {
    let output = required(
        root,
        &["worktree", "list", "--porcelain", "-z"],
        MAX_LISTING_OUTPUT,
    )?;
    let mut worktrees = parse_worktrees(&output);
    for worktree in &mut worktrees {
        let path = Path::new(&worktree.path);
        worktree.current = same_path(path, shown);
        worktree.terminal = same_path(path, terminal_root);
    }
    Ok(worktrees)
}

fn parse_worktrees(bytes: &[u8]) -> Vec<GitWorktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<GitWorktree> = None;
    let mut bare = false;
    for field in bytes.split(|byte| *byte == 0).map(text) {
        if field.is_empty() {
            if let Some(worktree) = current.take()
                && !bare
                && worktrees.len() < MAX_WORKTREES
            {
                worktrees.push(worktree);
            }
            bare = false;
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((&field, ""));
        match key {
            "worktree" => {
                current = Some(GitWorktree {
                    path: value.to_owned(),
                    branch: None,
                    head: None,
                    current: false,
                    terminal: false,
                    locked: false,
                    prunable: false,
                });
            }
            "bare" => bare = true,
            _ => {
                if let Some(worktree) = current.as_mut() {
                    match key {
                        "HEAD" => worktree.head = Some(short(value)),
                        "branch" => {
                            worktree.branch = Some(
                                value
                                    .strip_prefix("refs/heads/")
                                    .unwrap_or(value)
                                    .to_owned(),
                            );
                        }
                        "locked" => worktree.locked = true,
                        "prunable" => worktree.prunable = true,
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some(worktree) = current
        && !bare
        && worktrees.len() < MAX_WORKTREES
    {
        worktrees.push(worktree);
    }
    worktrees
}

/// One page of history from `HEAD`, newest first, and where the next page starts.
pub fn log(
    root: &Path,
    offset: u32,
    limit: u16,
) -> Result<(Vec<GitCommit>, Option<u32>), GitError> {
    let skip = format!("--skip={offset}");
    let count = format!("--max-count={}", u32::from(limit) + 1);
    let output = run(
        root,
        &[
            "log",
            "--no-color",
            "--decorate=short",
            "--format=%H%x1f%h%x1f%an%x1f%at%x1f%P%x1f%D%x1f%s%x1e",
            &skip,
            &count,
            "HEAD",
            "--",
        ],
        MAX_LISTING_OUTPUT,
        READ_TIMEOUT,
    )?;
    if !output.success {
        // A branch with no commits yet has no history to show.
        if output.stderr.contains("does not have any commits")
            || output.stderr.contains("bad default revision")
            || output.stderr.contains("unknown revision")
        {
            return Ok((Vec::new(), None));
        }
        return Err(GitError::Failed);
    }
    let mut commits = parse_log(&output.stdout);
    // One more than asked for was read, to know whether another page follows.
    let mut more = commits.len() > usize::from(limit);
    commits.truncate(usize::from(limit));
    while serde_json::to_vec(&commits).map_or(0, |bytes| bytes.len()) > MAX_GIT_RESPONSE_BYTES
        && commits.len() > 1
    {
        commits.truncate(commits.len() / 2);
        more = true;
    }
    let next = more.then(|| offset.saturating_add(commits.len() as u32));
    Ok((commits, next))
}

fn parse_log(bytes: &[u8]) -> Vec<GitCommit> {
    text(bytes)
        .split('\x1e')
        .filter_map(|record| {
            let record = record.trim_start_matches(['\n', '\r']);
            let fields: Vec<&str> = record.split('\x1f').collect();
            let [oid, short, author, time, parents, refs, subject] = fields[..] else {
                return None;
            };
            Some(GitCommit {
                oid: oid.to_owned(),
                short: short.to_owned(),
                author: clip_chars(author, 120),
                unix_time: time.parse().unwrap_or(0),
                parents: parents.split_whitespace().count() as u32,
                refs: refs
                    .split(", ")
                    .filter(|name| !name.is_empty())
                    .map(|name| name.trim_start_matches("HEAD -> ").to_owned())
                    .take(16)
                    .collect(),
                subject: clip_chars(subject, MAX_SUBJECT_CHARS),
            })
        })
        .collect()
}

/// A commit's changes against its first parent, or the worktree's uncommitted changes against
/// `HEAD` when no commit is named. `path` narrows the patch to one file.
pub fn diff(root: &Path, commit: Option<&str>, path: Option<&str>) -> Result<GitDiff, GitError> {
    let (detail, base, target) = match commit {
        Some(commit) => {
            let detail = commit_detail(root, commit)?;
            let base = detail
                .parents
                .first()
                .cloned()
                .unwrap_or_else(|| EMPTY_TREE.to_owned());
            let target = detail.oid.clone();
            (Some(detail), base, Some(target))
        }
        None => {
            let head = run(
                root,
                &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
                1024,
                READ_TIMEOUT,
            )?;
            let base = if head.success {
                text(&head.stdout).trim().to_owned()
            } else {
                EMPTY_TREE.to_owned()
            };
            (None, base, None)
        }
    };
    let mut range = vec![base.as_str()];
    if let Some(target) = target.as_deref() {
        range.push(target);
    }
    let mut files = changed_files(root, &range)?;
    if commit.is_none() {
        let status = parse_status(&required(
            root,
            &["status", "--porcelain=v2", "-z", "--untracked-files=normal"],
            MAX_LISTING_OUTPUT,
        )?);
        files.extend(
            status
                .changes
                .into_iter()
                .filter(|change| change.status == GitChangeStatus::Untracked)
                .map(|change| GitDiffFile {
                    path: change.path,
                    original_path: None,
                    status: GitChangeStatus::Untracked,
                    additions: None,
                    deletions: None,
                }),
        );
    }
    let pathspec = path.map(|path| format!(":(top,literal){path}"));
    let mut patch_args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "-M",
        "--patch",
    ];
    patch_args.extend(&range);
    patch_args.push("--");
    if let Some(pathspec) = pathspec.as_deref() {
        patch_args.push(pathspec);
    }
    let output = run(root, &patch_args, MAX_GIT_DIFF_PATCH_BYTES, READ_TIMEOUT)?;
    if !output.success && !output.overflowed {
        return Err(GitError::Failed);
    }
    let (patch, patch_truncated) = clip_lines(&text(&output.stdout), output.overflowed);
    let mut diff = GitDiff {
        commit: detail,
        files,
        files_truncated: false,
        patch,
        patch_truncated,
    };
    fit_diff(&mut diff);
    Ok(diff)
}

fn commit_detail(root: &Path, commit: &str) -> Result<GitCommitDetail, GitError> {
    let target = format!("{commit}^{{commit}}");
    let output = run(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &target,
        ],
        1024,
        READ_TIMEOUT,
    )?;
    if !output.success {
        return Err(GitError::UnknownCommit);
    }
    let oid = text(&output.stdout).trim().to_owned();
    let shown = required(
        root,
        &[
            "show",
            "-s",
            "--no-color",
            "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%at%x1f%P%x1f%B",
            &oid,
            "--",
        ],
        64 * 1024,
    )?;
    let shown = text(&shown);
    let fields: Vec<&str> = shown.splitn(7, '\x1f').collect();
    let [oid, short, author, email, time, parents, message] = fields[..] else {
        return Err(GitError::Failed);
    };
    Ok(GitCommitDetail {
        oid: oid.to_owned(),
        short: short.to_owned(),
        author: clip_chars(author, 120),
        email: clip_chars(email, 200),
        unix_time: time.trim().parse().unwrap_or(0),
        parents: parents.split_whitespace().map(str::to_owned).collect(),
        message: clip_bytes(message.trim_end(), MAX_MESSAGE_BYTES),
    })
}

/// Files changed in `range`, with their line counts; a binary file counts no lines.
fn changed_files(root: &Path, range: &[&str]) -> Result<Vec<GitDiffFile>, GitError> {
    let mut names = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "-M",
        "-z",
        "--name-status",
    ];
    names.extend(range);
    names.push("--");
    let mut counts = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "-M",
        "-z",
        "--numstat",
    ];
    counts.extend(range);
    counts.push("--");
    let mut files = parse_name_status(&required(root, &names, MAX_LISTING_OUTPUT)?);
    for (path, additions, deletions) in parse_numstat(&required(root, &counts, MAX_LISTING_OUTPUT)?)
    {
        if let Some(file) = files.iter_mut().find(|file| file.path == path) {
            file.additions = additions;
            file.deletions = deletions;
        }
    }
    files.truncate(MAX_FILES);
    Ok(files)
}

fn parse_name_status(bytes: &[u8]) -> Vec<GitDiffFile> {
    let mut fields = bytes.split(|byte| *byte == 0).map(text);
    let mut files = Vec::new();
    while let Some(code) = fields.next() {
        let Some(letter) = code.chars().next() else {
            continue;
        };
        let (original_path, path) = if matches!(letter, 'R' | 'C') {
            let original = fields.next();
            (original, fields.next())
        } else {
            (None, fields.next())
        };
        let Some(path) = path else {
            break;
        };
        files.push(GitDiffFile {
            path,
            original_path,
            status: change_status(letter),
            additions: None,
            deletions: None,
        });
    }
    files
}

/// `(path, additions, deletions)`; counts are `None` for a binary file.
fn parse_numstat(bytes: &[u8]) -> Vec<(String, Option<u32>, Option<u32>)> {
    let mut fields = bytes.split(|byte| *byte == 0).map(text);
    let mut counts = Vec::new();
    while let Some(field) = fields.next() {
        let mut parts = field.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // A rename leaves the path empty and gives the old and new paths as the next fields.
        let path = if path.is_empty() {
            let _original = fields.next();
            fields.next().unwrap_or_default()
        } else {
            path.to_owned()
        };
        counts.push((path, added.parse().ok(), deleted.parse().ok()));
    }
    counts
}

fn fit_diff(diff: &mut GitDiff) {
    while serde_json::to_vec(diff).map_or(0, |bytes| bytes.len()) > MAX_GIT_RESPONSE_BYTES {
        if diff.patch.len() > 1024 {
            let keep = diff.patch.len() / 2;
            let (patch, _) =
                clip_lines(&diff.patch[..floor_char_boundary(&diff.patch, keep)], true);
            diff.patch = patch;
            diff.patch_truncated = true;
        } else if diff.files.len() > 1 {
            diff.files.truncate(diff.files.len() / 2);
            diff.files_truncated = true;
        } else {
            diff.patch.clear();
            diff.patch_truncated = true;
            if let Some(commit) = diff.commit.as_mut() {
                commit.message = clip_bytes(&commit.message, 1024);
            }
            break;
        }
    }
}

/// Switches the worktree at `root` to a local branch, or creates a local branch tracking a
/// remote one. Git's own refusal, such as local changes a switch would overwrite, comes back as
/// `Ok((false, message))`.
pub fn switch(root: &Path, branch: &str) -> Result<(bool, String), GitError> {
    let local = format!("refs/heads/{branch}");
    let remote = format!("refs/remotes/{branch}");
    let exists = |reference: &str| {
        run(
            root,
            &["show-ref", "--verify", "--quiet", reference],
            1024,
            READ_TIMEOUT,
        )
        .map(|output| output.success)
    };
    let args: Vec<&str> = if exists(&local)? {
        vec!["switch", "--no-guess", branch]
    } else if exists(&remote)? {
        vec!["switch", "--track", branch]
    } else {
        return Err(GitError::UnknownBranch);
    };
    let output = run(root, &args, 64 * 1024, SWITCH_TIMEOUT)?;
    let mut message = output.stderr.trim().to_owned();
    if message.is_empty() {
        message = text(&output.stdout).trim().to_owned();
    }
    Ok((output.success, clip_bytes(&message, 2 * 1024)))
}

fn short(oid: &str) -> String {
    oid.chars().take(12).collect()
}

/// Keeps whole lines of `text`, saying whether any were cut.
fn clip_lines(text: &str, cut: bool) -> (String, bool) {
    if !cut {
        return (text.to_owned(), false);
    }
    match text.rfind('\n') {
        Some(end) => (text[..=end].to_owned(), true),
        None => (String::new(), true),
    }
}

fn clip_chars(text: &str, maximum: usize) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .take(maximum)
        .collect()
}

fn clip_bytes(text: &str, maximum: usize) -> String {
    text[..floor_char_boundary(text, maximum)].to_owned()
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut index = index;
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with two commits on `main`, a `feature` branch, a second worktree, and
    /// uncommitted changes. `None` where git is not installed.
    fn fixture() -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("repo");
        std::fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(&root)
                .args(["-c", "user.name=Ada", "-c", "user.email=ada@example.com"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok()?;
            status.success().then_some(())
        };
        git(&["init", "-q", "-b", "main"])?;
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&["add", "a.txt"])?;
        git(&["commit", "-q", "-m", "First commit"])?;
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        git(&["add", "a.txt", "b.bin"])?;
        git(&["commit", "-q", "-m", "Second commit\n\nWith a body."])?;
        git(&["branch", "feature"])?;
        git(&["worktree", "add", "-q", "../repo-wt", "-b", "wt-branch"])?;
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(root.join("new.txt"), "fresh\n").unwrap();
        Some((dir, root))
    }

    #[test]
    fn a_folder_outside_any_repository_has_nothing_to_show() {
        let dir = tempfile::tempdir().unwrap();
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        assert_eq!(view(dir.path(), None), Ok(None));
    }

    #[test]
    fn the_overview_names_the_branch_changes_branches_and_worktrees() {
        let Some((_dir, root)) = fixture() else {
            return;
        };
        let nested = root.join("sub");
        std::fs::create_dir(&nested).unwrap();
        let shown = view(&nested, None).unwrap().unwrap();
        assert!(same_path(&shown, &root));
        let repository = overview(&nested, &shown, &shown).unwrap();
        assert_eq!(repository.branch.as_deref(), Some("main"));
        assert!(repository.head.is_some());
        let changes: Vec<(&str, GitChangeStatus)> = repository
            .changes
            .iter()
            .map(|change| (change.path.as_str(), change.status))
            .collect();
        assert!(
            changes.contains(&("a.txt", GitChangeStatus::Modified)),
            "{changes:?}"
        );
        assert!(
            changes.contains(&("new.txt", GitChangeStatus::Untracked)),
            "{changes:?}"
        );
        let names: Vec<&str> = repository
            .branches
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert!(
            names.contains(&"main") && names.contains(&"feature"),
            "{names:?}"
        );
        assert!(
            repository
                .branches
                .iter()
                .any(|b| b.name == "main" && b.current)
        );
        assert_eq!(repository.worktrees.len(), 2);
        assert!(repository.worktrees.iter().any(|w| w.terminal && w.current));
        let other = repository.worktrees.iter().find(|w| !w.terminal).unwrap();
        assert_eq!(other.branch.as_deref(), Some("wt-branch"));

        // Another worktree of the same repository may be shown; any other folder may not.
        let elsewhere = view(&nested, Some(&other.path)).unwrap().unwrap();
        assert!(same_path(&elsewhere, Path::new(&other.path)));
        assert_eq!(view(&nested, Some("/tmp")), Err(GitError::UnknownWorktree));
    }

    #[test]
    fn history_pages_newest_first() {
        let Some((_dir, root)) = fixture() else {
            return;
        };
        let (first, next) = log(&root, 0, 1).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].subject, "Second commit");
        assert!(
            first[0].refs.iter().any(|name| name == "main"),
            "{:?}",
            first[0].refs
        );
        assert_eq!(next, Some(1));
        let (second, next) = log(&root, 1, 1).unwrap();
        assert_eq!(second[0].subject, "First commit");
        assert_eq!(second[0].parents, 0);
        assert_eq!(next, None);
    }

    #[test]
    fn diffs_cover_a_commit_one_file_and_the_uncommitted_changes() {
        let Some((_dir, root)) = fixture() else {
            return;
        };
        let (commits, _) = log(&root, 0, 1).unwrap();
        let commit = diff(&root, Some(&commits[0].short), None).unwrap();
        let detail = commit.commit.as_ref().unwrap();
        assert!(detail.message.contains("With a body."));
        assert_eq!(detail.parents.len(), 1);
        let text_file = commit.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(
            (text_file.additions, text_file.deletions),
            (Some(1), Some(0))
        );
        let binary = commit.files.iter().find(|f| f.path == "b.bin").unwrap();
        assert_eq!(binary.status, GitChangeStatus::Added);
        assert_eq!(binary.additions, None);
        assert!(commit.patch.contains("+two"));

        let one = diff(&root, Some(&commits[0].oid), Some("a.txt")).unwrap();
        assert!(one.patch.contains("a.txt") && !one.patch.contains("b.bin"));

        let working = diff(&root, None, None).unwrap();
        assert!(working.commit.is_none());
        assert!(working.patch.contains("+three"));
        assert!(
            working
                .files
                .iter()
                .any(|f| f.path == "new.txt" && f.status == GitChangeStatus::Untracked)
        );

        assert_eq!(
            diff(&root, Some("0000000000"), None).map(|_| ()),
            Err(GitError::UnknownCommit)
        );
    }

    #[test]
    fn switching_follows_git_and_refuses_unknown_branches() {
        let Some((_dir, root)) = fixture() else {
            return;
        };
        // Uncommitted changes to a file both branches share carry over, as git does.
        let (switched, _) = switch(&root, "feature").unwrap();
        assert!(switched);
        assert_eq!(
            overview(&root, &root, &root).unwrap().branch.as_deref(),
            Some("feature")
        );
        // A branch checked out in another worktree is refused by git, with its reason.
        let (switched, message) = switch(&root, "wt-branch").unwrap();
        assert!(!switched);
        assert!(!message.is_empty());
        assert_eq!(switch(&root, "nope"), Err(GitError::UnknownBranch));
    }

    #[test]
    fn the_shell_folder_of_a_running_process_is_found() {
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new("sleep")
            .arg("5")
            .current_dir(dir.path())
            .spawn()
            .unwrap();
        let found = process_folder(u64::from(child.id()));
        let _ = child.kill();
        let _ = child.wait();
        assert!(same_path(&found.unwrap(), dir.path()));
        assert_eq!(process_folder(0), None);
    }

    #[test]
    fn large_answers_are_trimmed_to_one_frame() {
        let mut diff = GitDiff {
            commit: None,
            files: Vec::new(),
            files_truncated: false,
            patch: "+line\n".repeat(40_000),
            patch_truncated: false,
        };
        fit_diff(&mut diff);
        assert!(serde_json::to_vec(&diff).unwrap().len() <= MAX_GIT_RESPONSE_BYTES);
        assert!(diff.patch_truncated && diff.patch.ends_with('\n'));
    }
}
