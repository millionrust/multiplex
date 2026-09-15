use std::path::PathBuf;

use termirust_update_trust::{
    FileTrustStateStore, RepositorySource, SystemClock, UpdateChannel, UpdateTargetName,
    VerificationRequest, verify_and_commit,
};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() != 2 {
        eprintln!("Usage: verify_update_repository <fixture-directory> <trust-state-file>");
        std::process::exit(2);
    }
    let fixture = PathBuf::from(&arguments[0]);
    let source = RepositorySource {
        trusted_root: fixture.join("metadata/1.root.json"),
        metadata_dir: fixture.join("metadata"),
    };
    let request = VerificationRequest::new(
        UpdateTargetName::parse("stable/macos/aarch64/termirust-1.2.3.tar.zst")
            .expect("constant target name is valid"),
        UpdateChannel::Stable,
        "macos",
        "aarch64",
        2,
        1,
    )
    .expect("constant compatibility request is valid");
    let state = FileTrustStateStore::new(PathBuf::from(&arguments[1]));
    match verify_and_commit(
        &source,
        &request,
        &state,
        &SystemClock,
        &CancellationToken::new(),
    )
    .await
    {
        Ok(target) => println!(
            "verified name={} version={} length={} sha256={}",
            target.name.as_str(),
            target.version,
            target.length,
            target.hashes["sha256"]
        ),
        Err(error) => {
            eprintln!("verification failed: {:?}", error.code);
            std::process::exit(1);
        }
    }
}
