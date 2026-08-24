use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use termirust_ui_contract::lint::{verify_baseline, write_baseline};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("verify-design-tokens: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut write = false;
    for argument in env::args().skip(1) {
        match argument.as_str() {
            "--all-ui" | "--no-new-baseline" => {}
            "--write-baseline" => write = true,
            "--help" | "-h" => {
                println!(
                    "Usage: verify-design-tokens --all-ui --no-new-baseline\n\
                     Baseline maintenance requires TERMIRUST_MAINTENANCE_ALLOW_BASELINE_WRITE=1."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    let root = workspace_root();
    let baseline = root.join("design/legacy-visual-literals.toml");
    if write {
        if env::var("TERMIRUST_MAINTENANCE_ALLOW_BASELINE_WRITE").as_deref() != Ok("1") {
            return Err(
                "baseline writes are maintenance-only; normal checks may not add exceptions"
                    .to_string(),
            );
        }
        let count = write_baseline(&root, &baseline).map_err(|error| error.to_string())?;
        println!("wrote {count} immutable legacy visual literal fingerprints");
    } else {
        let count = verify_baseline(&root, &baseline).map_err(|error| error.to_string())?;
        println!("verified {count} legacy visual literal fingerprints; no new baseline entries");
    }
    Ok(())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("ui contract crate must live in workspace/crates")
        .to_path_buf()
}
