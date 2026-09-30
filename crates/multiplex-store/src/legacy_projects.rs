//! Reading a store written while Projects existed.
//!
//! Up to 0.0.5 a Project sat between a session and the folder it ran in: `projects.json` held the
//! Projects together with the groups and worktrees under them, and each session named its Project
//! rather than its folder. Projects are gone, so this carries such a store forward once: the
//! groups and worktrees move to `library.json`, and each session is given the folder its Project
//! stood for. `projects.json` is then renamed rather than deleted, so nothing a person had is
//! lost if the move needs checking later.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::StoreError;
use crate::atomic::AtomicWriter;
use crate::library::read_regular_bounded;

pub(crate) const LEGACY_PROJECTS_FILE: &str = "projects.json";
const LEGACY_PROJECTS_BACKUP_FILE: &str = "projects.last-good.json";
const MIGRATED_PROJECTS_FILE: &str = "projects.migrated.json";
const MIGRATED_PROJECTS_BACKUP_FILE: &str = "projects.last-good.migrated.json";
const SESSIONS_FILE: &str = "sessions.json";
const SESSIONS_BACKUP_FILE: &str = "sessions.last-good.json";
const MAX_LEGACY_BYTES: u64 = 32 * 1024 * 1024;

/// The legacy projects document, from the primary file or its last-good copy.
pub(crate) fn read_legacy(root: &Path) -> Option<Value> {
    [LEGACY_PROJECTS_FILE, LEGACY_PROJECTS_BACKUP_FILE]
        .into_iter()
        .filter(|name| root.join(name).exists())
        .find_map(|name| {
            let bytes = read_regular_bounded(&root.join(name), name, MAX_LEGACY_BYTES).ok()?;
            serde_json::from_slice::<Value>(&bytes)
                .ok()
                .filter(Value::is_object)
        })
}

/// What `library.json` holds for a legacy document: its groups and worktrees, with every
/// reference to a Project taken out.
pub(crate) fn library_from_legacy(legacy: &Value) -> Value {
    let field = |name: &str| {
        legacy
            .get(name)
            .cloned()
            .unwrap_or(Value::Array(Vec::new()))
    };
    let mut groups = field("groups");
    strip_each(&mut groups, &[], &["project_id"]);
    let mut intents = field("worktree_intents");
    strip_each(
        &mut intents,
        &["plan"],
        &["source_project_id", "child_project_id"],
    );
    let mut worktrees = field("worktrees");
    strip_each(
        &mut worktrees,
        &[],
        &["source_project_id", "child_project_id"],
    );
    let mut library = Map::new();
    library.insert(
        "revision".into(),
        legacy.get("revision").cloned().unwrap_or(Value::from(0)),
    );
    library.insert("groups".into(), groups);
    library.insert("worktree_intents".into(), intents);
    library.insert("worktrees".into(), worktrees);
    Value::Object(library)
}

/// Each Project's folder, by the Project's id.
pub(crate) fn legacy_folders(legacy: &Value) -> HashMap<String, Value> {
    legacy
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|project| {
            Some((
                project.get("id")?.as_str()?.to_owned(),
                project.get("canonical_root")?.clone(),
            ))
        })
        .collect()
}

/// Gives each session in a sessions document the folder its Project stood for.
///
/// Returns whether anything changed. A session naming a Project that no longer existed was
/// already orphaned — nothing could open it — and has no folder to be given, so it is dropped.
pub(crate) fn upgrade_sessions(document: &mut Value, folders: &HashMap<String, Value>) -> bool {
    let Some(sessions) = document.get_mut("sessions").and_then(Value::as_array_mut) else {
        return false;
    };
    let before = sessions.len();
    let mut changed = false;
    sessions.retain_mut(|session| {
        let Some(record) = session.as_object_mut() else {
            return true;
        };
        let Some(project) = record.remove("project_id") else {
            return true;
        };
        changed = true;
        if record.contains_key("folder") {
            return true;
        }
        match project.as_str().and_then(|id| folders.get(id)) {
            Some(folder) => {
                record.insert("folder".into(), folder.clone());
                true
            }
            None => false,
        }
    });
    changed || sessions.len() != before
}

