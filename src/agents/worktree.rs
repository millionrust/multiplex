use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

use crate::models::{SavedManagedWorktree, SavedManagedWorktreeDisposition};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagedWorktreeStatus {
    pub dirty: bool,
    pub has_commits_after_base: bool,
    pub changed_paths: usize,
    pub diff_summary: String,
}

pub fn create_managed_worktree(
    working_directory: &Path,
    managed_root: &Path,
    node_hint: &str,
    label: &str,
) -> Result<SavedManagedWorktree> {
    let repository_root = git_stdout(
        working_directory,
        [OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
    )?;
    let repository_root = PathBuf::from(repository_root);
    let repository_root = repository_root
        .canonicalize()
        .context("Unable to canonicalize the Git repository root")?;
    if repository_root.join(".gitmodules").exists() {
        bail!(
            "This repository uses submodules. Choose Shared directory or Read only after reviewing Git's multiple-worktree limitations."
        );
    }
    let base_revision = git_stdout(
        &repository_root,
        [OsStr::new("rev-parse"), OsStr::new("HEAD")],
    )?;
    fs::create_dir_all(managed_root).with_context(|| {
        format!(
            "Unable to create managed worktree directory {}",
            managed_root.display()
        )
    })?;
    let managed_root = managed_root
        .canonicalize()
        .context("Unable to canonicalize the managed worktree directory")?;
    if managed_root.starts_with(&repository_root) {
        bail!("Managed worktrees must be stored outside the repository");
    }

    let hint = slug(node_hint, "agent");
    let label = slug(label, "task");
    let repository_name = repository_root
        .file_name()
        .and_then(OsStr::to_str)
        .map(|name| slug(name, "repository"))
        .unwrap_or_else(|| "repository".to_string());
    let parent = managed_root.join(repository_name);
    fs::create_dir_all(&parent)
        .with_context(|| format!("Unable to create {}", parent.display()))?;

    for suffix in 0..1000_u32 {
        let unique = if suffix == 0 {
            hint.clone()
        } else {
            format!("{hint}-{suffix}")
        };
        let branch = format!("termirust/agent/{unique}-{label}");
        let worktree_path = parent.join(&unique);
        if worktree_path.exists() || git_branch_exists(&repository_root, &branch)? {
            continue;
        }
        let arguments = [
            OsString::from("worktree"),
            OsString::from("add"),
            OsString::from("-b"),
            OsString::from(&branch),
            worktree_path.as_os_str().to_os_string(),
            OsString::from(&base_revision),
        ];
        run_git(&repository_root, arguments.iter().map(OsString::as_os_str))?;
        let canonical_path = worktree_path
            .canonicalize()
            .context("Git created a worktree that could not be canonicalized")?;
        if !canonical_path.starts_with(&managed_root) {
            bail!("Git created a worktree outside the managed directory");
        }
        return Ok(SavedManagedWorktree {
            repository_root: repository_root.display().to_string(),
            path: canonical_path.display().to_string(),
            branch,
            base_revision,
            owner_id: Some(node_hint.to_string()),
            disposition: SavedManagedWorktreeDisposition::Active,
        });
    }
    bail!("Unable to allocate a unique managed worktree name")
}

pub fn managed_worktree_status(worktree: &SavedManagedWorktree) -> Result<ManagedWorktreeStatus> {
    let path = Path::new(&worktree.path);
    let status = run_git(
        path,
        [
            OsStr::new("status"),
            OsStr::new("--porcelain=v2"),
            OsStr::new("-z"),
        ],
    )?;
    let head = git_stdout(path, [OsStr::new("rev-parse"), OsStr::new("HEAD")])?;
    let diff = run_git(
        path,
        [
            OsStr::new("diff"),
            OsStr::new("--shortstat"),
            OsStr::new(&worktree.base_revision),
            OsStr::new("--"),
        ],
    )?;
    let changed_paths = status
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| matches!(record.first(), Some(b'1' | b'2' | b'u' | b'?' | b'!')))
        .count();
    Ok(ManagedWorktreeStatus {
        dirty: !status.stdout.is_empty(),
        has_commits_after_base: head != worktree.base_revision,
        changed_paths,
        diff_summary: String::from_utf8_lossy(&diff.stdout).trim().to_string(),
    })
}

