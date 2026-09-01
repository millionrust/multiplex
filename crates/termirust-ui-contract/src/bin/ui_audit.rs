use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const THEMES: [&str; 4] = ["light", "dark", "high-contrast", "recording-friendly"];
const LOCALES: [&str; 4] = ["en-US", "en-XA", "ar-XB", "cjk-fixture"];
const SCALES: [u16; 3] = [100, 200, 400];
const MOTIONS: [&str; 2] = ["full", "reduced"];
const INPUT_MODES: [&str; 3] = ["keyboard", "voiceover", "pointer"];

#[derive(Debug, Deserialize)]
struct Inventory {
    version: u32,
    baseline_commit: String,
    synthetic_canaries: Vec<String>,
    cases: Vec<AuditCase>,
}

#[derive(Debug, Deserialize)]
struct AuditCase {
    id: String,
    screen_id: String,
    surface: String,
    route_fixture: String,
    state: String,
    viewport: String,
    scale: u16,
    theme: String,
    locale: String,
    motion: String,
    input_mode: String,
    reader_steps: Vec<String>,
    expected_semantics: Vec<String>,
    privacy_class: String,
    coverage: String,
    n_a_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct AuditReport {
    schema_version: u32,
    inventory_sha256: String,
    inventory_cases: usize,
    expanded_automated_variants: usize,
    platform: String,
    reader: String,
    surface_results: BTreeMap<String, String>,
    findings: Vec<AuditFinding>,
}

#[derive(Debug, Serialize)]
struct AuditFinding {
    id: String,
    severity: String,
    case_selector: String,
    evidence: String,
    owning_surface: String,
    release_impact: String,
    follow_up_goal: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ui-audit: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    let command = arguments
        .next()
        .ok_or_else(|| "a command is required".to_string())?;
    let arguments = arguments.collect::<Vec<_>>();
    match command.as_str() {
        "validate" => {
            let inventory = required_path(&arguments, "--inventory")?;
            let inventory = load_inventory(&inventory)?;
            let variants = validate_inventory(&inventory)?;
            println!(
                "validated {} frozen cases and {variants} deterministic automated variants",
                inventory.cases.len()
            );
            Ok(())
        }
        "freeze" => freeze(&arguments),
        "contrast" => contrast(&arguments),
        "visuals" => visuals(&arguments),
        "run" => run_audit(&arguments),
        _ => Err(format!("unknown command {command:?}")),
    }
}

fn required_value(arguments: &[String], flag: &str) -> Result<String, String> {
    arguments
        .iter()
        .position(|argument| argument == flag)
        .and_then(|index| arguments.get(index + 1))
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn required_path(arguments: &[String], flag: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(required_value(arguments, flag)?))
}

fn load_inventory(path: &Path) -> Result<Inventory, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("unable to read {}: {error}", path.display()))?;
    toml::from_str(&source).map_err(|error| format!("invalid {}: {error}", path.display()))
}

