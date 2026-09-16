//! The one tmux integration shared by the desktop app and the Controller listener.
//!
//! It finds the tmux binary, lists the sessions on the user's default tmux server, and
//! builds the arguments that attach a second client without resizing the user's window.
//! It never ends a session it did not start. Session names and commands are user data, so the
//! `Debug` output of [`TmuxSession`] redacts them.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub mod appearance;
pub mod shell_integration;

/// Overrides binary discovery with one exact tmux path.
pub const TMUX_PATH_ENV: &str = "TERMIRUST_TMUX_PATH";
/// The directory tmux keeps its server sockets under.
pub const TMUX_TMPDIR_ENV: &str = "TMUX_TMPDIR";
/// Listing stops after this many sessions.
pub const MAX_LISTED_SESSIONS: usize = 256;
/// Longer session names are skipped rather than truncated, so a name never aliases another.
pub const MAX_SESSION_NAME_BYTES: usize = 1024;
/// The foreground command is display-only, so it is truncated to this many characters.
pub const MAX_COMMAND_CHARS: usize = 128;
/// The oldest tmux that understands `attach-session -f ignore-size`.
pub const MINIMUM_ATTACH_VERSION: (u32, u32) = (3, 2);
/// Sessions the shell setup starts are named `termirust-<directory>-<pid>`.
pub const WRAPPED_SESSION_PREFIX: &str = "termirust-";
/// The server option an earlier version of the shell setup set to keep tmux clients off the
/// alternate screen. tmux redraws with scroll regions, so lines never reached the terminal
/// app's scrollback and programs such as Claude Code could not be scrolled back at all.
/// Applying or removing the setup now clears it.
pub const LEGACY_SCROLLBACK_OVERRIDE_TARGET: &str = "terminal-overrides[97]";

const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_STDOUT_BYTES: usize = 512 * 1024;
const MAX_STDERR_BYTES: usize = 8 * 1024;
const MAX_DIAGNOSTIC_CHARS: usize = 240;
const FIELD_SEPARATOR: char = '\t';
// tmux rejects tabs and newlines in session names, and the name is followed only by the
// command, which is split off last. So a name containing `|`, `:`, `.`, or spaces parses
// exactly.
const LIST_SESSIONS_FORMAT: &str = "#{session_id}\t#{session_created}\t#{session_attached}\t#{session_windows}\t#{session_name}\t#{pane_current_command}";
const WELL_KNOWN_LOCATIONS: [&str; 3] = [
    "/opt/homebrew/bin/tmux",
    "/usr/local/bin/tmux",
    "/usr/bin/tmux",
];
// Variables a tmux client needs to reach the same server and render the same text.
const FORWARDED_ENVIRONMENT: [&str; 9] = [
    "HOME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LOGNAME",
    "PATH",
    "SHELL",
    "TMUX_TMPDIR",
    "USER",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TmuxError {
    /// No candidate path held a file.
    Unavailable,
    /// A binary exists but `tmux -V` failed.
    VersionProbe(String),
    /// The tmux process could not be started or waited on.
    Spawn(io::ErrorKind),
    /// tmux did not exit within the command deadline and was killed.
    TimedOut,
    /// tmux wrote more output than a listing can hold.
    OutputTooLarge,
    /// tmux exited unsuccessfully for a reason other than "no server".
    CommandFailed {
        status: Option<i32>,
        diagnostic: String,
    },
}

impl fmt::Display for TmuxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => {
                formatter.write_str("tmux is not installed or is not available in the app's PATH")
            }
            Self::VersionProbe(diagnostic) => formatter.write_str(diagnostic),
            Self::Spawn(kind) => write!(formatter, "Unable to run tmux: {kind}"),
            Self::TimedOut => formatter.write_str("tmux did not respond in time"),
            Self::OutputTooLarge => formatter.write_str("tmux output exceeded the listing limit"),
            Self::CommandFailed { status, diagnostic } if diagnostic.is_empty() => match status {
                Some(code) => write!(formatter, "tmux exited with status {code}"),
                None => formatter.write_str("tmux was terminated by a signal"),
            },
            Self::CommandFailed { diagnostic, .. } => formatter.write_str(diagnostic),
        }
    }
}

