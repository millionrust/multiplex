//! A "Multiplex" profile in the terminals people already use, so the ones they open with it run
//! through `multiplex-cli shell` and paired devices can reach them.
//!
//! Unlike the tmux startup change, nothing here touches how any shell starts: only a window opened
//! with the Multiplex profile is wrapped, and scripts, `cmd /c`, `powershell -Command`, and every
//! other program that starts a shell keep starting the plain one. Each change is previewed as a
//! [`ChangePlan`] before it is written and removed exactly:
//!
//! - **Windows Terminal** loads profiles from a *fragment*, a file of its own under
//!   `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\Multiplex\`, so its `settings.json` is
//!   never edited. Removing the file removes the profile.
//! - **iTerm2** loads *dynamic profiles* from files under
//!   `~/Library/Application Support/iTerm2/DynamicProfiles/`, the same way.
//! - **VS Code** has no such folder, so one marked block is inserted at the top of its user
//!   `settings.json` and removed byte for byte. When the file already sets terminal profiles for
//!   this platform, nothing is merged into them: the block would be overridden or would override,
//!   and neither is safe to do unseen, so the status says so and shows the text to add by hand.

use std::path::{Path, PathBuf};

use crate::shell_integration::{ChangePlan, FileChange, IntegrationError, read_optional};

/// The name the profile has in every terminal.
pub const PROFILE_NAME: &str = "Multiplex";
/// Marks the block this module adds to VS Code's settings; JSON with comments allows it.
pub const VSCODE_BLOCK_START: &str = "// >>> multiplex terminal profile >>>";
pub const VSCODE_BLOCK_END: &str = "// <<< multiplex terminal profile <<<";
/// The iTerm2 profile's fixed identity, so installing twice replaces rather than duplicates.
const ITERM2_PROFILE_GUID: &str = "6D8F3B51-3F2C-4E47-9C39-5E7A1B0F4D21";

/// A terminal that can offer the Multiplex profile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProfileTarget {
    WindowsTerminal,
    VsCode,
    ITerm2,
}

impl ProfileTarget {
    pub const ALL: [Self; 3] = [Self::WindowsTerminal, Self::VsCode, Self::ITerm2];

    pub fn label(self) -> &'static str {
        match self {
            Self::WindowsTerminal => "Windows Terminal",
            Self::VsCode => "Visual Studio Code",
            Self::ITerm2 => "iTerm2",
        }
    }
}