/// Carries a legacy store forward. Runs under the metadata lock, and does nothing once
/// `projects.json` is gone.
pub(crate) fn migrate(root: &Path, writer: &dyn AtomicWriter) -> Result<(), StoreError> {
    if !root.join(LEGACY_PROJECTS_FILE).exists() && !root.join(LEGACY_PROJECTS_BACKUP_FILE).exists()
    {
        return Ok(());
    }
    let Some(legacy) = read_legacy(root) else {
        // Unreadable either way: leave it where it is for recovery to look at, and start the
        // library empty rather than refuse to open.
        return Ok(());
    };
    let library_path = root.join(crate::library::LIBRARY_FILE);
    if !library_path.exists() {
        write_json(writer, &library_path, &library_from_legacy(&legacy))?;
    }
    let folders = legacy_folders(&legacy);
    for name in [SESSIONS_FILE, SESSIONS_BACKUP_FILE] {
        let path = root.join(name);
        if !path.exists() {
            continue;
        }
        let Ok(bytes) = read_regular_bounded(&path, name, MAX_LEGACY_BYTES) else {
            continue;
        };
        let Ok(mut document) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if upgrade_sessions(&mut document, &folders) {
            write_json(writer, &path, &document)?;
        }
    }
    for (from, to) in [
        (LEGACY_PROJECTS_FILE, MIGRATED_PROJECTS_FILE),
        (LEGACY_PROJECTS_BACKUP_FILE, MIGRATED_PROJECTS_BACKUP_FILE),
    ] {
        if root.join(from).exists() {
            fs::rename(root.join(from), root.join(to)).map_err(|error| StoreError::Io {
                operation: "retire projects",
                kind: error.kind(),
            })?;
        }
    }
    Ok(())
}

fn write_json(writer: &dyn AtomicWriter, path: &Path, value: &Value) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| StoreError::Corrupt {
        name: "serialization",
    })?;
    writer
        .write(path, &bytes)
        .map(|_| ())
        .map_err(|error| StoreError::Io {
            operation: "migrate projects",
            kind: error.kind(),
        })
}

/// Removes `keys` from every object in `items`, or from the object at `within` inside each.
fn strip_each(items: &mut Value, within: &[&str], keys: &[&str]) {
    let Some(items) = items.as_array_mut() else {
        return;
    };
    for item in items {
        let mut target = Some(item);
        for step in within {
            target = target.and_then(|value| value.get_mut(*step));
        }
        if let Some(object) = target.and_then(Value::as_object_mut) {
            for key in keys {
                object.remove(*key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn groups_and_worktrees_keep_everything_but_their_project() {
        let legacy = json!({
            "revision": 9,
            "projects": [{"id": "p1", "canonical_root": {"path": "/work"}}],
            "groups": [{"id": "g1", "project_id": "p1", "name": "Review"}],
            "worktree_intents": [{"plan": {"id": "w1", "source_project_id": "p1", "child_project_id": "p2"}, "state": "planned"}],
            "worktrees": [{"id": "w2", "source_project_id": "p1", "child_project_id": "p3", "branch": "b"}]
        });
        let library = library_from_legacy(&legacy);
        assert_eq!(library["revision"], 9);
        assert_eq!(library["groups"], json!([{"id": "g1", "name": "Review"}]));
        assert_eq!(
            library["worktree_intents"],
            json!([{"plan": {"id": "w1"}, "state": "planned"}])
        );
        assert_eq!(library["worktrees"], json!([{"id": "w2", "branch": "b"}]));
        assert!(library.get("projects").is_none());
    }

    #[test]
    fn a_session_is_given_its_projects_folder_and_an_orphan_is_dropped() {
        let folders = HashMap::from([("p1".to_owned(), json!({"path": "/work"}))]);
        let mut sessions = json!({
            "revision": 3,
            "sessions": [
                {"id": "s1", "project_id": "p1", "title": "Build"},
                {"id": "s2", "project_id": "gone", "title": "Orphan"},
                {"id": "s3", "folder": {"path": "/already"}, "title": "New"}
            ]
        });
        assert!(upgrade_sessions(&mut sessions, &folders));
        assert_eq!(
            sessions["sessions"],
            json!([
                {"id": "s1", "folder": {"path": "/work"}, "title": "Build"},
                {"id": "s3", "folder": {"path": "/already"}, "title": "New"}
            ])
        );
        assert!(!upgrade_sessions(&mut sessions, &folders));
    }
}
