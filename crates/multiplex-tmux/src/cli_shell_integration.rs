//! Reviewed, reversible startup hooks for external interactive zsh/bash terminals.
//! The CLI's Session Host supplies the same sessions to desktop and mobile listings.

use std::path::{Path, PathBuf};

use crate::shell_integration::{
    ChangePlan, FileChange, IntegrationError, IntegrationStatus, Shell, find_marked_blocks,
    read_optional, resolve_write_path, shell_single_quote,
};

pub const BLOCK_START: &str = "# >>> multiplex cli terminals >>>";
pub const BLOCK_END: &str = "# <<< multiplex cli terminals <<<";

#[derive(Clone, Debug)]
pub struct CliShellIntegration {
    home: PathBuf,
    zsh_directory: PathBuf,
    launcher: PathBuf,
}

impl CliShellIntegration {
    pub fn new(home: impl Into<PathBuf>, launcher: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            zsh_directory: home.clone(),
            home,
            launcher: launcher.into(),
        }
    }

    /// zsh uses ZDOTDIR in place of HOME for its startup files.
    pub fn with_zsh_directory(mut self, directory: PathBuf) -> Self {
        self.zsh_directory = directory;
        self
    }

    fn startup_files(&self) -> Vec<(Shell, PathBuf)> {
        // Interactive login bash reads only the first existing file in this order. Editors
        // commonly run non-login bash instead, which reads .bashrc directly.
        let bash_login = [".bash_profile", ".bash_login", ".profile"]
            .into_iter()
            .map(|name| self.home.join(name))
            .find(|path| path.symlink_metadata().is_ok())
            .unwrap_or_else(|| self.home.join(".bash_profile"));
        vec![
            (Shell::Zsh, self.zsh_directory.join(".zshrc")),
            (Shell::Bash, self.home.join(".bashrc")),
            (Shell::Bash, bash_login),
        ]
    }

    pub fn launcher_available(&self) -> bool {
        self.launcher.is_file()
            && self
                .launcher
                .parent()
                .is_some_and(|parent| parent.join("multiplex-session-host").is_file())
    }

    pub fn status(&self) -> IntegrationStatus {
        let mut found = 0;
        let files = self.startup_files();
        for (shell, path) in &files {
            match read_optional(path) {
                Ok(Some(contents)) => {
                    match find_marked_blocks(&contents, &[BLOCK_START], &[BLOCK_END]) {
                        Ok(blocks) if blocks.is_empty() => {}
                        Ok(blocks)
                            if blocks.len() == 1
                                && contents.contains(&self.startup_block(*shell)) =>
                        {
                            found += 1
                        }
                        _ => return IntegrationStatus::Partial,
                    }
                }
                Ok(None) => {}
                Err(_) => return IntegrationStatus::Partial,
            }
        }
        if found == 0 {
            IntegrationStatus::Off
        } else if found == files.len() {
            IntegrationStatus::On(vec![Shell::Zsh, Shell::Bash])
        } else {
            IntegrationStatus::Partial
        }
    }

    pub fn plan_enable(&self) -> Result<ChangePlan, IntegrationError> {
        self.plan(true)
    }

    pub fn plan_disable(&self) -> Result<ChangePlan, IntegrationError> {
        self.plan(false)
    }

    fn plan(&self, enable: bool) -> Result<ChangePlan, IntegrationError> {
        let mut plan = ChangePlan::default();
        let mut files = self.startup_files();
        if !enable {
            // A new login file may have shadowed the one we wrote. Remove our hooks from all
            // candidates, while preserving every other line and symlink.
            for name in [".bash_profile", ".bash_login", ".profile"] {
                let path = self.home.join(name);
                if !files.iter().any(|(_, existing)| existing == &path) {
                    files.push((Shell::Bash, path));
                }
            }
        }
        for (shell, path) in files {
            let write_path = resolve_write_path(&path)?;
            let before = read_optional(&write_path)?;
            let current = before.as_deref().unwrap_or_default();
            let without = remove_cli_blocks(current)
                .map_err(|()| IntegrationError::MalformedBlock(path.clone()))?;
            // Before any existing tmux hook or shell startup command. The hosted child reads
            // the original startup file with MULTIPLEX_SHELL_SESSION set and skips this block.
            let after = if enable {
                format!("{}\n{without}", self.startup_block(shell))
            } else {
                without
            };
            if current == after {
                continue;
            }
            // A newly created dotfile becomes empty on removal rather than being deleted: it
            // is user-owned once created, unlike an app-owned init file.
            plan.changes.push(FileChange {
                path,
                write_path,
                before,
                after: Some(after),
            });
        }
        Ok(plan)
    }

    pub fn startup_block(&self, shell: Shell) -> String {
        let launcher = shell_single_quote(&self.launcher.to_string_lossy());
        let host = self
            .launcher
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("multiplex-session-host");
        let host = shell_single_quote(&host.to_string_lossy());
        let name = shell.name();
        let login_test = match shell {
            Shell::Zsh => "[[ -o login ]]",
            Shell::Bash => "shopt -q login_shell",
        };
        let version = match shell {
            Shell::Zsh => "ZSH_VERSION",
            Shell::Bash => "BASH_VERSION",
        };
        format!(
            r#"{BLOCK_START}
case "$-" in *i*)
if [ -n "${{{version}:-}}" ] && [ -t 0 ] && [ -t 1 ] && [ -z "${{MULTIPLEX_SHELL_SESSION:-}}" ] && [ -z "${{MULTIPLEX_NO_WRAP:-}}" ] && [ -z "${{TERMIRUST_NO_WRAP:-}}" ] && [ -z "${{TMUX:-}}" ] && [ -z "${{SSH_CONNECTION:-}}" ] && [ "${{TERM_PROGRAM:-}}" != Multiplex ]; then
  if [ -x {launcher} ] && [ -x {host} ]; then
    case "${{SHELL##*/}}" in
      {name}) multiplex_cli_shell="$SHELL" ;;
      *) multiplex_cli_shell="$(command -v {name})" ;;
    esac
    if [ -n "$multiplex_cli_shell" ]; then
      if {login_test}; then
        MULTIPLEX_NO_WRAP=1 exec {launcher} shell -- "$multiplex_cli_shell" -il
      else
        MULTIPLEX_NO_WRAP=1 exec {launcher} shell -- "$multiplex_cli_shell" -i
      fi
    fi
    unset multiplex_cli_shell
  fi
