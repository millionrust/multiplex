use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tough::editor::RepositoryEditor;
use tough::key_source::{KeySource, LocalKeySource};
use tough::schema::{Hashes, PathPattern, PathSet, Target};

const TARGET_NAME: &str = "stable/macos/aarch64/termirust-1.2.3.tar.zst";
const SYNTHETIC_TARGET: &[u8] = b"TERMIRUST SYNTHETIC UPDATE TARGET - NEVER EXECUTE\n";

#[tokio::test]
#[ignore = "writes synthetic fixtures only when TERMIRUST_REGENERATE_UPDATE_FIXTURES=1"]
async fn generate_update_fixtures() {
    assert_eq!(
        std::env::var("TERMIRUST_REGENERATE_UPDATE_FIXTURES").as_deref(),
        Ok("1"),
        "set TERMIRUST_REGENERATE_UPDATE_FIXTURES=1 explicitly"
    );
    generate().await.unwrap();
}

#[tokio::test]
#[ignore = "writes a synthetic fixture only when TERMIRUST_REGENERATE_UPDATE_FIXTURES=1"]
async fn generate_delegated_fixture() {
    assert_eq!(
        std::env::var("TERMIRUST_REGENERATE_UPDATE_FIXTURES").as_deref(),
        Ok("1"),
        "set TERMIRUST_REGENERATE_UPDATE_FIXTURES=1 explicitly"
    );
    let root = workspace_root();
    let fixture_root = root.join("tests/fixtures/update-tuf");
    let source = fixture_root.join("source");
    let output = fixture_root.join("valid-delegated/metadata");
    assert!(
        !output.exists(),
        "refusing to overwrite {}",
        output.display()
    );
    std::fs::create_dir_all(&output).unwrap();
    generate_delegated_repository(
        &source.join("bootstrap-root.json"),
        &source.join("SYNTHETIC-TEST-KEY.pem"),
        &output,
    )
    .await
    .unwrap();
}

async fn generate() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let fixture_root = root.join("tests/fixtures/update-tuf");
    let source = fixture_root.join("source");
    for version in [1_u64, 2] {
        let output = fixture_root.join(format!("valid-v{version}/metadata"));
        if output.exists() {
            return Err(format!("refusing to overwrite {}", output.display()).into());
        }
        std::fs::create_dir_all(&output)?;
        generate_repository(
            &source.join("bootstrap-root.json"),
            &source.join("SYNTHETIC-TEST-KEY.pem"),
            &output,
            version,
        )
        .await?;
    }
    Ok(())
}

async fn generate_repository(
    root: &Path,
    key: &Path,
    output: &Path,
    version: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let version = NonZeroU64::new(version).expect("fixture version is nonzero");
    let expires = "2099-01-01T00:00:00Z".parse::<Timestamp>()?;
    let mut editor = RepositoryEditor::new(root).await?;
    editor
        .targets_version(version)?
        .targets_expires(expires)?
        .snapshot_version(version)
        .snapshot_expires(expires)
        .timestamp_version(version)
        .timestamp_expires(expires)
        .add_target(TARGET_NAME, synthetic_target())?;
    let keys: &[Box<dyn KeySource>] = &[Box::new(LocalKeySource {
        path: key.to_path_buf(),
    })];
    editor.sign(keys).await?.write(output).await?;
    Ok(())
}

async fn generate_delegated_repository(
    root: &Path,
    key: &Path,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let version = NonZeroU64::new(1).expect("fixture version is nonzero");
    let expires = "2099-01-01T00:00:00Z".parse::<Timestamp>()?;
    let keys: &[Box<dyn KeySource>] = &[Box::new(LocalKeySource {
        path: key.to_path_buf(),
    })];
    let mut editor = RepositoryEditor::new(root).await?;
    editor
        .targets_version(version)?
        .targets_expires(expires)?
        .snapshot_version(version)
        .snapshot_expires(expires)
        .timestamp_version(version)
        .timestamp_expires(expires)
        .delegate_role(
            "stable-macos-aarch64",
            keys,
            PathSet::Paths(vec![PathPattern::new("stable/macos/aarch64/*")?]),
            true,
            version,
            expires,
            version,
        )
        .await?
        .sign_targets_editor(keys)
        .await?
        .change_delegated_targets("stable-macos-aarch64")?
        .targets_version(version)?
        .targets_expires(expires)?
        .add_target(TARGET_NAME, synthetic_target())?
        .sign_targets_editor(keys)
        .await?
        .change_delegated_targets("targets")?
        .targets_version(version)?
        .targets_expires(expires)?;
    editor.sign(keys).await?.write(output).await?;
    Ok(())
}

fn synthetic_target() -> Target {
    Target {
        length: SYNTHETIC_TARGET.len() as u64,
        hashes: Hashes {
            sha256: Sha256::digest(SYNTHETIC_TARGET).to_vec().into(),
            _extra: HashMap::new(),
        },
        custom: HashMap::from([(
            "termirust".to_string(),
            json!({
                "schema_version": 1,
                "version": "1.2.3",
                "channel": "stable",
                "platform": "macos",
                "arch": "aarch64",
                "store_range": { "min": 1, "max": 3 },
                "protocol_range": { "min": 1, "max": 2 },
                "rollout": 100,
                "emergency_rollback": false
            }),
        )]),
        _extra: HashMap::new(),
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate is nested directly under workspace/crates")
        .to_path_buf()
}