/// The platform whose conventions paths and settings keys follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// The suffix VS Code's per-platform terminal settings use.
    fn vscode_key(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::MacOs => "osx",
            Self::Linux => "linux",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileStatus {
    /// The terminal is not installed here.
    Unavailable,
    Off,
    On,
    /// The profile is there but points at another copy of the launcher, as after the app moved;
    /// adding it again repairs it.
    Outdated,
    /// VS Code's settings already set terminal profiles for this platform, which this does not
    /// merge into. `manual` is the text a person can add to them.
    NeedsManualEdit {
        manual: String,
    },
}

/// Where each terminal keeps its settings on this machine, and the launcher a profile runs.
#[derive(Clone, Debug)]
pub struct TerminalProfiles {
    platform: Platform,
    home: PathBuf,
    /// `%LOCALAPPDATA%` on Windows.
    local_app_data: Option<PathBuf>,
    /// `%APPDATA%` on Windows.
    app_data: Option<PathBuf>,
    launcher: PathBuf,
}

impl TerminalProfiles {
    /// The profiles for this machine, run by `launcher`, the `multiplex-cli` beside the app.
    pub fn for_this_machine(home: impl Into<PathBuf>, launcher: impl Into<PathBuf>) -> Self {
        Self {
            platform: Platform::current(),
            home: home.into(),
            local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            app_data: std::env::var_os("APPDATA").map(PathBuf::from),
            launcher: launcher.into(),
        }
    }

    /// Everything explicit, for tests and for rendering the same files on every platform.
    pub fn new(
        platform: Platform,
        home: impl Into<PathBuf>,
        local_app_data: Option<PathBuf>,
        app_data: Option<PathBuf>,
        launcher: impl Into<PathBuf>,
    ) -> Self {
        Self {
            platform,
            home: home.into(),
            local_app_data,
            app_data,
            launcher: launcher.into(),
        }
    }

    /// The file a target's profile lives in, or `None` where the terminal does not exist.
    pub fn profile_path(&self, target: ProfileTarget) -> Option<PathBuf> {
        match (target, self.platform) {
            (ProfileTarget::WindowsTerminal, Platform::Windows) => {
                self.local_app_data.as_ref().map(|root| {
                    root.join("Microsoft")
                        .join("Windows Terminal")
                        .join("Fragments")
                        .join(PROFILE_NAME)
                        .join("multiplex.json")
                })
            }
            (ProfileTarget::ITerm2, Platform::MacOs) => Some(
                self.home
                    .join("Library/Application Support/iTerm2/DynamicProfiles/multiplex.json"),
            ),
            (ProfileTarget::VsCode, Platform::Windows) => self
                .app_data
                .as_ref()
                .map(|root| root.join("Code").join("User").join("settings.json")),
            (ProfileTarget::VsCode, Platform::MacOs) => Some(
                self.home
                    .join("Library/Application Support/Code/User/settings.json"),
            ),
            (ProfileTarget::VsCode, Platform::Linux) => {
                Some(self.home.join(".config/Code/User/settings.json"))
            }
            _ => None,
        }
    }

    /// Whether the terminal is installed: its settings folder exists. The profile's own folder
    /// (a fragment's, say) is created when the profile is added.
    fn installed(&self, target: ProfileTarget) -> bool {
        let Some(path) = self.profile_path(target) else {
            return false;
        };
        match target {
            // The Store and Preview builds keep their data in their package folders, an unpackaged
            // one under Microsoft\Windows Terminal; every one of them loads the user's fragments.
            ProfileTarget::WindowsTerminal => self.local_app_data.as_ref().is_some_and(|root| {
                [
                    root.join("Packages")
                        .join("Microsoft.WindowsTerminal_8wekyb3d8bbwe"),
                    root.join("Packages")
                        .join("Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe"),
                    root.join("Microsoft").join("Windows Terminal"),
                ]
                .iter()
                .any(|folder| folder.is_dir())
            }),
            // iTerm2: its support folder. VS Code: its User folder.
            ProfileTarget::ITerm2 => path.ancestors().nth(2).is_some_and(Path::is_dir),
            ProfileTarget::VsCode => path.parent().is_some_and(Path::is_dir),
        }
    }

    pub fn status(&self, target: ProfileTarget) -> ProfileStatus {
        if !self.installed(target) {
            return ProfileStatus::Unavailable;
        }
        let Some(path) = self.profile_path(target) else {
            return ProfileStatus::Unavailable;
        };
        let current = read_optional(&path).ok().flatten();
        match target {
            ProfileTarget::WindowsTerminal | ProfileTarget::ITerm2 => match current {
                None => ProfileStatus::Off,
                Some(contents) if contents == self.drop_in_file(target) => ProfileStatus::On,
                Some(_) => ProfileStatus::Outdated,
            },
            ProfileTarget::VsCode => {
                let contents = current.unwrap_or_default();
                match find_block(&contents) {
                    Some(block)
                        if block == self.vscode_block(&contents_without_block(&contents)) =>
                    {
                        ProfileStatus::On
                    }
                    Some(_) => ProfileStatus::Outdated,
                    None if self.vscode_conflicts(&contents) => ProfileStatus::NeedsManualEdit {
                        manual: self.vscode_manual_text(),
                    },
                    None => ProfileStatus::Off,
                }
            }
        }
    }

    /// The change that adds the profile to `target`, or an empty plan when there is nothing to do.
    pub fn enable_plan(&self, target: ProfileTarget) -> Result<ChangePlan, IntegrationError> {
        let Some(path) = self.profile_path(target).filter(|_| self.installed(target)) else {
            return Ok(ChangePlan::default());
        };
        let before = read_optional(&path)?;
        let after = match target {
            ProfileTarget::WindowsTerminal | ProfileTarget::ITerm2 => self.drop_in_file(target),
            ProfileTarget::VsCode => {
                let base = contents_without_block(before.as_deref().unwrap_or_default());
                if self.vscode_conflicts(&base) {
                    return Ok(ChangePlan::default());
                }
                insert_block(&base, &self.vscode_block(&base))
            }
        };
        Ok(plan_for(path, before, Some(after)))
    }

    /// The change that takes the profile out of `target` again, leaving everything else as it was.
    pub fn disable_plan(&self, target: ProfileTarget) -> Result<ChangePlan, IntegrationError> {
        let Some(path) = self.profile_path(target) else {
            return Ok(ChangePlan::default());
        };
        let before = read_optional(&path)?;
        let after = match (&before, target) {
            (None, _) => return Ok(ChangePlan::default()),
            (Some(_), ProfileTarget::WindowsTerminal | ProfileTarget::ITerm2) => None,
            (Some(contents), ProfileTarget::VsCode) => {
                if find_block(contents).is_none() {
                    return Ok(ChangePlan::default());
                }
                Some(contents_without_block(contents))
            }
        };
        Ok(plan_for(path, before, after))
    }

    /// The launcher as JSON string content, backslashes and quotes escaped.
    fn launcher_json(&self) -> String {
        json_string(&self.launcher.display().to_string())
    }

    fn drop_in_file(&self, target: ProfileTarget) -> String {
        match target {
            ProfileTarget::WindowsTerminal => {
                // `commandline` is one string Windows Terminal splits itself, so the path is
                // quoted inside it.
                let commandline = json_string(&format!("\"{}\" shell", self.launcher.display()));
                format!(
                    "{{\n  \"$help\": \"Added by Multiplex. Terminals opened with this profile can be reached from your paired devices. Turn it off in Multiplex, or delete this file.\",\n  \"profiles\": [\n    {{\n      \"name\": \"{PROFILE_NAME}\",\n      \"commandline\": \"{commandline}\"\n    }}\n  ]\n}}\n"
                )
            }
            ProfileTarget::ITerm2 => {
                // iTerm2 runs `Command` through a login shell, so the path is single-quoted.
                let command = json_string(&format!(
                    "'{}' shell",
                    self.launcher.display().to_string().replace('\'', r"'\''")
                ));
                format!(
                    "{{\n  \"Profiles\": [\n    {{\n      \"Name\": \"{PROFILE_NAME}\",\n      \"Guid\": \"{ITERM2_PROFILE_GUID}\",\n      \"Custom Command\": \"Yes\",\n      \"Command\": \"{command}\"\n    }}\n  ]\n}}\n"
                )
            }
            ProfileTarget::VsCode => String::new(),
        }
    }

    fn vscode_profiles_key(&self) -> String {
        format!(
            "terminal.integrated.profiles.{}",
            self.platform.vscode_key()
        )
    }

    /// The settings already choose terminal profiles for this platform, outside this block.
    fn vscode_conflicts(&self, contents: &str) -> bool {
        contents_without_block(contents).contains(&format!("\"{}\"", self.vscode_profiles_key()))
    }

    fn vscode_profile_entry(&self) -> String {
        format!(
            "\"{PROFILE_NAME}\": {{ \"path\": \"{}\", \"args\": [\"shell\"] }}",
            self.launcher_json()
        )
    }

    fn vscode_manual_text(&self) -> String {
        format!(
            "Add this entry inside \"{}\" in your VS Code settings:\n{}",
            self.vscode_profiles_key(),
            self.vscode_profile_entry()
        )
    }

    /// The block, with a trailing comma when other settings follow it in `base`.
    fn vscode_block(&self, base: &str) -> String {
        let comma = if has_members(base) { "," } else { "" };
        format!(
            "  {VSCODE_BLOCK_START}\n  // Added by Multiplex: a terminal opened with the \"{PROFILE_NAME}\" profile can be reached from your paired devices. Turn it off in Multiplex, or delete these lines.\n  \"{key}\": {{\n    {entry}\n  }}{comma}\n  {VSCODE_BLOCK_END}\n",
            key = self.vscode_profiles_key(),
            entry = self.vscode_profile_entry(),
        )
    }
}

fn plan_for(path: PathBuf, before: Option<String>, after: Option<String>) -> ChangePlan {
    if before == after {
        return ChangePlan::default();
    }
    ChangePlan {
        changes: vec![FileChange::at(path, before, after)],
    }
}

fn json_string(value: &str) -> String {
    let quoted = serde_json::to_string(value).unwrap_or_default();
    quoted
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_default()
        .to_owned()
}

/// The block's lines, markers included, when the settings hold one.
fn find_block(contents: &str) -> Option<String> {
    let start = contents.find(VSCODE_BLOCK_START)?;
    let line_start = contents[..start].rfind('\n').map_or(0, |index| index + 1);
    let end = contents[start..].find(VSCODE_BLOCK_END)? + start + VSCODE_BLOCK_END.len();
    let line_end = contents[end..]
        .find('\n')
        .map_or(contents.len(), |index| end + index + 1);
    Some(contents[line_start..line_end].to_owned())
}

fn contents_without_block(contents: &str) -> String {
    match find_block(contents) {
        Some(block) => contents.replacen(&block, "", 1),
        None => contents.to_owned(),
    }
}

/// Whether the object in `settings` has any member after its opening brace, skipping comments.
fn has_members(settings: &str) -> bool {
    let Some(open) = settings.find('{') else {
        return false;
    };
    let mut rest = &settings[open + 1..];
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("//") {
            rest = after.find('\n').map_or("", |index| &after[index + 1..]);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |index| &after[index + 2..]);
        } else {
            return !rest.is_empty() && !rest.starts_with('}');
        }
    }
}

