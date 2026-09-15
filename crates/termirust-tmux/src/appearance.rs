//! How tmux behaves in the sessions the shell setup starts, so a wrapped tab feels like the
//! terminal it replaced: no status bar, the mouse scrolls and selects, a quiet selection with no
//! copy-mode position counter, a selection that stays put while scrolling and copies on release,
//! two lines per wheel step, and copy mode that ends on a click or at the bottom of the history.
//!
//! Options and the new-window hook are set on the wrapped session and its windows only. tmux key
//! bindings are global to a server, so each binding checks the session name and keeps tmux's
//! default behavior everywhere else.
//!
//! The same commands are rendered into the app-owned tmux configuration file that new tabs
//! source, and run against a live server when the setup is applied or removed.

/// A tmux format that is true in a session the shell setup started.
pub const WRAPPED_SESSION_FORMAT: &str = "#{m:termirust-*,#{session_name}}";
/// The selection and copy-mode highlight in wrapped sessions.
pub const SELECTION_STYLE: &str = "bg=#3b4252,fg=default";

const COPY_TABLES: [&str; 2] = ["copy-mode", "copy-mode-vi"];

/// One key binding: the command in wrapped sessions and tmux's default command elsewhere.
struct Binding {
    key: &'static str,
    wrapped: String,
    default: &'static str,
}

/// The tmux behavior for wrapped sessions on one platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrappedSessionAppearance {
    /// A program that receives copied text on standard input, such as `pbcopy`. Without one,
    /// tmux keeps the text in its buffer and sends it to the terminal's clipboard itself.
    copy_command: Option<String>,
}

impl WrappedSessionAppearance {
    pub fn new(copy_command: Option<String>) -> Self {
        Self { copy_command }
    }

    /// `pbcopy` on macOS; elsewhere tmux's own clipboard handling.
    pub fn for_this_platform() -> Self {
        Self::new(cfg!(target_os = "macos").then(|| "pbcopy".to_owned()))
    }

