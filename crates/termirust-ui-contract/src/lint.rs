use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_BASELINE_BYTES: usize = 2 * 1024 * 1024;
const MAX_UI_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_BASELINE_ENTRIES: usize = 10_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiteralFinding {
    pub file: String,
    pub line: usize,
    pub category: String,
    pub fingerprint: String,
    pub excerpt: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiteralLintError(String);

impl LiteralLintError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for LiteralLintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LiteralLintError {}

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

pub fn scan_ui_tree(root: &Path) -> Result<Vec<LiteralFinding>, LiteralLintError> {
    let ui_root = root.join("src/ui");
    let mut files = Vec::new();
    collect_rust_files(&ui_root, &mut files)?;
    files.sort();
    let mut findings = Vec::new();
    for path in files {
        if path.ends_with("theme.rs") {
            continue;
        }
        let metadata = fs::metadata(&path).map_err(|error| {
            LiteralLintError::new(format!("unable to inspect {}: {error}", path.display()))
        })?;
        let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if length > MAX_UI_FILE_BYTES {
            return Err(LiteralLintError::new(format!(
                "{} is {length} bytes; UI lint files are limited to {MAX_UI_FILE_BYTES} bytes",
                path.display()
            )));
        }
        let source = fs::read_to_string(&path).map_err(|error| {
            LiteralLintError::new(format!("unable to read {}: {error}", path.display()))
        })?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| LiteralLintError::new("UI path escaped workspace root"))?
            .to_string_lossy()
            .replace('\\', "/");
        findings.extend(scan_source(&relative, &source));
    }
    Ok(findings)
}

pub fn verify_baseline(root: &Path, baseline_path: &Path) -> Result<usize, LiteralLintError> {
    let findings = scan_ui_tree(root)?;
    let baseline = load_baseline(baseline_path)?;
    compare_baseline(&findings, &baseline)?;
    Ok(findings.len())
}

pub fn write_baseline(root: &Path, baseline_path: &Path) -> Result<usize, LiteralLintError> {
    let findings = scan_ui_tree(root)?;
    let mut output = String::from(
        "# Immutable inventory of pre-token visual literals. Normal verification never updates this file.\nversion = 1\n\n",
    );
    for finding in &findings {
        output.push_str("[[exceptions]]\n");
        output.push_str(&format!("file = {:?}\n", finding.file));
        output.push_str(&format!("line = {}\n", finding.line));
        output.push_str(&format!("category = {:?}\n", finding.category));
        output.push_str(&format!("fingerprint = {:?}\n", finding.fingerprint));
        let reason = if finding.file.contains("terminal") {
            "Pre-token terminal UI literal; terminal-grid metrics remain documented exceptions until G21.1.5.2."
        } else {
            "Pre-token UI literal scheduled for semantic-token migration under G21.1."
        };
        output.push_str(&format!("reason = {reason:?}\n"));
        output.push_str("owner = \"G21.1\"\n\n");
    }
    output.pop();
    write_atomic(baseline_path, &output)?;
    Ok(findings.len())
}

pub fn scan_source(file: &str, source: &str) -> Vec<LiteralFinding> {
    let mut findings = Vec::new();
    for (index, original) in source.lines().enumerate() {
        let code = strip_line_comment(original).trim();
        if code.is_empty() {
            continue;
        }
        let mut categories = categories_for_line(code);
        categories.sort_unstable();
        categories.dedup();
        for category in categories {
            let canonical = code.split_whitespace().collect::<Vec<_>>().join(" ");
            let fingerprint = format!(
                "sha256:{:x}",
                Sha256::digest(format!("{category}\0{canonical}").as_bytes())
            );
            findings.push(LiteralFinding {
                file: file.to_string(),
                line: index + 1,
                category: category.to_string(),
                fingerprint,
                excerpt: canonical.chars().take(180).collect(),
            });
        }
    }
    findings
}

