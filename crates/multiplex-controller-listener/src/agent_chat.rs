//! The conversation of a Claude Code or Codex session running in a terminal, read from the
//! agent's own session log.
//!
//! The agent is found under the terminal's shell by the name it was started by (or, for one
//! started through Node or Bun, by its script). Claude Code's log is
//! `~/.claude/projects/<folder>/<session>.jsonl`. The session is the one `CLAUDE_CODE_SESSION_ID`
//! names in the youngest command the agent is running, unless a conversation begun after that
//! command is newer; otherwise it is the newest log for the folder that no other running Claude
//! Code claims in its `~/.claude/sessions/<pid>.json` (which names the session it started in and
//! is not updated by `/clear` or `/resume`).
//! Codex keeps no such record, so its log is the newest
//! `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` whose first record names the agent's folder,
//! written since it started: two Codex sessions in one folder are told apart only by which
//! wrote last.
//!
//! The logs are the providers' own formats, which change between versions. Records this reader
//! does not recognise are skipped rather than guessed at; nothing read here becomes an action.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::git;
use crate::protocol::{
    AgentChat, AgentChatItem, AgentChatItemKind, AgentChatState, AgentProvider, AgentToolState,
    MAX_AGENT_CHAT_ID_BYTES, MAX_AGENT_CHAT_RESPONSE_BYTES,
};

/// The most of a log read: its newest part. Older history is reported as unavailable.
const MAX_LOG_TAIL_BYTES: u64 = 12 * 1024 * 1024;
/// The most items kept from one log, newest last.
const MAX_ITEMS: usize = 4_000;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;
const MAX_TOOL_OUTPUT_BYTES: usize = 2 * 1024;
const MAX_TITLE_CHARS: usize = 160;
/// How deep under the shell an agent is looked for, and how many processes at most.
const MAX_PROCESS_DEPTH: usize = 8;
const MAX_PROCESSES: usize = 512;
/// A log written this long before the agent started still counts: clocks and file times are
/// not exactly in step.
const START_SLACK: Duration = Duration::from_secs(5);
/// Codex day folders looked through, newest first: a resumed session appends to an old file.
const MAX_CODEX_DAYS: usize = 120;

pub struct ChatRequest {
    pub before: Option<String>,
    pub since_revision: Option<String>,
    pub limit: usize,
}

/// Where each provider keeps its logs.
#[derive(Clone, Debug)]
pub struct LogHomes {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
}

impl LogHomes {
    /// The providers' own defaults, honouring the variables each reads to move them.
    pub fn from_environment() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".claude")));
        let codex = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".codex")));
        Self { claude, codex }
    }
}

/// The agent's conversation for the terminal whose shell is `shell`, or `None` when no agent
/// runs under it.
pub fn read(shell: u64, request: &ChatRequest) -> Option<AgentChat> {
    read_with(shell, request, &LogHomes::from_environment())
}

pub fn read_with(shell: u64, request: &ChatRequest, homes: &LogHomes) -> Option<AgentChat> {
    let agent = find_agent(shell)?;
    let log = match agent.provider {
        AgentProvider::ClaudeCode => homes
            .claude
            .as_deref()
            .and_then(|root| claude_log(root, &agent)),
        AgentProvider::Codex => homes
            .codex
            .as_deref()
            .and_then(|root| codex_log(root, &agent.folder, agent.started)),
    };
    let Some(log) = log else {
        // Started, but nothing said yet.
        return Some(AgentChat {
            provider: agent.provider,
            title: None,
            state: AgentChatState::Ready,
            revision: format!("empty-{}", agent.process_id),
            unchanged: request.since_revision.as_deref()
                == Some(format!("empty-{}", agent.process_id).as_str()),
            items: Vec::new(),
            earlier: false,
            history_truncated: false,
        });
    };
    let parsed = parsed_log(&log, agent.provider).ok()?;
    Some(page(&parsed, agent.provider, request))
}

// --- Finding the agent ---------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Agent {
    provider: AgentProvider,
    process_id: u64,
    folder: PathBuf,
    started: SystemTime,
    /// The Claude Code session named by the youngest command the agent is running, with when
    /// that command started.
    current_session: Option<(String, SystemTime)>,
}

#[derive(Clone, Debug)]
struct ProcessInfo {
    executable: Option<PathBuf>,
    arguments: Vec<String>,
    started: SystemTime,
}

/// The variables Claude Code hands each command it runs: which session, and which process.
const SESSION_VARIABLE: &str = "CLAUDE_CODE_SESSION_ID=";
const PROCESS_VARIABLE: &str = "CLAUDE_PID=";

fn find_agent(shell: u64) -> Option<Agent> {
    let mut queue = std::collections::VecDeque::from([(shell, 0_usize)]);
    let mut seen = 0;
    let children = ChildIndex::new();
    while let Some((process_id, depth)) = queue.pop_front() {
        seen += 1;
        if seen > MAX_PROCESSES {
            break;
        }
        if let Some(info) = process_info(process_id)
            && let Some(provider) = provider_of(&info)
            && let Some(folder) = git::process_folder(process_id)
        {
            let current_session = (provider == AgentProvider::ClaudeCode)
                .then(|| running_command_session(&children, process_id))
                .flatten();
            return Some(Agent {
                provider,
                process_id,
                folder,
                started: info.started,
                current_session,
            });
        }
        if depth < MAX_PROCESS_DEPTH {
            for child in children.of(process_id) {
                queue.push_back((child, depth + 1));
            }
        }
    }
    None
}

/// The session named by the youngest command Claude Code is running, which is the session it
/// was in when that command started. `/clear` and `/resume` change the session without updating
/// the agent's own record, and a long-lived child such as an MCP server keeps the one the agent
/// started in, so the youngest command is the best witness.
fn running_command_session(children: &ChildIndex, agent: u64) -> Option<(String, SystemTime)> {
    let mut queue: std::collections::VecDeque<(u64, usize)> = children
        .of(agent)
        .into_iter()
        .map(|child| (child, 1))
        .collect();
    let mut seen = 0;
    let mut youngest: Option<(String, SystemTime)> = None;
    while let Some((process_id, depth)) = queue.pop_front() {
        seen += 1;
        if seen > 64 {
            break;
        }
        let environment = claude_environment(process_id);
        let session = environment
            .iter()
            .find_map(|entry| entry.strip_prefix(SESSION_VARIABLE));
        let owner = environment
            .iter()
            .find_map(|entry| entry.strip_prefix(PROCESS_VARIABLE))
            .and_then(|owner| owner.parse::<u64>().ok());
        if let Some(session) = session.filter(|session| valid_session(session))
            && owner.is_none_or(|owner| owner == agent)
            && let Some(started) = process_info(process_id).map(|info| info.started)
            && youngest.as_ref().is_none_or(|(_, known)| started > *known)
        {
            youngest = Some((session.to_owned(), started));
        }
        if depth < 4 {
            queue.extend(
                children
                    .of(process_id)
                    .into_iter()
                    .map(|child| (child, depth + 1)),
            );
        }
    }
    youngest
}