pub fn remove_managed_worktree(worktree: &SavedManagedWorktree, managed_root: &Path) -> Result<()> {
    let managed_root = managed_root
        .canonicalize()
        .context("Managed worktree directory does not exist")?;
    let worktree_path = Path::new(&worktree.path)
        .canonicalize()
        .context("Managed worktree path does not exist")?;
    if worktree_path == managed_root || !worktree_path.starts_with(&managed_root) {
        bail!("Refusing to remove a path outside the managed worktree directory");
    }
    let repository_root = Path::new(&worktree.repository_root)
        .canonicalize()
        .context("Repository root does not exist")?;
    if !registered_worktree_paths(&repository_root)?
        .iter()
        .any(|path| path == &worktree_path)
    {
        bail!("Refusing to remove a path that Git does not list as a worktree");
    }
    let status = managed_worktree_status(worktree)?;
    if status.dirty {
        bail!("Worktree has tracked or untracked changes and cannot be removed");
    }
    if status.has_commits_after_base {
        bail!(
            "Worktree contains commits after its base revision and cannot be removed automatically"
        );
    }
    run_git(
        &repository_root,
        [
            OsStr::new("worktree"),
            OsStr::new("remove"),
            worktree_path.as_os_str(),
        ],
    )?;
    Ok(())
}

fn registered_worktree_paths(repository_root: &Path) -> Result<Vec<PathBuf>> {
    let nul_output = Command::new("git")
        .current_dir(repository_root)
        .args(["worktree", "list", "--porcelain", "-z"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Unable to list Git worktrees")?;
    if nul_output.status.success() {
        return parse_worktree_paths(&nul_output.stdout);
    }

    let output = run_git(
        repository_root,
        [
            OsStr::new("worktree"),
            OsStr::new("list"),
            OsStr::new("--porcelain"),
        ],
    )?;
    parse_legacy_worktree_paths(&output.stdout)
}

fn parse_worktree_paths(output: &[u8]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for field in output.split(|byte| *byte == 0) {
        let Some(value) = field.strip_prefix(b"worktree ") else {
            continue;
        };
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            paths.push(PathBuf::from(OsString::from_vec(value.to_vec())));
        }
        #[cfg(not(unix))]
        {
            paths.push(PathBuf::from(String::from_utf8(value.to_vec())?));
        }
    }
    Ok(paths)
}

fn parse_legacy_worktree_paths(output: &[u8]) -> Result<Vec<PathBuf>> {
    let text = String::from_utf8(output.to_vec()).context("Git returned non-UTF-8 path data")?;
    Ok(text
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect())
}

fn git_branch_exists(repository_root: &Path, branch: &str) -> Result<bool> {
    let status = Command::new("git")
        .current_dir(repository_root)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .stdin(Stdio::null())
        .status()
        .context("Unable to check the proposed worktree branch")?;
    Ok(status.success())
}

fn git_stdout<'a>(
    working_directory: &Path,
    arguments: impl IntoIterator<Item = &'a OsStr>,
) -> Result<String> {
    let output = run_git(working_directory, arguments)?;
    let value = String::from_utf8(output.stdout).context("Git returned non-UTF-8 path data")?;
    Ok(value.trim().to_string())
}

fn run_git<'a>(
    working_directory: &Path,
    arguments: impl IntoIterator<Item = &'a OsStr>,
) -> Result<Output> {
    let output = Command::new("git")
        .current_dir(working_directory)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("Unable to run Git in {}", working_directory.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Git failed in {}: {}",
            working_directory.display(),
            stderr.trim()
        );
    }
    Ok(output)
}