fn categories_for_line(code: &str) -> Vec<&'static str> {
    let mut categories = Vec::new();
    let lower = code.to_ascii_lowercase();

    let raw_color_constructor = ["rgb(0x", "rgba(0x", "hsla(", "color(0x"]
        .iter()
        .any(|needle| lower.contains(needle));
    let raw_hex_string = contains_hex_color(code);
    if raw_color_constructor || raw_hex_string {
        categories.push("color");
    }

    if lower.contains("font_family(") || lower.contains("font_weight(") {
        categories.push("font");
    }
    if lower.contains("duration::from_millis(")
        || lower.contains("duration::from_secs_f32(")
        || lower.contains(".duration(")
    {
        categories.push("motion");
    }
    if lower.contains("z_index(") || visual_constant(&lower, &["z_index", "z_order", "layer"]) {
        categories.push("z_order");
    }
    if lower.contains("box_shadow(") || lower.contains("shadow(") {
        categories.push("elevation");
    }

    if contains_numeric_call(&lower, "px(") {
        if lower.contains("text_size") || lower.contains("line_height") {
            categories.push("font");
        } else if lower.contains("rounded") || lower.contains("radius") {
            categories.push("radius");
        } else if spacing_context(&lower) {
            categories.push("spacing");
        } else {
            categories.push("dimension");
        }
    }

    if visual_constant(
        &lower,
        &[
            "width",
            "height",
            "size",
            "padding",
            "margin",
            "gap",
            "radius",
            "stroke",
            "sidebar",
            "inspector",
            "header",
            "footer",
            "toolbar",
            "font",
            "spacing",
        ],
    ) {
        let category = if lower.contains("radius") {
            "radius"
        } else if ["padding", "margin", "gap", "spacing"]
            .iter()
            .any(|needle| lower.contains(needle))
        {
            "spacing"
        } else if lower.contains("font") {
            "font"
        } else {
            "dimension"
        };
        categories.push(category);
    }
    categories
}

fn contains_hex_color(code: &str) -> bool {
    let bytes = code.as_bytes();
    for index in 0..bytes.len().saturating_sub(6) {
        if bytes[index] != b'#' {
            continue;
        }
        let remaining = &code[index + 1..];
        let digits = remaining
            .chars()
            .take_while(|character| character.is_ascii_hexdigit())
            .count();
        if digits == 6 || digits == 8 {
            return true;
        }
    }
    false
}

fn contains_numeric_call(code: &str, needle: &str) -> bool {
    let Some(index) = code.find(needle) else {
        return false;
    };
    code[index + needle.len()..]
        .trim_start()
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit() || character == '-' || character == '.')
}

fn spacing_context(code: &str) -> bool {
    [
        ".p(", ".pt(", ".pr(", ".pb(", ".pl(", ".px(", ".py(", ".m(", ".mt(", ".mr(", ".mb(",
        ".ml(", ".mx(", ".my(", ".gap(", "padding", "margin",
    ]
    .iter()
    .any(|needle| code.contains(needle))
}