fn valid_session(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 128
        && session
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn provider_of(info: &ProcessInfo) -> Option<AgentProvider> {
    let name = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    };
    let executable = info.executable.as_deref().map(name).unwrap_or_default();
    let named = |value: &str| match value {
        "claude" | "claude-code" => Some(AgentProvider::ClaudeCode),
        "codex" => Some(AgentProvider::Codex),
        _ => None,
    };
    // The native installs name the binary after its version (`…/claude/versions/2.1.3`), so the
    // name it was started by counts too.
    let started_as = info
        .arguments
        .first()
        .map(|first| name(Path::new(first)))
        .unwrap_or_default();
    if let Some(provider) = named(&executable).or_else(|| named(&started_as)) {
        return Some(provider);
    }
    if info
        .executable
        .as_deref()
        .is_some_and(|path| path.to_string_lossy().contains("/claude/versions/"))
    {
        return Some(AgentProvider::ClaudeCode);
    }
    // `node …/claude-code/cli.js` and `node …/bin/codex.js`: the script names the agent.
    if matches!(executable.as_str(), "node" | "bun" | "deno") {
        for argument in info.arguments.iter().skip(1).take(4) {
            let lowered = argument.to_ascii_lowercase();
            if lowered.starts_with('-') {
                continue;
            }
            if lowered.contains("claude-code") || name(Path::new(&lowered)) == "claude" {
                return Some(AgentProvider::ClaudeCode);
            }
            if lowered.contains("@openai/codex")
                || matches!(name(Path::new(&lowered)).as_str(), "codex" | "codex.js")
            {
                return Some(AgentProvider::Codex);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
struct ChildIndex;

#[cfg(target_os = "macos")]
impl ChildIndex {
    fn new() -> Self {
        Self
    }

    fn of(&self, process_id: u64) -> Vec<u64> {
        let Ok(parent) = i32::try_from(process_id) else {
            return Vec::new();
        };
        let mut buffer = vec![0_i32; 256];
        let bytes = (buffer.len() * std::mem::size_of::<i32>()) as libc::c_int;
        // SAFETY: `buffer` is writable for `bytes` bytes for the call's duration.
        let written =
            unsafe { libc::proc_listchildpids(parent, buffer.as_mut_ptr().cast(), bytes) };
        if written <= 0 {
            return Vec::new();
        }
        // It answers with a count of ids on some releases and a byte length on others.
        let count = usize::try_from(written).unwrap_or(0);
        let count = if count > buffer.len() {
            count / std::mem::size_of::<i32>()
        } else {
            count
        };
        buffer
            .into_iter()
            .take(count)
            .filter(|id| *id > 0)
            .map(|id| id as u64)
            .collect()
    }
}

#[cfg(target_os = "macos")]
fn process_info(process_id: u64) -> Option<ProcessInfo> {
    use std::os::unix::ffi::OsStrExt as _;

    let pid = i32::try_from(process_id).ok().filter(|id| *id > 0)?;
    // SAFETY: an all-zero `proc_bsdinfo` is valid; the kernel fills it in.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a writable buffer of exactly `size` bytes for the call's duration.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if written != size {
        return None;
    }
    let started = UNIX_EPOCH + Duration::from_secs(info.pbi_start_tvsec);
    let mut path = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `path` is writable for its whole length for the call's duration.
    let length = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    let executable =
        (length > 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&path[..length as usize])));
    Some(ProcessInfo {
        arguments: process_arguments(pid),
        executable,
        started,
    })
}

/// A process's arguments, from `KERN_PROCARGS2`: argc, the executable path, then argv.
#[cfg(target_os = "macos")]
fn process_arguments(pid: i32) -> Vec<String> {
    procargs(pid).0
}

#[cfg(target_os = "macos")]
fn claude_environment(process_id: u64) -> Vec<String> {
    i32::try_from(process_id)
        .map(|pid| procargs(pid).1)
        .unwrap_or_default()
}

/// A process's arguments and the Claude Code variables in its environment, from
/// `KERN_PROCARGS2`: argc, the executable path, argv, then the environment.
#[cfg(target_os = "macos")]
fn procargs(pid: i32) -> (Vec<String>, Vec<String>) {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: asks only for the size the next call needs.
    let sized = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if sized != 0 || size < 4 {
        return (Vec::new(), Vec::new());
    }
    let size_limit = 256 * 1024;
    let mut buffer = vec![0_u8; size.min(size_limit)];
    let mut length = buffer.len();
    // SAFETY: `buffer` is writable for `length` bytes for the call's duration.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &raw mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 || length < 4 {
        return (Vec::new(), Vec::new());
    }
    let buffer = &buffer[..length];
    let argc = i32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]).max(0) as usize;
    let mut fields = buffer[4..]
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let _executable = fields.next();
    let arguments = fields
        .by_ref()
        .take(argc)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .take(16)
        .collect();
    let environment = fields
        .take(4_096)
        .filter(|field| field.starts_with(b"CLAUDE_"))
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .filter(|entry| entry.starts_with(SESSION_VARIABLE) || entry.starts_with(PROCESS_VARIABLE))
        .collect();
    (arguments, environment)
}

#[cfg(target_os = "linux")]
struct ChildIndex(HashMap<u64, Vec<u64>>);

#[cfg(target_os = "linux")]
impl ChildIndex {
    fn new() -> Self {
        let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(process_id) = entry.file_name().to_str().and_then(|n| n.parse().ok())
                else {
                    continue;
                };
                if let Some(parent) = linux_stat(process_id).map(|stat| stat.parent) {
                    children.entry(parent).or_default().push(process_id);
                }
            }
        }
        Self(children)
    }

    fn of(&self, process_id: u64) -> Vec<u64> {
        self.0.get(&process_id).cloned().unwrap_or_default()
    }
}

