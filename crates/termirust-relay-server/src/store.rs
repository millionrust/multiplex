use crate::RelayServerError;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use termirust_relay_protocol::{
    MAX_REGISTERED_ROUTES, RelayDiagnosticCode, RelayRouteRegistration,
};

const STATE_FORMAT: &str = "relay-state-v1";
const STATE_FORMAT_VERSION: u32 = 1;
const MAX_STATE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RelayStoreFault {
    #[default]
    None,
    BeforeStageWrite,
    AfterStageWrite,
    AfterStageSync,
    AfterRename,
    AfterParentSync,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedRelayState {
    format: String,
    format_version: u32,
    routes: Vec<RelayRouteRegistration>,
}

pub struct RelayMetadataStore {
    path: PathBuf,
    stage_path: PathBuf,
    _lock: File,
}

impl std::fmt::Debug for RelayMetadataStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayMetadataStore")
            .field("state", &"[PROTECTED_PATH]")
            .finish()
    }
}

impl RelayMetadataStore {
    pub fn acquire(path: impl Into<PathBuf>) -> Result<Self, RelayServerError> {
        let path = path.into();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::InvalidConfig))?;
        fs::create_dir_all(parent).map_err(map_state_io)?;
        set_private_directory_permissions(parent)?;

        let lock_path = append_suffix(&path, ".lock");
        let stage_path = append_suffix(&path, ".stage");
        let lock = create_private_lock(&lock_path).map_err(map_state_io)?;
        lock.try_lock_exclusive()
            .map_err(|_| RelayServerError::new(RelayDiagnosticCode::StateLocked))?;
        Ok(Self {
            path,
            stage_path,
            _lock: lock,
        })
    }

    pub fn load(&self) -> Result<Vec<RelayRouteRegistration>, RelayServerError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        ensure_private_file(&self.path)?;
        let file = File::open(&self.path).map_err(map_state_io)?;
        if file.metadata().map_err(map_state_io)?.len() > MAX_STATE_BYTES {
            return Err(RelayServerError::new(RelayDiagnosticCode::StateCorrupt));
        }
        let mut bytes = Vec::new();
        file.take(MAX_STATE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(map_state_io)?;
        let state: PersistedRelayState = serde_json::from_slice(&bytes).map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::StateCorrupt, error)
        })?;
        if state.format != STATE_FORMAT || state.format_version != STATE_FORMAT_VERSION {
            return Err(RelayServerError::new(
                RelayDiagnosticCode::StateVersionUnsupported,
            ));
        }
        validate_routes(&state.routes)?;
        Ok(state.routes)
    }

    pub fn commit(&self, routes: &[RelayRouteRegistration]) -> Result<(), RelayServerError> {
        self.commit_with_fault(routes, RelayStoreFault::None)
    }

    pub fn commit_with_fault(
        &self,
        routes: &[RelayRouteRegistration],
        fault: RelayStoreFault,
    ) -> Result<(), RelayServerError> {
        validate_routes(routes)?;
        let state = PersistedRelayState {
            format: STATE_FORMAT.to_owned(),
            format_version: STATE_FORMAT_VERSION,
            routes: routes.to_vec(),
        };
        let bytes = serde_json::to_vec_pretty(&state).map_err(|error| {
            RelayServerError::with_source(RelayDiagnosticCode::StateWriteFailed, error)
        })?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err(RelayServerError::new(RelayDiagnosticCode::StateWriteFailed));
        }

        let mut stage = create_private_truncate(&self.stage_path).map_err(map_state_io)?;
        if fault == RelayStoreFault::BeforeStageWrite {
            return Err(RelayServerError::new(RelayDiagnosticCode::StateWriteFailed));
        }
        stage.write_all(&bytes).map_err(map_state_io)?;
        if fault == RelayStoreFault::AfterStageWrite {
            return Err(RelayServerError::new(RelayDiagnosticCode::StateWriteFailed));
        }
        stage.sync_all().map_err(map_state_io)?;
        if fault == RelayStoreFault::AfterStageSync {
            return Err(RelayServerError::new(RelayDiagnosticCode::StateWriteFailed));
        }
        fs::rename(&self.stage_path, &self.path).map_err(map_state_io)?;
        if fault == RelayStoreFault::AfterRename {
            return Err(RelayServerError::new(RelayDiagnosticCode::StateWriteFailed));
        }
        sync_parent(&self.path)?;
        if fault == RelayStoreFault::AfterParentSync {
            return Err(RelayServerError::new(RelayDiagnosticCode::StateWriteFailed));
        }
        Ok(())
    }

    pub fn state_path(&self) -> &Path {
        &self.path
    }
}

fn validate_routes(routes: &[RelayRouteRegistration]) -> Result<(), RelayServerError> {
    if routes.len() > MAX_REGISTERED_ROUTES {
        return Err(RelayServerError::new(RelayDiagnosticCode::InvalidConfig));
    }
    let mut ids = BTreeSet::new();
    for route in routes {
        route.validate()?;
        if !ids.insert(route.route_id) {
            return Err(RelayServerError::new(RelayDiagnosticCode::StateCorrupt));
        }
    }
    Ok(())
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn map_state_io(error: std::io::Error) -> RelayServerError {
    let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
        RelayDiagnosticCode::StatePermissionDenied
    } else {
        RelayDiagnosticCode::StateWriteFailed
    };
    RelayServerError::with_source(code, error)
}

fn sync_parent(path: &Path) -> Result<(), RelayServerError> {
    let parent = path
        .parent()
        .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::InvalidConfig))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(map_state_io)
}

#[cfg(unix)]
fn create_private_lock(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_lock(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

#[cfg(unix)]
fn create_private_truncate(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_truncate(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), RelayServerError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(map_state_io)
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), RelayServerError> {
    Ok(())
}

#[cfg(unix)]
fn ensure_private_file(path: &Path) -> Result<(), RelayServerError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(map_state_io)?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(RelayServerError::new(
            RelayDiagnosticCode::StatePermissionDenied,
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_file(_path: &Path) -> Result<(), RelayServerError> {
    Ok(())
}