/// `block` placed as the first member of the settings object, or a new object holding only it.
fn insert_block(settings: &str, block: &str) -> String {
    match settings.find('{') {
        Some(open) => {
            let (head, tail) = settings.split_at(open + 1);
            let tail = tail.strip_prefix('\n').unwrap_or(tail);
            format!("{head}\n{block}{tail}")
        }
        None => format!("{{\n{block}}}\n"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn profiles(platform: Platform, root: &Path) -> TerminalProfiles {
        TerminalProfiles::new(
            platform,
            root.join("home"),
            Some(root.join("local")),
            Some(root.join("roaming")),
            match platform {
                Platform::Windows => PathBuf::from(r"C:\Program Files\Multiplex\multiplex-cli.exe"),
                _ => PathBuf::from("/Applications/Multiplex.app/Contents/MacOS/multiplex-cli"),
            },
        )
    }

    fn install(path: &Path) {
        fs::create_dir_all(path).unwrap();
    }

    #[test]
    fn a_terminal_that_is_not_installed_offers_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let profiles = profiles(Platform::Windows, temp.path());
        assert_eq!(
            profiles.status(ProfileTarget::WindowsTerminal),
            ProfileStatus::Unavailable
        );
        assert!(
            profiles
                .enable_plan(ProfileTarget::WindowsTerminal)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            profiles.status(ProfileTarget::ITerm2),
            ProfileStatus::Unavailable
        );
    }

    #[test]
    fn windows_terminal_gets_a_fragment_and_loses_it_again() {
        let temp = tempfile::tempdir().unwrap();
        // The Store build, whose own folder is a package; the fragment goes to the shared place.
        install(
            &temp
                .path()
                .join("local/Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe"),
        );
        let profiles = profiles(Platform::Windows, temp.path());
        assert_eq!(
            profiles.status(ProfileTarget::WindowsTerminal),
            ProfileStatus::Off
        );
        let plan = profiles
            .enable_plan(ProfileTarget::WindowsTerminal)
            .unwrap();
        let written = plan.changes[0].after.clone().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["profiles"][0]["name"], PROFILE_NAME);
        assert_eq!(
            parsed["profiles"][0]["commandline"],
            r#""C:\Program Files\Multiplex\multiplex-cli.exe" shell"#
        );
        plan.apply().unwrap();
        assert_eq!(
            profiles.status(ProfileTarget::WindowsTerminal),
            ProfileStatus::On
        );
        profiles
            .disable_plan(ProfileTarget::WindowsTerminal)
            .unwrap()
            .apply()
            .unwrap();
        assert_eq!(
            profiles.status(ProfileTarget::WindowsTerminal),
            ProfileStatus::Off
        );
    }

    #[test]
    fn a_profile_left_by_a_moved_app_is_outdated() {
        let temp = tempfile::tempdir().unwrap();
        install(&temp.path().join("local/Microsoft/Windows Terminal"));
        profiles(Platform::Windows, temp.path())
            .enable_plan(ProfileTarget::WindowsTerminal)
            .unwrap()
            .apply()
            .unwrap();
        let moved = TerminalProfiles::new(
            Platform::Windows,
            temp.path().join("home"),
            Some(temp.path().join("local")),
            None,
            r"D:\Multiplex\multiplex-cli.exe",
        );
        assert_eq!(
            moved.status(ProfileTarget::WindowsTerminal),
            ProfileStatus::Outdated
        );
    }

    #[test]
    fn iterm2_gets_a_dynamic_profile() {
        let temp = tempfile::tempdir().unwrap();
        install(&temp.path().join("home/Library/Application Support/iTerm2"));
        let profiles = profiles(Platform::MacOs, temp.path());
        let plan = profiles.enable_plan(ProfileTarget::ITerm2).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(plan.changes[0].after.as_deref().unwrap()).unwrap();
        assert_eq!(
            parsed["Profiles"][0]["Command"],
            "'/Applications/Multiplex.app/Contents/MacOS/multiplex-cli' shell"
        );
        plan.apply().unwrap();
        assert_eq!(profiles.status(ProfileTarget::ITerm2), ProfileStatus::On);
    }

    #[test]
    fn vscode_settings_get_one_block_and_are_restored_byte_for_byte() {
        let temp = tempfile::tempdir().unwrap();
        let user = temp.path().join("roaming/Code/User");
        install(&user);
        let original = "{\n    // my settings\n    \"editor.fontSize\": 14,\n    \"files.autoSave\": \"afterDelay\"\n}\n";
        fs::write(user.join("settings.json"), original).unwrap();
        let profiles = profiles(Platform::Windows, temp.path());
        assert_eq!(profiles.status(ProfileTarget::VsCode), ProfileStatus::Off);

        profiles
            .enable_plan(ProfileTarget::VsCode)
            .unwrap()
            .apply()
            .unwrap();
        let written = fs::read_to_string(user.join("settings.json")).unwrap();
        assert!(written.contains(VSCODE_BLOCK_START));
        assert!(written.contains("\"terminal.integrated.profiles.windows\""));
        assert!(written.contains(r#""path": "C:\\Program Files\\Multiplex\\multiplex-cli.exe""#));
        assert!(written.contains("\"editor.fontSize\": 14"));
        assert_eq!(profiles.status(ProfileTarget::VsCode), ProfileStatus::On);

        profiles
            .disable_plan(ProfileTarget::VsCode)
            .unwrap()
            .apply()
            .unwrap();
        assert_eq!(
            fs::read_to_string(user.join("settings.json")).unwrap(),
            original
        );
    }

    #[test]
    fn empty_or_missing_vscode_settings_get_a_block_without_a_dangling_comma() {
        let temp = tempfile::tempdir().unwrap();
        let user = temp
            .path()
            .join("home/Library/Application Support/Code/User");
        install(&user);
        let profiles = profiles(Platform::MacOs, temp.path());
        let plan = profiles.enable_plan(ProfileTarget::VsCode).unwrap();
        let created = plan.changes[0].after.clone().unwrap();
        assert!(created.contains("\"terminal.integrated.profiles.osx\""));
        assert!(!created.contains("},\n  // <<<"));
        fs::write(user.join("settings.json"), "{}\n").unwrap();
        let plan = profiles.enable_plan(ProfileTarget::VsCode).unwrap();
        assert!(
            !plan.changes[0]
                .after
                .as_deref()
                .unwrap()
                .contains("},\n  // <<<")
        );
    }

    #[test]
    fn existing_terminal_profiles_are_left_for_a_person_to_edit() {
        let temp = tempfile::tempdir().unwrap();
        let user = temp.path().join("roaming/Code/User");
        install(&user);
        fs::write(
            user.join("settings.json"),
            "{\n  \"terminal.integrated.profiles.windows\": { \"Git Bash\": { \"path\": \"bash.exe\" } }\n}\n",
        )
        .unwrap();
        let profiles = profiles(Platform::Windows, temp.path());
        let ProfileStatus::NeedsManualEdit { manual } = profiles.status(ProfileTarget::VsCode)
        else {
            panic!("expected a manual edit");
        };
        assert!(manual.contains("\"Multiplex\": { \"path\""));
        assert!(
            profiles
                .enable_plan(ProfileTarget::VsCode)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_settings_file_changed_after_the_preview_is_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let user = temp.path().join("roaming/Code/User");
        install(&user);
        fs::write(user.join("settings.json"), "{\n  \"a\": 1\n}\n").unwrap();
        let profiles = profiles(Platform::Windows, temp.path());
        let plan = profiles.enable_plan(ProfileTarget::VsCode).unwrap();
        fs::write(user.join("settings.json"), "{\n  \"a\": 2\n}\n").unwrap();
        assert!(plan.apply().is_err());
        assert_eq!(
            fs::read_to_string(user.join("settings.json")).unwrap(),
            "{\n  \"a\": 2\n}\n"
        );
    }
}