fn validate_inventory(inventory: &Inventory) -> Result<usize, String> {
    if inventory.version != 1 {
        return Err("inventory version must be 1".to_string());
    }
    if inventory.baseline_commit.len() != 40
        || !inventory
            .baseline_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("baseline_commit must be a full Git hash".to_string());
    }
    if inventory.synthetic_canaries.len() < 3
        || inventory
            .synthetic_canaries
            .iter()
            .any(|value| !value.starts_with("TERMIRUST_AUDIT_"))
    {
        return Err("inventory must contain at least three synthetic audit canaries".to_string());
    }
    if inventory.cases.is_empty() || inventory.cases.len() > 2_000 {
        return Err("inventory case count must be within 1..=2000".to_string());
    }

    let mut ids = BTreeSet::new();
    let mut screens = BTreeMap::<&str, BTreeSet<&str>>::new();
    let mut seen_themes = BTreeSet::new();
    let mut seen_locales = BTreeSet::new();
    let mut seen_scales = BTreeSet::new();
    let mut seen_motion = BTreeSet::new();
    let mut seen_input = BTreeSet::new();
    let mut variants = 0_usize;
    for case in &inventory.cases {
        if !ids.insert(case.id.as_str()) {
            return Err(format!("duplicate case id {}", case.id));
        }
        if case.screen_id.is_empty()
            || case.surface.is_empty()
            || case.route_fixture.is_empty()
            || case.state.is_empty()
            || case.viewport.is_empty()
            || case.reader_steps.is_empty()
            || case.expected_semantics.is_empty()
        {
            return Err(format!("case {} has an empty required field", case.id));
        }
        if !THEMES.contains(&case.theme.as_str())
            || !LOCALES.contains(&case.locale.as_str())
            || !SCALES.contains(&case.scale)
            || !MOTIONS.contains(&case.motion.as_str())
            || !INPUT_MODES.contains(&case.input_mode.as_str())
        {
            return Err(format!(
                "case {} has an unsupported audit dimension",
                case.id
            ));
        }
        if !matches!(case.coverage.as_str(), "pairwise" | "full") {
            return Err(format!("case {} has invalid coverage", case.id));
        }
        if case.n_a_reason.as_deref().is_some_and(str::is_empty) {
            return Err(format!("case {} has an empty N/A rationale", case.id));
        }
        if !matches!(
            case.privacy_class.as_str(),
            "synthetic" | "synthetic-secret"
        ) {
            return Err(format!("case {} has invalid privacy_class", case.id));
        }
        screens
            .entry(case.screen_id.as_str())
            .or_default()
            .insert(case.state.as_str());
        seen_themes.insert(case.theme.as_str());
        seen_locales.insert(case.locale.as_str());
        seen_scales.insert(case.scale);
        seen_motion.insert(case.motion.as_str());
        seen_input.insert(case.input_mode.as_str());
        if case.n_a_reason.is_none() {
            variants = variants.saturating_add(if case.coverage == "full" { 216 } else { 36 });
        }
    }
    if seen_themes != THEMES.into_iter().collect()
        || seen_locales != LOCALES.into_iter().collect()
        || seen_scales != SCALES.into_iter().collect()
        || seen_motion != MOTIONS.into_iter().collect()
        || seen_input != INPUT_MODES.into_iter().collect()
    {
        return Err(
            "inventory does not force every theme, locale, scale, motion and input mode"
                .to_string(),
        );
    }
    for required in [
        "first-run",
        "shell-navigation",
        "projects",
        "sessions",
        "presets-runtimes",
        "worktrees-artifacts",
        "hosts-connections",
        "sftp",
        "vault-keys-snippets",
        "settings",
        "agent-canvas",
        "terminal-chrome",
        "destructive-confirmation",
    ] {
        if !screens.contains_key(required) {
            return Err(format!("inventory is missing required screen {required}"));
        }
    }
    for required_state in [
        "normal",
        "loading",
        "empty",
        "filter-empty",
        "partial",
        "offline",
        "permission-denied",
        "error",
        "cancelled",
        "recovery",
    ] {
        if !screens
            .values()
            .any(|states| states.contains(required_state))
        {
            return Err(format!(
                "inventory is missing required state {required_state}"
            ));
        }
    }
    Ok(variants)
}

fn freeze(arguments: &[String]) -> Result<(), String> {
    let inventory_path = required_path(arguments, "--inventory")?;
    let hash_path = required_path(arguments, "--hash-file")?;
    let bytes = fs::read(&inventory_path)
        .map_err(|error| format!("unable to read {}: {error}", inventory_path.display()))?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if arguments.iter().any(|argument| argument == "--write") {
        fs::write(
            &hash_path,
            format!("{digest}  {}\n", inventory_path.display()),
        )
        .map_err(|error| format!("unable to write {}: {error}", hash_path.display()))?;
        println!("froze UI audit inventory sha256:{digest}");
        return Ok(());
    }
    let expected = fs::read_to_string(&hash_path)
        .map_err(|error| format!("unable to read {}: {error}", hash_path.display()))?;
    let expected = expected
        .split_whitespace()
        .next()
        .ok_or_else(|| "inventory hash file is empty".to_string())?;
    if expected != digest {
        return Err(format!(
            "inventory drift: expected sha256:{expected}, found sha256:{digest}"
        ));
    }
    println!("verified frozen UI audit inventory sha256:{digest}");
    Ok(())
}

