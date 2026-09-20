//! The opt-in change that starts new terminals inside tmux, so paired devices can reach them.
//!
//! Two kinds of file change, both previewed before they happen:
//!
//! - an app-owned init file per shell under `~/.config/termirust/`, safe to delete, and
//! - one marked block in that shell's startup file that sources it.
//!
//! Nothing here writes without a [`ChangePlan`] the user has seen. Applying a plan refuses
//! when any file changed after the preview, and disabling removes exactly the marked
//! block, so enabling then disabling restores the startup file byte for byte.

use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::appearance::{self, WrappedSessionAppearance};

/// First line of the block added to a shell startup file.
pub const BLOCK_START: &str = "# >>> termirust remote terminals >>>";
/// Last line of the block added to a shell startup file.
pub const BLOCK_END: &str = "# <<< termirust remote terminals <<<";
/// Set to any non-empty value in an app's environment to keep its terminals out of tmux.
pub const NO_WRAP_ENV: &str = "TERMIRUST_NO_WRAP";
/// `TERM_PROGRAM` values whose new terminals start inside tmux.
pub const WRAPPED_TERMINAL_PROGRAMS: [&str; 6] = [
    "Apple_Terminal",
    "zed",
    "iTerm.app",
    "ghostty",
    "WezTerm",
    "vscode",
];
/// `TERM_PROGRAM` values whose terminals understand synchronized updates. Terminal.app does
/// not, so its tabs are left to draw as they always have.
pub const SYNCHRONIZED_UPDATE_PROGRAMS: [&str; 5] =
    ["zed", "iTerm.app", "ghostty", "WezTerm", "vscode"];
/// Unchanged lines kept around each change in a preview.
pub const DIFF_CONTEXT_LINES: usize = 2;

const CONFIG_DIRECTORY: &str = ".config/termirust";
/// The app-owned tmux configuration wrapped tabs source, shared by every shell.
const TMUX_CONFIG_FILE: &str = "tmux.conf";
/// The first line of the file this app writes, and how it recognises its own. It still says
/// TermiRust: an installed copy has already written this line into shell startup files, and
/// what those files say moves with the rest of the installed identity and its migration.
const INIT_FILE_HEADER: &str = "# Managed by TermiRust. Turn off \"Open new terminals in tmux\" in TermiRust, or delete this file and the marked block in your shell startup file.";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Shell {
    Zsh,
    Bash,
}

impl Shell {
    pub const ALL: [Self; 2] = [Self::Zsh, Self::Bash];

    pub fn name(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
        }
    }

    /// The startup file an interactive shell of this kind reads.
    pub fn startup_file_name(self) -> &'static str {
        match self {
            Self::Zsh => ".zshrc",
            Self::Bash => ".bashrc",
        }
    }

    fn init_file_name(self) -> &'static str {
        match self {
            Self::Zsh => "shell-init.zsh",
            Self::Bash => "shell-init.bash",
        }
    }

    /// Recognizes a login shell path such as `/bin/zsh` or `/opt/homebrew/bin/bash`.
    pub fn from_login_shell(path: &str) -> Option<Self> {
        match Path::new(path).file_name()?.to_str()? {
            "zsh" => Some(Self::Zsh),
            "bash" => Some(Self::Bash),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrationError {
    /// Reading or writing a file failed.
    Io(io::ErrorKind),
    /// A file changed after its preview was made.
    Changed(PathBuf),
    /// A startup file holds a start marker without an end marker, so the block cannot be
    /// found safely.
    MalformedBlock(PathBuf),
    /// A path that should be a regular file is something else.
    NotAFile(PathBuf),
}

impl fmt::Display for IntegrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(kind) => write!(formatter, "file operation failed: {kind}"),
            Self::Changed(_) => formatter.write_str("a file changed after it was previewed"),
            Self::MalformedBlock(_) => {
                formatter.write_str("a startup file has an unterminated Multiplex block")
            }
            Self::NotAFile(_) => formatter.write_str("a startup file is not a regular file"),
        }
    }
}

impl std::error::Error for IntegrationError {}

impl From<io::Error> for IntegrationError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

/// Where the integration is installed right now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegrationStatus {
    Off,
    /// Every listed shell has both its init file and its startup block.
    On(Vec<Shell>),
    /// Some pieces exist without the rest, or a block is malformed. Enabling repairs it.
    Partial,
}

/// One file before and after a planned change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileChange {
    /// The path as the user knows it, such as `~/.zshrc`.
    pub path: PathBuf,
    /// The file actually written. Differs from `path` when a dotfile manager symlinks it.
    write_path: PathBuf,
    pub before: Option<String>,
    /// `None` deletes the file. Only app-owned init files are ever deleted.
    pub after: Option<String>,
}