impl std::error::Error for TmuxError {}

/// A located tmux binary and the server it talks to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tmux {
    executable: PathBuf,
    version: String,
    socket_directory: Option<PathBuf>,
}

impl Tmux {
    /// Finds tmux the way the desktop app always has: `TERMIRUST_TMUX_PATH` exactly when
    /// set, otherwise `PATH` followed by the Homebrew and system locations. An app started
    /// from Finder has a minimal `PATH`, which is why the fixed locations matter.
    pub fn discover() -> Result<Self, TmuxError> {
        Self::discover_from(
            std::env::var_os(TMUX_PATH_ENV),
            std::env::var_os("PATH"),
            Path::is_file,
            version_at,
        )
    }

    /// [`Tmux::discover`] with every environment read and probe injected.
    pub fn discover_from<E: fmt::Display>(
        override_path: Option<OsString>,
        search_path: Option<OsString>,
        is_file: impl FnMut(&Path) -> bool,
        version_probe: impl FnMut(&Path) -> Result<String, E>,
    ) -> Result<Self, TmuxError> {
        let candidates = match override_path {
            Some(path) => vec![PathBuf::from(path)],
            None => default_candidates(search_path),
        };
        Self::select(candidates, is_file, version_probe)
    }

    /// Picks the first candidate that is a file, then probes its version.
    pub fn select<E: fmt::Display>(
        candidates: impl IntoIterator<Item = PathBuf>,
        mut is_file: impl FnMut(&Path) -> bool,
        mut version_probe: impl FnMut(&Path) -> Result<String, E>,
    ) -> Result<Self, TmuxError> {
        let executable = candidates
            .into_iter()
            .find(|candidate| is_file(candidate))
            .ok_or(TmuxError::Unavailable)?;
        let version = version_probe(&executable)
            .map_err(|error| TmuxError::VersionProbe(bounded_diagnostic(&error.to_string())))?;
        Ok(Self {
            executable,
            version,
            socket_directory: None,
        })
    }

    /// Uses exactly this binary.
    pub fn at(executable: impl Into<PathBuf>) -> Result<Self, TmuxError> {
        let executable = executable.into();
        Self::select([executable], Path::is_file, version_at)
    }

    /// Talks to the server under `directory` instead of the inherited `TMUX_TMPDIR`, and starts
    /// that server without reading any tmux configuration file. Tests use this so they never
    /// touch the developer's own server and behave the same whatever `~/.tmux.conf` sets.
    pub fn with_socket_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.socket_directory = Some(directory.into());
        self
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// The executable with symlinks resolved. Homebrew installs tmux as a symlink, and the
    /// Session Host refuses to launch one.
    pub fn canonical_executable(&self) -> io::Result<PathBuf> {
        std::fs::canonicalize(&self.executable)
    }

    /// The `tmux -V` output, such as `tmux 3.7c`.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Whether this tmux can attach a client that never sizes the window.
    pub fn supports_ignore_size(&self) -> bool {
        parse_version(&self.version).is_none_or(|version| version >= MINIMUM_ATTACH_VERSION)
    }

