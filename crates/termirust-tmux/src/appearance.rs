//! How tmux behaves in the sessions the shell setup starts, so a wrapped tab feels like the
//! terminal it replaced: no status bar, the mouse scrolls and selects, a quiet selection with no
//! copy-mode position counter, a selection that stays put while scrolling and copies on release,
//! two lines per wheel step, and copy mode that ends at the bottom of the history, on a click
//! there, or as soon as anything is typed or dropped into the tab.
//!
//! Options and the new-window hook are set on the wrapped session and its windows only. tmux key
//! bindings are global to a server, so each binding checks the session name and keeps tmux's
//! default behavior everywhere else.
//!
//! The same commands are rendered into the app-owned tmux configuration file that new tabs
//! source, and run against a live server when the setup is applied or removed.

/// A tmux format that is true in a session the shell setup started.
pub const WRAPPED_SESSION_FORMAT: &str = "#{m:termirust-*,#{session_name}}";
/// A tmux format that is true when a binding was run by a key rather than by the mouse: a
/// mouse binding has the pointer's position in its formats and a typed key has not. Without it
/// the catch-all binding below would answer a button release and end copy mode on every click.
const TYPED_KEY_FORMAT: &str = "#{==:#{mouse_x},}";
/// The selection and copy-mode highlight in wrapped sessions.
pub const SELECTION_STYLE: &str = "bg=#3b4252,fg=default";

const COPY_TABLES: [&str; 2] = ["copy-mode", "copy-mode-vi"];

/// Writes UTF-8 whatever the locale says, which a tab started by launchd or a bare shell has
/// none of.
pub const UTF8_FLAG: &str = "-u";
/// Declares terminal features to tmux for this client.
pub const FEATURES_FLAG: &str = "-T";
/// The terminal can show 24-bit color. tmux otherwise converts every 24-bit color to the
/// nearest of 256, and a wrapped tab looks unlike the tab it replaced. tmux works this out by
/// itself with a newer terminal, but not on tmux 3.2 or 3.3.
pub const TRUECOLOR_FEATURE: &str = "RGB";
/// The terminal understands synchronized updates (DEC mode 2026). tmux then draws each frame
/// between a begin and an end, so the terminal never paints half of one. Without it, a program
/// that redraws constantly, such as a coding agent's spinner, flickers in a wrapped tab.
pub const SYNCHRONIZED_UPDATE_FEATURE: &str = "sync";

/// The flags a tmux client starts with, given what its terminal can do.
pub fn client_flags(features: &[&str]) -> Vec<String> {
    let mut flags = vec![UTF8_FLAG.to_owned()];
    if !features.is_empty() {
        flags.push(FEATURES_FLAG.to_owned());
        flags.push(features.join(","));
    }
    flags
}

/// One key binding: the command in wrapped sessions and tmux's default command elsewhere.
/// `default` is `None` for a key tmux leaves unbound, where doing nothing is the default and
/// removing the setup unbinds the key again.
struct Binding {
    key: String,
    wrapped: String,
    default: Option<&'static str>,
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
        let mut bindings = vec![
            Binding {
                key: "MouseDragEnd1Pane".to_owned(),
                wrapped: format!("{copy} ; send-keys -X stop-selection"),
                default: Some("send-keys -X copy-pipe-and-cancel"),
            },
            Binding {
                // At the bottom of the history a click leaves copy mode, because keys typed in
                // copy mode drive copy mode and a tab left in it looks like it stopped taking
                // input. Further back it only clears the selection: leaving copy mode there
                // would jump the view to the bottom, and the text would move under the click.
                key: "MouseDown1Pane".to_owned(),
                wrapped: format!(
                    "select-pane ; {}",
                    command_string(&[
                        "if-shell",
                        "-F",
                        "#{==:#{scroll_position},0}",
                        "send-keys -X cancel",
                        "send-keys -X clear-selection",
                    ])
                ),
                default: Some("select-pane"),
            },
            Binding {
                key: "WheelUpPane".to_owned(),
                wrapped: "select-pane ; send-keys -X -N 2 scroll-up".to_owned(),
                default: Some("select-pane ; send-keys -X -N 5 scroll-up"),
            },
            Binding {
                key: "WheelDownPane".to_owned(),
                // Scrolling back to the bottom leaves copy mode, however it was entered.
                wrapped: "select-pane ; send-keys -X -N 2 scroll-down-and-cancel".to_owned(),
                default: Some("select-pane ; send-keys -X -N 5 scroll-down"),
            },
        ];