fn slug(value: &str, fallback: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut previous_dash = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash && !result.is_empty() {
            result.push('-');
            previous_dash = true;
        }
    }
    while result.ends_with('-') {
        result.pop();
    }
    if result.is_empty() {
        fallback.to_string()
    } else {
        result.chars().take(36).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        create_managed_worktree, managed_worktree_status, parse_worktree_paths,
        remove_managed_worktree,
    };
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("termirust-{label}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn git(path: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .current_dir(path)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            arguments,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> PathBuf {
        let path = temp_directory("worktree-repo");
        git(&path, &["init", "-q"]);
        git(&path, &["config", "user.name", "TermiRust Test"]);
        git(&path, &["config", "user.email", "test@termirust.invalid"]);
        fs::write(path.join("README.md"), "base\n").unwrap();
        git(&path, &["add", "README.md"]);
        git(&path, &["commit", "-q", "-m", "base"]);
        path
    }

    #[test]
    fn parses_porcelain_z_worktree_paths() {
        let parsed = parse_worktree_paths(
            b"worktree /tmp/main\0HEAD abc\0branch refs/heads/main\0\0worktree /tmp/with space\0HEAD def\0detached\0\0",
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![PathBuf::from("/tmp/main"), PathBuf::from("/tmp/with space")]
        );
    }

    #[test]
    fn creates_and_removes_clean_managed_worktree() {
        let repository = repository();
        let managed_root = temp_directory("managed-root");
        let worktree =
            create_managed_worktree(&repository, &managed_root, "node 123", "Codex task").unwrap();
        assert!(Path::new(&worktree.path).is_dir());
        assert!(worktree.branch.starts_with("termirust/agent/"));
        assert!(!managed_worktree_status(&worktree).unwrap().dirty);
        remove_managed_worktree(&worktree, &managed_root).unwrap();
        assert!(!Path::new(&worktree.path).exists());
    }

    #[test]
    fn refuses_to_remove_dirty_or_committed_worktree() {
        let repository = repository();
        let managed_root = temp_directory("managed-dirty");
        let dirty = create_managed_worktree(&repository, &managed_root, "dirty", "task").unwrap();
        fs::write(Path::new(&dirty.path).join("untracked.txt"), "keep\n").unwrap();
        let dirty_status = managed_worktree_status(&dirty).unwrap();
        assert!(dirty_status.dirty);
        assert_eq!(dirty_status.changed_paths, 1);
        assert!(
            remove_managed_worktree(&dirty, &managed_root)
                .unwrap_err()
                .to_string()
                .contains("changes")
        );

        fs::remove_file(Path::new(&dirty.path).join("untracked.txt")).unwrap();
        fs::write(Path::new(&dirty.path).join("README.md"), "changed\n").unwrap();
        git(Path::new(&dirty.path), &["add", "README.md"]);
        git(
            Path::new(&dirty.path),
            &["commit", "-q", "-m", "agent work"],
        );
        assert!(
            managed_worktree_status(&dirty)
                .unwrap()
                .has_commits_after_base
        );
        assert!(
            managed_worktree_status(&dirty)
                .unwrap()
                .diff_summary
                .contains("1 file changed")
        );
        assert!(
            remove_managed_worktree(&dirty, &managed_root)
                .unwrap_err()
                .to_string()
                .contains("commits")
        );
    }

    #[test]
    fn refuses_cleanup_outside_managed_root() {
        let repository = repository();
        let managed_root = temp_directory("managed-boundary");
        let fake = crate::models::SavedManagedWorktree {
            repository_root: repository.display().to_string(),
            path: repository.display().to_string(),
            branch: "main".to_string(),
            base_revision: "unknown".to_string(),
            owner_id: None,
            disposition: crate::models::SavedManagedWorktreeDisposition::Active,
        };
        assert!(
            remove_managed_worktree(&fake, &managed_root)
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
    }

    #[test]
    fn git_arguments_are_not_shell_parsed() {
        let repository = repository();
        let managed_root = temp_directory("managed-$(touch should-not-run)");
        let worktree =
            create_managed_worktree(&repository, &managed_root, "node; echo no", "quote ' task")
                .unwrap();
        assert!(Path::new(&worktree.path).is_dir());
        assert!(!Path::new("should-not-run").exists());
        let _ = OsStr::new(&worktree.branch);
    }
}
