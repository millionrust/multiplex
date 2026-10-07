//! Sessions `multiplex-cli shell` started: a person's own shell, run by a Session Host so paired
//! devices can reach it, recorded under the configuration root beside the durable sessions.
//!
//! They are not durable sessions in the Session library: nothing is kept about them once the shell
//! exits, and they carry no project or preset, the way a tmux session a person started carries
//! none. The launcher writes one record per session; the Controller listener lists the ones whose
//! Host is still serving.

use std::fs;
use std::path::{Path, PathBuf};

use multiplex_domain::{HostLifecycle, HostedSessionId, OccupantGeneration};
use serde::{Deserialize, Serialize};

use crate::lease::read_host_metadata;

/// The folder, beside the durable-session data folder, that holds one folder per session.
pub const CONSOLE_SESSIONS_DIR: &str = "console-sessions";
/// The record the launcher leaves in each session's folder.
pub const CONSOLE_SESSION_RECORD: &str = "console-session.json";
/// More than anyone keeps open at once; a folder with more is not listed past this.
const MAX_LISTED: usize = 256;
/// A record is a few hundred bytes; anything much larger was not written by the launcher.
const MAX_RECORD_BYTES: u64 = 16 * 1024;

/// What paired devices are told about a session the launcher started.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConsoleSessionRecord {
    pub schema_version: u16,
    pub session_id: HostedSessionId,
    /// The shell, as a person would name it: `pwsh`, `cmd`, `zsh`.
    pub program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_override: Option<String>,
    pub working_directory: PathBuf,
    pub started_at: u64,
}

impl ConsoleSessionRecord {
    pub const SCHEMA_VERSION: u16 = 1;

    /// "pwsh in projects", the way a tab would be named.
    pub fn title(&self) -> String {
        if let Some(title) = self
            .title_override
            .as_ref()
            .and_then(|title| multiplex_domain::SessionTitle::new(title).ok())
        {
            return title.as_str().to_owned();
        }
        match self
            .working_directory
            .file_name()
            .and_then(|name| name.to_str())
        {
            Some(folder) => format!("{} in {folder}", self.program),
            None => self.program.clone(),
        }
    }
}

/// A launcher session whose Host is serving, with the generation commands to it must present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveConsoleSession {
    pub record: ConsoleSessionRecord,
    pub generation: OccupantGeneration,
}

/// Where the launcher's sessions live, given the durable-session data folder beside it.
pub fn console_sessions_root(session_data_root: &Path) -> PathBuf {
    session_data_root
        .parent()
        .map(|parent| parent.join(CONSOLE_SESSIONS_DIR))
        .unwrap_or_else(|| PathBuf::from(CONSOLE_SESSIONS_DIR))
}

pub fn write_console_session(
    session_dir: &Path,
    record: &ConsoleSessionRecord,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    fs::write(session_dir.join(CONSOLE_SESSION_RECORD), bytes)
}

/// Rename only the record belonging to this session, preserving launcher metadata.
pub fn rename_console_session(
    session_dir: &Path,
    session_id: HostedSessionId,
    title: &str,
) -> std::io::Result<()> {
    use crate::AtomicWriter as _;
    let title = multiplex_domain::SessionTitle::new(title).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid session title")
    })?;
    let mut record = read_console_session(session_dir)
        .filter(|record| record.session_id == session_id)
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "session record unavailable")
        })?;
    record.title_override = Some(title.as_str().to_owned());
    let bytes = serde_json::to_vec_pretty(&record).map_err(std::io::Error::other)?;
    crate::SystemAtomicWriter.write(&session_dir.join(CONSOLE_SESSION_RECORD), &bytes)?;
    Ok(())
}

/// The record in `session_dir`, when it is one the launcher wrote for that folder's session.
pub fn read_console_session(session_dir: &Path) -> Option<ConsoleSessionRecord> {
    let path = session_dir.join(CONSOLE_SESSION_RECORD);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
        return None;
    }
    let record: ConsoleSessionRecord = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    let folder_is_its_own = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == record.session_id.to_string());
    (record.schema_version == ConsoleSessionRecord::SCHEMA_VERSION && folder_is_its_own)
        .then_some(record)
}

/// The generation of a launcher session whose Host is serving, or `None`.
pub fn console_session_generation(
    root: &Path,
    runtime_parent: &Path,
    session_id: HostedSessionId,
) -> Option<OccupantGeneration> {
    live(root, runtime_parent, &root.join(session_id.to_string()))
        .filter(|session| session.record.session_id == session_id)
        .map(|session| session.generation)
}