fn contrast(arguments: &[String]) -> Result<(), String> {
    let token_path = required_path(arguments, "--tokens")?;
    let source = fs::read_to_string(&token_path)
        .map_err(|error| format!("unable to read {}: {error}", token_path.display()))?;
    let document = source
        .parse::<toml::Table>()
        .map_err(|error| format!("invalid token file: {error}"))?;
    let token_rows = document
        .get("tokens")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "token file has no tokens array".to_string())?;
    let mut colors = BTreeMap::<String, BTreeMap<String, String>>::new();
    for row in token_rows {
        let Some(table) = row.as_table() else {
            continue;
        };
        if table.get("category").and_then(toml::Value::as_str) != Some("color") {
            continue;
        }
        let key = table
            .get("key")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| "color token has no key".to_string())?;
        let values = table
            .get("values")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| format!("color token {key} has no values"))?;
        colors.insert(
            key.to_string(),
            values
                .iter()
                .filter_map(|(theme, value)| {
                    value
                        .as_str()
                        .map(|value| (theme.clone(), value.to_string()))
                })
                .collect(),
        );
    }
    let pairs = [
        ("color.text.primary", "color.bg.canvas", 4.5_f64),
        ("color.text.primary", "color.bg.surface", 4.5),
        ("color.text.secondary", "color.bg.surface", 4.5),
        ("color.text.muted", "color.bg.surface", 4.5),
        ("color.action.primary_text", "color.action.primary", 4.5),
        ("color.focus", "color.bg.surface", 3.0),
        ("color.status.done", "color.bg.surface", 3.0),
        ("color.status.attention", "color.bg.surface", 3.0),
        ("color.status.error", "color.bg.surface", 3.0),
        ("color.status.offline", "color.bg.surface", 3.0),
    ];
    let mut failures = Vec::new();
    for (foreground, background, minimum) in pairs {
        for theme in [
            "system",
            "light",
            "dark",
            "high_contrast",
            "recording_friendly",
        ] {
            let foreground_value = resolve_color(&colors, foreground, theme, 0)?;
            let background_value = resolve_color(&colors, background, theme, 0)?;
            let ratio =
                contrast_ratio(parse_rgb(&foreground_value)?, parse_rgb(&background_value)?);
            if ratio + f64::EPSILON < minimum {
                failures.push(format!(
                    "{theme}: {foreground} on {background} is {ratio:.2}:1, requires {minimum:.1}:1"
                ));
            }
        }
    }
    if failures.is_empty() {
        println!("verified 50 WCAG contrast token/state pairs");
        Ok(())
    } else {
        Err(format!("contrast findings:\n{}", failures.join("\n")))
    }
}

fn resolve_color(
    colors: &BTreeMap<String, BTreeMap<String, String>>,
    key: &str,
    theme: &str,
    depth: usize,
) -> Result<String, String> {
    if depth > 8 {
        return Err(format!("color reference cycle at {key}"));
    }
    let value = colors
        .get(key)
        .and_then(|values| values.get(theme))
        .ok_or_else(|| format!("missing {theme} value for {key}"))?;
    if let Some(reference) = value.strip_prefix('$') {
        resolve_color(colors, reference, theme, depth + 1)
    } else {
        Ok(value.clone())
    }
}

fn parse_rgb(value: &str) -> Result<[f64; 3], String> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| format!("unsupported color {value}"))?;
    if !matches!(hex.len(), 6 | 8) {
        return Err(format!("unsupported color {value}"));
    }
    let channel = |offset| {
        u8::from_str_radix(&hex[offset..offset + 2], 16)
            .map(|value| f64::from(value) / 255.0)
            .map_err(|_| format!("invalid color {value}"))
    };
    Ok([channel(0)?, channel(2)?, channel(4)?])
}

