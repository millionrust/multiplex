use std::path::Path;

use crate::{ProjectRepository, ProjectSnapshot, SessionRepository, SessionSnapshot, StoreError};

/// One bounded, validated, non-mutating view of the local project and Session stores.
#[derive(Clone, Eq, PartialEq)]
pub struct FleetStoreSnapshot {
    pub projects: ProjectSnapshot,
    pub sessions: SessionSnapshot,
}

/// Reads existing metadata without creating a store, lock file, backup, or Session data directory.
pub fn load_fleet_read_only(root: impl AsRef<Path>) -> Result<FleetStoreSnapshot, StoreError> {
    let root = root.as_ref();
    let projects = ProjectRepository::load_existing_read_only(root)?;
    let sessions = SessionRepository::load_existing_read_only(root)?;
    Ok(FleetStoreSnapshot { projects, sessions })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;
    use crate::{ProjectRepository, SessionRepository};

    fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut result = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.is_file() {
                    result.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
        result
    }

    #[test]
    fn read_only_fleet_load_does_not_change_existing_store() {
        let fixture = TempDir::new().unwrap();
        let metadata = fixture.path().join("metadata");
        let session_data = fixture.path().join("sessions");
        ProjectRepository::open(&metadata).unwrap();
        SessionRepository::open(&metadata, &session_data).unwrap();
        let before = files(fixture.path());

        let snapshot = load_fleet_read_only(&metadata).unwrap();

        assert!(snapshot.projects.projects.is_empty());
        assert!(snapshot.sessions.sessions.is_empty());
        assert_eq!(files(fixture.path()), before);
    }

    #[test]
    fn read_only_fleet_load_does_not_initialize_missing_store() {
        let fixture = TempDir::new().unwrap();
        let missing = fixture.path().join("missing");

        assert!(load_fleet_read_only(&missing).is_err());
        assert!(!missing.exists());
        assert!(files(fixture.path()).is_empty());
    }
}