impl FileChange {
    /// A compact line diff with [`DIFF_CONTEXT_LINES`] of context.
    pub fn diff(&self) -> Vec<DiffLine> {
        line_diff(
            self.before.as_deref().unwrap_or_default(),
            self.after.as_deref().unwrap_or_default(),
        )
    }

    pub fn creates(&self) -> bool {
        self.before.is_none() && self.after.is_some()
    }

    pub fn deletes(&self) -> bool {
        self.before.is_some() && self.after.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
    /// A run of unchanged lines left out of the preview.
    Skipped(usize),
}

impl DiffLine {
    /// The line with a `+`, `-`, or space gutter. `Skipped` has no text of its own.
    pub fn gutter_text(&self) -> Option<String> {
        match self {
            Self::Context(line) => Some(format!("  {line}")),
            Self::Added(line) => Some(format!("+ {line}")),
            Self::Removed(line) => Some(format!("- {line}")),
            Self::Skipped(_) => None,
        }
    }
}

/// A reviewed set of file changes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangePlan {
    pub changes: Vec<FileChange>,
}

impl ChangePlan {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Writes the plan. Every file is checked against its preview before anything is
    /// written, and each write replaces the file atomically. Init files are written before
    /// startup files, so a failure part way never leaves a block pointing at nothing.
    pub fn apply(&self) -> Result<(), IntegrationError> {
        for change in &self.changes {
            if read_optional(&change.write_path)? != change.before {
                return Err(IntegrationError::Changed(change.path.clone()));
            }
        }
        for change in &self.changes {
            match &change.after {
                Some(contents) => write_atomically(&change.write_path, contents)?,
                None => match fs::remove_file(&change.write_path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                },
            }
        }
        Ok(())
    }
}

/// The shell integration for one home directory and one tmux binary.
#[derive(Clone, Debug)]
pub struct ShellIntegration {
    home: PathBuf,
    tmux: PathBuf,
    appearance: WrappedSessionAppearance,
}

