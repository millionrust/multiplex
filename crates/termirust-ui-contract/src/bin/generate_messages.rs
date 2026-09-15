use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use termirust_ui_contract::{generate_message_artifacts, load_catalog, load_message_schema};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("generate-messages: {error}");
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
                println!("Usage: generate-messages [--check]");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }

    let root = workspace_root();
    let schema_path = root.join("locales/schema.toml");
    let english_path = root.join("locales/en-US.ftl");
    let rust_path = root.join("crates/termirust-ui-contract/src/generated_messages.rs");
    let en_xa_path = root.join("locales/en-XA.ftl");
    let ar_xb_path = root.join("locales/ar-XB.ftl");
    let (schema, schema_source) =
        load_message_schema(&schema_path).map_err(|error| error.to_string())?;
    let english_source = load_catalog(&english_path).map_err(|error| error.to_string())?;
    let artifacts = generate_message_artifacts(&schema, &schema_source, &english_source)
        .map_err(|error| error.to_string())?;
    let formatted_rust = format_rust(&artifacts.rust)?;

    let outputs = [
        (&rust_path, formatted_rust.as_str()),
        (&en_xa_path, artifacts.en_xa.as_str()),
        (&ar_xb_path, artifacts.ar_xb.as_str()),
    ];
    if check {
        for (path, expected) in outputs {
            check_file(path, expected)?;
        }
        println!(
            "localization messages are current (sha256:{})",
            artifacts.source_hash
        );
    } else {
        for (path, contents) in outputs {
            write_atomic(path, contents)?;
        }
        println!(
            "generated localization messages (sha256:{})",
            artifacts.source_hash
        );
    }
    Ok(())
}

fn format_rust(source: &str) -> Result<String, String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("unable to start rustfmt for generated messages: {error}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "rustfmt stdin was unavailable".to_string())?
        .write_all(source.as_bytes())
        .map_err(|error| format!("unable to send generated messages to rustfmt: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("unable to wait for rustfmt: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "rustfmt rejected generated messages with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("rustfmt produced non-UTF-8 output: {error}"))
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
            "{} is stale; run `cargo run -p termirust-ui-contract --bin generate-messages`",
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
