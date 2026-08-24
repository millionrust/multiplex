use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_TOKENS: usize = 256;
const MAX_STATUSES: usize = 32;
const SUPPORTED_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
enum Category {
    Color,
    FontFamily,
    Typography,
    Spacing,
    Dimension,
    Border,
    Radius,
    Elevation,
    Shadow,
    Icon,
    Text,
    Duration,
    Easing,
    ZIndex,
}

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Self::Color => "color",
            Self::FontFamily => "font_family",
            Self::Typography => "typography",
            Self::Spacing => "spacing",
            Self::Dimension => "dimension",
            Self::Border => "border",
            Self::Radius => "radius",
            Self::Elevation => "elevation",
            Self::Shadow => "shadow",
            Self::Icon => "icon",
            Self::Text => "text",
            Self::Duration => "duration",
            Self::Easing => "easing",
            Self::ZIndex => "z_index",
        }
    }

    fn rust_type(self) -> &'static str {
        match self {
            Self::Color => "ColorValue",
            Self::FontFamily => "FontFamilyValue",
            Self::Typography => "TypographyValue",
            Self::Spacing => "SpacingValue",
            Self::Dimension => "DimensionValue",
            Self::Border => "BorderValue",
            Self::Radius => "RadiusValue",
            Self::Elevation => "ElevationValue",
            Self::Shadow => "ShadowValue",
            Self::Icon => "IconValue",
            Self::Text => "TextValue",
            Self::Duration => "DurationValue",
            Self::Easing => "EasingValue",
            Self::ZIndex => "ZIndexValue",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Theme {
    System,
    Light,
    Dark,
    HighContrast,
    RecordingFriendly,
}

impl Theme {
    const ALL: [Self; 5] = [
        Self::System,
        Self::Light,
        Self::Dark,
        Self::HighContrast,
        Self::RecordingFriendly,
    ];

    fn field(self, values: &ThemeValues) -> &str {
        match self {
            Self::System => &values.system,
            Self::Light => &values.light,
            Self::Dark => &values.dark,
            Self::HighContrast => &values.high_contrast,
            Self::RecordingFriendly => &values.recording_friendly,
        }
    }

    fn manifest_name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
            Self::HighContrast => "high_contrast",
            Self::RecordingFriendly => "recording_friendly",
        }
    }

    fn rust_name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::HighContrast => "HighContrast",
            Self::RecordingFriendly => "RecordingFriendly",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeValues {
    system: String,
    light: String,
    dark: String,
    high_contrast: String,
    recording_friendly: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenDefinition {
    key: String,
    category: Category,
    description: String,
    allowed_use: String,
    values: ThemeValues,
    #[serde(default)]
    reduced_motion: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusDefinition {
    kind: String,
    color: String,
    icon: String,
    text: String,
    shape: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenManifest {
    version: u16,
    tokens: Vec<TokenDefinition>,
    statuses: Vec<StatusDefinition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractError(String);

impl ContractError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ContractError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationArtifacts {
    pub rust: String,
    pub platform_contract_json: String,
    pub source_hash: String,
}

pub fn load_manifest(path: &Path) -> Result<(TokenManifest, Vec<u8>), ContractError> {
    let metadata = fs::metadata(path).map_err(|error| {
        ContractError::new(format!("unable to inspect {}: {error}", path.display()))
    })?;
    let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if length > MAX_MANIFEST_BYTES {
        return Err(ContractError::new(format!(
            "{} is {length} bytes; token manifests are limited to {MAX_MANIFEST_BYTES} bytes",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| {
        ContractError::new(format!("unable to read {}: {error}", path.display()))
    })?;
    let manifest = parse_manifest(&bytes)?;
    Ok((manifest, bytes))
}

pub fn parse_manifest(bytes: &[u8]) -> Result<TokenManifest, ContractError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ContractError::new(format!(
            "token manifest is {} bytes; limit is {MAX_MANIFEST_BYTES}",
            bytes.len()
        )));
    }
    let source = std::str::from_utf8(bytes)
        .map_err(|error| ContractError::new(format!("token manifest is not UTF-8: {error}")))?;
    let mut manifest: TokenManifest = toml::from_str(source)
        .map_err(|error| ContractError::new(format!("invalid token manifest TOML: {error}")))?;
    manifest
        .tokens
        .sort_by(|left, right| left.key.cmp(&right.key));
    manifest
        .statuses
        .sort_by(|left, right| left.kind.cmp(&right.kind));
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn generate_artifacts(
    manifest: &TokenManifest,
    source: &[u8],
) -> Result<GenerationArtifacts, ContractError> {
    validate_manifest(manifest)?;
    let source_hash = format!("{:x}", Sha256::digest(source));
    let resolved = resolve_all(manifest)?;
    let rust = generate_rust(manifest, &resolved, &source_hash)?;
    let platform_contract_json = generate_platform_contract(manifest, &resolved, &source_hash)?;
    Ok(GenerationArtifacts {
        rust,
        platform_contract_json,
        source_hash,
    })
}

fn validate_manifest(manifest: &TokenManifest) -> Result<(), ContractError> {
    if manifest.version != SUPPORTED_VERSION {
        return Err(ContractError::new(format!(
            "unsupported token manifest version {}; expected {SUPPORTED_VERSION}",
            manifest.version
        )));
    }
    if manifest.tokens.is_empty() || manifest.tokens.len() > MAX_TOKENS {
        return Err(ContractError::new(format!(
            "token count {} is outside 1..={MAX_TOKENS}",
            manifest.tokens.len()
        )));
    }
    if manifest.statuses.is_empty() || manifest.statuses.len() > MAX_STATUSES {
        return Err(ContractError::new(format!(
            "status count {} is outside 1..={MAX_STATUSES}",
            manifest.statuses.len()
        )));
    }

    let mut definitions = BTreeMap::new();
    for token in &manifest.tokens {
        if !is_canonical_key(&token.key) {
            return Err(ContractError::new(format!(
                "token key {:?} is not canonical lowercase dot notation",
                token.key
            )));
        }
        if token.description.trim().is_empty() || token.allowed_use.trim().is_empty() {
            return Err(ContractError::new(format!(
                "token {} requires non-empty description and allowed_use",
                token.key
            )));
        }
        if definitions.insert(token.key.as_str(), token).is_some() {
            return Err(ContractError::new(format!(
                "duplicate token key {}",
                token.key
            )));
        }
        for theme in Theme::ALL {
            if theme.field(&token.values).trim().is_empty() {
                return Err(ContractError::new(format!(
                    "token {} is missing a {} value",
                    token.key,
                    theme.manifest_name()
                )));
            }
        }
    }

    for token in &manifest.tokens {
        for theme in Theme::ALL {
            validate_reference(token, theme, &definitions)?;
        }
        if let Some(replacement) = &token.reduced_motion {
            if token.category != Category::Duration {
                return Err(ContractError::new(format!(
                    "token {} has reduced_motion but is not a duration",
                    token.key
                )));
            }
            let replacement = definitions.get(replacement.as_str()).ok_or_else(|| {
                ContractError::new(format!(
                    "token {} references missing reduced-motion token {}",
                    token.key, replacement
                ))
            })?;
            if replacement.category != Category::Duration {
                return Err(ContractError::new(format!(
                    "token {} reduced-motion replacement {} is not a duration",
                    token.key, replacement.key
                )));
            }
        }
    }
    validate_reduced_motion_cycles(manifest, &definitions)?;
    let resolved = resolve_all(manifest)?;
    for token in &manifest.tokens {
        for theme in Theme::ALL {
            let value = resolved
                .get(&(token.key.clone(), theme))
                .ok_or_else(|| ContractError::new("internal missing resolved token"))?;
            parse_typed_value(token.category, value).map_err(|error| {
                ContractError::new(format!(
                    "token {} {} value {:?} is invalid for {}: {error}",
                    token.key,
                    theme.manifest_name(),
                    value,
                    token.category.as_str()
                ))
            })?;
        }
    }
    validate_statuses(manifest, &definitions)?;
    Ok(())
}

fn validate_reference(
    token: &TokenDefinition,
    theme: Theme,
    definitions: &BTreeMap<&str, &TokenDefinition>,
) -> Result<(), ContractError> {
    let value = theme.field(&token.values).trim();
    let Some(reference) = value.strip_prefix('$') else {
        return Ok(());
    };
    let target = definitions.get(reference).ok_or_else(|| {
        ContractError::new(format!(
            "token {} {} references missing token {}",
            token.key,
            theme.manifest_name(),
            reference
        ))
    })?;
    if target.category != token.category {
        return Err(ContractError::new(format!(
            "token {} ({}) cannot reference {} ({})",
            token.key,
            token.category.as_str(),
            target.key,
            target.category.as_str()
        )));
    }
    Ok(())
}

fn validate_reduced_motion_cycles(
    manifest: &TokenManifest,
    definitions: &BTreeMap<&str, &TokenDefinition>,
) -> Result<(), ContractError> {
    for token in &manifest.tokens {
        let mut seen = BTreeSet::new();
        let mut current = token;
        while let Some(next) = &current.reduced_motion {
            if !seen.insert(current.key.as_str()) {
                return Err(ContractError::new(format!(
                    "reduced-motion reference cycle includes {}",
                    current.key
                )));
            }
            current = definitions
                .get(next.as_str())
                .copied()
                .ok_or_else(|| ContractError::new("missing reduced-motion token"))?;
        }
    }
    Ok(())
}

fn validate_statuses(
    manifest: &TokenManifest,
    definitions: &BTreeMap<&str, &TokenDefinition>,
) -> Result<(), ContractError> {
    const EXPECTED: [(&str, &str); 8] = [
        ("attention", "diamond"),
        ("busy", "filled_circle"),
        ("done", "check_circle"),
        ("error", "octagon"),
        ("idle", "hollow_circle"),
        ("offline", "broken_link"),
        ("orphaned", "question_diamond"),
        ("permission_denied", "lock"),
    ];
    let statuses = manifest
        .statuses
        .iter()
        .map(|status| (status.kind.as_str(), status))
        .collect::<BTreeMap<_, _>>();
    if statuses.len() != manifest.statuses.len() {
        return Err(ContractError::new("duplicate status kind"));
    }
    for (kind, shape) in EXPECTED {
        let status = statuses.get(kind).ok_or_else(|| {
            ContractError::new(format!("required semantic status {kind} is missing"))
        })?;
        if status.shape != shape {
            return Err(ContractError::new(format!(
                "status {kind} must use fixed shape {shape}, found {}",
                status.shape
            )));
        }
        validate_status_reference(definitions, kind, "color", &status.color, Category::Color)?;
        validate_status_reference(definitions, kind, "icon", &status.icon, Category::Icon)?;
        validate_status_reference(definitions, kind, "text", &status.text, Category::Text)?;
    }
    if statuses.len() != EXPECTED.len() {
        return Err(ContractError::new(format!(
            "status inventory has {} entries; expected exactly {}",
            statuses.len(),
            EXPECTED.len()
        )));
    }
    Ok(())
}

fn validate_status_reference(
    definitions: &BTreeMap<&str, &TokenDefinition>,
    kind: &str,
    field: &str,
    key: &str,
    category: Category,
) -> Result<(), ContractError> {
    let token = definitions.get(key).ok_or_else(|| {
        ContractError::new(format!(
            "status {kind} {field} references missing token {key}"
        ))
    })?;
    if token.category != category {
        return Err(ContractError::new(format!(
            "status {kind} {field} token {key} must be {}",
            category.as_str()
        )));
    }
    Ok(())
}

fn resolve_all(
    manifest: &TokenManifest,
) -> Result<BTreeMap<(String, Theme), String>, ContractError> {
    let definitions = manifest
        .tokens
        .iter()
        .map(|token| (token.key.as_str(), token))
        .collect::<BTreeMap<_, _>>();
    let mut resolved = BTreeMap::new();
    for token in &manifest.tokens {
        for theme in Theme::ALL {
            let mut visiting = Vec::new();
            resolve_value(
                &token.key,
                theme,
                &definitions,
                &mut resolved,
                &mut visiting,
            )?;
        }
    }
    Ok(resolved)
}

fn resolve_value(
    key: &str,
    theme: Theme,
    definitions: &BTreeMap<&str, &TokenDefinition>,
    resolved: &mut BTreeMap<(String, Theme), String>,
    visiting: &mut Vec<String>,
) -> Result<String, ContractError> {
    let cache_key = (key.to_string(), theme);
    if let Some(value) = resolved.get(&cache_key) {
        return Ok(value.clone());
    }
    if let Some(position) = visiting.iter().position(|candidate| candidate == key) {
        let mut cycle = visiting[position..].to_vec();
        cycle.push(key.to_string());
        return Err(ContractError::new(format!(
            "token reference cycle for {}: {}",
            theme.manifest_name(),
            cycle.join(" -> ")
        )));
    }
    let token = definitions
        .get(key)
        .ok_or_else(|| ContractError::new(format!("missing token {key}")))?;
    visiting.push(key.to_string());
    let raw = theme.field(&token.values).trim();
    let value = if let Some(reference) = raw.strip_prefix('$') {
        resolve_value(reference, theme, definitions, resolved, visiting)?
    } else {
        raw.to_string()
    };
    visiting.pop();
    resolved.insert(cache_key, value.clone());
    Ok(value)
}

#[derive(Clone, Debug, PartialEq)]
enum ParsedValue {
    Color(u32),
    Float(f32),
    Unsigned(u16),
    Signed(i16),
    Text(String),
    Typography(f32, f32, u16),
    Shadow(Option<ParsedShadow>),
    Easing(f32, f32, f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
struct ParsedShadow {
    x: f32,
    y: f32,
    blur: f32,
    spread: f32,
    color: u32,
}

fn parse_typed_value(category: Category, value: &str) -> Result<ParsedValue, &'static str> {
    match category {
        Category::Color => parse_color(value).map(ParsedValue::Color),
        Category::FontFamily | Category::Icon | Category::Text => {
            if value.is_empty() || value.len() > 128 || value.chars().any(char::is_whitespace) {
                Err("must be a non-empty identifier of at most 128 bytes")
            } else {
                Ok(ParsedValue::Text(value.to_string()))
            }
        }
        Category::Typography => {
            let parts = split_f32(value, 3)?;
            if parts[2].fract() != 0.0 || !(0.0..=u16::MAX as f32).contains(&parts[2]) {
                return Err("typography weight must be an integer");
            }
            let weight = parts[2] as u16;
            if parts[0] <= 0.0 || parts[1] < parts[0] || !(100..=900).contains(&weight) {
                return Err("must be size/line-height/weight with valid positive metrics");
            }
            Ok(ParsedValue::Typography(parts[0], parts[1], weight))
        }
        Category::Spacing | Category::Dimension | Category::Border | Category::Radius => {
            let scalar = parse_f32_exact(value)?;
            if !scalar.is_finite() || !(0.0..=4096.0).contains(&scalar) {
                Err("must be a finite scalar in 0..=4096")
            } else {
                Ok(ParsedValue::Float(scalar))
            }
        }
        Category::Elevation => {
            let value = parse_u16_exact(value)?;
            if value > 3 {
                Err("must be a named elevation level in 0..=3")
            } else {
                Ok(ParsedValue::Unsigned(value))
            }
        }
        Category::Shadow => parse_shadow(value).map(ParsedValue::Shadow),
        Category::Duration => {
            let millis = value
                .strip_suffix("ms")
                .ok_or("must end in ms")
                .and_then(parse_u16_exact)?;
            if millis > 10_000 {
                Err("must be at most 10000ms")
            } else {
                Ok(ParsedValue::Unsigned(millis))
            }
        }
        Category::Easing => {
            let parts = split_f32(value, 4)?;
            if parts
                .iter()
                .any(|part| !part.is_finite() || !(0.0..=1.0).contains(part))
            {
                Err("must contain four cubic-bezier values in 0..=1")
            } else {
                Ok(ParsedValue::Easing(parts[0], parts[1], parts[2], parts[3]))
            }
        }
        Category::ZIndex => {
            let value = value.parse::<i16>().map_err(|_| "must be an i16")?;
            if !(0..=1000).contains(&value) {
                Err("must be a named layer in 0..=1000")
            } else {
                Ok(ParsedValue::Signed(value))
            }
        }
    }
}

fn parse_color(value: &str) -> Result<u32, &'static str> {
    let digits = value.strip_prefix('#').ok_or("must start with #")?;
    let rgba = match digits.len() {
        6 => u32::from_str_radix(digits, 16)
            .map(|rgb| (rgb << 8) | 0xff)
            .map_err(|_| "must contain hexadecimal digits")?,
        8 => u32::from_str_radix(digits, 16).map_err(|_| "must contain hexadecimal digits")?,
        _ => return Err("must be #RRGGBB or #RRGGBBAA"),
    };
    Ok(rgba)
}

fn parse_shadow(value: &str) -> Result<Option<ParsedShadow>, &'static str> {
    if value == "none" {
        return Ok(None);
    }
    let mut parts = value.split('/');
    let x = parts.next().ok_or("missing x")?;
    let y = parts.next().ok_or("missing y")?;
    let blur = parts.next().ok_or("missing blur")?;
    let spread = parts.next().ok_or("missing spread")?;
    let color = parts.next().ok_or("missing color")?;
    if parts.next().is_some() {
        return Err("must be x/y/blur/spread/color");
    }
    let parsed = ParsedShadow {
        x: parse_f32_exact(x)?,
        y: parse_f32_exact(y)?,
        blur: parse_f32_exact(blur)?,
        spread: parse_f32_exact(spread)?,
        color: parse_color(color)?,
    };
    if !(0.0..=128.0).contains(&parsed.blur) || parsed.spread.abs() > 64.0 {
        return Err("shadow blur or spread is out of bounds");
    }
    Ok(Some(parsed))
}

fn split_f32(value: &str, expected: usize) -> Result<Vec<f32>, &'static str> {
    let parts = value
        .split('/')
        .map(parse_f32_exact)
        .collect::<Result<Vec<_>, _>>()?;
    if parts.len() != expected {
        return Err("has the wrong number of slash-separated fields");
    }
    Ok(parts)
}

fn parse_f32_exact(value: &str) -> Result<f32, &'static str> {
    value.parse::<f32>().map_err(|_| "must be a number")
}

fn parse_u16_exact(value: &str) -> Result<u16, &'static str> {
    value.parse::<u16>().map_err(|_| "must be a u16")
}

fn generate_rust(
    manifest: &TokenManifest,
    resolved: &BTreeMap<(String, Theme), String>,
    source_hash: &str,
) -> Result<String, ContractError> {
    let mut output = String::new();
    output.push_str("// @generated by generate-tokens; do not edit.\n");
    output.push_str(&format!(
        "// Source: design/tokens.toml (sha256:{source_hash})\n\n"
    ));
    output.push_str("#[rustfmt::skip]\nmod token_data {\n");
    output.push_str(&format!(
        "pub const TOKEN_SCHEMA_VERSION: u16 = {};\n",
        manifest.version
    ));
    output.push_str(&format!(
        "pub const TOKEN_SOURCE_SHA256: &str = {source_hash:?};\n\n"
    ));
    output.push_str(GENERATED_TYPES);

    output.push_str("impl DesignTokens {\n");
    output.push_str("    pub const fn new(theme: ThemeKind) -> Self { Self { theme } }\n\n");
    output.push_str("    pub const fn theme(self) -> ThemeKind { self.theme }\n\n");
    for token in &manifest.tokens {
        let method = rust_identifier(&token.key);
        let return_type = token.category.rust_type();
        output.push_str(&format!(
            "    /// {} Allowed use: {}\n",
            token.description, token.allowed_use
        ));
        if token.category == Category::Duration {
            output.push_str(&format!(
                "    pub const fn {method}(self, reduced_motion: bool) -> {return_type} {{\n"
            ));
            if let Some(replacement) = &token.reduced_motion {
                output.push_str(&format!(
                    "        if reduced_motion {{ return self.{}(false); }}\n",
                    rust_identifier(replacement)
                ));
            } else {
                output.push_str("        let _ = reduced_motion;\n");
            }
        } else {
            output.push_str(&format!(
                "    pub const fn {method}(self) -> {return_type} {{\n"
            ));
        }
        output.push_str("        match self.theme {\n");
        for theme in Theme::ALL {
            let raw = resolved
                .get(&(token.key.clone(), theme))
                .ok_or_else(|| ContractError::new("internal missing generated value"))?;
            let value = parse_typed_value(token.category, raw).map_err(ContractError::new)?;
            output.push_str(&format!(
                "            ThemeKind::{} => {},\n",
                theme.rust_name(),
                rust_value(&value, token.category)
            ));
        }
        output.push_str("        }\n    }\n\n");
    }
    output.push_str("    pub const fn status(self, kind: StatusKind) -> StatusVisual {\n");
    output.push_str("        match kind {\n");
    for status in &manifest.statuses {
        output.push_str(&format!(
            "            StatusKind::{} => StatusVisual {{ color: self.{}(), icon: self.{}(), text: self.{}(), shape: {:?} }},\n",
            rust_variant(&status.kind),
            rust_identifier(&status.color),
            rust_identifier(&status.icon),
            rust_identifier(&status.text),
            status.shape
        ));
    }
    output.push_str("        }\n    }\n}\n");
    output.push_str("}\n\npub use token_data::*;\n");
    Ok(output)
}

fn generate_platform_contract(
    manifest: &TokenManifest,
    resolved: &BTreeMap<(String, Theme), String>,
    source_hash: &str,
) -> Result<String, ContractError> {
    let tokens = manifest
        .tokens
        .iter()
        .map(|token| {
            let values = Theme::ALL
                .iter()
                .map(|theme| {
                    let value = resolved
                        .get(&(token.key.clone(), *theme))
                        .cloned()
                        .ok_or_else(|| ContractError::new("internal missing fixture value"))?;
                    Ok((theme.manifest_name().to_string(), value))
                })
                .collect::<Result<BTreeMap<_, _>, ContractError>>()?;
            Ok(json!({
                "allowedUse": token.allowed_use,
                "category": token.category.as_str(),
                "description": token.description,
                "key": token.key,
                "reducedMotion": token.reduced_motion,
                "values": values,
            }))
        })
        .collect::<Result<Vec<_>, ContractError>>()?;
    let statuses = manifest
        .statuses
        .iter()
        .map(|status| {
            json!({
                "color": status.color,
                "icon": status.icon,
                "kind": status.kind,
                "shape": status.shape,
                "text": status.text,
            })
        })
        .collect::<Vec<_>>();
    let fixture = json!({
        "schemaVersion": manifest.version,
        "sourceSha256": source_hash,
        "statuses": statuses,
        "themes": Theme::ALL.map(Theme::manifest_name),
        "tokens": tokens,
    });
    serde_json::to_string_pretty(&fixture)
        .map(|mut json| {
            json.push('\n');
            json
        })
        .map_err(|error| ContractError::new(format!("unable to generate JSON fixture: {error}")))
}

fn rust_value(value: &ParsedValue, category: Category) -> String {
    match (value, category) {
        (ParsedValue::Color(rgba), Category::Color) => {
            format!("ColorValue::from_rgba(0x{rgba:08x})")
        }
        (ParsedValue::Float(value), Category::Spacing) => {
            format!("SpacingValue({})", rust_float(*value))
        }
        (ParsedValue::Float(value), Category::Dimension) => {
            format!("DimensionValue({})", rust_float(*value))
        }
        (ParsedValue::Float(value), Category::Border) => {
            format!("BorderValue({})", rust_float(*value))
        }
        (ParsedValue::Float(value), Category::Radius) => {
            format!("RadiusValue({})", rust_float(*value))
        }
        (ParsedValue::Unsigned(value), Category::Elevation) => format!("ElevationValue({value})"),
        (ParsedValue::Unsigned(value), Category::Duration) => format!("DurationValue({value})"),
        (ParsedValue::Signed(value), Category::ZIndex) => format!("ZIndexValue({value})"),
        (ParsedValue::Text(value), Category::FontFamily) => {
            format!("FontFamilyValue({value:?})")
        }
        (ParsedValue::Text(value), Category::Icon) => format!("IconValue({value:?})"),
        (ParsedValue::Text(value), Category::Text) => format!("TextValue({value:?})"),
        (ParsedValue::Typography(size, line, weight), Category::Typography) => format!(
            "TypographyValue {{ size: {}, line_height: {}, weight: {weight} }}",
            rust_float(*size),
            rust_float(*line)
        ),
        (ParsedValue::Shadow(None), Category::Shadow) => "ShadowValue::NONE".to_string(),
        (ParsedValue::Shadow(Some(shadow)), Category::Shadow) => format!(
            "ShadowValue {{ x: {}, y: {}, blur: {}, spread: {}, color: ColorValue::from_rgba(0x{rgba:08x}), visible: true }}",
            rust_float(shadow.x),
            rust_float(shadow.y),
            rust_float(shadow.blur),
            rust_float(shadow.spread),
            rgba = shadow.color,
        ),
        (ParsedValue::Easing(x1, y1, x2, y2), Category::Easing) => format!(
            "EasingValue {{ x1: {}, y1: {}, x2: {}, y2: {} }}",
            rust_float(*x1),
            rust_float(*y1),
            rust_float(*x2),
            rust_float(*y2)
        ),
        _ => "compile_error!(\"invalid generated token type\")".to_string(),
    }
}

fn rust_float(value: f32) -> String {
    let mut rendered = format!("{value:.4}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.push('0');
    }
    rendered.push_str("f32");
    rendered
}

fn rust_identifier(key: &str) -> String {
    key.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn rust_variant(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn is_canonical_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 96
        && key.split('.').all(|segment| {
            !segment.is_empty()
                && segment.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        })
}

const GENERATED_TYPES: &str = r#"
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeKind {
    System,
    Light,
    Dark,
    HighContrast,
    RecordingFriendly,
}

impl ThemeKind {
    pub const ALL: [Self; 5] = [
        Self::System,
        Self::Light,
        Self::Dark,
        Self::HighContrast,
        Self::RecordingFriendly,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorValue {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl ColorValue {
    pub const fn from_rgba(rgba: u32) -> Self {
        Self {
            red: ((rgba >> 24) & 0xff) as u8,
            green: ((rgba >> 16) & 0xff) as u8,
            blue: ((rgba >> 8) & 0xff) as u8,
            alpha: (rgba & 0xff) as u8,
        }
    }

    pub const fn rgba(self) -> u32 {
        ((self.red as u32) << 24)
            | ((self.green as u32) << 16)
            | ((self.blue as u32) << 8)
            | self.alpha as u32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontFamilyValue(pub &'static str);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypographyValue { pub size: f32, pub line_height: f32, pub weight: u16 }
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpacingValue(pub f32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DimensionValue(pub f32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderValue(pub f32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadiusValue(pub f32);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElevationValue(pub u16);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowValue {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: ColorValue,
    pub visible: bool,
}
impl ShadowValue {
    pub const NONE: Self = Self {
        x: 0.0,
        y: 0.0,
        blur: 0.0,
        spread: 0.0,
        color: ColorValue::from_rgba(0),
        visible: false,
    };
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconValue(pub &'static str);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextValue(pub &'static str);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurationValue(pub u16);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EasingValue { pub x1: f32, pub y1: f32, pub x2: f32, pub y2: f32 }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZIndexValue(pub i16);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusKind { Attention, Busy, Done, Error, Idle, Offline, Orphaned, PermissionDenied }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusVisual {
    pub color: ColorValue,
    pub icon: IconValue,
    pub text: TextValue,
    pub shape: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesignTokens { theme: ThemeKind }

"#;

#[cfg(test)]
mod tokens_tests {
    use super::*;

    const MINIMAL: &str = r##"
version = 1
[[tokens]]
key = "color.status.idle"
category = "color"
description = "Idle"
allowed_use = "Status"
values = { system = "#000000", light = "#000000", dark = "#000000", high_contrast = "#FFFFFF", recording_friendly = "#000000" }
[[tokens]]
key = "icon.status.idle"
category = "icon"
description = "Idle"
allowed_use = "Status"
values = { system = "hollow_circle", light = "hollow_circle", dark = "hollow_circle", high_contrast = "hollow_circle", recording_friendly = "hollow_circle" }
[[tokens]]
key = "text.status.idle"
category = "text"
description = "Idle"
allowed_use = "Status"
values = { system = "status.idle", light = "status.idle", dark = "status.idle", high_contrast = "status.idle", recording_friendly = "status.idle" }
[[statuses]]
kind = "idle"
color = "color.status.idle"
icon = "icon.status.idle"
text = "text.status.idle"
shape = "hollow_circle"
"##;

    fn production_manifest() -> (TokenManifest, Vec<u8>) {
        let source = include_bytes!("../../../design/tokens.toml").to_vec();
        let manifest = parse_manifest(&source).expect("production token manifest should parse");
        (manifest, source)
    }

    #[test]
    fn tokens_production_manifest_is_complete_and_deterministic() {
        let (manifest, source) = production_manifest();
        let first = generate_artifacts(&manifest, &source).expect("generation should succeed");
        let second = generate_artifacts(&manifest, &source).expect("generation should repeat");
        assert_eq!(first, second);
        assert!(first.rust.contains("pub enum ThemeKind"));
        assert!(first.platform_contract_json.contains("recording_friendly"));
    }

    #[test]
    fn tokens_reject_unsupported_schema_and_oversized_input() {
        let unsupported = MINIMAL.replacen("version = 1", "version = 2", 1);
        assert!(
            parse_manifest(unsupported.as_bytes())
                .unwrap_err()
                .to_string()
                .contains("version")
        );
        let oversized = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert!(
            parse_manifest(&oversized)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
    }

    #[test]
    fn tokens_reject_missing_wrong_type_and_incomplete_values() {
        let (_, source) = production_manifest();
        let source = String::from_utf8(source).expect("manifest is UTF-8");
        let missing = source.replacen(
            "color = \"color.status.idle\"",
            "color = \"color.status.absent\"",
            1,
        );
        assert!(parse_manifest(missing.as_bytes()).is_err());
        let wrong = source.replacen(
            "color = \"color.status.idle\"",
            "color = \"icon.status.idle\"",
            1,
        );
        assert!(parse_manifest(wrong.as_bytes()).is_err());
        let incomplete = source.replacen(", recording_friendly = \"#17191D\"", "", 1);
        assert!(parse_manifest(incomplete.as_bytes()).is_err());
    }

    #[test]
    fn tokens_reject_reference_cycles() {
        let (manifest, source) = production_manifest();
        let key = "values = { system = \"#101318\"";
        let cyclic = String::from_utf8(source)
            .expect("manifest is UTF-8")
            .replacen(key, "values = { system = \"$color.bg.canvas\"", 1);
        let error = parse_manifest(cyclic.as_bytes()).unwrap_err();
        assert!(error.to_string().contains("cycle"));
        drop(manifest);
    }

    #[test]
    fn tokens_reject_fixed_status_shape_changes() {
        let (_, source) = production_manifest();
        let changed = String::from_utf8(source)
            .expect("manifest is UTF-8")
            .replacen("shape = \"hollow_circle\"", "shape = \"octagon\"", 1);
        assert!(
            parse_manifest(changed.as_bytes())
                .unwrap_err()
                .to_string()
                .contains("shape")
        );
    }
}