impl ShellIntegration {
    /// `tmux` should be an absolute path, so the wrapper keeps working when a terminal app
    /// starts with a minimal `PATH`.
    pub fn new(home: impl Into<PathBuf>, tmux: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            tmux: tmux.into(),
            appearance: WrappedSessionAppearance::for_this_platform(),
        }
    }

    /// Uses `appearance` instead of this platform's, so tests render the same file everywhere.
    pub fn with_appearance(mut self, appearance: WrappedSessionAppearance) -> Self {
        self.appearance = appearance;
        self
    }

    /// The shells to set up: the login shell, plus any supported shell whose startup file
    /// already exists.
    pub fn target_shells(&self, login_shell: Option<&str>) -> Vec<Shell> {
        let login = login_shell.and_then(Shell::from_login_shell);
        Shell::ALL
            .into_iter()
            .filter(|shell| {
                Some(*shell) == login || self.startup_path(*shell).symlink_metadata().is_ok()
            })
            .collect()
    }

    pub fn status(&self) -> IntegrationStatus {
        let mut installed = Vec::new();
        let mut partial = false;
        for shell in Shell::ALL {
            // An init file written by an older version counts as partial, so enabling again
            // offers to update it.
            let init = match read_optional(&self.init_path(shell)) {
                Ok(Some(contents)) if contents == self.init_file(shell) => true,
                Ok(Some(_)) => {
                    partial = true;
                    true
                }
                Ok(None) => false,
                Err(_) => {
                    partial = true;
                    false
                }
            };
            let block = match read_optional(&self.startup_path(shell)) {
                Ok(Some(contents)) => match find_blocks(&contents) {
                    Ok(blocks) => !blocks.is_empty(),
                    Err(()) => {
                        partial = true;
                        false
                    }
                },
                Ok(None) => false,
                Err(_) => {
                    partial = true;
                    false
                }
            };
            match (init, block) {
                (true, true) => installed.push(shell),
                (false, false) => {}
                _ => partial = true,
            }
        }
        // The tmux configuration belongs with the init files: missing or outdated while they
        // are installed, or left behind without them, needs a repair.
        match read_optional(&self.tmux_config_path()) {
            Ok(Some(contents)) => {
                if installed.is_empty() || contents != self.appearance.configuration_file() {
                    partial = true;
                }
            }
            Ok(None) => partial |= !installed.is_empty(),
            Err(_) => partial = true,
        }
        match (partial, installed.is_empty()) {
            (true, _) => IntegrationStatus::Partial,
            (false, true) => IntegrationStatus::Off,
            (false, false) => IntegrationStatus::On(installed),
        }
    }

    /// Brings Multiplex's own tmux configuration up to this version, when it is installed and
    /// still names itself managed in its first line. Returns whether the file was rewritten.
    ///
    /// Every new wrapped tab sources this file, and key bindings are global to the tmux server,
    /// so a server brought up to date while the file stays old is put back the next time a tab
    /// opens. That is how a click in a scrolled-back tab kept leaving copy mode and dropping the
    /// reader at the bottom long after it had been fixed: the file only changed through a repair
    /// in Settings that nothing prompted anyone to find. This file is Multiplex's alone and says
    /// so; the user's shell startup files still change only through a reviewed plan, and a
    /// configuration file whose first line is no longer ours is left as it is.
    pub fn refresh_managed_config(&self) -> Result<bool, IntegrationError> {
        let path = self.tmux_config_path();
        let Some(installed) = read_optional(&path)? else {
            return Ok(false);
        };
        let current = self.appearance.configuration_file();
        if installed == current
            || installed.lines().next() != Some(appearance::CONFIGURATION_FILE_HEADER)
        {
            return Ok(false);
        }
        write_atomically(&path, &current)?;
        Ok(true)
    }

    /// Changes that install or repair the integration for `shells`. Empty when everything
    /// is already in place.
    pub fn plan_enable(&self, shells: &[Shell]) -> Result<ChangePlan, IntegrationError> {
        let mut plan = ChangePlan::default();
        // The tmux configuration first: an init file that sources it never runs before it exists.
        let config_path = self.tmux_config_path();
        let before = read_optional(&config_path)?;
        let after = self.appearance.configuration_file();
        if before.as_deref() != Some(after.as_str()) {
            plan.changes.push(FileChange {
                path: self.display_path(&config_path),
                write_path: config_path,
                before,
                after: Some(after),
            });
        }
        for shell in shells {
            let init_path = self.init_path(*shell);
            let before = read_optional(&init_path)?;
            let after = self.init_file(*shell);
            if before.as_deref() != Some(after.as_str()) {
                plan.changes.push(FileChange {
                    path: self.display_path(&init_path),
                    write_path: init_path,
                    before,
                    after: Some(after),
                });
            }
        }
        for shell in shells {
            let path = self.startup_path(*shell);
            let write_path = resolve_write_path(&path)?;
            let before = read_optional(&write_path)?;
            let current = before.clone().unwrap_or_default();
            let without = remove_blocks(&current)
                .map_err(|()| IntegrationError::MalformedBlock(self.display_path(&path)))?;
            let after = append_block(&without, &self.startup_block(*shell));
            if before.as_deref() != Some(after.as_str()) {
                plan.changes.push(FileChange {
                    path: self.display_path(&path),
                    write_path,
                    before,
                    after: Some(after),
                });
            }
        }
        Ok(plan)
    }

    /// Changes that remove every piece of the integration, for every supported shell.
    pub fn plan_disable(&self) -> Result<ChangePlan, IntegrationError> {
        let mut plan = ChangePlan::default();
        for shell in Shell::ALL {
            let path = self.startup_path(shell);
            let write_path = resolve_write_path(&path)?;
            let Some(before) = read_optional(&write_path)? else {
                continue;
            };
            let after = remove_blocks(&before)
                .map_err(|()| IntegrationError::MalformedBlock(self.display_path(&path)))?;
            if after != before {
                plan.changes.push(FileChange {
                    path: self.display_path(&path),
                    write_path,
                    before: Some(before),
                    after: Some(after),
                });
            }
        }
        for shell in Shell::ALL {
            let init_path = self.init_path(shell);
            if let Some(before) = read_optional(&init_path)? {
                plan.changes.push(FileChange {
                    path: self.display_path(&init_path),
                    write_path: init_path,
                    before: Some(before),
                    after: None,
                });
            }
        }
        let config_path = self.tmux_config_path();
        if let Some(before) = read_optional(&config_path)? {
            plan.changes.push(FileChange {
                path: self.display_path(&config_path),
                write_path: config_path,
                before: Some(before),
                after: None,
            });
        }
        // Blocks go first so a failure part way never leaves one pointing at nothing.
        Ok(plan)
    }

    fn init_path(&self, shell: Shell) -> PathBuf {
        self.home
            .join(CONFIG_DIRECTORY)
            .join(shell.init_file_name())
    }

    fn tmux_config_path(&self) -> PathBuf {
        self.home.join(CONFIG_DIRECTORY).join(TMUX_CONFIG_FILE)
    }

    fn startup_path(&self, shell: Shell) -> PathBuf {
        self.home.join(shell.startup_file_name())
    }

    fn display_path(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.home)
            .map(|relative| Path::new("~").join(relative))
            .unwrap_or_else(|_| path.to_path_buf())
    }

    fn startup_block(&self, shell: Shell) -> String {
        format!(
            "{BLOCK_START}\n[ -f \"$HOME/{CONFIG_DIRECTORY}/{name}\" ] && . \"$HOME/{CONFIG_DIRECTORY}/{name}\"\n{BLOCK_END}\n",
            name = shell.init_file_name()
        )
    }

    fn init_file(&self, shell: Shell) -> String {
        let tmux = shell_single_quote(&self.tmux.to_string_lossy());
        let programs = WRAPPED_TERMINAL_PROGRAMS.join("|");
        let config = shell_single_quote(&self.tmux_config_path().to_string_lossy());
        // `&& exit` rather than `exec`: if tmux cannot start, the terminal keeps a plain
        // shell instead of closing the moment it opens. Sourcing the app's tmux configuration
        // right after `new-session` applies its options to that session only; `-q` keeps a
        // missing file from failing the tab.
        //
        // The tab tells tmux what its terminal can do, because tmux only works that out for
        // itself from a terminal that answers its questions. Without 24-bit color, tmux
        // converts every color to the nearest of 256; without synchronized updates, the
        // terminal paints half-drawn frames while a program redraws.
        let utf8 = appearance::UTF8_FLAG;
        let features = appearance::FEATURES_FLAG;
        let truecolor = appearance::TRUECOLOR_FEATURE;
        let sync = appearance::SYNCHRONIZED_UPDATE_FEATURE;
        let sync_programs = SYNCHRONIZED_UPDATE_PROGRAMS.join("|");
        let session = match shell {
            Shell::Zsh => "multiplex-${PWD:t}-$$",
            Shell::Bash => "multiplex-${PWD##*/}-$$",
        };
        let start = format!(
            "{tmux} {utf8} $termirust_features new-session -s \"{session}\" \\; source-file -q {config} && exit"
        );
        match shell {
            Shell::Zsh => format!(
                "{INIT_FILE_HEADER}\nif [[ -o interactive && -z \"$TMUX\" && -z \"${NO_WRAP_ENV}\" ]]; then\n  case \"$TERM_PROGRAM\" in\n    {programs})\n      if [[ -x {tmux} ]]; then\n        termirust_terminal=()\n        [[ $COLORTERM == (truecolor|24bit) ]] && termirust_terminal+={truecolor}\n        case \"$TERM_PROGRAM\" in\n          {sync_programs}) termirust_terminal+={sync} ;;\n        esac\n        termirust_features=()\n        (( ${{#termirust_terminal}} )) && termirust_features=({features} ${{(j:,:)termirust_terminal}})\n        {start}\n        unset termirust_terminal termirust_features\n      fi\n      ;;\n  esac\nfi\n"
            ),
            Shell::Bash => format!(
                "{INIT_FILE_HEADER}\nif [[ $- == *i* && -z \"$TMUX\" && -z \"${NO_WRAP_ENV}\" ]]; then\n  case \"$TERM_PROGRAM\" in\n    {programs})\n      if [[ -x {tmux} ]]; then\n        termirust_terminal=\"\"\n        case \"$COLORTERM\" in\n          truecolor|24bit) termirust_terminal={truecolor} ;;\n        esac\n        case \"$TERM_PROGRAM\" in\n          {sync_programs}) termirust_terminal=\"${{termirust_terminal:+$termirust_terminal,}}{sync}\" ;;\n        esac\n        termirust_features=\"\"\n        [ -n \"$termirust_terminal\" ] && termirust_features=\"{features} $termirust_terminal\"\n        {start}\n        unset termirust_terminal termirust_features\n      fi\n      ;;\n  esac\nfi\n"
            ),
        }
    }
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Line ranges of complete blocks. `Err` when a start marker has no end marker.
fn find_blocks(contents: &str) -> Result<Vec<(usize, usize)>, ()> {
    let lines = contents.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut start = None;
    for (index, line) in lines.iter().enumerate() {
        match line.trim_end() {
            BLOCK_START if start.is_none() => start = Some(index),
            BLOCK_START => return Err(()),
            BLOCK_END => match start.take() {
                Some(first) => blocks.push((first, index)),
                None => return Err(()),
            },
            _ => {}
        }
    }
    if start.is_some() {
        return Err(());
    }
    Ok(blocks)
}