fn visual_constant(code: &str, names: &[&str]) -> bool {
    if !(code.contains("const ") || code.contains("static ")) || !code.contains('=') {
        return false;
    }
    names.iter().any(|name| code.contains(name))
        && code
            .split_once('=')
            .is_some_and(|(_, value)| value.chars().any(|character| character.is_ascii_digit()))
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

fn collect_rust_files(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), LiteralLintError> {
    let entries = fs::read_dir(path).map_err(|error| {
        LiteralLintError::new(format!("unable to list {}: {error}", path.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| LiteralLintError::new(error.to_string()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| LiteralLintError::new(error.to_string()))?;
        if file_type.is_symlink() {
            return Err(LiteralLintError::new(format!(
                "refusing symlink in UI lint tree: {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_rust_files(&path, output)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
    Ok(())
}

fn load_baseline(path: &Path) -> Result<Baseline, LiteralLintError> {
    let metadata = fs::metadata(path).map_err(|error| {
        LiteralLintError::new(format!("unable to inspect {}: {error}", path.display()))
    })?;
    let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if length > MAX_BASELINE_BYTES {
        return Err(LiteralLintError::new(format!(
            "{} is {length} bytes; baseline limit is {MAX_BASELINE_BYTES}",
            path.display()
        )));
    }
    let source = fs::read_to_string(path).map_err(|error| {
        LiteralLintError::new(format!("unable to read {}: {error}", path.display()))
    })?;
    let baseline: Baseline = toml::from_str(&source)
        .map_err(|error| LiteralLintError::new(format!("invalid baseline TOML: {error}")))?;
    if baseline.version != 1 {
        return Err(LiteralLintError::new(format!(
            "unsupported legacy visual baseline version {}",
            baseline.version
        )));
    }
    if baseline.exceptions.len() > MAX_BASELINE_ENTRIES {
        return Err(LiteralLintError::new(format!(
            "baseline has {} entries; limit is {MAX_BASELINE_ENTRIES}",
            baseline.exceptions.len()
        )));
    }
    for entry in &baseline.exceptions {
        if entry.file.starts_with('/')
            || entry.file.contains("..")
            || entry.line == 0
            || entry.category.trim().is_empty()
            || !entry.fingerprint.starts_with("sha256:")
            || entry.reason.trim().is_empty()
            || !entry.owner.starts_with("G21")
        {
            return Err(LiteralLintError::new(format!(
                "invalid legacy baseline entry for {}:{}",
                entry.file, entry.line
            )));
        }
    }
    Ok(baseline)
}

fn compare_baseline(
    findings: &[LiteralFinding],
    baseline: &Baseline,
) -> Result<(), LiteralLintError> {
    let mut actual = BTreeMap::<FindingKey, usize>::new();
    for finding in findings {
        *actual.entry(finding_key(finding)).or_default() += 1;
    }
    let mut expected = BTreeMap::<FindingKey, usize>::new();
    for entry in &baseline.exceptions {
        *expected
            .entry(FindingKey {
                file: entry.file.clone(),
                category: entry.category.clone(),
                fingerprint: entry.fingerprint.clone(),
            })
            .or_default() += 1;
    }

    let mut errors = Vec::new();
    for finding in findings {
        let key = finding_key(finding);
        if actual.get(&key).copied().unwrap_or_default()
            > expected.get(&key).copied().unwrap_or_default()
            && !errors
                .iter()
                .any(|error: &String| error.contains(&finding.fingerprint))
        {
            errors.push(format!(
                "new {} literal at {}:{} [{}]: {}",
                finding.category, finding.file, finding.line, finding.fingerprint, finding.excerpt
            ));
        }
    }
    for (key, count) in &expected {
        let actual_count = actual.get(key).copied().unwrap_or_default();
        if actual_count < *count {
            errors.push(format!(
                "stale baseline entry for {} {} [{}]: expected {}, found {}",
                key.file, key.category, key.fingerprint, count, actual_count
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        errors.truncate(25);
        Err(LiteralLintError::new(format!(
            "visual literal policy failed:\n{}",
            errors.join("\n")
        )))
    }
}

fn finding_key(finding: &LiteralFinding) -> FindingKey {
    FindingKey {
        file: finding.file.clone(),
        category: finding.category.clone(),
        fingerprint: finding.fingerprint.clone(),
    }
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), LiteralLintError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| LiteralLintError::new("baseline path has no UTF-8 file name"))?;
    let temporary = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    fs::write(&temporary, contents).map_err(|error| {
        LiteralLintError::new(format!("unable to write {}: {error}", temporary.display()))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        LiteralLintError::new(format!(
            "unable to atomically replace {}: {error}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tokens_tests {
    use super::*;

    #[test]
    fn tokens_lint_finds_known_visual_literal_categories() {
        let source = r##"
            let fill = gpui::rgb(0xff00aa);
            let width = px(37.0);
            let title = text_size(px(19.0));
            let radius = rounded(px(7.0));
            let wait = Duration::from_millis(123);
            const PANEL_GAP: f32 = 11.0;
        "##;
        let categories = scan_source("src/ui/example.rs", source)
            .into_iter()
            .map(|finding| finding.category)
            .collect::<Vec<_>>();
        assert!(categories.contains(&"color".to_string()));
        assert!(categories.contains(&"dimension".to_string()));
        assert!(categories.contains(&"font".to_string()));
        assert!(categories.contains(&"radius".to_string()));
        assert!(categories.contains(&"motion".to_string()));
        assert!(categories.contains(&"spacing".to_string()));
    }

    #[test]
    fn tokens_lint_fingerprint_ignores_indentation_but_not_value_changes() {
        let left = scan_source("src/ui/example.rs", "  let x = px(37.0);");
        let shifted = scan_source("src/ui/example.rs", "        let x = px(37.0);");
        let changed = scan_source("src/ui/example.rs", "  let x = px(38.0);");
        assert_eq!(left[0].fingerprint, shifted[0].fingerprint);
        assert_ne!(left[0].fingerprint, changed[0].fingerprint);
    }

    #[test]
    fn tokens_lint_ignores_comments_and_dynamic_colors() {
        let source = "// gpui::rgb(0xff00aa)\nlet color = gpui::rgb(tag.rgb_hex());";
        assert!(scan_source("src/ui/example.rs", source).is_empty());
    }

    #[test]
    fn tokens_lint_rejects_new_and_stale_fingerprints() {
        let finding = scan_source("src/ui/example.rs", "let width = px(37.0);")
            .into_iter()
            .next()
            .expect("known literal should be found");
        let empty = Baseline {
            version: 1,
            exceptions: Vec::new(),
        };
        assert!(compare_baseline(std::slice::from_ref(&finding), &empty).is_err());

        let stale = Baseline {
            version: 1,
            exceptions: vec![BaselineEntry {
                file: finding.file.clone(),
                line: finding.line,
                category: finding.category.clone(),
                fingerprint: finding.fingerprint.clone(),
                reason: "Legacy fixture".to_string(),
                owner: "G21.1".to_string(),
            }],
        };
        assert!(compare_baseline(&[], &stale).is_err());
    }
}