    /// A command for this binary that reaches the configured server. `TMUX` and
    /// `TMUX_PANE` are removed so a listener started from inside tmux neither refuses to
    /// nest nor follows a non-default socket.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.env_remove("TMUX").env_remove("TMUX_PANE");
        // Without a UTF-8 locale, as under launchd, tmux replaces the listing's tab separators
        // and every non-ASCII byte with `_`, and no session parses.
        command.arg("-u");
        if let Some(directory) = &self.socket_directory {
            command.env(TMUX_TMPDIR_ENV, directory);
            command.args(["-f", "/dev/null"]);
        }
        command
    }

    /// Identifies the server this instance talks to, so a session id reused by another
    /// server never aliases. Stable for as long as the socket location is.
    pub fn server_identity(&self) -> String {
        let directory = self
            .socket_directory
            .clone()
            .or_else(|| std::env::var_os(TMUX_TMPDIR_ENV).map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        format!("{}/tmux-{}/default", directory.display(), effective_uid())
    }

    /// The environment a tmux client launched in a cleared environment needs, read from
    /// the current process. `TMUX_TMPDIR` follows [`Tmux::with_socket_directory`].
    pub fn client_environment(&self) -> Vec<(String, String)> {
        let mut environment = FORWARDED_ENVIRONMENT
            .iter()
            .filter(|name| **name != TMUX_TMPDIR_ENV || self.socket_directory.is_none())
            .filter_map(|name| Some(((*name).to_owned(), std::env::var(name).ok()?)))
            .collect::<Vec<_>>();
        if let Some(directory) = &self.socket_directory {
            environment.push((
                TMUX_TMPDIR_ENV.to_owned(),
                directory.to_string_lossy().into_owned(),
            ));
        }
        environment.sort();
        environment
    }

    /// Lists the sessions on the server. No server running is an empty list, not an error.
    pub fn list_sessions(&self) -> Result<SessionListing, TmuxError> {
        let mut command = self.command();
        command.args(["list-sessions", "-F", LIST_SESSIONS_FORMAT]);
        let output = run_bounded(command, COMMAND_TIMEOUT)?;
        if output.status.success() {
            if output.stdout_overflowed {
                return Err(TmuxError::OutputTooLarge);
            }
            return Ok(parse_list_sessions(&output.stdout));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_no_server_diagnostic(&stderr) {
            return Ok(SessionListing::default());
        }
        Err(TmuxError::CommandFailed {
            status: output.status.code(),
            diagnostic: bounded_diagnostic(stderr.trim()),
        })
    }

    /// Makes the running server match the shell setup. With `setup_on`, sessions it started
    /// (named [`WRAPPED_SESSION_PREFIX`]…) and their windows get the
    /// [`appearance::WrappedSessionAppearance`] options and new-window hook, and the guarded key
    /// bindings are installed; otherwise those options and the hook are removed and tmux's
    /// default bindings for the same keys are put back. Either way the
    /// [`LEGACY_SCROLLBACK_OVERRIDE_TARGET`] an earlier version set is removed. Other sessions
    /// keep their own options. Returns how many sessions changed; with no server running there
    /// is nothing to change.
    pub fn apply_wrapped_session_appearance(&self, setup_on: bool) -> Result<usize, TmuxError> {
        let listing = self.list_sessions()?;
        if listing.sessions.is_empty() {
            return Ok(0);
        }
        let appearance = appearance::WrappedSessionAppearance::for_this_platform();
        self.run_arguments(["set-option", "-s", "-u", LEGACY_SCROLLBACK_OVERRIDE_TARGET])?;
        let bindings = if setup_on {
            appearance.bind_commands()
        } else {
            appearance.default_bind_commands()
        };
        for binding in bindings {
            self.run_arguments(binding)?;
        }

        let hook = appearance::WrappedSessionAppearance::new_window_hook();
        let mut changed = 0;
        for session in listing
            .sessions
            .iter()
            .filter(|session| session.name.starts_with(WRAPPED_SESSION_PREFIX))
        {
            let id = session.id();
            let mut applied = true;
            for [_, name, value] in appearance::WrappedSessionAppearance::session_options() {
                let output = if setup_on {
                    self.run_arguments(["set-option", "-t", id, name, value])?
                } else {
                    self.run_arguments(["set-option", "-u", "-t", id, name])?
                };
                applied &= output.status.success();
            }
            let output = if setup_on {
                self.run_arguments(["set-hook", "-t", id, "after-new-window", hook.as_str()])?
            } else {
                self.run_arguments(["set-hook", "-u", "-t", id, "after-new-window"])?
            };
            applied &= output.status.success();
            let windows = self.run_arguments(["list-windows", "-t", id, "-F", "#{window_id}"])?;
            for window in String::from_utf8_lossy(&windows.stdout).lines() {
                for [_, _, _, name, value] in appearance::WrappedSessionAppearance::window_options()
                {
                    let output = if setup_on {
                        self.run_arguments(["set-option", "-q", "-w", "-t", window, name, value])?
                    } else {
                        self.run_arguments(["set-option", "-q", "-w", "-u", "-t", window, name])?
                    };
                    applied &= output.status.success();
                }
            }
            if applied {
                changed += 1;
            }
        }
        Ok(changed)
    }

    fn run_arguments<I, S>(&self, arguments: I) -> Result<BoundedOutput, TmuxError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = self.command();
        command.args(arguments);
        run_bounded(command, COMMAND_TIMEOUT)
    }

    /// Proves the path a paired device uses: starts a throwaway detached session, finds it
    /// in a listing, and ends it. Only the throwaway session is ever ended.
    pub fn verify_listing(&self) -> Result<(), VerificationError> {
        if !self.supports_ignore_size() {
            return Err(VerificationError::TooOld);
        }
        let name = format!(
            "termirust-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let mut command = self.command();
        command.args([
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_id}",
            "-s",
            &name,
            "/bin/sh",
        ]);
        let output = run_bounded(command, COMMAND_TIMEOUT).map_err(VerificationError::Tmux)?;
        if !output.status.success() {
            return Err(VerificationError::Tmux(TmuxError::CommandFailed {
                status: output.status.code(),
                diagnostic: bounded_diagnostic(String::from_utf8_lossy(&output.stderr).trim()),
            }));
        }
        let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let listed = self.list_sessions();
        if id.starts_with('$') {
            let mut kill = self.command();
            kill.args(["kill-session", "-t", &id]);
            let _ = run_bounded(kill, COMMAND_TIMEOUT);
        }
        let listing = listed.map_err(VerificationError::Tmux)?;
        if listing
            .sessions
            .iter()
            .any(|session| session.id() == id && session.name == name)
        {
            Ok(())
        } else {
            Err(VerificationError::NotListed)
        }
    }
}