    /// Session options, each without a target.
    pub fn session_options() -> [[&'static str; 3]; 2] {
        [
            ["set-option", "status", "off"],
            ["set-option", "mouse", "on"],
        ]
    }

    /// Window options, each without a target. `-q` lets tmux older than 3.5, which has no
    /// `copy-mode-position-format`, skip that option and keep applying the rest.
    pub fn window_options() -> [[&'static str; 5]; 2] {
        [
            ["set-option", "-q", "-w", "copy-mode-position-format", ""],
            ["set-option", "-q", "-w", "mode-style", SELECTION_STYLE],
        ]
    }

    /// The `after-new-window` hook command that gives new windows the window options.
    pub fn new_window_hook() -> String {
        Self::window_options()
            .iter()
            .map(|command| command_string(command))
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    fn bindings(&self) -> Vec<Binding> {
        let copy = match &self.copy_command {
            Some(program) => format!("send-keys -X copy-pipe-no-clear {program}"),
            None => "send-keys -X copy-pipe-no-clear".to_owned(),
        };
        vec![
            Binding {
                key: "MouseDragEnd1Pane",
                wrapped: format!("{copy} ; send-keys -X stop-selection"),
                default: "send-keys -X copy-pipe-and-cancel",
            },
            Binding {
                // Leaving copy mode, not only clearing the selection: keys typed in copy mode
                // drive copy mode, so a tab left in it looks like it stopped taking input.
                key: "MouseDown1Pane",
                wrapped: "select-pane ; send-keys -X cancel".to_owned(),
                default: "select-pane",
            },
            Binding {
                key: "WheelUpPane",
                wrapped: "select-pane ; send-keys -X -N 2 scroll-up".to_owned(),
                default: "select-pane ; send-keys -X -N 5 scroll-up",
            },
            Binding {
                key: "WheelDownPane",
                // Scrolling back to the bottom leaves copy mode, however it was entered.
                wrapped: "select-pane ; send-keys -X -N 2 scroll-down-and-cancel".to_owned(),
                default: "select-pane ; send-keys -X -N 5 scroll-down",
            },
        ]
    }

    /// `bind-key` commands that behave as described in wrapped sessions and as tmux's defaults
    /// elsewhere.
    pub fn bind_commands(&self) -> Vec<Vec<String>> {
        let bindings = self.bindings();
        COPY_TABLES
            .iter()
            .flat_map(|table| {
                bindings.iter().map(move |binding| {
                    vec![
                        "bind-key".to_owned(),
                        "-T".to_owned(),
                        (*table).to_owned(),
                        binding.key.to_owned(),
                        command_string(&[
                            "if-shell",
                            "-F",
                            WRAPPED_SESSION_FORMAT,
                            &binding.wrapped,
                            binding.default,
                        ]),
                    ]
                })
            })
            .collect()
    }

    /// `bind-key` commands that put tmux's default bindings back for the same keys.
    pub fn default_bind_commands(&self) -> Vec<Vec<String>> {
        let bindings = self.bindings();
        COPY_TABLES
            .iter()
            .flat_map(|table| {
                bindings.iter().map(move |binding| {
                    vec![
                        "bind-key".to_owned(),
                        "-T".to_owned(),
                        (*table).to_owned(),
                        binding.key.to_owned(),
                        binding.default.to_owned(),
                    ]
                })
            })
            .collect()
    }

    /// The tmux configuration file a wrapped tab sources right after `new-session`, so its
    /// options apply to that session.
    pub fn configuration_file(&self) -> String {
        let mut lines = vec![
            "# Managed by TermiRust for the tmux sessions it starts (named termirust-*). Turn off \"Open new terminals in tmux\" in TermiRust to remove it.".to_owned(),
            "# Options below apply to the session sourcing this file. Key bindings are global to the tmux server, so each one keeps tmux's default outside TermiRust's sessions.".to_owned(),
        ];
        lines.extend(
            Self::session_options()
                .iter()
                .map(|command| config_line(command)),
        );
        lines.extend(
            Self::window_options()
                .iter()
                .map(|command| config_line(command)),
        );
        lines.push(config_line(&[
            "set-hook",
            "after-new-window",
            &Self::new_window_hook(),
        ]));
        lines.extend(
            self.bind_commands()
                .iter()
                .map(|command| config_line(command)),
        );
        let mut file = lines.join("\n");
        file.push('\n');
        file
    }
}

/// Joins arguments into one tmux command string, double-quoting arguments that need it.
fn command_string<S: AsRef<str>>(arguments: &[S]) -> String {
    arguments
        .iter()
        .map(|argument| {
            let argument = argument.as_ref();
            if !argument.is_empty()
                && argument
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.,:=/@+%".contains(c))
            {
                argument.to_owned()
            } else {
                format!(
                    "\"{}\"",
                    argument.replace('\\', "\\\\").replace('"', "\\\"")
                )
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One configuration-file line. Arguments that need quoting are single-quoted, which tmux takes
/// literally; none of these arguments contain a single quote.
fn config_line<S: AsRef<str>>(arguments: &[S]) -> String {
    arguments
        .iter()
        .map(|argument| {
            let argument = argument.as_ref();
            debug_assert!(!argument.contains('\''));
            if !argument.is_empty()
                && argument
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.,:=/@+%".contains(c))
            {
                argument.to_owned()
            } else {
                format!("'{argument}'")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_file_scopes_options_and_guards_every_binding() {
        let file = WrappedSessionAppearance::new(Some("pbcopy".to_owned())).configuration_file();
        assert!(file.contains("\nset-option status off\n"));
        assert!(file.contains("\nset-option mouse on\n"));
        assert!(file.contains("\nset-option -q -w copy-mode-position-format ''\n"));
        assert!(file.contains(
            "\nset-hook after-new-window 'set-option -q -w copy-mode-position-format \"\" ; set-option -q -w mode-style \"bg=#3b4252,fg=default\"'\n"
        ));
        let bindings = file
            .lines()
            .filter(|line| line.starts_with("bind-key"))
            .collect::<Vec<_>>();
        assert_eq!(bindings.len(), 8, "four keys in two copy-mode tables");
        assert!(
            bindings
                .iter()
                .all(|line| line.contains("if-shell -F \"#{m:termirust-*,#{session_name}}\""))
        );
        assert!(file.contains(
            "bind-key -T copy-mode MouseDragEnd1Pane 'if-shell -F \"#{m:termirust-*,#{session_name}}\" \"send-keys -X copy-pipe-no-clear pbcopy ; send-keys -X stop-selection\" \"send-keys -X copy-pipe-and-cancel\"'"
        ));
        assert!(!file.lines().any(|line| line.starts_with("set-option -g")));
    }

    #[test]
    fn without_a_copy_program_tmux_handles_the_clipboard() {
        let commands = WrappedSessionAppearance::new(None).bind_commands();
        assert!(commands.iter().any(|command| {
            command[4].contains("\"send-keys -X copy-pipe-no-clear ; send-keys -X stop-selection\"")
        }));
    }

    #[test]
    fn default_bindings_match_tmux() {
        let defaults = WrappedSessionAppearance::new(None).default_bind_commands();
        assert_eq!(defaults.len(), 8);
        assert!(defaults.contains(&vec![
            "bind-key".to_owned(),
            "-T".to_owned(),
            "copy-mode-vi".to_owned(),
            "WheelUpPane".to_owned(),
            "select-pane ; send-keys -X -N 5 scroll-up".to_owned(),
        ]));
    }
}
