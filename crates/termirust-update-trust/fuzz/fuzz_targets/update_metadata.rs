#![no_main]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use libfuzzer_sys::fuzz_target;
use termirust_update_trust::{
    Clock, MAX_ROLE_BYTES, RepositorySource, TrustError, TrustStateStore, TrustedState,
    UpdateChannel, UpdateTargetName, VerificationRequest, verify_and_commit,
};
use tokio_util::sync::CancellationToken;

const TARGET_NAME: &str = "stable/macos/aarch64/termirust-1.2.3.tar.zst";

struct NoopState;

impl TrustStateStore for NoopState {
    fn load(&self) -> Result<Option<TrustedState>, TrustError> {
        Ok(None)
    }

    fn commit(&self, _: &TrustedState) -> Result<(), TrustError> {
        Ok(())
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn unix_seconds(&self) -> i64 {
        1_788_220_800
    }
}

struct Harness {
    _temporary: tempfile::TempDir,
    source: RepositorySource,
    runtime: tokio::runtime::Runtime,
}

impl Harness {
    fn new() -> Self {
        let fixture = workspace_root().join("tests/fixtures/update-tuf/valid-v1/metadata");
        let temporary = tempfile::tempdir().expect("create fuzz metadata directory");
        for entry in fs::read_dir(&fixture).expect("read checked-in fixture") {
            let entry = entry.expect("read fixture entry");
            fs::copy(entry.path(), temporary.path().join(entry.file_name()))
                .expect("copy fixture entry");
        }
        let source = RepositorySource {
            trusted_root: temporary.path().join("1.root.json"),
            metadata_dir: temporary.path().to_path_buf(),
        };
        Self {
            _temporary: temporary,
            source,
            runtime: tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("create fuzz runtime"),
        }
    }

    fn run(&mut self, data: &[u8]) {
        let bounded = &data[..data.len().min(MAX_ROLE_BYTES as usize + 1)];
        fs::write(self.source.metadata_dir.join("delegated.json"), bounded)
            .expect("write fuzz case");
        let request = VerificationRequest::new(
            UpdateTargetName::parse(TARGET_NAME).expect("valid target name"),
            UpdateChannel::Stable,
            "macos",
            "aarch64",
            2,
            1,
        )
        .expect("valid request");
        let _ = self.runtime.block_on(verify_and_commit(
            &self.source,
            &request,
            &NoopState,
            &FixedClock,
            &CancellationToken::new(),
        ));
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("fuzz crate is nested under workspace/crates/update-trust")
        .to_path_buf()
}

static HARNESS: OnceLock<Mutex<Harness>> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    HARNESS
        .get_or_init(|| Mutex::new(Harness::new()))
        .lock()
        .expect("fuzz harness lock")
        .run(data);
});
