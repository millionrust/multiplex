use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use termirust_ui_contract::{generate_artifacts, load_manifest};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("generate-tokens: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut check = false;
    for argument in env::args().skip(1) {
        match argument.as_str() {
            "--check" => check = true,
            "--help" | "-h" => {
                println!("Usage: generate-tokens [--check]");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }

    let root = workspace_root();
    let manifest_path = root.join("design/tokens.toml");
    let rust_path = root.join("crates/termirust-ui-contract/src/generated.rs");
    let fixture_path = root.join("design/generated/tokens-contract.json");
    let (manifest, source) = load_manifest(&manifest_path).map_err(|error| error.to_string())?;
    let artifacts = generate_artifacts(&manifest, &source).map_err(|error| error.to_string())?;

    if check {
        check_file(&rust_path, &artifacts.rust)?;
        check_file(&fixture_path, &artifacts.platform_contract_json)?;
        println!(
            "design tokens are current (sha256:{})",
            artifacts.source_hash
        );
        return Ok(());
    }

    fs::create_dir_all(
        fixture_path
            .parent()
            .ok_or_else(|| "generated fixture has no parent".to_string())?,
    )
    .map_err(|error| format!("unable to create generated fixture directory: {error}"))?;
    write_atomic(&rust_path, &artifacts.rust)?;
    write_atomic(&fixture_path, &artifacts.platform_contract_json)?;
    println!("generated design tokens (sha256:{})", artifacts.source_hash);
    Ok(())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("ui contract crate must live in workspace/crates")
        .to_path_buf()
}

fn check_file(path: &Path, expected: &str) -> Result<(), String> {
    let actual = fs::read_to_string(path)
        .map_err(|error| format!("{} is missing or unreadable: {error}", path.display()))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{} is stale; run `cargo run -p termirust-ui-contract --bin generate-tokens`",
            path.display()
        ))
    }
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no UTF-8 file name", path.display()))?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    fs::write(&temporary, contents)
        .map_err(|error| format!("unable to write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("unable to atomically replace {}: {error}", path.display())
    })
}