fn contrast_ratio(foreground: [f64; 3], background: [f64; 3]) -> f64 {
    let luminance = |rgb: [f64; 3]| {
        let linear = |value: f64| {
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
    };
    let first = luminance(foreground);
    let second = luminance(background);
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

fn visuals(arguments: &[String]) -> Result<(), String> {
    let inventory_path = required_path(arguments, "--inventory")?;
    let inventory = load_inventory(&inventory_path)?;
    let variants = validate_inventory(&inventory)?;
    if required_value(arguments, "--themes")? != "all"
        || required_value(arguments, "--scales")? != "100,200,400"
    {
        return Err("visual audit requires all themes and scales 100,200,400".to_string());
    }
    println!(
        "validated {variants} deterministic theme/scale/state variants; no repository screenshot baseline or deterministic GPUI capture driver exists"
    );
    Err(
        "AUD-VISUAL-001: visual clipping/reflow evidence requires follow-up Goal 21.1.8"
            .to_string(),
    )
}

fn run_audit(arguments: &[String]) -> Result<(), String> {
    let inventory_path = required_path(arguments, "--inventory")?;
    let platform = required_value(arguments, "--platform")?;
    let reader = required_value(arguments, "--reader")?;
    if platform != "macos" || reader != "voiceover" {
        return Err(
            "this frozen audit supports only --platform macos --reader voiceover".to_string(),
        );
    }
    let inventory = load_inventory(&inventory_path)?;
    let variants = validate_inventory(&inventory)?;
    let inventory_bytes = fs::read(&inventory_path)
        .map_err(|error| format!("unable to read {}: {error}", inventory_path.display()))?;
    let inventory_sha256 = format!("{:x}", Sha256::digest(&inventory_bytes));
    let root = workspace_root();
    let mut surfaces = inventory
        .cases
        .iter()
        .map(|case| case.surface.clone())
        .collect::<BTreeSet<_>>();
    surfaces.remove("cross-screen");
    let mut surface_results = BTreeMap::new();
    for surface in surfaces {
        let status = Command::new(root.join("scripts/verify-ui-surface.sh"))
            .current_dir(&root)
            .args([
                "--surface",
                surface.as_str(),
                "--states",
                "all",
                "--locales",
                "en-US,en-XA,ar-XB",
                "--themes",
                "all",
            ])
            .status()
            .map_err(|error| format!("unable to run {surface} verifier: {error}"))?;
        surface_results.insert(
            surface,
            if status.success() { "pass" } else { "fail" }.to_string(),
        );
    }
    let harness_status = Command::new(root.join("scripts/verify-accessibility-harness.sh"))
        .current_dir(&root)
        .args(["--platform", "macos", "--locale", "en-US,en-XA,ar-XB"])
        .status()
        .map_err(|error| format!("unable to run accessibility harness: {error}"))?;
    surface_results.insert(
        "accessibility-harness".to_string(),
        if harness_status.success() {
            "pass"
        } else {
            "fail"
        }
        .to_string(),
    );

    let mut findings = Vec::new();
    if surface_results.values().any(|status| status != "pass") {
        findings.push(AuditFinding {
            id: "AUD-AUTOMATED-001".to_string(),
            severity: "A1".to_string(),
            case_selector: "all cases on failed surfaces".to_string(),
            evidence: "one or more per-surface semantic/token/localization verifiers failed"
                .to_string(),
            owning_surface: "cross-screen".to_string(),
            release_impact: "release-blocking until the failed surface verifier is green"
                .to_string(),
            follow_up_goal: "21.1.9".to_string(),
        });
    }
    findings.push(AuditFinding {
        id: "AUD-AX-MANUAL-001".to_string(),
        severity: "A1".to_string(),
        case_selector: "manual_required=true".to_string(),
        evidence: "automated semantics cannot prove human-audible keyboard-only and VoiceOver traversal for every frozen route"
            .to_string(),
        owning_surface: "cross-screen".to_string(),
        release_impact: "blocks whole-product WCAG/VoiceOver conformance claims"
            .to_string(),
        follow_up_goal: "21.1.7".to_string(),
    });
    findings.push(AuditFinding {
        id: "AUD-VISUAL-001".to_string(),
        severity: "A1".to_string(),
        case_selector: "all themes/scales/viewports".to_string(),
        evidence: "no deterministic GPUI screenshot baseline/capture driver exists for cross-screen 100/200/400% clipping and reflow inspection"
            .to_string(),
        owning_surface: "cross-screen".to_string(),
        release_impact: "blocks whole-product visual reflow and contrast-rendering claims"
            .to_string(),
        follow_up_goal: "21.1.8".to_string(),
    });
    let report = AuditReport {
        schema_version: 1,
        inventory_sha256,
        inventory_cases: inventory.cases.len(),
        expanded_automated_variants: variants,
        platform,
        reader,
        surface_results,
        findings,
    };
    let report_path = root.join("tests/ui/audit-results.json");
    let output = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("unable to serialize audit report: {error}"))?;
    fs::write(&report_path, format!("{output}\n"))
        .map_err(|error| format!("unable to write {}: {error}", report_path.display()))?;
    println!(
        "audit completed: {} frozen cases, {} variants, {} finding(s); report={}",
        report.inventory_cases,
        report.expanded_automated_variants,
        report.findings.len(),
        report_path.display()
    );
    Ok(())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("ui contract crate must live in workspace/crates")
        .to_path_buf()
}
