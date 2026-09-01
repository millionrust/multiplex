use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::surface_scope::read_surface_sources;

const MAX_BASELINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_UI_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_BASELINE_ENTRIES: usize = 20_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopyFinding {
    pub file: String,
    pub line: usize,
    pub category: String,
    pub fingerprint: String,
    pub excerpt: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopyLintError(String);

impl CopyLintError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CopyLintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CopyLintError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    version: u16,
    exceptions: Vec<BaselineEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineEntry {
    file: String,
    line: usize,
    category: String,
    fingerprint: String,
    reason: String,
    owner: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FindingKey {
    file: String,
    category: String,
    fingerprint: String,
}

pub fn scan_ui_copy_tree(root: &Path) -> Result<Vec<CopyFinding>, CopyLintError> {
    let ui_root = root.join("src/ui");
    let mut files = Vec::new();
    collect_rust_files(&ui_root, &mut files)?;
    files.sort();
    let mut findings = Vec::new();
    for path in files {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            CopyLintError::new(format!("unable to inspect {}: {error}", path.display()))
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(CopyLintError::new(format!(
                "{} must be a regular Rust source file",
                path.display()
            )));
        }
        let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if length > MAX_UI_FILE_BYTES {
            return Err(CopyLintError::new(format!(
                "{} is {length} bytes; UI copy lint files are limited to {MAX_UI_FILE_BYTES} bytes",
                path.display()
            )));
        }
        let source = fs::read_to_string(&path).map_err(|error| {
            CopyLintError::new(format!("unable to read {}: {error}", path.display()))
        })?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| CopyLintError::new("UI path escaped workspace root"))?
            .to_string_lossy()
            .replace('\\', "/");
        findings.extend(scan_copy_source(&relative, &source));
    }
    findings.sort_by(|left, right| {
        (&left.file, left.line, &left.category, &left.fingerprint).cmp(&(
            &right.file,
            right.line,
            &right.category,
            &right.fingerprint,
        ))
    });
    Ok(findings)
}

pub fn scan_ui_copy_paths(
    root: &Path,
    paths: &[PathBuf],
) -> Result<Vec<CopyFinding>, CopyLintError> {
    let mut findings = Vec::new();
    for relative in paths {
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if relative.is_absolute()
            || !normalized.starts_with("src/ui/")
            || normalized.contains("..")
            || relative
                .extension()
                .is_none_or(|extension| extension != "rs")
        {
            return Err(CopyLintError::new(format!(
                "scoped UI copy path must be a Rust file below src/ui: {normalized}"
            )));
        }
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            CopyLintError::new(format!("unable to inspect {}: {error}", path.display()))
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(CopyLintError::new(format!(
                "{} must be a regular Rust source file",
                path.display()
            )));
        }
        if usize::try_from(metadata.len()).unwrap_or(usize::MAX) > MAX_UI_FILE_BYTES {
            return Err(CopyLintError::new(format!(
                "{} exceeds the UI copy lint file size limit",
                path.display()
            )));
        }
        let source = fs::read_to_string(&path).map_err(|error| {
            CopyLintError::new(format!("unable to read {}: {error}", path.display()))
        })?;
        findings.extend(scan_copy_source(&normalized, &source));
    }
    Ok(findings)
}

