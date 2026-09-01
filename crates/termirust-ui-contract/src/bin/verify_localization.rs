use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use termirust_ui_contract::localization_lint::{
    verify_copy_baseline, verify_zero_copy_paths, verify_zero_copy_surface, write_copy_baseline,
};
use termirust_ui_contract::{load_catalog, load_message_schema, validate_catalog_set};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("verify-localization: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut write = false;
    let mut locales = None;
    let mut paths = None;
    let mut surface = None;
    let mut zero_legacy = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--locales" => {
                locales = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--locales requires a value".to_string())?,
                );
            }
            "--no-new-baseline" => {}
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
                    "Usage: verify-localization --locales en-US,en-XA,ar-XB --no-new-baseline\n\
                     Baseline maintenance requires TERMIRUST_MAINTENANCE_ALLOW_COPY_BASELINE_WRITE=1."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    if locales.as_deref() != Some("en-US,en-XA,ar-XB") {
        return Err("--locales must be exactly en-US,en-XA,ar-XB".to_string());
    }

    let root = workspace_root();
    let (schema, _) = load_message_schema(&root.join("locales/schema.toml"))
        .map_err(|error| error.to_string())?;
    let english =
        load_catalog(&root.join("locales/en-US.ftl")).map_err(|error| error.to_string())?;
    let en_xa = load_catalog(&root.join("locales/en-XA.ftl")).map_err(|error| error.to_string())?;
    let ar_xb = load_catalog(&root.join("locales/ar-XB.ftl")).map_err(|error| error.to_string())?;
    validate_catalog_set(&schema, &english, &en_xa, &ar_xb).map_err(|error| error.to_string())?;

    let baseline = root.join("design/legacy-user-copy.toml");
    if paths.is_some() && surface.is_some() {
        return Err("--paths and --surface are mutually exclusive".to_string());
    }
    let count = if let Some(surface) = surface {
        if write || !zero_legacy {
            return Err("--surface requires --zero-legacy and cannot write a baseline".to_string());
        }
        verify_zero_copy_surface(&root, &surface).map_err(|error| error.to_string())?
    } else if let Some(paths) = paths {
        if write || !zero_legacy {
            return Err("--paths requires --zero-legacy and cannot write a baseline".to_string());
        }
        let paths = paths.split(',').map(PathBuf::from).collect::<Vec<_>>();
        verify_zero_copy_paths(&root, &paths).map_err(|error| error.to_string())?
    } else if write {
        if env::var("TERMIRUST_MAINTENANCE_ALLOW_COPY_BASELINE_WRITE").as_deref() != Ok("1") {
            return Err(
                "copy baseline writes are maintenance-only; normal checks may not add exceptions"
                    .to_string(),
            );
        }
        write_copy_baseline(&root, &baseline).map_err(|error| error.to_string())?
    } else {
        verify_copy_baseline(&root, &baseline).map_err(|error| error.to_string())?
    };
    println!("verified all three catalogs and {count} immutable legacy copy fingerprints");
    Ok(())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("ui contract crate must live in workspace/crates")
        .to_path_buf()
}