/// Why [`Tmux::verify_listing`] failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerificationError {
    TooOld,
    Tmux(TmuxError),
    NotListed,
}

/// `PATH` entries joined with `tmux`, then the well-known install locations.
pub fn default_candidates(search_path: Option<OsString>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = search_path {
        candidates.extend(std::env::split_paths(&path).map(|directory| directory.join("tmux")));
    }
    for location in WELL_KNOWN_LOCATIONS {
        let location = PathBuf::from(location);
        if !candidates.contains(&location) {
            candidates.push(location);
        }
    }
    candidates
}

/// One session on the tmux server.
#[derive(Clone, Eq, PartialEq)]
pub struct TmuxSession {
    id: String,
    pub created_unix_seconds: u64,
    pub attached_clients: u32,
    pub windows: u32,
    pub name: String,
    pub current_command: String,
}

impl TmuxSession {
    /// The server-assigned id, such as `$3`. Unique while the server runs; a restarted
    /// server reuses ids, so pair it with [`TmuxSession::created_unix_seconds`].
    pub fn id(&self) -> &str {
        &self.id
    }

    /// A `target-session` that cannot be confused by a name containing `:` or `.`.
    pub fn session_target(&self) -> &str {
        &self.id
    }

    /// Whether `other` is the same session: same id and same creation time.
    pub fn same_session(&self, other: &Self) -> bool {
        self.id == other.id && self.created_unix_seconds == other.created_unix_seconds
    }