/// Every launcher session whose Host is serving, newest first.
pub fn live_console_sessions(root: &Path, runtime_parent: &Path) -> Vec<LiveConsoleSession> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut sessions: Vec<LiveConsoleSession> = entries
        .filter_map(Result::ok)
        .take(MAX_LISTED)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| live(root, runtime_parent, &entry.path()))
        .collect();
    sessions.sort_by(|left, right| {
        right
            .record
            .started_at
            .cmp(&left.record.started_at)
            .then_with(|| {
                left.record
                    .session_id
                    .to_string()
                    .cmp(&right.record.session_id.to_string())
            })
    });
    sessions
}

/// A session is live while its Host says it is serving and its endpoint is still there: a Host
/// that exited removes its endpoint and records the exit.
fn live(root: &Path, runtime_parent: &Path, session_dir: &Path) -> Option<LiveConsoleSession> {
    if session_dir.parent() != Some(root) {
        return None;
    }
    let record = read_console_session(session_dir)?;
    let metadata = read_host_metadata(session_dir).ok()?;
    if metadata.session_id != record.session_id
        || !matches!(
            metadata.lifecycle,
            HostLifecycle::Starting | HostLifecycle::Ready | HostLifecycle::Stopping
        )
    {
        return None;
    }
    let endpoint = runtime_parent
        .join(record.session_id.to_string())
        .join(&metadata.endpoint_name);
    fs::symlink_metadata(endpoint).ok()?;
    Some(LiveConsoleSession {
        record,
        generation: metadata.activity.generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(session_id: HostedSessionId) -> ConsoleSessionRecord {
        ConsoleSessionRecord {
            schema_version: ConsoleSessionRecord::SCHEMA_VERSION,
            session_id,
            program: "pwsh".to_owned(),
            title_override: None,
            working_directory: PathBuf::from("projects").join("multiplex"),
            started_at: 42,
        }
    }

    #[test]
    fn a_session_is_titled_by_its_shell_and_folder() {
        assert_eq!(record(HostedSessionId::new()).title(), "pwsh in multiplex");
    }

    #[test]
    fn a_renamed_console_title_survives_reload_and_preserves_identity() {
        let root = tempfile::tempdir().unwrap();
        let id = HostedSessionId::new();
        let dir = root.path().join(id.to_string());
        fs::create_dir(&dir).unwrap();
        let original = record(id);
        write_console_session(&dir, &original).unwrap();
        rename_console_session(&dir, id, "Release monitor").unwrap();
        let renamed = read_console_session(&dir).unwrap();
        assert_eq!(renamed.title(), "Release monitor");
        assert_eq!(renamed.session_id, id);
        assert_eq!(renamed.working_directory, original.working_directory);
        assert!(rename_console_session(&dir, HostedSessionId::new(), "Wrong target").is_err());
        assert!(rename_console_session(&dir, id, " ").is_err());
        assert_eq!(read_console_session(&dir).unwrap(), renamed);
    }

    #[test]
    fn the_root_sits_beside_the_durable_session_data() {
        assert_eq!(
            console_sessions_root(&PathBuf::from("config").join("durable-sessions")),
            PathBuf::from("config").join(CONSOLE_SESSIONS_DIR)
        );
    }

    #[test]
    fn a_record_is_read_back_only_from_its_own_folder() {
        let temp = tempfile::tempdir().unwrap();
        let session_id = HostedSessionId::new();
        let own = temp.path().join(session_id.to_string());
        fs::create_dir_all(&own).unwrap();
        write_console_session(&own, &record(session_id)).unwrap();
        assert_eq!(read_console_session(&own), Some(record(session_id)));

        let elsewhere = temp.path().join(HostedSessionId::new().to_string());
        fs::create_dir_all(&elsewhere).unwrap();
        write_console_session(&elsewhere, &record(session_id)).unwrap();
        assert_eq!(read_console_session(&elsewhere), None);
    }

    #[test]
    fn a_session_without_a_serving_host_is_not_listed() {
        let temp = tempfile::tempdir().unwrap();
        let session_id = HostedSessionId::new();
        let dir = temp.path().join(session_id.to_string());
        fs::create_dir_all(&dir).unwrap();
        write_console_session(&dir, &record(session_id)).unwrap();
        assert!(live_console_sessions(temp.path(), &temp.path().join("runtime")).is_empty());
        assert_eq!(
            console_session_generation(temp.path(), &temp.path().join("runtime"), session_id),
            None
        );
    }
}
