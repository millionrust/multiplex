use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use termirust_ui_contract::lint::{
    verify_baseline, verify_zero_paths, verify_zero_surface, write_baseline,
};

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
    let mut paths = None;
    let mut surface = None;
    let mut zero_legacy = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--all-ui" | "--no-new-baseline" => {}
            "--paths" => {
                paths = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--paths requires a comma-separated value".to_string())?,
                );
            }
            "--surface" => {
                surface = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--surface requires a value".to_string())?,
                );
            }
            "--zero-legacy" => zero_legacy = true,
            "--write-baseline" => write = true,
            "--help" | "-h" => {
                println!(
                    "Usage: verify-design-tokens --all-ui --no-new-baseline\n\
                     Or: verify-design-tokens --paths src/ui/a.rs,src/ui/b.rs --zero-legacy\n\
                     Or: verify-design-tokens --surface vault-keys-snippets --zero-legacy\n\
                     Baseline maintenance requires TERMIRUST_MAINTENANCE_ALLOW_BASELINE_WRITE=1."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    let root = workspace_root();
    let baseline = root.join("design/legacy-visual-literals.toml");
    if paths.is_some() && surface.is_some() {
        return Err("--paths and --surface are mutually exclusive".to_string());
    }
    if let Some(surface) = surface {
        if write || !zero_legacy {
            return Err("--surface requires --zero-legacy and cannot write a baseline".to_string());
        }
        verify_zero_surface(&root, &surface).map_err(|error| error.to_string())?;
        println!("verified zero legacy visual literals for {surface}");
    } else if let Some(paths) = paths {
        if write || !zero_legacy {
            return Err("--paths requires --zero-legacy and cannot write a baseline".to_string());
        }
        let paths = paths.split(',').map(PathBuf::from).collect::<Vec<_>>();
        verify_zero_paths(&root, &paths).map_err(|error| error.to_string())?;
        println!("verified zero scoped legacy visual literals");
    } else if write {
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