#[cfg(target_os = "linux")]
struct LinuxStat {
    parent: u64,
    start_ticks: u64,
}

#[cfg(target_os = "linux")]
fn linux_stat(process_id: u64) -> Option<LinuxStat> {
    let stat = fs::read_to_string(format!("/proc/{process_id}/stat")).ok()?;
    // The command name is in parentheses and may hold spaces; fields follow its last ')'.
    let fields: Vec<&str> = stat.get(stat.rfind(')')? + 2..)?.split(' ').collect();
    Some(LinuxStat {
        parent: fields.get(1)?.parse().ok()?,
        start_ticks: fields.get(19)?.parse().ok()?,
    })
}

#[cfg(target_os = "linux")]
fn claude_environment(process_id: u64) -> Vec<String> {
    fs::read(format!("/proc/{process_id}/environ"))
        .map(|bytes| {
            bytes
                .split(|byte| *byte == 0)
                .filter(|field| field.starts_with(b"CLAUDE_"))
                .map(|field| String::from_utf8_lossy(field).into_owned())
                .filter(|entry| {
                    entry.starts_with(SESSION_VARIABLE) || entry.starts_with(PROCESS_VARIABLE)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn process_info(process_id: u64) -> Option<ProcessInfo> {
    let stat = linux_stat(process_id)?;
    let boot = fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime ")?.trim().parse::<u64>().ok())?;
    // SAFETY: `sysconf` has no preconditions.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    let started = UNIX_EPOCH
        + Duration::from_secs(boot)
        + Duration::from_millis(stat.start_ticks.saturating_mul(1000) / ticks);
    let arguments = fs::read(format!("/proc/{process_id}/cmdline"))
        .map(|bytes| {
            bytes
                .split(|byte| *byte == 0)
                .filter(|field| !field.is_empty())
                .take(16)
                .map(|field| String::from_utf8_lossy(field).into_owned())
                .collect()
        })
        .unwrap_or_default();
    Some(ProcessInfo {
        executable: fs::read_link(format!("/proc/{process_id}/exe")).ok(),
        arguments,
        started,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
struct ChildIndex;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl ChildIndex {
    fn new() -> Self {
        Self
    }

    fn of(&self, _process_id: u64) -> Vec<u64> {
        Vec::new()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn claude_environment(_process_id: u64) -> Vec<String> {
    Vec::new()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_info(_process_id: u64) -> Option<ProcessInfo> {
    None
}

// --- Finding the log -----------------------------------------------------------------------

/// Claude Code names a project's folder after its path with every other character a dash.
fn claude_project_name(folder: &Path) -> String {
    folder
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn claude_log(root: &Path, agent: &Agent) -> Option<PathBuf> {
    let directory = claude_project_directory(root, &agent.folder)?;
    let claimed = other_claims(root, agent);
    let candidates = logs_since(
        fs::read_dir(&directory).ok()?.flatten().map(|e| e.path()),
        agent.started,
    );
    let unclaimed: Vec<&(SystemTime, PathBuf)> = candidates
        .iter()
        .filter(|(_, path)| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_none_or(|stem| !claimed.contains(stem))
        })
        .collect();
    if let Some((session, command_started)) = &agent.current_session {
        let own = directory.join(format!("{session}.jsonl"));
        if let Ok(own_modified) = fs::metadata(&own).and_then(|metadata| metadata.modified()) {
            // A conversation begun after that command started, and written since, is newer
            // than what the command knew.
            let superseded = unclaimed.iter().any(|(modified, path)| {
                *path != own
                    && *modified > own_modified
                    && fs::metadata(path)
                        .and_then(|metadata| metadata.created())
                        .is_ok_and(|created| created > *command_started)
            });
            if !superseded {
                return Some(own);
            }
        }
    }
    unclaimed.first().map(|(_, path)| path.clone())
}

/// The sessions other running Claude Code processes started in, by their records.
fn other_claims(root: &Path, agent: &Agent) -> std::collections::HashSet<String> {
    let mut claimed = std::collections::HashSet::new();
    if let Ok(entries) = fs::read_dir(root.join("sessions")) {
        for entry in entries.flatten().take(1_024) {
            let Some(process_id) = entry
                .path()
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse::<u64>().ok())
            else {
                continue;
            };
            if process_id == agent.process_id || !alive(process_id) {
                continue;
            }
            let session = fs::read_to_string(entry.path())
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|record| record.get("sessionId")?.as_str().map(str::to_owned));
            if let Some(session) = session.filter(|session| valid_session(session)) {
                claimed.insert(session);
            }
        }
    }
    claimed
}

#[cfg(unix)]
fn alive(process_id: u64) -> bool {
    let Ok(pid) = i32::try_from(process_id) else {
        return false;
    };
    // SAFETY: signal 0 only asks whether the process exists.
    let answered = unsafe { libc::kill(pid, 0) };
    answered == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// No agent is found on other platforms, so there is no other process to ask about.
#[cfg(not(unix))]
fn alive(_process_id: u64) -> bool {
    false
}

fn claude_project_directory(root: &Path, folder: &Path) -> Option<PathBuf> {
    let name = claude_project_name(folder);
    let projects = root.join("projects");
    let mut directory = projects.join(&name);
    if !directory.is_dir() && name.len() > 200 {
        // A long path is cut short and given a suffix; match on the part kept.
        let prefix = &name[..200];
        directory = fs::read_dir(&projects)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix))
            })?;
    }
    Some(directory)
}

fn codex_log(root: &Path, folder: &Path, started: SystemTime) -> Option<PathBuf> {
    let sessions = root.join("sessions");
    let mut days = Vec::new();
    for year in sorted_directories(&sessions) {
        for month in sorted_directories(&year) {
            days.extend(sorted_directories(&month));
        }
    }
    days.sort();
    days.reverse();
    days.truncate(MAX_CODEX_DAYS);
    let wanted = canonical(folder);
    newest_log(
        days.iter()
            .filter_map(|day| fs::read_dir(day).ok())
            .flat_map(|entries| entries.flatten().map(|entry| entry.path())),
        started,
        |path| codex_log_folder(path).is_some_and(|logged| canonical(&logged) == wanted),
    )
}

fn sorted_directories(path: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// The folder a Codex rollout's first record says the session runs in.
fn codex_log_folder(path: &Path) -> Option<PathBuf> {
    let file = File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file.take(256 * 1024))
        .read_line(&mut line)
        .ok()?;
    let record: Value = serde_json::from_str(&line).ok()?;
    (record.get("type")?.as_str()? == "session_meta")
        .then(|| record.pointer("/payload/cwd")?.as_str().map(PathBuf::from))?
}

/// The most recently written `.jsonl` among `candidates` since `started`, that `accept` takes.
fn newest_log(
    candidates: impl Iterator<Item = PathBuf>,
    started: SystemTime,
    accept: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    logs_since(candidates, started)
        .into_iter()
        .map(|(_, path)| path)
        .find(|path| accept(path))
}

/// The `.jsonl` files among `candidates` written since `started`, newest first.
fn logs_since(
    candidates: impl Iterator<Item = PathBuf>,
    started: SystemTime,
) -> Vec<(SystemTime, PathBuf)> {
    let since = started.checked_sub(START_SLACK).unwrap_or(started);
    let mut found: Vec<(SystemTime, PathBuf)> = candidates
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .filter_map(|path| {
            let modified = fs::metadata(&path).ok()?.modified().ok()?;
            (modified >= since).then_some((modified, path))
        })
        .collect();
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found
}

// --- Reading the log -----------------------------------------------------------------------

#[derive(Debug)]
struct ParsedLog {
    revision: String,
    title: Option<String>,
    state: AgentChatState,
    items: Vec<AgentChatItem>,
    history_truncated: bool,
}

type Cache = Mutex<HashMap<PathBuf, (u64, SystemTime, Arc<ParsedLog>)>>;

/// The last few logs read, so a phone asking every second costs a `stat`, not a parse.
fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn parsed_log(path: &Path, provider: AgentProvider) -> io::Result<Arc<ParsedLog>> {
    let metadata = fs::metadata(path)?;
    let length = metadata.len();
    let modified = metadata.modified()?;
    if let Some((cached_length, cached_modified, parsed)) = cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(path)
        && *cached_length == length
        && *cached_modified == modified
    {
        return Ok(Arc::clone(parsed));
    }
    let (offset, text) = read_tail(path, length)?;
    let mut parsed = match provider {
        AgentProvider::ClaudeCode => parse_claude(&text, offset),
        AgentProvider::Codex => parse_codex(&text, offset),
    };
    parsed.history_truncated = offset > 0;
    let nanos = modified
        .duration_since(UNIX_EPOCH)
        .map(|age| age.as_nanos())
        .unwrap_or_default();
    parsed.revision = format!("{length:x}-{nanos:x}");
    let parsed = Arc::new(parsed);
    let mut cache = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    if cache.len() >= 8 {
        cache.clear();
    }
    cache.insert(path.to_owned(), (length, modified, Arc::clone(&parsed)));
    Ok(parsed)
}

/// The newest part of a log, starting at a whole line, with the offset it starts at.
fn read_tail(path: &Path, length: u64) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut offset = length.saturating_sub(MAX_LOG_TAIL_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity((length - offset) as usize);
    file.take(MAX_LOG_TAIL_BYTES).read_to_end(&mut bytes)?;
    if offset > 0 {
        let skip = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |at| at + 1);
        bytes.drain(..skip);
        offset += skip as u64;
    }
    Ok((offset, String::from_utf8_lossy(&bytes).into_owned()))
}

/// Each line of `text` that parses, with the file offset it starts at.
fn records(text: &str, offset: u64) -> impl Iterator<Item = (u64, Value)> + '_ {
    let mut position = offset;
    text.split('\n').filter_map(move |line| {
        let at = position;
        position += line.len() as u64 + 1;
        serde_json::from_str::<Value>(line)
            .ok()
            .map(|value| (at, value))
    })
}

struct Builder {
    items: Vec<AgentChatItem>,
    tools: HashMap<String, usize>,
}

impl Builder {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            tools: HashMap::new(),
        }
    }

    fn message(&mut self, id: String, kind: AgentChatItemKind, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let (text, truncated) = clip(text, MAX_MESSAGE_BYTES);
        self.items.push(AgentChatItem {
            id: bounded_id(id),
            kind,
            title: None,
            text,
            tool_state: None,
            truncated,
        });
    }

    fn tool_started(&mut self, id: String, title: String) {
        if let Some(index) = self.tools.get(&id) {
            self.items[*index].title = Some(title);
            return;
        }
        self.tools.insert(id.clone(), self.items.len());
        self.items.push(AgentChatItem {
            id: bounded_id(id),
            kind: AgentChatItemKind::Tool,
            title: Some(title),
            text: String::new(),
            tool_state: Some(AgentToolState::Running),
            truncated: false,
        });
    }

    fn tool_finished(&mut self, id: &str, failed: bool, output: &str) {
        let Some(index) = self.tools.get(id) else {
            return;
        };
        let item = &mut self.items[*index];
        let (text, truncated) = clip(output.trim(), MAX_TOOL_OUTPUT_BYTES);
        item.text = text;
        item.truncated = truncated;
        item.tool_state = Some(if failed {
            AgentToolState::Failed
        } else {
            AgentToolState::Succeeded
        });
    }

    fn stop_running_tools(&mut self) {
        for item in &mut self.items {
            if item.tool_state == Some(AgentToolState::Running) {
                item.tool_state = Some(AgentToolState::Failed);
            }
        }
    }

    fn has_running_tool(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.tool_state == Some(AgentToolState::Running))
    }

    fn finish(mut self, title: Option<String>, state: AgentChatState) -> ParsedLog {
        if self.items.len() > MAX_ITEMS {
            self.items.drain(..self.items.len() - MAX_ITEMS);
        }
        ParsedLog {
            revision: String::new(),
            title,
            state,
            items: self.items,
            history_truncated: false,
        }
    }
}

/// What the last thing in a Claude Code log means for the turn.
#[derive(PartialEq)]
enum Last {
    Nothing,
    Prompt,
    Answer { finished: bool },
    Interrupted,
}

fn parse_claude(text: &str, offset: u64) -> ParsedLog {
    let mut builder = Builder::new();
    let mut title = None;
    let mut last = Last::Nothing;
    for (at, record) in records(text, offset) {
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || record.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let id = record
            .get("uuid")
            .and_then(Value::as_str)
            .map_or_else(|| format!("o{at}"), str::to_owned);
        match record.get("type").and_then(Value::as_str) {
            Some("ai-title") => {
                if let Some(value) = record.get("aiTitle").and_then(Value::as_str) {
                    title = Some(clip_chars(value, MAX_TITLE_CHARS));
                }
            }
            Some("summary") if title.is_none() => {
                if let Some(value) = record.get("summary").and_then(Value::as_str) {
                    title = Some(clip_chars(value, MAX_TITLE_CHARS));
                }
            }
            Some("user") => match record.pointer("/message/content") {
                Some(Value::String(content)) => {
                    if let Some(next) = claude_user_text(&mut builder, id, content) {
                        last = next;
                    }
                }
                Some(Value::Array(blocks)) => {
                    for (index, block) in blocks.iter().enumerate() {
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                let content =
                                    block.get("text").and_then(Value::as_str).unwrap_or("");
                                if let Some(next) =
                                    claude_user_text(&mut builder, format!("{id}#{index}"), content)
                                {
                                    last = next;
                                }
                            }
                            Some("tool_result") => {
                                let Some(tool) = block.get("tool_use_id").and_then(Value::as_str)
                                else {
                                    continue;
                                };
                                let failed =
                                    block.get("is_error").and_then(Value::as_bool) == Some(true);
                                let output = content_text(block.get("content"));
                                builder.tool_finished(tool, failed, &output);
                                last = Last::Prompt;
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            Some("assistant") => {
                let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array)
                else {
                    continue;
                };
                for (index, block) in blocks.iter().enumerate() {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => builder.message(
                            format!("{id}#{index}"),
                            AgentChatItemKind::Assistant,
                            block.get("text").and_then(Value::as_str).unwrap_or(""),
                        ),
                        Some("tool_use") => {
                            let Some(tool) = block.get("id").and_then(Value::as_str) else {
                                continue;
                            };
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("Tool");
                            builder.tool_started(
                                tool.to_owned(),
                                tool_title(name, block.get("input")),
                            );
                        }
                        _ => {}
                    }
                }
                let stop = record
                    .pointer("/message/stop_reason")
                    .and_then(Value::as_str);
                last = Last::Answer {
                    finished: matches!(stop, Some("end_turn" | "stop_sequence" | "max_tokens")),
                };
            }
            Some("system")
                if record.get("subtype").and_then(Value::as_str) == Some("compact_boundary") =>
            {
                builder.message(
                    id,
                    AgentChatItemKind::Notice,
                    "Earlier conversation was compacted.",
                );
            }
            _ => {}
        }
    }
    let state = if last == Last::Interrupted {
        builder.stop_running_tools();
        AgentChatState::Ready
    } else if builder.has_running_tool() {
        AgentChatState::Working
    } else {
        match last {
            Last::Nothing => AgentChatState::Ready,
            Last::Prompt => AgentChatState::Working,
            Last::Answer { finished } if finished => AgentChatState::Ready,
            Last::Answer { .. } => AgentChatState::Working,
            Last::Interrupted => AgentChatState::Ready,
        }
    };
    builder.finish(title, state)
}

/// A prompt as the person typed it; markup Claude Code adds around commands and context is
/// not something they said. Answers what the prompt means for the turn, or `None` to skip it.
fn claude_user_text(builder: &mut Builder, id: String, content: &str) -> Option<Last> {
    let content = content.trim();
    if content.starts_with("[Request interrupted") {
        builder.message(id, AgentChatItemKind::Notice, "Interrupted.");
        return Some(Last::Interrupted);
    }
    if let Some(command) = between(content, "<command-name>", "</command-name>") {
        let arguments = between(content, "<command-args>", "</command-args>").unwrap_or("");
        let command = if command.starts_with('/') {
            command.to_owned()
        } else {
            format!("/{command}")
        };
        builder.message(
            id,
            AgentChatItemKind::User,
            format!("{command} {arguments}").trim(),
        );
        return Some(Last::Prompt);
    }
    if content.is_empty() || content.starts_with('<') || content.starts_with("Caveat:") {
        return None;
    }
    builder.message(id, AgentChatItemKind::User, content);
    Some(Last::Prompt)
}

fn parse_codex(text: &str, offset: u64) -> ParsedLog {
    #[derive(Clone, Copy, PartialEq)]
    enum Source {
        Completed,
        Event,
        Response,
    }
    let mut builder = Builder::new();
    // Codex records a prompt more than once across versions; one source is kept, chosen below.
    let mut prompts: Vec<(usize, Source)> = Vec::new();
    let mut state = AgentChatState::Unknown;
    for (at, record) in records(text, offset) {
        let fallback = format!("o{at}");
        let Some(payload) = record.get("payload") else {
            continue;
        };
        let kind = payload.get("type").and_then(Value::as_str);
        match record.get("type").and_then(Value::as_str) {
            Some("event_msg") => match kind {
                Some("item_completed") => {
                    let Some(item) = payload.get("item") else {
                        continue;
                    };
                    if item.get("type").and_then(Value::as_str) == Some("UserMessage") {
                        let id = item.get("id").and_then(Value::as_str).unwrap_or(&fallback);
                        let before = builder.items.len();
                        builder.message(
                            format!("u{id}"),
                            AgentChatItemKind::User,
                            &content_text(item.get("content")),
                        );
                        if builder.items.len() > before {
                            prompts.push((before, Source::Completed));
                        }
                    }
                }
                Some("user_message") => {
                    let before = builder.items.len();
                    builder.message(
                        fallback,
                        AgentChatItemKind::User,
                        payload.get("message").and_then(Value::as_str).unwrap_or(""),
                    );
                    if builder.items.len() > before {
                        prompts.push((before, Source::Event));
                    }
                }
                Some("task_started") => state = AgentChatState::Working,
                Some("task_complete") => state = AgentChatState::Ready,
                Some("turn_aborted") => {
                    builder.stop_running_tools();
                    builder.message(fallback, AgentChatItemKind::Notice, "Interrupted.");
                    state = AgentChatState::Ready;
                }
                _ => {}
            },
            Some("response_item") => match kind {
                Some("message") => {
                    let id = payload
                        .get("id")
                        .and_then(Value::as_str)
                        .map_or_else(|| fallback.clone(), str::to_owned);
                    match payload.get("role").and_then(Value::as_str) {
                        Some("assistant") => builder.message(
                            id,
                            AgentChatItemKind::Assistant,
                            &content_text(payload.get("content")),
                        ),
                        Some("user") => {
                            // Codex also sends instructions and context as user messages; only
                            // ones it marks as typed text, or plain text without markup, count.
                            let kinds = payload
                                .pointer("/internal_chat_message_metadata_passthrough/content_item_kinds")
                                .and_then(Value::as_array);
                            let typed = kinds.map_or_else(
                                || {
                                    let text = content_text(payload.get("content"));
                                    let text = text.trim_start();
                                    !text.starts_with('<') && !text.starts_with('#')
                                },
                                |kinds| kinds.iter().any(|kind| kind.as_str() == Some("user.text")),
                            );
                            if typed {
                                let before = builder.items.len();
                                builder.message(
                                    id,
                                    AgentChatItemKind::User,
                                    &content_text(payload.get("content")),
                                );
                                if builder.items.len() > before {
                                    prompts.push((before, Source::Response));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some("function_call" | "custom_tool_call" | "local_shell_call") => {
                    let Some(call) = payload.get("call_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let name = payload
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("shell");
                    let input = payload
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
                        .or_else(|| payload.get("input").cloned())
                        .or_else(|| payload.get("action").cloned());
                    builder.tool_started(call.to_owned(), tool_title(name, input.as_ref()));
                }
                Some(
                    "function_call_output" | "custom_tool_call_output" | "local_shell_call_output",
                ) => {
                    let Some(call) = payload.get("call_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let output = content_text(payload.get("output"));
                    builder.tool_finished(call, false, &output);
                }
                _ => {}
            },
            Some("compacted") => builder.message(
                fallback,
                AgentChatItemKind::Notice,
                "Earlier conversation was compacted.",
            ),
            _ => {}
        }
    }
    let keep = [Source::Completed, Source::Event, Source::Response]
        .into_iter()
        .find(|source| prompts.iter().any(|(_, found)| found == source));
    let drop: std::collections::HashSet<usize> = prompts
        .iter()
        .filter(|(_, source)| Some(*source) != keep)
        .map(|(index, _)| *index)
        .collect();
    if !drop.is_empty() {
        let mut index = 0;
        builder.items.retain(|_| {
            let kept = !drop.contains(&index);
            index += 1;
            kept
        });
        builder.tools = builder
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.kind == AgentChatItemKind::Tool)
            .map(|(index, item)| (item.id.clone(), index))
            .collect();
    }
    if state == AgentChatState::Unknown && builder.has_running_tool() {
        state = AgentChatState::Working;
    }
    builder.finish(None, state)
}

/// The text of a content value: a string, or text blocks of any of the providers' kinds.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| match block {
                Value::String(text) => Some(text.as_str()),
                Value::Object(_) => match block.get("type").and_then(Value::as_str) {
                    Some("image" | "input_image") => Some("[image]"),
                    _ => block.get("text").and_then(Value::as_str),
                },
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A tool's name and what it acts on: its command, file, pattern or address.
fn tool_title(name: &str, input: Option<&Value>) -> String {
    let detail = input.and_then(|input| match input {
        Value::String(text) => Some(between(text, "cmd:\"", "\"").unwrap_or(text).to_owned()),
        Value::Object(_) => [
            "command",
            "cmd",
            "file_path",
            "path",
            "pattern",
            "url",
            "query",
            "description",
            "prompt",
        ]
        .iter()
        .find_map(|key| match input.get(*key)? {
            Value::String(text) => Some(text.clone()),
            Value::Array(parts) => Some(
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        }),
        _ => None,
    });
    let detail = detail
        .as_deref()
        .and_then(|detail| detail.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or("");
    let title = if detail.is_empty() {
        name.to_owned()
    } else {
        format!("{name} · {detail}")
    };
    clip_chars(&title, MAX_TITLE_CHARS)
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let to = text[from..].find(end)? + from;
    Some(text[from..to].trim())
}

fn bounded_id(id: String) -> String {
    if id.len() <= MAX_AGENT_CHAT_ID_BYTES && !id.chars().any(char::is_control) {
        return id;
    }
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(id.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn clip(text: &str, maximum: usize) -> (String, bool) {
    if text.len() <= maximum {
        return (text.to_owned(), false);
    }
    let mut end = maximum;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

fn clip_chars(text: &str, maximum: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(maximum) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

// --- Answering -----------------------------------------------------------------------------

fn page(parsed: &ParsedLog, provider: AgentProvider, request: &ChatRequest) -> AgentChat {
    let mut chat = AgentChat {
        provider,
        title: parsed.title.clone(),
        state: parsed.state,
        revision: parsed.revision.clone(),
        unchanged: false,
        items: Vec::new(),
        earlier: false,
        history_truncated: parsed.history_truncated,
    };
    if request.before.is_none() && request.since_revision.as_deref() == Some(&parsed.revision) {
        chat.unchanged = true;
        return chat;
    }
    let end = match &request.before {
        Some(before) => parsed
            .items
            .iter()
            .position(|item| &item.id == before)
            .unwrap_or(0),
        None => parsed.items.len(),
    };
    let start = end.saturating_sub(request.limit.max(1));
    chat.items = parsed.items[start..end].to_vec();
    chat.earlier = start > 0;
    // Trim oldest first until the answer fits one frame, with room for its envelope.
    while !chat.items.is_empty()
        && serde_json::to_vec(&chat).map_or(usize::MAX, |bytes| bytes.len())
            > MAX_AGENT_CHAT_RESPONSE_BYTES - 512
    {
        chat.items.remove(0);
        chat.earlier = true;
    }
    chat
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(records: &[Value]) -> String {
        records
            .iter()
            .map(|record| serde_json::to_string(record).expect("record"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_claude_log_reads_as_prompts_answers_and_tool_cards() {
        let log = lines(&[
            serde_json::json!({"type":"ai-title","aiTitle":"Fix the build"}),
            serde_json::json!({"type":"user","uuid":"u1","message":{"role":"user","content":"<local-command-caveat>Caveat: ignore</local-command-caveat>"}}),
            serde_json::json!({"type":"user","uuid":"u2","message":{"role":"user","content":"Why does the build fail?"}}),
            serde_json::json!({"type":"assistant","uuid":"a1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hidden"}],"stop_reason":null}}),
            serde_json::json!({"type":"assistant","uuid":"a2","message":{"role":"assistant","content":[{"type":"text","text":"Let me look."}],"stop_reason":null}}),
            serde_json::json!({"type":"assistant","uuid":"a3","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build\n--verbose"}}],"stop_reason":"tool_use"}}),
            serde_json::json!({"type":"user","uuid":"r1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"error[E0425]","is_error":true}]}}),
            serde_json::json!({"type":"assistant","uuid":"a4","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"a subagent"}]}}),
            serde_json::json!({"type":"assistant","uuid":"a5","message":{"role":"assistant","content":[{"type":"text","text":"A name is missing.\n```rust\nlet x = y;\n```"}],"stop_reason":"end_turn"}}),
            serde_json::json!({"type":"user","uuid":"u3","message":{"role":"user","content":"<command-name>/compact</command-name>\n<command-args>keep it short</command-args>"}}),
        ]);
        let parsed = parse_claude(&log, 0);
        assert_eq!(parsed.title.as_deref(), Some("Fix the build"));
        let summary: Vec<(AgentChatItemKind, &str)> = parsed
            .items
            .iter()
            .map(|item| {
                (
                    item.kind,
                    item.title.as_deref().unwrap_or(item.text.as_str()),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (AgentChatItemKind::User, "Why does the build fail?"),
                (AgentChatItemKind::Assistant, "Let me look."),
                (AgentChatItemKind::Tool, "Bash · cargo build"),
                (
                    AgentChatItemKind::Assistant,
                    "A name is missing.\n```rust\nlet x = y;\n```"
                ),
                (AgentChatItemKind::User, "/compact keep it short"),
            ]
        );
        assert_eq!(parsed.items[2].tool_state, Some(AgentToolState::Failed));
        assert_eq!(parsed.items[2].text, "error[E0425]");
        assert_eq!(
            parsed.state,
            AgentChatState::Working,
            "a prompt was sent last"
        );
    }

    #[test]
    fn a_claude_turn_ends_ready_or_interrupted() {
        let answered = lines(&[
            serde_json::json!({"type":"user","uuid":"u","message":{"content":"hi"}}),
            serde_json::json!({"type":"assistant","uuid":"a","message":{"content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn"}}),
        ]);
        assert_eq!(parse_claude(&answered, 0).state, AgentChatState::Ready);
        let interrupted = lines(&[
            serde_json::json!({"type":"assistant","uuid":"a","message":{"content":[{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"/x"}}],"stop_reason":"tool_use"}}),
            serde_json::json!({"type":"user","uuid":"i","message":{"content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}),
        ]);
        let parsed = parse_claude(&interrupted, 0);
        assert_eq!(parsed.state, AgentChatState::Ready);
        assert_eq!(parsed.items[0].tool_state, Some(AgentToolState::Failed));
        assert_eq!(parsed.items[1].kind, AgentChatItemKind::Notice);
    }

    #[test]
    fn a_codex_rollout_keeps_one_copy_of_each_prompt_and_pairs_tools() {
        let log = lines(&[
            serde_json::json!({"type":"session_meta","payload":{"cwd":"/r"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"task_started"}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","id":"m0","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions"}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["agents_md.instructions"]}}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","id":"m1","role":"user","content":[{"type":"input_text","text":"check now"}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]}}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"x1","content":[{"type":"text","text":"check now"}]}}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","id":"m2","role":"assistant","content":[{"type":"output_text","text":"Checking the release."}]}}),
            serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":[\"gh\",\"release\",\"view\"]}","call_id":"c1"}}),
            serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"v0.0.12 draft"}}),
            serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"text(await tools.exec_command({cmd:\"cargo test\"}))","call_id":"c2"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"turn_aborted"}}),
        ]);
        let parsed = parse_codex(&log, 0);
        let summary: Vec<(AgentChatItemKind, String)> = parsed
            .items
            .iter()
            .map(|item| {
                (
                    item.kind,
                    item.title.clone().unwrap_or_else(|| item.text.clone()),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (AgentChatItemKind::User, "check now".to_owned()),
                (
                    AgentChatItemKind::Assistant,
                    "Checking the release.".to_owned()
                ),
                (
                    AgentChatItemKind::Tool,
                    "exec_command · gh release view".to_owned()
                ),
                (AgentChatItemKind::Tool, "exec · cargo test".to_owned()),
                (AgentChatItemKind::Notice, "Interrupted.".to_owned()),
            ]
        );
        assert_eq!(parsed.items[2].tool_state, Some(AgentToolState::Succeeded));
        assert_eq!(parsed.items[3].tool_state, Some(AgentToolState::Failed));
        assert_eq!(parsed.state, AgentChatState::Ready);
    }

    #[test]
    fn pages_read_back_and_unchanged_logs_answer_nothing() {
        let mut builder = Builder::new();
        for index in 0..10 {
            builder.message(
                format!("m{index}"),
                AgentChatItemKind::User,
                &format!("{index}"),
            );
        }
        let mut parsed = builder.finish(None, AgentChatState::Ready);
        parsed.revision = "r1".to_owned();
        let request = |before: Option<&str>, since: Option<&str>| ChatRequest {
            before: before.map(str::to_owned),
            since_revision: since.map(str::to_owned),
            limit: 4,
        };
        let latest = page(&parsed, AgentProvider::Codex, &request(None, None));
        assert_eq!(
            latest.items.first().map(|item| item.id.as_str()),
            Some("m6")
        );
        assert!(latest.earlier);
        let older = page(&parsed, AgentProvider::Codex, &request(Some("m6"), None));
        assert_eq!(
            older
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["m2", "m3", "m4", "m5"]
        );
        let same = page(&parsed, AgentProvider::Codex, &request(None, Some("r1")));
        assert!(same.unchanged && same.items.is_empty());
    }

    #[test]
    fn an_answer_fits_one_frame() {
        let mut builder = Builder::new();
        for index in 0..100 {
            builder.message(
                format!("m{index}"),
                AgentChatItemKind::Assistant,
                &"x".repeat(9_000),
            );
        }
        let parsed = builder.finish(None, AgentChatState::Ready);
        assert!(parsed.items.iter().all(|item| item.truncated));
        let chat = page(
            &parsed,
            AgentProvider::ClaudeCode,
            &ChatRequest {
                before: None,
                since_revision: None,
                limit: 100,
            },
        );
        assert!(serde_json::to_vec(&chat).expect("encodes").len() <= MAX_AGENT_CHAT_RESPONSE_BYTES);
        assert!(chat.earlier && !chat.items.is_empty());
        assert_eq!(chat.items.last().map(|item| item.id.as_str()), Some("m99"));
    }

    #[test]
    fn claude_names_a_project_folder_with_dashes() {
        assert_eq!(
            claude_project_name(Path::new("/Users/me/.dot/my_app")),
            "-Users-me--dot-my-app"
        );
    }

    /// A `claude` started from a shell is found, with its folder, and its log is the newest one
    /// for that folder; a shell running nothing has no conversation.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn an_agent_under_the_shell_is_found_with_its_log() {
        use std::os::unix::process::CommandExt as _;
        let root = std::env::temp_dir().join(format!("agent-chat-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let work = root.join("work");
        let bin = root.join("bin");
        fs::create_dir_all(&work).expect("work");
        fs::create_dir_all(&bin).expect("bin");
        // A copy of this test binary named `claude`, running the stand-in below: a system
        // binary copied elsewhere is killed on macOS, and a script runs as its interpreter.
        let stand_in = bin.join("claude");
        fs::copy(std::env::current_exe().expect("test binary"), &stand_in).expect("copy");
        let work = fs::canonicalize(&work).expect("canonical");

        let mut shell = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "'{}' --exact agent_chat::tests::stand_in_agent --ignored --test-threads=1 >/dev/null; true",
                stand_in.display()
            ))
            .env("MULTIPLEX_AGENT_STAND_IN", "1")
            .process_group(0)
            .current_dir(&work)
            .spawn()
            .expect("shell");
        let homes = LogHomes {
            claude: Some(root.join("claude-home")),
            codex: None,
        };
        let request = ChatRequest {
            before: None,
            since_revision: None,
            limit: 20,
        };
        let shell_id = u64::from(shell.id());
        let mut chat = None;
        for _ in 0..100 {
            chat = read_with(shell_id, &request, &homes);
            if chat.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let chat = chat.expect("the agent is found");
        assert_eq!(chat.provider, AgentProvider::ClaudeCode);
        assert!(chat.items.is_empty(), "nothing said yet");

        let project = root
            .join("claude-home/projects")
            .join(claude_project_name(&work));
        fs::create_dir_all(&project).expect("project");
        fs::write(
            project.join("s.jsonl"),
            lines(&[serde_json::json!({"type":"user","uuid":"u","message":{"content":"hello"}})]),
        )
        .expect("log");
        let chat = read_with(shell_id, &request, &homes).expect("still found");
        assert_eq!(chat.items.len(), 1);
        assert_eq!(chat.items[0].text, "hello");

        // A newer log in the folder that another running Claude Code claims is not this
        // conversation. This test's own process stands in for that other agent.
        fs::write(
            project.join("mine.jsonl"),
            lines(&[serde_json::json!({"type":"user","uuid":"m","message":{"content":"mine"}})]),
        )
        .expect("own log");
        std::thread::sleep(Duration::from_millis(20));
        fs::write(project.join("other.jsonl"), "{}").expect("another session's log");
        fs::create_dir_all(root.join("claude-home/sessions")).expect("sessions");
        fs::write(
            root.join("claude-home/sessions")
                .join(format!("{}.json", std::process::id())),
            serde_json::json!({"pid": std::process::id(), "sessionId": "other"}).to_string(),
        )
        .expect("record");
        let chat = read_with(shell_id, &request, &homes).expect("still found");
        assert_eq!(chat.items[0].text, "mine");

        // SAFETY: signals the test's own process group, which holds only the shell and agent.
        unsafe { libc::kill(-(shell.id() as i32), libc::SIGKILL) };
        let _ = shell.wait();
        let _ = fs::remove_dir_all(&root);
        let idle = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 5")
            .spawn()
            .expect("idle shell");
        let idle_id = u64::from(idle.id());
        std::thread::sleep(Duration::from_millis(100));
        assert!(read_with(idle_id, &request, &homes).is_none());
        let mut idle = idle;
        let _ = idle.kill();
        let _ = idle.wait();
    }

    /// What the copied test binary runs as the agent: it waits to be stopped.
    #[test]
    #[ignore = "run by an_agent_under_the_shell_is_found_with_its_log"]
    fn stand_in_agent() {
        if std::env::var_os("MULTIPLEX_AGENT_STAND_IN").is_some() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }
}

#[cfg(test)]
mod probe {
    #[test]
    #[ignore = "reads a real agent's log: MULTIPLEX_AGENT_CHAT_PROBE=<shell pid>"]
    fn probe_a_real_agent() {
        let shell: u64 = std::env::var("MULTIPLEX_AGENT_CHAT_PROBE")
            .unwrap()
            .parse()
            .unwrap();
        let agent = super::find_agent(shell).expect("agent");
        println!("agent {:?}", agent);
        let homes = super::LogHomes::from_environment();
        let root = homes.claude.clone().unwrap();
        println!(
            "dir {:?}",
            super::claude_project_directory(&root, &agent.folder)
        );
        println!("claims {:?}", super::other_claims(&root, &agent));
        println!("log {:?}", super::claude_log(&root, &agent));
        let chat = super::read(
            shell,
            &super::ChatRequest {
                before: None,
                since_revision: None,
                limit: 12,
            },
        )
        .expect("an agent");
        println!(
            "{:?} {:?} {:?} items={} earlier={} truncated={} bytes={}",
            chat.provider,
            chat.title,
            chat.state,
            chat.items.len(),
            chat.earlier,
            chat.history_truncated,
            serde_json::to_vec(&chat).unwrap().len()
        );
        for item in &chat.items {
            println!(
                "- {:?} {:?} {:?} | {}",
                item.kind,
                item.tool_state,
                item.title,
                item.text
                    .chars()
                    .take(90)
                    .collect::<String>()
                    .replace('\n', " / ")
            );
        }
    }
}