/// Removes every block, plus the one blank line [`append_block`] puts before it.
fn remove_blocks(contents: &str) -> Result<String, ()> {
    let blocks = find_blocks(contents)?;
    if blocks.is_empty() {
        return Ok(contents.to_owned());
    }
    let lines = contents.split_inclusive('\n').collect::<Vec<_>>();
    let mut removed = vec![false; lines.len()];
    for (start, end) in blocks {
        removed[start..=end].fill(true);
        if start > 0 && lines[start - 1].trim().is_empty() && !removed[start - 1] {
            removed[start - 1] = true;
        }
    }
    Ok(lines
        .iter()
        .zip(removed)
        .filter(|(_, removed)| !removed)
        .map(|(line, _)| *line)
        .collect())
}

fn append_block(contents: &str, block: &str) -> String {
    let mut result = contents.to_owned();
    if !result.is_empty() {
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push('\n');
    }
    result.push_str(block);
    result
}

fn read_optional(path: &Path) -> Result<Option<String>, IntegrationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(fs::read_to_string(path)?)),
        Ok(metadata) if metadata.file_type().is_symlink() => match fs::metadata(path) {
            Ok(target) if target.is_file() => Ok(Some(fs::read_to_string(path)?)),
            Ok(_) => Err(IntegrationError::NotAFile(path.to_path_buf())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        },
        Ok(_) => Err(IntegrationError::NotAFile(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Follows a symlinked startup file to the file a dotfile manager keeps, so replacing it
/// never turns the link into a copy.
fn resolve_write_path(path: &Path) -> Result<PathBuf, IntegrationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => match fs::canonicalize(path) {
            Ok(target) => Ok(target),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(IntegrationError::NotAFile(path.to_path_buf()))
            }
            Err(error) => Err(error.into()),
        },
        _ => Ok(path.to_path_buf()),
    }
}

