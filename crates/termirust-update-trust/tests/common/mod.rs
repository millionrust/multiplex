#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use termirust_update_trust::{
    Clock, RepositorySource, TrustError, TrustErrorCode, TrustStateStore, TrustedState,
    UpdateChannel, UpdateTargetName, VerificationRequest,
};

pub const FIXED_NOW: i64 = 1_788_220_800;

pub fn fixture(name: &str) -> RepositorySource {
    let root = workspace_root()
        .join("tests/fixtures/update-tuf")
        .join(name);
    RepositorySource {
        trusted_root: root.join("metadata/1.root.json"),
        metadata_dir: root.join("metadata"),
    }
}

pub fn reference_request() -> VerificationRequest {
    VerificationRequest::new(
        UpdateTargetName::parse("stable/macos/aarch64/termirust-1.2.3.tar.zst").unwrap(),
        UpdateChannel::Stable,
        "macos",
        "aarch64",
        2,
        1,
    )
    .unwrap()
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

#[derive(Clone, Default)]
pub struct MemoryStateStore {
    state: Arc<Mutex<Option<TrustedState>>>,
    fail_commit: bool,
}

impl MemoryStateStore {
    pub fn failing() -> Self {
        Self {
            fail_commit: true,
            ..Self::default()
        }
    }

    pub fn current(&self) -> Option<TrustedState> {
        self.state.lock().unwrap().clone()
    }
}

impl TrustStateStore for MemoryStateStore {
    fn load(&self) -> Result<Option<TrustedState>, TrustError> {
        Ok(self.current())
    }

    fn commit(&self, state: &TrustedState) -> Result<(), TrustError> {
        if self.fail_commit {
            return Err(TrustError::new(TrustErrorCode::StateIo));
        }
        *self.state.lock().unwrap() = Some(state.clone());
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn unix_seconds(&self) -> i64 {
        self.0
    }
}

pub fn copy_fixture(source: &RepositorySource) -> (tempfile::TempDir, RepositorySource) {
    let temporary = tempfile::tempdir().unwrap();
    let metadata = temporary.path().join("metadata");
    std::fs::create_dir(&metadata).unwrap();
    for entry in std::fs::read_dir(&source.metadata_dir).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), metadata.join(entry.file_name())).unwrap();
    }
    (
        temporary,
        RepositorySource {
            trusted_root: metadata.join("1.root.json"),
            metadata_dir: metadata,
        },
    )
}
