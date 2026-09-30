use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FOLDER_FILE_LIMIT: u64 = 1024 * 1024;
const FOLDER_GIT_OUTPUT_LIMIT: usize = 256 * 1024;
const FOLDER_DIRECTORY_ENTRY_LIMIT: usize = 5000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CanvasFolderEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_directory: bool,
}

#[derive(Clone, Debug)]
pub(super) struct CanvasFolderPanelState {
    pub workspace_id: u64,
    pub root: PathBuf,
    pub current_directory: PathBuf,
    pub entries: Vec<CanvasFolderEntry>,
    pub selected_file: Option<PathBuf>,
    pub original_contents: String,
    pub git_status: String,
    pub git_diff: String,
}

fn path_within_root(root: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("Unable to open folder: {error}"))?;
    let path = path
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("Unable to open folder path: {error}"))?;
    if !path.starts_with(&root) {
        anyhow::bail!("Refusing to access a path outside the selected folder");
    }
    Ok(path)
}

pub(super) fn load_folder_directory(
    root: &Path,
    directory: &Path,
) -> anyhow::Result<Vec<CanvasFolderEntry>> {
    let directory = path_within_root(root, directory)?;
    if !directory.is_dir() {
        anyhow::bail!("The selected folder path is not a directory");
    }
    let mut entries = fs::read_dir(&directory)
        .map_err(|error| anyhow::anyhow!("Unable to read folder: {error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some(CanvasFolderEntry {
                path: entry.path(),
                name,
                is_directory: metadata.is_dir(),
            })
        })
        .take(FOLDER_DIRECTORY_ENTRY_LIMIT)
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .is_directory
            .cmp(&left.is_directory)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}

pub(super) fn read_folder_file(root: &Path, path: &Path) -> anyhow::Result<String> {
    let path = path_within_root(root, path)?;
    let metadata = path
        .metadata()
        .map_err(|error| anyhow::anyhow!("Unable to inspect the file: {error}"))?;
    if !metadata.is_file() {
        anyhow::bail!("The selected folder path is not a regular file");
    }
    if metadata.len() > FOLDER_FILE_LIMIT {
        anyhow::bail!("Files larger than 1 MB are not opened in the canvas editor");
    }
    let bytes =
        fs::read(&path).map_err(|error| anyhow::anyhow!("Unable to read the file: {error}"))?;
    if bytes.contains(&0) {
        anyhow::bail!("Binary files are not opened in the canvas editor");
    }
    String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("Only UTF-8 text files are supported in the canvas editor"))
}

pub(super) fn write_folder_file(root: &Path, path: &Path, contents: &str) -> anyhow::Result<()> {
    let path = path_within_root(root, path)?;
    if !path.is_file() {
        anyhow::bail!("The selected folder path is not a regular file");
    }
    fs::write(path, contents).map_err(|error| anyhow::anyhow!("Unable to save the file: {error}"))
}

fn run_git(root: &Path, arguments: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| anyhow::anyhow!("Unable to run Git: {error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!(if message.is_empty() {
            "Git command failed".to_string()
        } else {
            message
        });
    }
    let mut output = output.stdout;
    if output.len() > FOLDER_GIT_OUTPUT_LIMIT {
        output.truncate(FOLDER_GIT_OUTPUT_LIMIT);
        output.extend_from_slice(b"\n[output truncated]\n");
    }
    Ok(String::from_utf8_lossy(&output).trim_end().to_string())
}

pub(super) fn git_snapshot(root: &Path, selected_file: Option<&Path>) -> (String, String) {
    let status = run_git(root, &["status", "--short", "--untracked-files=all"])
        .unwrap_or_else(|error| format!("Git unavailable: {error}"));
    let diff = selected_file
        .and_then(|path| path.strip_prefix(root).ok())
        .and_then(|path| path.to_str())
        .map(|relative| {
            run_git(root, &["diff", "--no-ext-diff", "--", relative])
                .unwrap_or_else(|error| format!("Unable to load diff: {error}"))
        })
        .unwrap_or_default();
    (status, diff)
}

#[cfg(test)]
mod tests {
    use super::{
        FOLDER_FILE_LIMIT, git_snapshot, load_folder_directory, read_folder_file, write_folder_file,
    };

    #[test]
    fn folder_files_are_rooted_text_only_and_explicitly_writable() {
        let fixture = std::env::temp_dir().join(format!(
            "termirust-canvas-folder-files-{}",
            crate::ui::util::current_unix_millis()
        ));
        let root = fixture.join("folder");
        let outside = fixture.join("outside.txt");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("binary.bin"), [0, 1, 2]).unwrap();
        std::fs::write(&outside, "outside\n").unwrap();

        let entries = load_folder_directory(&root, &root).unwrap();
        assert_eq!(
            entries.first().map(|entry| entry.name.as_str()),
            Some("src")
        );
        assert_eq!(
            read_folder_file(&root, &root.join("src/main.rs")).unwrap(),
            "fn main() {}\n"
        );
        write_folder_file(&root, &root.join("src/main.rs"), "fn main() { run(); }\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("src/main.rs")).unwrap(),
            "fn main() { run(); }\n"
        );
        assert!(read_folder_file(&root, &root.join("binary.bin")).is_err());
        assert!(read_folder_file(&root, &outside).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("escape-link")).unwrap();
            assert!(read_folder_file(&root, &root.join("escape-link")).is_err());
        }

        let oversized = root.join("oversized.txt");
        let file = std::fs::File::create(&oversized).unwrap();
        file.set_len(FOLDER_FILE_LIMIT + 1).unwrap();
        assert!(read_folder_file(&root, &oversized).is_err());
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[test]
    fn folder_git_snapshot_reports_status_and_selected_file_diff() {
        let root = std::env::temp_dir().join(format!(
            "termirust-canvas-folder-git-{}",
            crate::ui::util::current_unix_millis()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for arguments in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Multiplex Test"],
            vec!["config", "user.email", "test@termirust.invalid"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(arguments)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let path = root.join("main.rs");
        std::fs::write(&path, "base\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["add", "main.rs"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(&path, "changed\n").unwrap();

        let (status, diff) = git_snapshot(&root, Some(&path));
        assert!(status.contains("main.rs"));
        assert!(diff.contains("-base"));
        assert!(diff.contains("+changed"));
        let _ = std::fs::remove_dir_all(root);
    }
}