    /// Arguments that attach one more client to this session. `ignore-size` keeps the
    /// client out of window sizing, so a phone never reflows the desktop layout. The client
    /// flags force UTF-8, because the Session Host starts the client without a locale, and
    /// declare 24-bit color, which TermiRust's own terminals draw.
    pub fn attach_arguments(&self) -> Vec<String> {
        appearance::client_flags(true)
            .into_iter()
            .map(str::to_owned)
            .chain([
                "attach-session".to_owned(),
                "-f".to_owned(),
                "ignore-size".to_owned(),
                "-t".to_owned(),
                self.id.clone(),
            ])
            .collect()
    }
}

impl fmt::Debug for TmuxSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TmuxSession")
            .field("id", &self.id)
            .field("created_unix_seconds", &self.created_unix_seconds)
            .field("attached_clients", &self.attached_clients)
            .field("windows", &self.windows)
            .field("name", &"[REDACTED]")
            .field("current_command", &"[REDACTED]")
            .finish()
    }
}

/// A bounded `list-sessions` result.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionListing {
    pub sessions: Vec<TmuxSession>,
    /// Lines that did not match the expected format and were skipped.
    pub skipped_lines: usize,
    /// More than [`MAX_LISTED_SESSIONS`] sessions existed.
    pub truncated: bool,
}

/// Parses `list-sessions` output produced with this crate's format.
pub fn parse_list_sessions(stdout: &[u8]) -> SessionListing {
    let mut listing = SessionListing::default();
    for line in stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        if listing.sessions.len() == MAX_LISTED_SESSIONS {
            listing.truncated = true;
            break;
        }
        match parse_session_line(&String::from_utf8_lossy(line)) {
            Some(session) => listing.sessions.push(session),
            None => listing.skipped_lines += 1,
        }
    }
    listing
}

fn parse_session_line(line: &str) -> Option<TmuxSession> {
    let mut fields = line.splitn(6, FIELD_SEPARATOR);
    let id = fields.next()?;
    let created_unix_seconds = fields.next()?.parse().ok()?;
    let attached_clients = fields.next()?.parse().ok()?;
    let windows = fields.next()?.parse().ok()?;
    let name = fields.next()?;
    let current_command = fields.next()?;
    let valid_id = id
        .strip_prefix('$')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    if !valid_id || name.len() > MAX_SESSION_NAME_BYTES {
        return None;
    }
    Some(TmuxSession {
        id: id.to_owned(),
        created_unix_seconds,
        attached_clients,
        windows,
        name: name.to_owned(),
        current_command: current_command
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_COMMAND_CHARS)
            .collect(),
    })
}

/// Whether tmux's stderr says there is simply no server to talk to.
pub fn is_no_server_diagnostic(stderr: &str) -> bool {
    stderr.contains("no server running on")
        || (stderr.contains("error connecting to")
            && (stderr.contains("No such file or directory")
                || stderr.contains("Connection refused")))
}

/// Parses `tmux 3.7c`, `tmux next-3.6`, or `tmux 3.2` into `(major, minor)`.
pub fn parse_version(version: &str) -> Option<(u32, u32)> {
    let start = version.find(|character: char| character.is_ascii_digit())?;
    let mut parts = version[start..].split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, minor))
}

fn version_at(executable: &Path) -> Result<String, TmuxError> {
    let mut command = Command::new(executable);
    command.arg("-V");
    let output = run_bounded(command, COMMAND_TIMEOUT)?;
    if !output.status.success() {
        return Err(TmuxError::VersionProbe(match output.status.code() {
            Some(code) => format!("tmux -V exited with status {code}"),
            None => "tmux -V was terminated by a signal".to_owned(),
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

struct BoundedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stdout_overflowed: bool,
    stderr: Vec<u8>,
}

fn run_bounded(mut command: Command, timeout: Duration) -> Result<BoundedOutput, TmuxError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| TmuxError::Spawn(error.kind()))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_STDOUT_BYTES));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES));
    let status = wait_with_deadline(&mut child, timeout)?;
    let (stdout, stdout_overflowed) = stdout_reader.join().unwrap_or_default();
    let (stderr, _) = stderr_reader.join().unwrap_or_default();
    Ok(BoundedOutput {
        status,
        stdout,
        stdout_overflowed,
        stderr,
    })
}