fi
;; esac
{BLOCK_END}
"#
        )
    }
}

/// Our separator follows the prepended block. Keep the original startup bytes intact.
fn remove_cli_blocks(contents: &str) -> Result<String, ()> {
    let blocks = find_marked_blocks(contents, &[BLOCK_START], &[BLOCK_END])?;
    let lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    let mut removed = vec![false; lines.len()];
    for (start, end) in blocks {
        removed[start..=end].fill(true);
        if lines.get(end + 1).is_some_and(|line| *line == "\n") {
            removed[end + 1] = true;
        }
    }
    Ok(lines
        .iter()
        .zip(removed)
        .filter(|(_, removed)| !removed)
        .map(|(line, _)| *line)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn enable_is_idempotent_and_removal_preserves_startup_files() {
        let home = tempfile::tempdir().unwrap();
        for name in [".zshrc", ".bashrc", ".bash_profile"] {
            fs::write(home.path().join(name), "# my config\nexport EDITOR=vim\n").unwrap();
        }
        let integration = CliShellIntegration::new(
            home.path(),
            "/Applications/Multiplex.app/Contents/MacOS/multiplex-cli",
        );
        assert_eq!(integration.status(), IntegrationStatus::Off);
        integration.plan_enable().unwrap().apply().unwrap();
        assert!(matches!(integration.status(), IntegrationStatus::On(_)));
        assert!(integration.plan_enable().unwrap().is_empty());
        integration.plan_disable().unwrap().apply().unwrap();
        assert_eq!(integration.status(), IntegrationStatus::Off);
        for name in [".zshrc", ".bashrc", ".bash_profile"] {
            assert_eq!(
                fs::read_to_string(home.path().join(name)).unwrap(),
                "# my config\nexport EDITOR=vim\n"
            );
        }
    }

    #[test]
    fn changed_files_and_malformed_blocks_are_rejected() {
        let home = tempfile::tempdir().unwrap();
        let integration = CliShellIntegration::new(home.path(), "/bin/multiplex-cli");
        let plan = integration.plan_enable().unwrap();
        fs::write(home.path().join(".zshrc"), "# changed\n").unwrap();
        assert!(matches!(plan.apply(), Err(IntegrationError::Changed(_))));
        assert!(!home.path().join(".bashrc").exists());
        fs::write(home.path().join(".zshrc"), BLOCK_START).unwrap();
        assert!(matches!(
            integration.plan_enable(),
            Err(IntegrationError::MalformedBlock(_))
        ));
    }

    #[test]
    fn bash_login_precedence_and_custom_zdotdir_are_respected() {
        let home = tempfile::tempdir().unwrap();
        let zdotdir = tempfile::tempdir().unwrap();
        fs::write(home.path().join(".profile"), "# shared profile\n").unwrap();
        let integration = CliShellIntegration::new(home.path(), "/bin/multiplex-cli")
            .with_zsh_directory(zdotdir.path().to_owned());
        integration.plan_enable().unwrap().apply().unwrap();
        assert!(
            fs::read_to_string(zdotdir.path().join(".zshrc"))
                .unwrap()
                .contains(BLOCK_START)
        );
        assert!(!home.path().join(".zshrc").exists());
        assert!(!home.path().join(".bash_profile").exists());
        assert!(
            fs::read_to_string(home.path().join(".profile"))
                .unwrap()
                .contains(BLOCK_START)
        );
        fs::write(home.path().join(".bash_profile"), "# new login\n").unwrap();
        integration.plan_disable().unwrap().apply().unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join(".profile")).unwrap(),
            "# shared profile\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dotfile_manager_symlinks_are_preserved() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("managed-zshrc");
        fs::write(&target, "# managed\n").unwrap();
        std::os::unix::fs::symlink(&target, home.path().join(".zshrc")).unwrap();
        let integration = CliShellIntegration::new(home.path(), "/bin/multiplex-cli");
        integration.plan_enable().unwrap().apply().unwrap();
        integration.plan_disable().unwrap().apply().unwrap();
        assert!(
            home.path()
                .join(".zshrc")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(target).unwrap(), "# managed\n");
    }
}