pub fn verify_zero_copy_paths(root: &Path, paths: &[PathBuf]) -> Result<usize, CopyLintError> {
    let findings = scan_ui_copy_paths(root, paths)?;
    if findings.is_empty() {
        return Ok(0);
    }
    let summary = findings
        .iter()
        .take(25)
        .map(|finding| {
            format!(
                "{}:{} {}: {}",
                finding.file, finding.line, finding.category, finding.excerpt
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(CopyLintError::new(format!(
        "scoped user-copy policy found {} exception(s):\n{summary}",
        findings.len()
    )))
}

pub fn verify_zero_copy_surface(root: &Path, surface: &str) -> Result<usize, CopyLintError> {
    let sources = read_surface_sources(root, surface).map_err(CopyLintError::new)?;
    let findings = sources
        .iter()
        .flat_map(|(file, source)| scan_copy_source(file, source))
        .collect::<Vec<_>>();
    if findings.is_empty() {
        return Ok(0);
    }
    let summary = findings
        .iter()
        .take(25)
        .map(|finding| {
            format!(
                "{}:{} {}: {}",
                finding.file, finding.line, finding.category, finding.excerpt
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(CopyLintError::new(format!(
        "{surface} user-copy policy found {} exception(s):\n{summary}",
        findings.len()
    )))
}

pub fn verify_copy_baseline(root: &Path, baseline_path: &Path) -> Result<usize, CopyLintError> {
    let findings = scan_ui_copy_tree(root)?;
    let baseline = load_baseline(baseline_path)?;
    compare_baseline(&findings, &baseline)?;
    Ok(findings.len())
}

pub fn write_copy_baseline(root: &Path, baseline_path: &Path) -> Result<usize, CopyLintError> {
    let findings = scan_ui_copy_tree(root)?;
    let mut output = String::from(
        "# Immutable inventory of pre-MessageId user-facing copy. Normal verification never updates this file.\nversion = 1\n\n",
    );
    if findings.is_empty() {
        output.push_str("exceptions = []\n");
    }
    for finding in &findings {
        output.push_str("[[exceptions]]\n");
        output.push_str(&format!("file = {:?}\n", finding.file));
        output.push_str(&format!("line = {}\n", finding.line));
        output.push_str(&format!("category = {:?}\n", finding.category));
        output.push_str(&format!("fingerprint = {:?}\n", finding.fingerprint));
        output.push_str(
            "reason = \"Pre-localization UI copy scheduled for MessageId migration under G21.1.\"\n",
        );
        output.push_str("owner = \"G21.1\"\n\n");
    }
    output.pop();
    write_atomic(baseline_path, &output)?;
    Ok(findings.len())
}

pub fn scan_copy_source(file: &str, source: &str) -> Vec<CopyFinding> {
    let mut findings = Vec::new();
    let mut previous_code = String::new();
    let mut cfg_test_pending = false;
    for (index, original) in source.lines().enumerate() {
        let code = strip_line_comment(original).trim();
        if code == "#[cfg(test)]" {
            cfg_test_pending = true;
            continue;
        }
        if cfg_test_pending {
            if code.starts_with("mod ") && code.ends_with('{') {
                break;
            }
            cfg_test_pending = false;
        }
        if code.is_empty() {
            continue;
        }
        let context = format!("{previous_code} {code}");
        let literals = string_literals(code);
        for literal in literals {
            if !likely_user_copy(&literal) {
                continue;
            }
            let visible_context = is_visible_copy_context(&context);
            let concatenated = is_sentence_concatenation(&context, &literal);
            if visible_context {
                findings.push(finding(file, index + 1, "user_copy", &literal));
            }
            if concatenated {
                findings.push(finding(file, index + 1, "sentence_concatenation", &literal));
            }
        }
        previous_code.clear();
        previous_code.push_str(code);
    }
    findings
}

fn finding(file: &str, line: usize, category: &str, literal: &str) -> CopyFinding {
    let canonical = literal.split_whitespace().collect::<Vec<_>>().join(" ");
    CopyFinding {
        file: file.to_string(),
        line,
        category: category.to_string(),
        fingerprint: format!(
            "sha256:{:x}",
            Sha256::digest(format!("{category}\0{canonical}").as_bytes())
        ),
        excerpt: canonical.chars().take(180).collect(),
    }
}

fn is_visible_copy_context(context: &str) -> bool {
    if [".debug_selector(", ".id("]
        .iter()
        .any(|needle| context.contains(needle))
    {
        return false;
    }
    [
        ".label(",
        ".child(",
        ".placeholder(",
        ".tooltip(",
        ".title(",
        ".description(",
        ".status_badge(",
        ".editor_section_card(",
        "status_message",
        "error_message",
        "message:",
        "label:",
        "title:",
    ]
    .iter()
    .any(|needle| context.contains(needle))
}

fn is_sentence_concatenation(context: &str, literal: &str) -> bool {
    let has_variable = literal.contains('{') && literal.contains('}');
    (is_visible_copy_context(context) && context.contains("format!(") && has_variable)
        || (context.contains('+') && context.contains('"') && literal.contains(' '))
}

fn likely_user_copy(literal: &str) -> bool {
    let trimmed = literal.trim();
    if trimmed.is_empty() || !trimmed.chars().any(char::is_alphabetic) {
        return false;
    }
    if [
        "icons/",
        "assets/",
        "http://",
        "https://",
        "src/",
        "tests/",
        "TERMIRUST_",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
    {
        return false;
    }
    if trimmed.starts_with('/') || trimmed.starts_with("./") || trimmed.starts_with("../") {
        return false;
    }
    let is_machine_identifier = !trimmed.contains(char::is_whitespace)
        && trimmed.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_:.".contains(&byte)
        });
    !is_machine_identifier
}

fn string_literals(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        index += 1;
        let mut literal = String::new();
        let mut escaped = false;
        while index < bytes.len() {
            let byte = bytes[index];
            index += 1;
            if escaped {
                literal.push('\\');
                literal.push(char::from(byte));
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                break;
            } else {
                literal.push(char::from(byte));
            }
        }
        literals.push(literal);
    }
    literals
}

fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;
    while index + 1 < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'/' && bytes[index + 1] == b'/' {
            return &line[..index];
        }
        index += 1;
    }
    line
}

fn collect_rust_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), CopyLintError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CopyLintError::new(format!("unable to read {}: {error}", directory.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            CopyLintError::new(format!(
                "unable to read {} entry: {error}",
                directory.display()
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CopyLintError::new(format!("unable to inspect {}: {error}", path.display()))
        })?;
        if file_type.is_symlink() {
            return Err(CopyLintError::new(format!(
                "UI copy lint refuses symlink {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_rust_files(&path, output)?;
        } else if file_type.is_file() && path.extension().is_some_and(|extension| extension == "rs")
        {
            output.push(path);
        }
    }
    Ok(())
}

fn load_baseline(path: &Path) -> Result<Baseline, CopyLintError> {
    let metadata = fs::metadata(path).map_err(|error| {
        CopyLintError::new(format!("unable to inspect {}: {error}", path.display()))
    })?;
    let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if length > MAX_BASELINE_BYTES {
        return Err(CopyLintError::new(format!(
            "{} is {length} bytes; baseline limit is {MAX_BASELINE_BYTES}",
            path.display()
        )));
    }
    let source = fs::read_to_string(path).map_err(|error| {
        CopyLintError::new(format!("unable to read {}: {error}", path.display()))
    })?;
    let baseline: Baseline = toml::from_str(&source)
        .map_err(|error| CopyLintError::new(format!("invalid copy baseline TOML: {error}")))?;
    if baseline.version != 1 {
        return Err(CopyLintError::new(format!(
            "unsupported copy baseline version {}",
            baseline.version
        )));
    }
    if baseline.exceptions.len() > MAX_BASELINE_ENTRIES {
        return Err(CopyLintError::new(format!(
            "copy baseline has {} entries; limit is {MAX_BASELINE_ENTRIES}",
            baseline.exceptions.len()
        )));
    }
    for entry in &baseline.exceptions {
        if entry.file.is_empty()
            || entry.line == 0
            || entry.category.is_empty()
            || !entry.fingerprint.starts_with("sha256:")
            || entry.reason.trim().is_empty()
            || entry.owner.trim().is_empty()
        {
            return Err(CopyLintError::new(
                "copy baseline entries require file, line, category, SHA-256, reason, and owner",
            ));
        }
    }
    Ok(baseline)
}

fn compare_baseline(findings: &[CopyFinding], baseline: &Baseline) -> Result<(), CopyLintError> {
    let actual = finding_counts(findings.iter().map(|finding| FindingKey {
        file: finding.file.clone(),
        category: finding.category.clone(),
        fingerprint: finding.fingerprint.clone(),
    }));
    let expected = finding_counts(baseline.exceptions.iter().map(|entry| FindingKey {
        file: entry.file.clone(),
        category: entry.category.clone(),
        fingerprint: entry.fingerprint.clone(),
    }));
    let mut errors = Vec::new();
    for (key, count) in &actual {
        let expected_count = expected.get(key).copied().unwrap_or(0);
        if *count > expected_count {
            let finding = findings
                .iter()
                .find(|finding| {
                    finding.file == key.file
                        && finding.category == key.category
                        && finding.fingerprint == key.fingerprint
                })
                .expect("actual count came from a finding");
            errors.push(format!(
                "new {} literal at {}:{} ({}) {:?}",
                key.category, key.file, finding.line, key.fingerprint, finding.excerpt
            ));
        }
    }
    for (key, count) in &expected {
        let actual_count = actual.get(key).copied().unwrap_or(0);
        if *count > actual_count {
            errors.push(format!(
                "stale baseline entry for {} in {} ({})",
                key.category, key.file, key.fingerprint
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        errors.sort();
        Err(CopyLintError::new(errors.join("\n")))
    }
}

fn finding_counts(items: impl Iterator<Item = FindingKey>) -> BTreeMap<FindingKey, usize> {
    let mut counts = BTreeMap::new();
    for item in items {
        *counts.entry(item).or_insert(0) += 1;
    }
    counts
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), CopyLintError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CopyLintError::new(format!("{} has no UTF-8 file name", path.display())))?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    fs::write(&temporary, contents).map_err(|error| {
        CopyLintError::new(format!("unable to write {}: {error}", temporary.display()))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        CopyLintError::new(format!(
            "unable to atomically replace {}: {error}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod localization_lint_tests {
    use super::*;

    #[test]
    fn localization_copy_lint_detects_new_button_and_sentence_concatenation() {
        let source = r#"
            Button::new("probe-id").label("Localization policy probe");
            status_message = format!("Copied {} project files", count);
        "#;
        let findings = scan_copy_source("src/ui/probe.rs", source);
        assert!(
            findings
                .iter()
                .any(|finding| finding.category == "user_copy")
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.category == "sentence_concatenation")
        );
        assert!(!findings.iter().any(|finding| finding.excerpt == "probe-id"));
    }

    #[test]
    fn localization_copy_lint_ignores_generated_message_calls_and_machine_ids() {
        let source = r#"
            Button::new("editor-save").label(localization::common_save());
            let path = "icons/save.svg";
        "#;
        assert!(scan_copy_source("src/ui/probe.rs", source).is_empty());
    }

    #[test]
    fn named_cfg_test_modules_do_not_create_product_copy_findings() {
        let source = r#"
            #[cfg(test)]
            mod local_controller_conformance_tests {
                fn fixture() -> &'static str { "Conformance project" }
            }
        "#;
        assert!(scan_copy_source("src/ui/app/session_library.rs", source).is_empty());
    }

    #[test]
    fn localization_copy_lint_rejects_new_and_stale_fingerprints() {
        let findings = scan_copy_source(
            "src/ui/probe.rs",
            "Button::new(\"id\").label(\"Visible label\");",
        );
        let empty = Baseline {
            version: 1,
            exceptions: Vec::new(),
        };
        assert!(compare_baseline(&findings, &empty).is_err());

        let stale = Baseline {
            version: 1,
            exceptions: vec![BaselineEntry {
                file: findings[0].file.clone(),
                line: findings[0].line,
                category: findings[0].category.clone(),
                fingerprint: "sha256:stale".to_string(),
                reason: "legacy".to_string(),
                owner: "G21.1".to_string(),
            }],
        };
        assert!(compare_baseline(&findings, &stale).is_err());
    }

    #[test]
    fn empty_copy_baseline_writer_round_trips_through_reader() {
        let root = std::env::temp_dir().join(format!(
            "termirust-empty-copy-baseline-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let ui = root.join("src/ui");
        fs::create_dir_all(&ui).unwrap();
        fs::write(ui.join("empty.rs"), "pub fn empty() {}\n").unwrap();
        let baseline_path = root.join("legacy-user-copy.toml");

        assert_eq!(write_copy_baseline(&root, &baseline_path).unwrap(), 0);
        assert_eq!(verify_copy_baseline(&root, &baseline_path).unwrap(), 0);
        assert!(
            fs::read_to_string(&baseline_path)
                .unwrap()
                .contains("exceptions = []")
        );

        fs::remove_dir_all(root).unwrap();
    }
}