fn wait_with_deadline(child: &mut Child, timeout: Duration) -> Result<ExitStatus, TmuxError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TmuxError::TimedOut);
            }
            Ok(None) => thread::sleep(COMMAND_POLL_INTERVAL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TmuxError::Spawn(error.kind()));
            }
        }
    }
}

// Keeps reading past the limit so a chatty child never blocks on a full pipe.
fn read_bounded(source: Option<impl Read>, limit: usize) -> (Vec<u8>, bool) {
    let Some(mut source) = source else {
        return (Vec::new(), false);
    };
    let mut kept = Vec::new();
    let mut overflowed = false;
    let mut buffer = [0_u8; 8192];
    loop {
        match source.read(&mut buffer) {
            Ok(0) => return (kept, overflowed),
            Ok(count) => {
                let room = limit.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..count.min(room)]);
                overflowed |= count > room;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return (kept, overflowed),
        }
    }
}

fn bounded_diagnostic(message: &str) -> String {
    if message.chars().count() <= MAX_DIAGNOSTIC_CHARS {
        return message.to_owned();
    }
    let mut bounded = message
        .chars()
        .take(MAX_DIAGNOSTIC_CHARS)
        .collect::<String>();
    bounded.push_str("...");
    bounded
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn effective_uid() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(fields: &[&str]) -> String {
        fields.join("\t")
    }

    #[test]
    fn listing_parses_real_output_and_names_with_punctuation() {
        let stdout = [
            line(&["$0", "1789453903", "1", "2", "demo", "zsh"]),
            line(&["$1", "1789453904", "0", "1", "a:b.c | pipe ", "vim"]),
            line(&["$12", "1789453905", "3", "1", "émoji 🚀", "cargo"]),
            line(&["$13", "1789453906", "0", "1", "", "sh"]),
        ]
        .join("\n")
            + "\n";
        let listing = parse_list_sessions(stdout.as_bytes());
        assert_eq!(listing.skipped_lines, 0);
        assert!(!listing.truncated);
        let names = listing
            .sessions
            .iter()
            .map(|session| session.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["demo", "a:b.c | pipe ", "émoji 🚀", ""]);
        let second = &listing.sessions[1];
        assert_eq!(second.id(), "$1");
        assert_eq!(second.session_target(), "$1");
        assert_eq!(second.created_unix_seconds, 1_789_453_904);
        assert_eq!(second.attached_clients, 0);
        assert_eq!(second.windows, 1);
        assert_eq!(second.current_command, "vim");
        assert_eq!(listing.sessions[2].attached_clients, 3);
    }

    #[test]
    fn listing_keeps_tabs_in_the_trailing_command_and_skips_malformed_lines() {
        let stdout = [
            line(&["$0", "10", "0", "1", "ok", "odd\tcommand\u{7}"]),
            line(&["0", "10", "0", "1", "no-dollar", "sh"]),
            line(&["$", "10", "0", "1", "no-digits", "sh"]),
            line(&["$1", "later", "0", "1", "bad-created", "sh"]),
            line(&["$2", "10", "-1", "1", "negative", "sh"]),
            line(&["$3", "10", "0", "1", "missing-command"]),
            "$4".to_owned(),
        ]
        .join("\n");
        let listing = parse_list_sessions(stdout.as_bytes());
        assert_eq!(listing.sessions.len(), 1);
        assert_eq!(listing.skipped_lines, 6);
        assert_eq!(listing.sessions[0].current_command, "oddcommand");
    }

    #[test]
    fn listing_is_bounded_in_count_name_and_command_length() {
        let long_name = "n".repeat(MAX_SESSION_NAME_BYTES + 1);
        let long_command = "c".repeat(MAX_COMMAND_CHARS * 2);
        let mut lines = vec![
            line(&["$0", "1", "0", "1", &long_name, "sh"]),
            line(&["$1", "1", "0", "1", "short", &long_command]),
        ];
        lines.extend(
            (2..MAX_LISTED_SESSIONS + 10).map(|index| format!("${index}\t1\t0\t1\ts{index}\tsh")),
        );
        let listing = parse_list_sessions(lines.join("\n").as_bytes());
        assert_eq!(listing.sessions.len(), MAX_LISTED_SESSIONS);
        assert!(listing.truncated);
        assert_eq!(listing.skipped_lines, 1);
        assert_eq!(listing.sessions[0].current_command.len(), MAX_COMMAND_CHARS);
    }

    #[test]
    fn empty_output_is_an_empty_listing() {
        assert_eq!(parse_list_sessions(b""), SessionListing::default());
        assert_eq!(parse_list_sessions(b"\n\n"), SessionListing::default());
    }

    #[test]
    fn no_server_diagnostics_are_recognized_across_tmux_versions() {
        assert!(is_no_server_diagnostic(
            "no server running on /private/tmp/tmux-501/default\n"
        ));
        assert!(is_no_server_diagnostic(
            "error connecting to /private/tmp/tmux-502/default (No such file or directory)\n"
        ));
        assert!(is_no_server_diagnostic(
            "error connecting to /tmp/tmux-1000/default (Connection refused)\n"
        ));
        assert!(!is_no_server_diagnostic(
            "error connecting to /tmp/tmux-1000/default (Permission denied)\n"
        ));
        assert!(!is_no_server_diagnostic("unknown option -- F\n"));
    }

    #[test]
    fn versions_parse_and_gate_ignore_size() {
        assert_eq!(parse_version("tmux 3.7c"), Some((3, 7)));
        assert_eq!(parse_version("tmux 3.2"), Some((3, 2)));
        assert_eq!(parse_version("tmux next-3.6"), Some((3, 6)));
        assert_eq!(parse_version("tmux 2.9a"), Some((2, 9)));
        assert_eq!(parse_version("tmux master"), None);
        let at = |version: &str| Tmux {
            executable: PathBuf::from("/fixture/tmux"),
            version: version.to_owned(),
            socket_directory: None,
        };
        assert!(at("tmux 3.7c").supports_ignore_size());
        assert!(at("tmux 3.2").supports_ignore_size());
        assert!(!at("tmux 3.1c").supports_ignore_size());
        assert!(!at("tmux 2.9a").supports_ignore_size());
        assert!(at("tmux master").supports_ignore_size());
    }

    #[test]
    fn discovery_honors_override_then_path_then_well_known_locations() {
        let probe = |_: &Path| Ok::<_, TmuxError>("tmux fixture".to_owned());
        let tmux = Tmux::discover_from(
            Some(OsString::from("/custom/tmux")),
            Some(OsString::from("/usr/bin")),
            |candidate| candidate == Path::new("/custom/tmux"),
            probe,
        )
        .unwrap();
        assert_eq!(tmux.executable(), Path::new("/custom/tmux"));
        assert_eq!(tmux.version(), "tmux fixture");

        assert_eq!(
            Tmux::discover_from(
                Some(OsString::from("/custom/tmux")),
                None,
                |candidate| candidate == Path::new("/usr/bin/tmux"),
                probe,
            ),
            Err(TmuxError::Unavailable),
            "an explicit override is exact and never falls back"
        );

        let tmux = Tmux::discover_from(
            None,
            Some(std::env::join_paths(["/first", "/second"]).unwrap()),
            |candidate| candidate == Path::new("/second/tmux"),
            probe,
        )
        .unwrap();
        assert_eq!(tmux.executable(), Path::new("/second/tmux"));

        let tmux = Tmux::discover_from(
            None,
            Some(OsString::from("/usr/bin:/bin")),
            |candidate| candidate == Path::new("/opt/homebrew/bin/tmux"),
            probe,
        )
        .unwrap();
        assert_eq!(tmux.executable(), Path::new("/opt/homebrew/bin/tmux"));
    }

    #[test]
    fn discovery_reports_missing_and_broken_binaries_distinctly() {
        let error =
            Tmux::discover_from(None, None, |_| false, |_| Ok::<_, TmuxError>(String::new()))
                .unwrap_err();
        assert_eq!(error, TmuxError::Unavailable);
        assert!(error.to_string().contains("tmux is not installed"));

        let error = Tmux::select(
            [PathBuf::from("fixture-tmux")],
            |_| true,
            |_| Err::<String, _>("synthetic tmux -V failure"),
        )
        .unwrap_err();
        assert_eq!(
            error,
            TmuxError::VersionProbe("synthetic tmux -V failure".to_owned())
        );
    }

    #[test]
    fn candidates_do_not_repeat_well_known_locations() {
        let candidates = default_candidates(Some(OsString::from("/usr/bin")));
        assert_eq!(
            candidates,
            [
                "/usr/bin/tmux",
                "/opt/homebrew/bin/tmux",
                "/usr/local/bin/tmux"
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn attach_arguments_target_the_id_and_never_size_the_window() {
        let session = parse_list_sessions(b"$7\t1\t0\t1\tname:with.dots\tsh").sessions[0].clone();
        assert_eq!(
            session.attach_arguments(),
            [
                "-u",
                "-T",
                "RGB",
                "attach-session",
                "-f",
                "ignore-size",
                "-t",
                "$7"
            ]
        );
    }

    #[test]
    fn session_identity_requires_matching_creation_time() {
        let first = parse_list_sessions(b"$0\t100\t0\t1\tone\tsh").sessions[0].clone();
        let renamed = parse_list_sessions(b"$0\t100\t1\t2\trenamed\tvim").sessions[0].clone();
        let reused = parse_list_sessions(b"$0\t200\t0\t1\tone\tsh").sessions[0].clone();
        assert!(first.same_session(&renamed));
        assert!(!first.same_session(&reused));
    }

    #[test]
    fn debug_output_redacts_names_and_commands() {
        let session =
            parse_list_sessions(b"$0\t100\t0\t1\tcanary-name\tcanary-command").sessions[0].clone();
        let debug = format!("{session:?}");
        assert!(debug.contains("$0"));
        assert!(!debug.contains("canary"));
    }

    #[test]
    fn socket_directory_is_forwarded_and_replaces_the_inherited_one() {
        let tmux = Tmux {
            executable: PathBuf::from("/fixture/tmux"),
            version: "tmux 3.7c".to_owned(),
            socket_directory: None,
        }
        .with_socket_directory("/fixture/sockets");
        let environment = tmux.client_environment();
        let socket_entries = environment
            .iter()
            .filter(|(name, _)| name == TMUX_TMPDIR_ENV)
            .collect::<Vec<_>>();
        assert_eq!(
            socket_entries,
            [&(TMUX_TMPDIR_ENV.to_owned(), "/fixture/sockets".to_owned())]
        );
        assert!(
            environment
                .iter()
                .all(|(name, _)| FORWARDED_ENVIRONMENT.contains(&name.as_str()))
        );
        assert!(tmux.server_identity().starts_with("/fixture/sockets/tmux-"));
        let command = tmux.command();
        let envs = command.get_envs().collect::<Vec<_>>();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-u", "-f", "/dev/null"],
            "UTF-8 output, and a private server ignores the developer's tmux configuration"
        );
        assert!(envs.contains(&(std::ffi::OsStr::new("TMUX"), None)));
        assert!(envs.contains(&(
            std::ffi::OsStr::new(TMUX_TMPDIR_ENV),
            Some(std::ffi::OsStr::new("/fixture/sockets"))
        )));
    }
}