        // Text that arrives while the view is scrolled back belongs to the program, not to copy
        // mode. A file dropped on the terminal is typed in as its path, and a tab that swallows
        // it looks broken, so the view returns to the prompt and the program gets what follows,
        // the way it does in a terminal with no tmux under it.
        //
        // A dropped path arrives bracketed, which tmux reads as a single key it has no name for,
        // so a catch-all is the only binding that can answer it; the path itself follows as
        // ordinary bytes. Whether that works depends on the tmux underneath: 3.6 and newer offer
        // the key to this binding and deliver the path whole, 3.4 and 3.5a consume the paste in
        // copy mode so the tab keeps the file, and 3.2a and 3.3a run the binding but do not
        // consume the paste's closing ESC[201~, so its bytes follow the path into the program.
        //
        // tmux answers this binding before any binding for the key itself and tells it nothing
        // about which key ran it, so a character typed by hand to a scrolled-back tab still ends
        // copy mode without reaching the program. Binding every printable key to send itself does
        // not work around that: tmux runs the catch-all instead, and a key sent from the binding
        // that is leaving copy mode is swallowed.
        bindings.push(Binding {
            key: "Any".to_owned(),
            wrapped: command_string(&["if-shell", "-F", TYPED_KEY_FORMAT, "copy-mode -q"]),
            default: None,
        });
        bindings
    }

    /// `bind-key` commands that behave as described in wrapped sessions and as tmux's defaults
    /// elsewhere.
    pub fn bind_commands(&self) -> Vec<Vec<String>> {
        COPY_TABLES
            .iter()
            .flat_map(|table| {
                self.bindings().into_iter().map(move |binding| {
                    let guarded = match binding.default {
                        Some(default) => command_string(&[
                            "if-shell",
                            "-F",
                            WRAPPED_SESSION_FORMAT,
                            &binding.wrapped,
                            default,
                        ]),
                        None => command_string(&[
                            "if-shell",
                            "-F",
                            WRAPPED_SESSION_FORMAT,
                            &binding.wrapped,
                        ]),
                    };
                    vec![
                        "bind-key".to_owned(),
                        "-T".to_owned(),
                        (*table).to_owned(),
                        binding.key,
                        guarded,
                    ]
                })
            })
            .collect()
    }

    /// Commands that put tmux's own behavior back for the same keys: its default binding where
    /// it has one, and no binding at all where it has none.
    pub fn default_bind_commands(&self) -> Vec<Vec<String>> {
        COPY_TABLES
            .iter()
            .flat_map(|table| {
                self.bindings()
                    .into_iter()
                    .map(move |binding| match binding.default {
                        Some(default) => vec![
                            "bind-key".to_owned(),
                            "-T".to_owned(),
                            (*table).to_owned(),
                            binding.key,
                            default.to_owned(),
                        ],
                        None => vec![
                            "unbind-key".to_owned(),
                            "-T".to_owned(),
                            (*table).to_owned(),
                            binding.key,
                        ],
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
        assert_eq!(
            bindings.len(),
            10,
            "four keys and the catch-all in two copy-mode tables"
        );
        assert!(
            bindings
                .iter()
                .all(|line| line.contains("if-shell -F \"#{m:termirust-*,#{session_name}}\""))
        );
        assert!(file.contains(
            "bind-key -T copy-mode MouseDragEnd1Pane 'if-shell -F \"#{m:termirust-*,#{session_name}}\" \"send-keys -X copy-pipe-no-clear pbcopy ; send-keys -X stop-selection\" \"send-keys -X copy-pipe-and-cancel\"'"
        ));
        // Anything typed or dropped hands the tab back to the program, but only when a key sent
        // it: the mouse goes on scrolling and selecting with the view where the reader left it.
        assert!(file.contains(
            "bind-key -T copy-mode Any 'if-shell -F \"#{m:termirust-*,#{session_name}}\" \"if-shell -F \\\"#{==:#{mouse_x},}\\\" \\\"copy-mode -q\\\"\"'"
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
        assert_eq!(defaults.len(), 10);
        assert!(defaults.contains(&vec![
            "bind-key".to_owned(),
            "-T".to_owned(),
            "copy-mode-vi".to_owned(),
            "WheelUpPane".to_owned(),
            "select-pane ; send-keys -X -N 5 scroll-up".to_owned(),
        ]));
        // tmux binds no catch-all of its own, so removing the setup leaves none behind.
        assert!(defaults.contains(&vec![
            "unbind-key".to_owned(),
            "-T".to_owned(),
            "copy-mode".to_owned(),
            "Any".to_owned(),
        ]));
    }
}