fn write_atomically(path: &Path, contents: &str) -> Result<(), IntegrationError> {
    let directory = path
        .parent()
        .ok_or(IntegrationError::NotAFile(path.to_path_buf()))?;
    fs::create_dir_all(directory)?;
    let file_name = path
        .file_name()
        .ok_or(IntegrationError::NotAFile(path.to_path_buf()))?
        .to_string_lossy();
    let temporary = directory.join(format!(".{file_name}.termirust-{}", std::process::id()));
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    Ok(result?)
}

fn line_diff(before: &str, after: &str) -> Vec<DiffLine> {
    let old = before.lines().collect::<Vec<_>>();
    let new = after.lines().collect::<Vec<_>>();
    let prefix = old
        .iter()
        .zip(&new)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let mut lines = Vec::new();
    let leading_context_start = prefix.saturating_sub(DIFF_CONTEXT_LINES);
    if leading_context_start > 0 {
        lines.push(DiffLine::Skipped(leading_context_start));
    }
    lines.extend(
        old[leading_context_start..prefix]
            .iter()
            .map(|line| DiffLine::Context((*line).to_owned())),
    );
    lines.extend(
        old[prefix..old.len() - suffix]
            .iter()
            .map(|line| DiffLine::Removed((*line).to_owned())),
    );
    lines.extend(
        new[prefix..new.len() - suffix]
            .iter()
            .map(|line| DiffLine::Added((*line).to_owned())),
    );
    let trailing = &old[old.len() - suffix..];
    let shown = trailing.len().min(DIFF_CONTEXT_LINES);
    lines.extend(
        trailing[..shown]
            .iter()
            .map(|line| DiffLine::Context((*line).to_owned())),
    );
    if trailing.len() > shown {
        lines.push(DiffLine::Skipped(trailing.len() - shown));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const TMUX: &str = "/opt/homebrew/Cellar/tmux/3.7c/bin/tmux";

    fn home() -> (tempfile::TempDir, ShellIntegration) {
        let home = tempfile::tempdir().unwrap();
        let integration = ShellIntegration::new(home.path(), TMUX);
        (home, integration)
    }

    #[test]
    fn an_outdated_managed_tmux_config_is_brought_up_to_date() {
        let (_home, integration) = home();
        let path = integration.tmux_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // As Multiplex wrote it on 2026-09-15, when a click in a scrolled-back tab still left
        // copy mode and dropped the reader at the bottom.
        let older = [
            appearance::CONFIGURATION_FILE_HEADER,
            r##"bind-key -T copy-mode MouseDown1Pane 'if-shell -F "#{m:termirust-*,#{session_name}}" "select-pane ; send-keys -X cancel" select-pane'"##,
        ]
        .join("\n");
        fs::write(&path, older).unwrap();

        assert!(integration.refresh_managed_config().unwrap());
        let refreshed = fs::read_to_string(&path).unwrap();
        assert_eq!(refreshed, integration.appearance.configuration_file());
        assert!(refreshed.contains("#{==:#{scroll_position},0}"));
        // Once it is current there is nothing left to do.
        assert!(!integration.refresh_managed_config().unwrap());
    }

    #[test]
    fn a_tmux_config_that_is_not_ours_or_not_there_is_left_alone() {
        let (_home, integration) = home();
        let path = integration.tmux_config_path();
        // Not installed: nothing is created, since the integration is off.
        assert!(!integration.refresh_managed_config().unwrap());
        assert!(!path.exists());

        // Taken over: its first line is no longer Multiplex's.
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let theirs = "# mine now\nset -g status on\n";
        fs::write(&path, theirs).unwrap();
        assert!(!integration.refresh_managed_config().unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), theirs);
    }

    #[test]
    fn enabling_writes_init_files_then_one_block_and_reports_on() {
        let (home, integration) = home();
        let original = "export PATH=\"$HOME/bin:$PATH\"\nalias ll='ls -l'\n";
        fs::write(home.path().join(".zshrc"), original).unwrap();
        assert_eq!(integration.status(), IntegrationStatus::Off);

        let plan = integration.plan_enable(&[Shell::Zsh]).unwrap();
        let paths = plan
            .changes
            .iter()
            .map(|change| change.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                PathBuf::from("~/.config/termirust/tmux.conf"),
                PathBuf::from("~/.config/termirust/shell-init.zsh"),
                PathBuf::from("~/.zshrc")
            ]
        );
        assert!(plan.changes[0].creates());
        plan.apply().unwrap();

        let zshrc = fs::read_to_string(home.path().join(".zshrc")).unwrap();
        assert!(zshrc.starts_with(original));
        assert_eq!(zshrc.matches(BLOCK_START).count(), 1);
        let init =
            fs::read_to_string(home.path().join(".config/termirust/shell-init.zsh")).unwrap();
        assert!(init.contains(&format!("'{TMUX}' -u $termirust_features new-session")));
        // What the terminal can do is worked out before tmux starts.
        assert!(init.contains("$COLORTERM == (truecolor|24bit)"));
        assert!(init.contains("zed|iTerm.app|ghostty|WezTerm|vscode) termirust_terminal+=sync"));
        assert!(init.contains("unset termirust_terminal termirust_features"));
        assert!(init.contains("Apple_Terminal|zed|"));
        assert!(init.contains("&& exit"));
        assert!(!init.contains("exec "));
        assert_eq!(
            integration.status(),
            IntegrationStatus::On(vec![Shell::Zsh])
        );
        assert!(
            integration.plan_enable(&[Shell::Zsh]).unwrap().is_empty(),
            "enabling twice changes nothing"
        );
    }

    #[test]
    fn disabling_restores_the_startup_file_byte_for_byte() {
        for original in [
            "",
            "setopt autocd\n",
            "setopt autocd\n\n# trailing comment\n",
            "no trailing newline",
        ] {
            let (home, integration) = home();
            let zshrc = home.path().join(".zshrc");
            if !original.is_empty() {
                fs::write(&zshrc, original).unwrap();
            }
            integration
                .plan_enable(&[Shell::Zsh])
                .unwrap()
                .apply()
                .unwrap();
            integration.plan_disable().unwrap().apply().unwrap();
            let restored = fs::read_to_string(&zshrc).unwrap();
            let expected = if original.is_empty() || original.ends_with('\n') {
                original.to_owned()
            } else {
                // A missing final newline is the one thing a round trip adds.
                format!("{original}\n")
            };
            assert_eq!(restored, expected, "original: {original:?}");
            assert!(
                !home
                    .path()
                    .join(".config/termirust/shell-init.zsh")
                    .exists()
            );
            assert_eq!(integration.status(), IntegrationStatus::Off);
        }
    }

    #[test]
    fn user_edits_around_the_block_survive_enable_and_disable() {
        let (home, integration) = home();
        let zshrc = home.path().join(".zshrc");
        fs::write(&zshrc, "first\n").unwrap();
        integration
            .plan_enable(&[Shell::Zsh])
            .unwrap()
            .apply()
            .unwrap();
        let mut edited = fs::read_to_string(&zshrc).unwrap();
        edited.push_str("added after\n");
        fs::write(&zshrc, &edited).unwrap();
        integration.plan_disable().unwrap().apply().unwrap();
        assert_eq!(fs::read_to_string(&zshrc).unwrap(), "first\nadded after\n");
    }

    #[test]
    fn duplicated_blocks_are_normalized_and_all_removed() {
        let (home, integration) = home();
        let zshrc = home.path().join(".zshrc");
        let block = integration.startup_block(Shell::Zsh);
        fs::write(&zshrc, format!("a\n\n{block}b\n\n{block}")).unwrap();
        let plan = integration.plan_enable(&[Shell::Zsh]).unwrap();
        plan.apply().unwrap();
        let contents = fs::read_to_string(&zshrc).unwrap();
        assert_eq!(contents.matches(BLOCK_START).count(), 1);
        assert!(contents.starts_with("a\nb\n"));
        integration.plan_disable().unwrap().apply().unwrap();
        assert_eq!(fs::read_to_string(&zshrc).unwrap(), "a\nb\n");
    }

    #[test]
    fn a_file_edited_after_preview_is_not_overwritten() {
        let (home, integration) = home();
        let zshrc = home.path().join(".zshrc");
        fs::write(&zshrc, "before\n").unwrap();
        let plan = integration.plan_enable(&[Shell::Zsh]).unwrap();
        fs::write(&zshrc, "edited meanwhile\n").unwrap();
        assert_eq!(
            plan.apply(),
            Err(IntegrationError::Changed(PathBuf::from("~/.zshrc")))
        );
        assert_eq!(fs::read_to_string(&zshrc).unwrap(), "edited meanwhile\n");
        assert!(
            !home
                .path()
                .join(".config/termirust/shell-init.zsh")
                .exists(),
            "nothing is written when any file changed"
        );
    }

    #[test]
    fn unterminated_blocks_are_refused_and_reported_partial() {
        let (home, integration) = home();
        let zshrc = home.path().join(".zshrc");
        fs::write(&zshrc, format!("a\n{BLOCK_START}\nsource something\n")).unwrap();
        assert_eq!(integration.status(), IntegrationStatus::Partial);
        assert_eq!(
            integration.plan_enable(&[Shell::Zsh]),
            Err(IntegrationError::MalformedBlock(PathBuf::from("~/.zshrc")))
        );
        assert_eq!(
            integration.plan_disable(),
            Err(IntegrationError::MalformedBlock(PathBuf::from("~/.zshrc")))
        );
        fs::write(&zshrc, format!("{BLOCK_END}\n")).unwrap();
        assert!(integration.plan_disable().is_err());
    }

    #[test]
    fn wrapped_tabs_source_the_tmux_configuration_and_older_files_are_updated() {
        let (home, integration) = home();
        integration
            .plan_enable(&[Shell::Zsh])
            .unwrap()
            .apply()
            .unwrap();
        // Built the way the code builds them: a path written with one separator and read back
        // with the platform's own does not compare equal on Windows.
        let init_path = home
            .path()
            .join(CONFIG_DIRECTORY)
            .join(Shell::Zsh.init_file_name());
        let config_path = home.path().join(CONFIG_DIRECTORY).join(TMUX_CONFIG_FILE);
        let init = fs::read_to_string(&init_path).unwrap();
        assert!(init.contains(&format!(
            "new-session -s \"multiplex-${{PWD:t}}-$$\" \\; source-file -q '{}' && exit",
            config_path.display()
        )));
        let config = fs::read_to_string(&config_path).unwrap();
        assert!(config.contains("\nset-option mouse on\n"));

        // The init file an earlier version wrote, which set the status bar itself.
        let old_line = format!(" \\; source-file -q '{}'", config_path.display());
        fs::write(
            &init_path,
            init.replace(&old_line, " \\; set-option status off"),
        )
        .unwrap();
        // And a tmux configuration someone edited.
        fs::write(&config_path, "set -g mouse off\n").unwrap();
        assert_eq!(integration.status(), IntegrationStatus::Partial);
        let update = integration.plan_enable(&[Shell::Zsh]).unwrap();
        assert_eq!(
            update
                .changes
                .iter()
                .map(|change| change.path.clone())
                .collect::<Vec<_>>(),
            [
                PathBuf::from("~/.config/termirust/tmux.conf"),
                PathBuf::from("~/.config/termirust/shell-init.zsh"),
            ]
        );
        update.apply().unwrap();
        assert_eq!(
            integration.status(),
            IntegrationStatus::On(vec![Shell::Zsh])
        );

        integration.plan_disable().unwrap().apply().unwrap();
        assert!(
            !config_path.exists(),
            "removing the setup deletes the tmux configuration"
        );
        assert_eq!(integration.status(), IntegrationStatus::Off);
    }

    #[test]
    fn missing_init_file_is_partial_and_enable_repairs_it() {
        let (home, integration) = home();
        integration
            .plan_enable(&[Shell::Zsh])
            .unwrap()
            .apply()
            .unwrap();
        fs::remove_file(home.path().join(".config/termirust/shell-init.zsh")).unwrap();
        assert_eq!(integration.status(), IntegrationStatus::Partial);
        let repair = integration.plan_enable(&[Shell::Zsh]).unwrap();
        assert_eq!(repair.changes.len(), 1);
        assert!(repair.changes[0].creates());
        repair.apply().unwrap();
        assert_eq!(
            integration.status(),
            IntegrationStatus::On(vec![Shell::Zsh])
        );
    }

    #[test]
    fn a_moved_tmux_updates_only_the_init_file() {
        let (home, integration) = home();
        integration
            .plan_enable(&[Shell::Zsh])
            .unwrap()
            .apply()
            .unwrap();
        let moved = ShellIntegration::new(home.path(), "/usr/local/bin/tmux");
        let plan = moved.plan_enable(&[Shell::Zsh]).unwrap();
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(
            plan.changes[0].path,
            PathBuf::from("~/.config/termirust/shell-init.zsh")
        );
        let diff = plan.changes[0].diff();
        assert!(diff.iter().any(
            |line| matches!(line, DiffLine::Added(text) if text.contains("/usr/local/bin/tmux"))
        ));
        assert!(
            diff.iter().any(
                |line| matches!(line, DiffLine::Removed(text) if text.contains("Cellar/tmux"))
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_startup_files_are_edited_through_the_link() {
        let (home, integration) = home();
        let dotfiles = home.path().join("dotfiles");
        fs::create_dir(&dotfiles).unwrap();
        fs::write(dotfiles.join("zshrc"), "managed\n").unwrap();
        std::os::unix::fs::symlink(dotfiles.join("zshrc"), home.path().join(".zshrc")).unwrap();
        integration
            .plan_enable(&[Shell::Zsh])
            .unwrap()
            .apply()
            .unwrap();
        assert!(
            fs::symlink_metadata(home.path().join(".zshrc"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link must stay a link"
        );
        assert!(
            fs::read_to_string(dotfiles.join("zshrc"))
                .unwrap()
                .contains(BLOCK_START)
        );
        integration.plan_disable().unwrap().apply().unwrap();
        assert_eq!(
            fs::read_to_string(dotfiles.join("zshrc")).unwrap(),
            "managed\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn startup_file_permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt as _;

        let (home, integration) = home();
        let zshrc = home.path().join(".zshrc");
        fs::write(&zshrc, "x\n").unwrap();
        fs::set_permissions(&zshrc, fs::Permissions::from_mode(0o600)).unwrap();
        integration
            .plan_enable(&[Shell::Zsh])
            .unwrap()
            .apply()
            .unwrap();
        assert_eq!(
            fs::metadata(&zshrc).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn target_shells_follow_login_shell_and_existing_startup_files() {
        let (home, integration) = home();
        assert_eq!(integration.target_shells(Some("/bin/zsh")), [Shell::Zsh]);
        assert!(integration.target_shells(Some("/usr/bin/fish")).is_empty());
        fs::write(home.path().join(".bashrc"), "").unwrap();
        assert_eq!(
            integration.target_shells(Some("/opt/homebrew/bin/zsh")),
            [Shell::Zsh, Shell::Bash]
        );
        assert_eq!(integration.target_shells(None), [Shell::Bash]);
    }

    #[test]
    fn tmux_paths_are_single_quoted_for_the_shell() {
        assert_eq!(shell_single_quote("/a b/tmux"), "'/a b/tmux'");
        assert_eq!(shell_single_quote("/it's/tmux"), "'/it'\\''s/tmux'");
    }

    #[test]
    fn diffs_show_changes_with_bounded_context() {
        let before = (1..=10).map(|n| format!("line {n}\n")).collect::<String>();
        let after = format!("{before}\n{BLOCK_START}\nsource\n{BLOCK_END}\n");
        let diff = line_diff(&before, &after);
        assert_eq!(
            diff,
            [
                DiffLine::Skipped(8),
                DiffLine::Context("line 9".into()),
                DiffLine::Context("line 10".into()),
                DiffLine::Added(String::new()),
                DiffLine::Added(BLOCK_START.into()),
                DiffLine::Added("source".into()),
                DiffLine::Added(BLOCK_END.into()),
            ]
        );
        assert_eq!(
            DiffLine::Added("x".into()).gutter_text().as_deref(),
            Some("+ x")
        );
        assert_eq!(
            DiffLine::Removed("x".into()).gutter_text().as_deref(),
            Some("- x")
        );
        assert_eq!(DiffLine::Skipped(3).gutter_text(), None);
        let middle = line_diff("a\nb\nc\nd\ne\nf\n", "a\nb\nc\nX\ne\nf\n");
        assert_eq!(
            middle,
            [
                DiffLine::Skipped(1),
                DiffLine::Context("b".into()),
                DiffLine::Context("c".into()),
                DiffLine::Removed("d".into()),
                DiffLine::Added("X".into()),
                DiffLine::Context("e".into()),
                DiffLine::Context("f".into()),
            ]
        );
    }
}
