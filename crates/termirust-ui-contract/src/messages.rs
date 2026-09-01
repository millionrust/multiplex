use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const SUPPORTED_SCHEMA_VERSION: u16 = 1;
const MAX_SCHEMA_BYTES: usize = 384 * 1024;
const MAX_CATALOG_BYTES: usize = 512 * 1024;
const MAX_MESSAGES: usize = 2_048;
const MAX_ARGS_PER_MESSAGE: usize = 16;
const MAX_MESSAGE_CHARS: usize = 8 * 1024;
const FSI: char = '\u{2068}';
const RLI: char = '\u{2067}';
const PDI: char = '\u{2069}';

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageError(String);

impl MessageError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for MessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MessageError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub enum ArgumentType {
    Text,
    Count,
    DateTime,
    ByteSize,
    KeyName,
    UserData,
}

impl ArgumentType {
    fn rust_type(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Count => "Count",
            Self::DateTime => "DateTime",
            Self::ByteSize => "ByteSize",
            Self::KeyName => "KeyName",
            Self::UserData => "UserData",
        }
    }

    fn value_variant(self) -> &'static str {
        self.rust_type()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    Sensitive,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BidiRule {
    None,
    Isolate,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemaArgument {
    name: String,
    #[serde(rename = "type")]
    kind: ArgumentType,
    sensitivity: Sensitivity,
    bidi: BidiRule,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemaMessage {
    id: String,
    context: String,
    description: String,
    #[serde(default)]
    args: Vec<SchemaArgument>,
    #[serde(default)]
    plural_arg: Option<String>,
    #[serde(default)]
    plural_variants: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageSchema {
    version: u16,
    default_locale: String,
    supported_locales: Vec<String>,
    messages: Vec<SchemaMessage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PatternPart {
    Text(String),
    Variable(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Pattern(Vec<PatternPart>);

#[derive(Clone, Debug, Eq, PartialEq)]
enum CatalogMessage {
    Pattern(Pattern),
    Select {
        selector: String,
        variants: BTreeMap<String, Pattern>,
        default: String,
    },
}

type Catalog = BTreeMap<String, CatalogMessage>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageGenerationArtifacts {
    pub rust: String,
    pub en_xa: String,
    pub ar_xb: String,
    pub source_hash: String,
}

pub fn load_message_schema(path: &Path) -> Result<(MessageSchema, Vec<u8>), MessageError> {
    let bytes = read_bounded(path, MAX_SCHEMA_BYTES, "message schema")?;
    let schema = parse_message_schema(&bytes)?;
    Ok((schema, bytes))
}

pub fn load_catalog(path: &Path) -> Result<Vec<u8>, MessageError> {
    read_bounded(path, MAX_CATALOG_BYTES, "message catalog")
}

fn read_bounded(path: &Path, cap: usize, kind: &str) -> Result<Vec<u8>, MessageError> {
    let metadata = fs::metadata(path).map_err(|error| {
        MessageError::new(format!("unable to inspect {}: {error}", path.display()))
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(MessageError::new(format!(
            "{} must be a regular {kind} file",
            path.display()
        )));
    }
    let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if length > cap {
        return Err(MessageError::new(format!(
            "{} is {length} bytes; {kind} files are limited to {cap} bytes",
            path.display()
        )));
    }
    fs::read(path)
        .map_err(|error| MessageError::new(format!("unable to read {}: {error}", path.display())))
}

pub fn parse_message_schema(bytes: &[u8]) -> Result<MessageSchema, MessageError> {
    if bytes.len() > MAX_SCHEMA_BYTES {
        return Err(MessageError::new(format!(
            "message schema is {} bytes; limit is {MAX_SCHEMA_BYTES}",
            bytes.len()
        )));
    }
    let source = std::str::from_utf8(bytes)
        .map_err(|error| MessageError::new(format!("message schema is not UTF-8: {error}")))?;
    let mut schema: MessageSchema = toml::from_str(source)
        .map_err(|error| MessageError::new(format!("invalid message schema TOML: {error}")))?;
    schema
        .messages
        .sort_by(|left, right| left.id.cmp(&right.id));
    validate_schema(&schema)?;
    Ok(schema)
}

fn validate_schema(schema: &MessageSchema) -> Result<(), MessageError> {
    if schema.version != SUPPORTED_SCHEMA_VERSION {
        return Err(MessageError::new(format!(
            "unsupported message schema version {}; expected {SUPPORTED_SCHEMA_VERSION}",
            schema.version
        )));
    }
    if schema.default_locale != "en-US" {
        return Err(MessageError::new("default_locale must be en-US"));
    }
    if schema.supported_locales != ["en-US", "en-XA", "ar-XB"] {
        return Err(MessageError::new(
            "supported_locales must be exactly en-US, en-XA, ar-XB",
        ));
    }
    if schema.messages.is_empty() || schema.messages.len() > MAX_MESSAGES {
        return Err(MessageError::new(format!(
            "message count {} is outside 1..={MAX_MESSAGES}",
            schema.messages.len()
        )));
    }

    let mut ids = BTreeSet::new();
    let mut rust_names = BTreeSet::new();
    for message in &schema.messages {
        if !is_canonical_id(&message.id) {
            return Err(MessageError::new(format!(
                "message id {:?} must use lowercase kebab-case",
                message.id
            )));
        }
        if !ids.insert(message.id.as_str()) {
            return Err(MessageError::new(format!(
                "duplicate message id {}",
                message.id
            )));
        }
        if !rust_names.insert(rust_name(&message.id)) {
            return Err(MessageError::new(format!(
                "message id {} collides after Rust name generation",
                message.id
            )));
        }
        if message.context.trim().is_empty() || message.description.trim().is_empty() {
            return Err(MessageError::new(format!(
                "message {} requires translator context and description",
                message.id
            )));
        }
        if message.args.len() > MAX_ARGS_PER_MESSAGE {
            return Err(MessageError::new(format!(
                "message {} has too many arguments",
                message.id
            )));
        }
        let mut args = BTreeSet::new();
        for argument in &message.args {
            if !is_identifier(&argument.name) || !args.insert(argument.name.as_str()) {
                return Err(MessageError::new(format!(
                    "message {} has invalid or duplicate argument {:?}",
                    message.id, argument.name
                )));
            }
            if argument.kind == ArgumentType::UserData
                && (argument.sensitivity != Sensitivity::Sensitive
                    || argument.bidi != BidiRule::Isolate)
            {
                return Err(MessageError::new(format!(
                    "message {} UserData argument {} must be sensitive and bidi-isolated",
                    message.id, argument.name
                )));
            }
        }
        match &message.plural_arg {
            Some(plural_arg) => {
                let Some(argument) = message.args.iter().find(|arg| arg.name == *plural_arg) else {
                    return Err(MessageError::new(format!(
                        "message {} plural_arg {} is not declared",
                        message.id, plural_arg
                    )));
                };
                if argument.kind != ArgumentType::Count {
                    return Err(MessageError::new(format!(
                        "message {} plural_arg {} must have Count type",
                        message.id, plural_arg
                    )));
                }
                validate_plural_variants(&message.id, &message.plural_variants)?;
            }
            None if !message.plural_variants.is_empty() => {
                return Err(MessageError::new(format!(
                    "message {} has plural_variants without plural_arg",
                    message.id
                )));
            }
            None => {}
        }
    }
    Ok(())
}

fn validate_plural_variants(id: &str, variants: &[String]) -> Result<(), MessageError> {
    if variants.is_empty() || !variants.iter().any(|variant| variant == "other") {
        return Err(MessageError::new(format!(
            "message {id} plural variants require an other branch"
        )));
    }
    let mut seen = BTreeSet::new();
    for variant in variants {
        if !matches!(
            variant.as_str(),
            "zero" | "one" | "two" | "few" | "many" | "other"
        ) || !seen.insert(variant)
        {
            return Err(MessageError::new(format!(
                "message {id} has invalid or duplicate plural variant {variant:?}"
            )));
        }
    }
    Ok(())
}

fn is_canonical_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.contains("--")
}

fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn parse_catalog(locale: &str, bytes: &[u8]) -> Result<Catalog, MessageError> {
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(MessageError::new(format!(
            "catalog {locale} is {} bytes; limit is {MAX_CATALOG_BYTES}",
            bytes.len()
        )));
    }
    let source = std::str::from_utf8(bytes)
        .map_err(|error| MessageError::new(format!("catalog {locale} is not UTF-8: {error}")))?;
    let lines: Vec<&str> = source.lines().collect();
    let mut catalog = BTreeMap::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if line.chars().next().is_some_and(char::is_whitespace) {
            return Err(MessageError::new(format!(
                "catalog {locale} line {index} has an orphan continuation"
            )));
        }
        let Some((raw_id, first_value)) = line.split_once('=') else {
            return Err(MessageError::new(format!(
                "catalog {locale} line {index} is not an id = value entry"
            )));
        };
        let id = raw_id.trim();
        if !is_canonical_id(id) {
            return Err(MessageError::new(format!(
                "catalog {locale} line {index} has invalid id {id:?}"
            )));
        }
        let mut value_lines = vec![first_value.trim().to_string()];
        while index < lines.len() && lines[index].chars().next().is_some_and(char::is_whitespace) {
            value_lines.push(lines[index].trim().to_string());
            index += 1;
        }
        let message = parse_catalog_message(locale, id, &value_lines)?;
        if catalog.insert(id.to_string(), message).is_some() {
            return Err(MessageError::new(format!(
                "catalog {locale} contains duplicate id {id}"
            )));
        }
    }
    if catalog.is_empty() {
        return Err(MessageError::new(format!("catalog {locale} is empty")));
    }
    Ok(catalog)
}

fn parse_catalog_message(
    locale: &str,
    id: &str,
    lines: &[String],
) -> Result<CatalogMessage, MessageError> {
    let joined_chars: usize = lines.iter().map(String::len).sum();
    if joined_chars > MAX_MESSAGE_CHARS {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} exceeds {MAX_MESSAGE_CHARS} characters"
        )));
    }
    if lines.len() > 1 || lines.first().is_some_and(|line| line.contains("->")) {
        parse_select(locale, id, lines)
    } else {
        let value = lines.first().map(String::as_str).unwrap_or_default();
        Ok(CatalogMessage::Pattern(parse_pattern(locale, id, value)?))
    }
}

fn parse_select(locale: &str, id: &str, lines: &[String]) -> Result<CatalogMessage, MessageError> {
    let Some(first) = lines.first() else {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} is empty"
        )));
    };
    let Some(selector) = first
        .strip_prefix("{ $")
        .and_then(|rest| rest.strip_suffix(" ->"))
    else {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} has invalid select header"
        )));
    };
    if !is_identifier(selector) || lines.last().map(String::as_str) != Some("}") {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} has malformed select syntax"
        )));
    }
    let mut variants = BTreeMap::new();
    let mut default = None;
    for line in &lines[1..lines.len() - 1] {
        let (is_default, rest) = if let Some(rest) = line.strip_prefix("*[") {
            (true, rest)
        } else if let Some(rest) = line.strip_prefix('[') {
            (false, rest)
        } else {
            return Err(MessageError::new(format!(
                "catalog {locale} message {id} has invalid variant {line:?}"
            )));
        };
        let Some((key, raw_pattern)) = rest.split_once("] ") else {
            return Err(MessageError::new(format!(
                "catalog {locale} message {id} has malformed variant {line:?}"
            )));
        };
        if !matches!(key, "zero" | "one" | "two" | "few" | "many" | "other") {
            return Err(MessageError::new(format!(
                "catalog {locale} message {id} has invalid variant {key:?}"
            )));
        }
        let pattern = parse_pattern(locale, id, raw_pattern)?;
        if variants.insert(key.to_string(), pattern).is_some() {
            return Err(MessageError::new(format!(
                "catalog {locale} message {id} repeats variant {key}"
            )));
        }
        if is_default && default.replace(key.to_string()).is_some() {
            return Err(MessageError::new(format!(
                "catalog {locale} message {id} has multiple default variants"
            )));
        }
    }
    let default = default.ok_or_else(|| {
        MessageError::new(format!(
            "catalog {locale} message {id} requires one default variant"
        ))
    })?;
    Ok(CatalogMessage::Select {
        selector: selector.to_string(),
        variants,
        default,
    })
}

fn parse_pattern(locale: &str, id: &str, source: &str) -> Result<Pattern, MessageError> {
    if source.trim().is_empty() {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} has an empty pattern"
        )));
    }
    if source.contains('<') || source.contains('>') {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} contains forbidden markup"
        )));
    }
    if ['⌘', '⇧', '⌥', '⌃']
        .iter()
        .any(|glyph| source.contains(*glyph))
    {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} embeds a platform shortcut glyph; use a KeyName argument"
        )));
    }

    let mut parts = Vec::new();
    let mut remaining = source;
    while let Some(start) = remaining.find('{') {
        if start > 0 {
            parts.push(PatternPart::Text(remaining[..start].to_string()));
        }
        let after_open = &remaining[start..];
        let Some(end) = after_open.find('}') else {
            return Err(MessageError::new(format!(
                "catalog {locale} message {id} has an unmatched opening brace"
            )));
        };
        let placeable = &after_open[..=end];
        let Some(variable) = placeable
            .strip_prefix("{ $")
            .and_then(|value| value.strip_suffix(" }"))
        else {
            return Err(MessageError::new(format!(
                "catalog {locale} message {id} has unsupported placeable {placeable:?}"
            )));
        };
        if !is_identifier(variable) {
            return Err(MessageError::new(format!(
                "catalog {locale} message {id} has invalid variable {variable:?}"
            )));
        }
        parts.push(PatternPart::Variable(variable.to_string()));
        remaining = &after_open[end + 1..];
    }
    if remaining.contains('}') {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} has an unmatched closing brace"
        )));
    }
    if !remaining.is_empty() {
        parts.push(PatternPart::Text(remaining.to_string()));
    }
    if !parts.iter().any(
        |part| matches!(part, PatternPart::Text(text) if text.chars().any(char::is_alphabetic)),
    ) {
        return Err(MessageError::new(format!(
            "catalog {locale} message {id} is a placeholder-only label"
        )));
    }
    Ok(Pattern(parts))
}

pub fn generate_message_artifacts(
    schema: &MessageSchema,
    schema_source: &[u8],
    english_source: &[u8],
) -> Result<MessageGenerationArtifacts, MessageError> {
    validate_schema(schema)?;
    let english = parse_catalog("en-US", english_source)?;
    validate_catalog_against_schema(schema, "en-US", &english, None)?;
    let en_xa_catalog = pseudo_catalog(&english, PseudoKind::Expansion);
    let ar_xb_catalog = pseudo_catalog(&english, PseudoKind::Bidi);
    validate_catalog_against_schema(schema, "en-XA", &en_xa_catalog, Some(&english))?;
    validate_catalog_against_schema(schema, "ar-XB", &ar_xb_catalog, Some(&english))?;
    validate_expansion(&english, &en_xa_catalog)?;

    let mut hasher = Sha256::new();
    hasher.update(schema_source);
    hasher.update([0]);
    hasher.update(english_source);
    let source_hash = format!("{:x}", hasher.finalize());
    Ok(MessageGenerationArtifacts {
        rust: generate_rust(schema, &source_hash),
        en_xa: serialize_catalog("en-XA", &en_xa_catalog),
        ar_xb: serialize_catalog("ar-XB", &ar_xb_catalog),
        source_hash,
    })
}

pub fn validate_catalog_set(
    schema: &MessageSchema,
    english_source: &[u8],
    en_xa_source: &[u8],
    ar_xb_source: &[u8],
) -> Result<(), MessageError> {
    let english = parse_catalog("en-US", english_source)?;
    let en_xa = parse_catalog("en-XA", en_xa_source)?;
    let ar_xb = parse_catalog("ar-XB", ar_xb_source)?;
    validate_catalog_against_schema(schema, "en-US", &english, None)?;
    validate_catalog_against_schema(schema, "en-XA", &en_xa, Some(&english))?;
    validate_catalog_against_schema(schema, "ar-XB", &ar_xb, Some(&english))?;
    validate_expansion(&english, &en_xa)
}

fn validate_catalog_against_schema(
    schema: &MessageSchema,
    locale: &str,
    catalog: &Catalog,
    english: Option<&Catalog>,
) -> Result<(), MessageError> {
    let schema_ids: BTreeSet<&str> = schema
        .messages
        .iter()
        .map(|message| message.id.as_str())
        .collect();
    let catalog_ids: BTreeSet<&str> = catalog.keys().map(String::as_str).collect();
    if schema_ids != catalog_ids {
        let missing: Vec<_> = schema_ids.difference(&catalog_ids).copied().collect();
        let extra: Vec<_> = catalog_ids.difference(&schema_ids).copied().collect();
        return Err(MessageError::new(format!(
            "catalog {locale} ID mismatch; missing={missing:?}, extra={extra:?}"
        )));
    }
    for definition in &schema.messages {
        let message = &catalog[&definition.id];
        validate_message_shape(locale, definition, message)?;
        if let Some(english) = english {
            let english_message = &english[&definition.id];
            if message_signature(message) != message_signature(english_message) {
                return Err(MessageError::new(format!(
                    "catalog {locale} message {} does not preserve English variables/plural branches",
                    definition.id
                )));
            }
        }
    }
    Ok(())
}

fn validate_message_shape(
    locale: &str,
    definition: &SchemaMessage,
    message: &CatalogMessage,
) -> Result<(), MessageError> {
    let expected_args: BTreeSet<String> =
        definition.args.iter().map(|arg| arg.name.clone()).collect();
    let used_args = variables_in_message(message);
    if expected_args != used_args {
        return Err(MessageError::new(format!(
            "catalog {locale} message {} argument mismatch; expected={expected_args:?}, used={used_args:?}",
            definition.id
        )));
    }
    match (&definition.plural_arg, message) {
        (None, CatalogMessage::Pattern(_)) => Ok(()),
        (
            Some(expected),
            CatalogMessage::Select {
                selector,
                variants,
                default,
            },
        ) => {
            let expected_variants: BTreeSet<&str> = definition
                .plural_variants
                .iter()
                .map(String::as_str)
                .collect();
            let actual_variants: BTreeSet<&str> = variants.keys().map(String::as_str).collect();
            if expected != selector || expected_variants != actual_variants || default != "other" {
                return Err(MessageError::new(format!(
                    "catalog {locale} message {} has wrong plural selector, branches, or default",
                    definition.id
                )));
            }
            Ok(())
        }
        _ => Err(MessageError::new(format!(
            "catalog {locale} message {} pattern/plural shape does not match schema",
            definition.id
        ))),
    }
}

fn variables_in_message(message: &CatalogMessage) -> BTreeSet<String> {
    let mut variables = BTreeSet::new();
    match message {
        CatalogMessage::Pattern(pattern) => {
            variables.extend(variables_in_pattern(pattern));
        }
        CatalogMessage::Select {
            selector, variants, ..
        } => {
            variables.insert(selector.clone());
            for pattern in variants.values() {
                variables.extend(variables_in_pattern(pattern));
            }
        }
    }
    variables
}

fn message_signature(message: &CatalogMessage) -> String {
    match message {
        CatalogMessage::Pattern(pattern) => format!("pattern:{:?}", variables_in_pattern(pattern)),
        CatalogMessage::Select {
            selector,
            variants,
            default,
        } => {
            let branches = variants
                .iter()
                .map(|(key, pattern)| format!("{key}:{:?}", variables_in_pattern(pattern)))
                .collect::<Vec<_>>()
                .join("|");
            format!("select:{selector}:{default}:{branches}")
        }
    }
}

fn variables_in_pattern(pattern: &Pattern) -> BTreeSet<String> {
    pattern
        .0
        .iter()
        .filter_map(|part| match part {
            PatternPart::Variable(variable) => Some(variable.clone()),
            PatternPart::Text(_) => None,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum PseudoKind {
    Expansion,
    Bidi,
}

fn pseudo_catalog(english: &Catalog, kind: PseudoKind) -> Catalog {
    english
        .iter()
        .map(|(id, message)| (id.clone(), pseudo_message(message, kind)))
        .collect()
}

fn pseudo_message(message: &CatalogMessage, kind: PseudoKind) -> CatalogMessage {
    match message {
        CatalogMessage::Pattern(pattern) => CatalogMessage::Pattern(pseudo_pattern(pattern, kind)),
        CatalogMessage::Select {
            selector,
            variants,
            default,
        } => CatalogMessage::Select {
            selector: selector.clone(),
            variants: variants
                .iter()
                .map(|(key, pattern)| (key.clone(), pseudo_pattern(pattern, kind)))
                .collect(),
            default: default.clone(),
        },
    }
}

fn pseudo_pattern(pattern: &Pattern, kind: PseudoKind) -> Pattern {
    Pattern(
        pattern
            .0
            .iter()
            .map(|part| match part {
                PatternPart::Variable(variable) => PatternPart::Variable(variable.clone()),
                PatternPart::Text(text) => PatternPart::Text(match kind {
                    PseudoKind::Expansion => expansion_text(text),
                    PseudoKind::Bidi => bidi_text(text),
                }),
            })
            .collect(),
    )
}

fn expansion_text(source: &str) -> String {
    if source.is_empty() {
        return String::new();
    }
    let accented: String = source
        .chars()
        .map(|character| match character {
            'a' => 'å',
            'A' => 'Å',
            'b' => 'ƀ',
            'B' => 'Ɓ',
            'c' => 'ç',
            'C' => 'Ç',
            'd' => 'ð',
            'D' => 'Ð',
            'e' => 'é',
            'E' => 'É',
            'f' => 'ƒ',
            'F' => 'Ƒ',
            'g' => 'ĝ',
            'G' => 'Ĝ',
            'h' => 'ĥ',
            'H' => 'Ĥ',
            'i' => 'î',
            'I' => 'Î',
            'j' => 'ĵ',
            'J' => 'Ĵ',
            'k' => 'ķ',
            'K' => 'Ķ',
            'l' => 'ļ',
            'L' => 'Ļ',
            'm' => 'ɱ',
            'M' => 'Ṁ',
            'n' => 'ñ',
            'N' => 'Ñ',
            'o' => 'ø',
            'O' => 'Ø',
            'p' => 'þ',
            'P' => 'Þ',
            'q' => 'ʠ',
            'Q' => 'Ɋ',
            'r' => 'ŕ',
            'R' => 'Ŕ',
            's' => 'š',
            'S' => 'Š',
            't' => 'ţ',
            'T' => 'Ţ',
            'u' => 'û',
            'U' => 'Û',
            'v' => 'ṽ',
            'V' => 'Ṽ',
            'w' => 'ŵ',
            'W' => 'Ŵ',
            'x' => 'ẋ',
            'X' => 'Ẋ',
            'y' => 'ý',
            'Y' => 'Ý',
            'z' => 'ž',
            'Z' => 'Ž',
            other => other,
        })
        .collect();
    let source_len = source.chars().count();
    let target_len = (source_len * 14).div_ceil(10);
    let pad = target_len.saturating_sub(accented.chars().count() + 2);
    format!("⟦{accented}{}⟧", "~".repeat(pad))
}

fn bidi_text(source: &str) -> String {
    if source.is_empty() {
        return String::new();
    }
    let mirrored: String = source
        .chars()
        .rev()
        .map(|character| match character {
            '(' => ')',
            ')' => '(',
            '[' => ']',
            ']' => '[',
            '<' => '>',
            '>' => '<',
            other => other,
        })
        .collect();
    format!("{RLI}{mirrored}{PDI}")
}

fn validate_expansion(english: &Catalog, pseudo: &Catalog) -> Result<(), MessageError> {
    for (id, english_message) in english {
        let english_count = literal_char_count(english_message);
        let pseudo_count = literal_char_count(&pseudo[id]);
        if pseudo_count * 10 < english_count * 14 {
            return Err(MessageError::new(format!(
                "en-XA message {id} expands {english_count} characters to only {pseudo_count}; require at least 40%"
            )));
        }
    }
    Ok(())
}

fn literal_char_count(message: &CatalogMessage) -> usize {
    let count_pattern = |pattern: &Pattern| {
        pattern
            .0
            .iter()
            .map(|part| match part {
                PatternPart::Text(text) => text.chars().count(),
                PatternPart::Variable(_) => 0,
            })
            .sum::<usize>()
    };
    match message {
        CatalogMessage::Pattern(pattern) => count_pattern(pattern),
        CatalogMessage::Select { variants, .. } => variants.values().map(count_pattern).sum(),
    }
}

fn serialize_catalog(locale: &str, catalog: &Catalog) -> String {
    let mut output =
        format!("# Generated {locale} pseudo-catalog. Do not edit; run generate-messages.\n");
    for (id, message) in catalog {
        match message {
            CatalogMessage::Pattern(pattern) => {
                output.push_str(&format!("{id} = {}\n", serialize_pattern(pattern)));
            }
            CatalogMessage::Select {
                selector,
                variants,
                default,
            } => {
                output.push_str(&format!("{id} = {{ ${selector} ->\n"));
                for (key, pattern) in variants {
                    let marker = if key == default { "*" } else { " " };
                    output.push_str(&format!(
                        "   {marker}[{key}] {}\n",
                        serialize_pattern(pattern)
                    ));
                }
                output.push_str("    }\n");
            }
        }
    }
    output
}

fn serialize_pattern(pattern: &Pattern) -> String {
    pattern
        .0
        .iter()
        .map(|part| match part {
            PatternPart::Text(text) => text.clone(),
            PatternPart::Variable(variable) => format!("{{ ${variable} }}"),
        })
        .collect()
}

fn generate_rust(schema: &MessageSchema, source_hash: &str) -> String {
    let mut output = format!(
        "// @generated by generate-messages. Do not edit.\n\
         // Source SHA-256: {source_hash}\n\n\
         use super::{{ArgSpec, Argument, ArgumentType, ArgumentValue, BidiRule, ByteSize, Count, DateTime, KeyName, MessageArguments, MessageSpec, Sensitivity, Text, UserData}};\n\n\
         pub const MESSAGE_SCHEMA_VERSION: u16 = {};\n\
         pub const MESSAGE_SOURCE_SHA256: &str = {:?};\n\n\
         #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]\n\
         pub enum MessageId {{\n",
        schema.version, source_hash
    );
    for message in &schema.messages {
        output.push_str(&format!("    {},\n", rust_name(&message.id)));
    }
    output.push_str("}\n\nimpl MessageId {\n    pub const ALL: &'static [Self] = &[\n");
    for message in &schema.messages {
        output.push_str(&format!("        Self::{},\n", rust_name(&message.id)));
    }
    output
        .push_str("    ];\n\n    pub const fn key(self) -> &'static str {\n        match self {\n");
    for message in &schema.messages {
        output.push_str(&format!(
            "            Self::{} => {:?},\n",
            rust_name(&message.id),
            message.id
        ));
    }
    output.push_str("        }\n    }\n}\n\n");

    for message in &schema.messages {
        let name = format!("{}Args", rust_name(&message.id));
        output.push_str("#[derive(Clone, Debug, Eq, PartialEq)]\n");
        if message.args.is_empty() {
            output.push_str(&format!("pub struct {name};\n\nimpl {name} {{\n    pub const fn new() -> Self {{ Self }}\n}}\n\n"));
            output.push_str(&format!(
                "impl Default for {name} {{\n    fn default() -> Self {{ Self::new() }}\n}}\n\n"
            ));
        } else {
            output.push_str(&format!("pub struct {name} {{\n"));
            for argument in &message.args {
                output.push_str(&format!(
                    "    {}: {},\n",
                    argument.name,
                    argument.kind.rust_type()
                ));
            }
            output.push_str(&format!("}}\n\nimpl {name} {{\n    pub fn new("));
            for (index, argument) in message.args.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                output.push_str(&format!("{}: {}", argument.name, argument.kind.rust_type()));
            }
            output.push_str(") -> Self { Self { ");
            for argument in &message.args {
                output.push_str(&format!("{}, ", argument.name));
            }
            output.push_str("} }\n}\n\n");
        }
        output.push_str(&format!(
            "impl MessageArguments for {name} {{\n    const ID: MessageId = MessageId::{};\n\n    fn arguments(&self) -> Vec<Argument<'_>> {{\n",
            rust_name(&message.id)
        ));
        if message.args.is_empty() {
            output.push_str("        Vec::new()\n");
        } else {
            output.push_str("        vec![\n");
            for argument in &message.args {
                let value = match argument.kind {
                    ArgumentType::Text | ArgumentType::KeyName | ArgumentType::UserData => format!(
                        "ArgumentValue::{}(self.{}.as_str())",
                        argument.kind.value_variant(),
                        argument.name
                    ),
                    ArgumentType::Count | ArgumentType::DateTime | ArgumentType::ByteSize => {
                        format!(
                            "ArgumentValue::{}(self.{}.0)",
                            argument.kind.value_variant(),
                            argument.name
                        )
                    }
                };
                output.push_str(&format!(
                    "            Argument::new({:?}, {value}),\n",
                    argument.name
                ));
            }
            output.push_str("        ]\n");
        }
        output.push_str("    }\n}\n\n");
    }

    output.push_str("pub const MESSAGE_SPECS: &[MessageSpec] = &[\n");
    for message in &schema.messages {
        output.push_str(&format!(
            "    MessageSpec {{ id: MessageId::{}, key: {:?}, args: &[",
            rust_name(&message.id),
            message.id
        ));
        for argument in &message.args {
            output.push_str(&format!(
                "ArgSpec {{ name: {:?}, kind: ArgumentType::{}, sensitivity: Sensitivity::{}, bidi: BidiRule::{} }}, ",
                argument.name,
                argument.kind.rust_type(),
                match argument.sensitivity { Sensitivity::Public => "Public", Sensitivity::Sensitive => "Sensitive" },
                match argument.bidi { BidiRule::None => "None", BidiRule::Isolate => "Isolate" },
            ));
        }
        output.push_str(&format!(
            "], plural_arg: {} }},\n",
            message
                .plural_arg
                .as_deref()
                .map(|value| format!("Some({value:?})"))
                .unwrap_or_else(|| "None".to_string())
        ));
    }
    output.push_str("];\n");
    output
}

fn rust_name(id: &str) -> String {
    id.split('-')
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArgSpec {
    pub name: &'static str,
    pub kind: ArgumentType,
    pub sensitivity: Sensitivity,
    pub bidi: BidiRule,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageSpec {
    pub id: MessageId,
    pub key: &'static str,
    pub args: &'static [ArgSpec],
    pub plural_arg: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Text(String);

impl Text {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Count(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DateTime(pub i64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteSize(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyName(String);

impl KeyName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserData(String);

impl UserData {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ArgumentValue<'a> {
    Text(&'a str),
    Count(u64),
    DateTime(i64),
    ByteSize(u64),
    KeyName(&'a str),
    UserData(&'a str),
}

impl ArgumentValue<'_> {
    fn kind(self) -> ArgumentType {
        match self {
            Self::Text(_) => ArgumentType::Text,
            Self::Count(_) => ArgumentType::Count,
            Self::DateTime(_) => ArgumentType::DateTime,
            Self::ByteSize(_) => ArgumentType::ByteSize,
            Self::KeyName(_) => ArgumentType::KeyName,
            Self::UserData(_) => ArgumentType::UserData,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Argument<'a> {
    name: &'static str,
    value: ArgumentValue<'a>,
}

impl<'a> Argument<'a> {
    pub const fn new(name: &'static str, value: ArgumentValue<'a>) -> Self {
        Self { name, value }
    }
}

pub trait MessageArguments {
    const ID: MessageId;
    fn arguments(&self) -> Vec<Argument<'_>>;
}

#[path = "generated_messages.rs"]
mod generated_messages;
pub use generated_messages::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Locale {
    EnUs,
    EnXa,
    ArXb,
}

impl Locale {
    pub const ALL: [Self; 3] = [Self::EnUs, Self::EnXa, Self::ArXb];

    pub const fn tag(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::EnXa => "en-XA",
            Self::ArXb => "ar-XB",
        }
    }

    pub const fn direction(self) -> TextDirection {
        match self {
            Self::EnUs | Self::EnXa => TextDirection::LeftToRight,
            Self::ArXb => TextDirection::RightToLeft,
        }
    }

    pub fn negotiate(requested: &str) -> Self {
        let normalized = requested.trim().replace('_', "-").to_ascii_lowercase();
        match normalized.as_str() {
            "en-xa" => Self::EnXa,
            "ar-xb" => Self::ArXb,
            "en" | "en-us" => Self::EnUs,
            _ if normalized.starts_with("en-") => Self::EnUs,
            _ => Self::EnUs,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextDirection {
    LeftToRight,
    RightToLeft,
}

struct EmbeddedCatalogs {
    english: Catalog,
    en_xa: Catalog,
    ar_xb: Catalog,
}

static EMBEDDED_CATALOGS: OnceLock<Result<EmbeddedCatalogs, MessageError>> = OnceLock::new();

fn embedded_catalogs() -> Result<&'static EmbeddedCatalogs, MessageError> {
    EMBEDDED_CATALOGS
        .get_or_init(|| {
            let schema = parse_message_schema(include_bytes!("../../../locales/schema.toml"))?;
            let english_source = include_bytes!("../../../locales/en-US.ftl");
            let en_xa_source = include_bytes!("../../../locales/en-XA.ftl");
            let ar_xb_source = include_bytes!("../../../locales/ar-XB.ftl");
            validate_catalog_set(&schema, english_source, en_xa_source, ar_xb_source)?;
            Ok(EmbeddedCatalogs {
                english: parse_catalog("en-US", english_source)?,
                en_xa: parse_catalog("en-XA", en_xa_source)?,
                ar_xb: parse_catalog("ar-XB", ar_xb_source)?,
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Localizer {
    locale: Locale,
}

impl Localizer {
    pub const fn english() -> Self {
        Self {
            locale: Locale::EnUs,
        }
    }

    pub fn try_new(requested_locale: &str) -> Result<Self, MessageError> {
        embedded_catalogs()?;
        Ok(Self {
            locale: Locale::negotiate(requested_locale),
        })
    }

    pub const fn locale(self) -> Locale {
        self.locale
    }

    pub fn format<M: MessageArguments>(&self, message: &M) -> String {
        let id = M::ID;
        let arguments = message.arguments();
        match self.try_format(id, &arguments) {
            Ok(formatted) => formatted,
            Err(_) => {
                eprintln!(
                    "[localization] formatting failed for MessageId={}",
                    id.key()
                );
                format!("[{}]", id.key())
            }
        }
    }

    pub fn format_static(&self, id: MessageId) -> Result<String, MessageError> {
        let spec = MESSAGE_SPECS
            .iter()
            .find(|spec| spec.id == id)
            .ok_or_else(|| MessageError::new(format!("unknown MessageId {}", id.key())))?;
        if !spec.args.is_empty() {
            return Err(MessageError::new(format!(
                "MessageId {} requires typed arguments",
                id.key()
            )));
        }
        self.try_format(id, &[])
    }

    pub fn format_arguments(&self, id: MessageId, arguments: &[Argument<'_>]) -> String {
        match self.try_format(id, arguments) {
            Ok(formatted) => formatted,
            Err(_) => {
                eprintln!(
                    "[localization] formatting failed for MessageId={}",
                    id.key()
                );
                format!("[{}]", id.key())
            }
        }
    }

    fn try_format(
        &self,
        id: MessageId,
        arguments: &[Argument<'_>],
    ) -> Result<String, MessageError> {
        let catalogs = embedded_catalogs()?;
        let selected = match self.locale {
            Locale::EnUs => &catalogs.english,
            Locale::EnXa => &catalogs.en_xa,
            Locale::ArXb => &catalogs.ar_xb,
        };
        match format_from_catalog(self.locale, selected, id, arguments) {
            Ok(formatted) => Ok(formatted),
            Err(_) if self.locale != Locale::EnUs => {
                format_from_catalog(Locale::EnUs, &catalogs.english, id, arguments)
            }
            Err(error) => Err(error),
        }
    }
}

fn format_from_catalog(
    locale: Locale,
    catalog: &Catalog,
    id: MessageId,
    arguments: &[Argument<'_>],
) -> Result<String, MessageError> {
    let spec = MESSAGE_SPECS
        .iter()
        .find(|spec| spec.id == id)
        .ok_or_else(|| MessageError::new(format!("unknown MessageId {}", id.key())))?;
    if arguments.len() != spec.args.len() {
        return Err(MessageError::new(format!(
            "MessageId {} received the wrong argument count",
            id.key()
        )));
    }
    let mut values = BTreeMap::new();
    for argument in arguments {
        if values.insert(argument.name, argument.value).is_some() {
            return Err(MessageError::new(format!(
                "MessageId {} received duplicate argument {}",
                id.key(),
                argument.name
            )));
        }
    }
    for expected in spec.args {
        let Some(actual) = values.get(expected.name) else {
            return Err(MessageError::new(format!(
                "MessageId {} is missing argument {}",
                id.key(),
                expected.name
            )));
        };
        if actual.kind() != expected.kind {
            return Err(MessageError::new(format!(
                "MessageId {} argument {} has the wrong type",
                id.key(),
                expected.name
            )));
        }
    }
    let message = catalog
        .get(id.key())
        .ok_or_else(|| MessageError::new(format!("catalog is missing MessageId {}", id.key())))?;
    match message {
        CatalogMessage::Pattern(pattern) => format_pattern(locale, pattern, &values),
        CatalogMessage::Select {
            selector,
            variants,
            default,
        } => {
            let Some(ArgumentValue::Count(count)) = values.get(selector.as_str()) else {
                return Err(MessageError::new(format!(
                    "MessageId {} selector has the wrong type",
                    id.key()
                )));
            };
            let branch = if *count == 0 && variants.contains_key("zero") {
                "zero"
            } else if *count == 1 && variants.contains_key("one") {
                "one"
            } else if *count > 1 && variants.contains_key("many") {
                "many"
            } else {
                default
            };
            format_pattern(locale, &variants[branch], &values)
        }
    }
}

fn format_pattern(
    locale: Locale,
    pattern: &Pattern,
    values: &BTreeMap<&str, ArgumentValue<'_>>,
) -> Result<String, MessageError> {
    let mut output = String::new();
    for part in &pattern.0 {
        match part {
            PatternPart::Text(text) => output.push_str(text),
            PatternPart::Variable(variable) => {
                let value = values.get(variable.as_str()).ok_or_else(|| {
                    MessageError::new(format!("missing catalog variable {variable}"))
                })?;
                output.push_str(&format_value(locale, *value));
            }
        }
    }
    Ok(output)
}

fn format_value(locale: Locale, value: ArgumentValue<'_>) -> String {
    match value {
        ArgumentValue::Text(value) => value.to_string(),
        ArgumentValue::Count(value) => format_count(locale, value),
        ArgumentValue::DateTime(value) => format_datetime(locale, value),
        ArgumentValue::ByteSize(value) => format_bytes(locale, value),
        ArgumentValue::KeyName(value) | ArgumentValue::UserData(value) => {
            format!("{FSI}{value}{PDI}")
        }
    }
}

fn format_count(locale: Locale, value: u64) -> String {
    let separator = match locale {
        Locale::ArXb => '٬',
        Locale::EnUs | Locale::EnXa => ',',
    };
    let digits = value.to_string();
    let mut output = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(separator);
        }
        output.push(character);
    }
    output
}

fn format_bytes(locale: Locale, value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit < UNITS.len() - 1 {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} {}", format_count(locale, value), UNITS[unit]);
    }
    let mut number = format!("{amount:.1}");
    if locale == Locale::ArXb {
        number = number.replace('.', "٫");
    }
    format!("{number} {}", UNITS[unit])
}

fn format_datetime(locale: Locale, unix_millis: i64) -> String {
    let days = unix_millis.div_euclid(86_400_000);
    let seconds = unix_millis.rem_euclid(86_400_000) / 1000;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3600;
    let minute = (seconds % 3600) / 60;
    match locale {
        Locale::EnUs | Locale::EnXa => {
            format!("{month:02}/{day:02}/{year:04} {hour:02}:{minute:02} UTC")
        }
        Locale::ArXb => {
            format!("{year:04}/{month:02}/{day:02} {hour:02}:{minute:02} UTC")
        }
    }
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_piece = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_piece + 2) / 5 + 1;
    let month = month_piece + if month_piece < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod localization_tests {
    use super::*;

    #[test]
    fn localization_generation_is_deterministic_and_complete() {
        let schema_source = include_bytes!("../../../locales/schema.toml");
        let english = include_bytes!("../../../locales/en-US.ftl");
        let schema = parse_message_schema(schema_source).unwrap();
        let first = generate_message_artifacts(&schema, schema_source, english).unwrap();
        let second = generate_message_artifacts(&schema, schema_source, english).unwrap();
        assert_eq!(first, second);
        assert_eq!(MessageId::ALL.len(), schema.messages.len());
        assert!(first.en_xa.contains("⟦"));
        assert!(first.ar_xb.contains(RLI));
    }

    #[test]
    fn localization_plural_zero_one_many_and_typed_values_format() {
        let localizer = Localizer::try_new("en-US").unwrap();
        assert_eq!(
            localizer.format(&SessionCountArgs::new(Count(0))),
            "No active sessions"
        );
        assert_eq!(
            localizer.format(&SessionCountArgs::new(Count(1))),
            "One active session"
        );
        assert_eq!(
            localizer.format(&SessionCountArgs::new(Count(12_345))),
            "12,345 active sessions"
        );
        assert_eq!(
            localizer.format(&TransferSummaryArgs::new(Count(1), ByteSize(1536))),
            "One file transferred (1.5 KB)"
        );
    }

    #[test]
    fn localization_user_data_and_key_names_are_directionally_isolated() {
        for locale in Locale::ALL {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            let host = localizer.format(&StatusConnectingArgs::new(UserData::new("prod/שרת")));
            assert!(host.contains("\u{2068}prod/שרת\u{2069}"));
            let shortcut = localizer.format(&ShortcutHintArgs::new(KeyName::new("Cmd+K")));
            assert!(shortcut.contains("\u{2068}Cmd+K\u{2069}"));
        }
        assert_eq!(Locale::ArXb.direction(), TextDirection::RightToLeft);
    }

    #[test]
    fn localization_negotiation_never_selects_a_pseudo_locale_by_language_fallback() {
        assert_eq!(Locale::negotiate("en-XA"), Locale::EnXa);
        assert_eq!(Locale::negotiate("ar-XB"), Locale::ArXb);
        assert_eq!(Locale::negotiate("en-GB"), Locale::EnUs);
        assert_eq!(Locale::negotiate("ar"), Locale::EnUs);
        assert_eq!(Locale::negotiate("unknown"), Locale::EnUs);
    }

    #[test]
    fn localization_rejects_hostile_markup_braces_and_shortcut_glyphs() {
        for hostile in [
            "probe = <script>alert(1)</script>",
            "probe = Hello { $name",
            "probe = Press ⌘K",
        ] {
            assert!(parse_catalog("en-US", hostile.as_bytes()).is_err());
        }
    }

    #[test]
    fn localization_rejects_missing_and_extra_catalog_variables() {
        let schema_source = include_bytes!("../../../locales/schema.toml");
        let schema = parse_message_schema(schema_source).unwrap();
        let english = std::str::from_utf8(include_bytes!("../../../locales/en-US.ftl")).unwrap();
        let missing = english.replace("Connecting to { $host }…", "Connecting…");
        assert!(generate_message_artifacts(&schema, schema_source, missing.as_bytes()).is_err());
        let extra = english.replace("common-save = Save", "common-save = Save { $unknown }");
        assert!(generate_message_artifacts(&schema, schema_source, extra.as_bytes()).is_err());
    }

    #[test]
    fn localization_rejects_placeholder_only_labels() {
        assert!(parse_catalog("en-US", b"probe = { $value }").is_err());
    }

    #[test]
    fn localization_formats_date_bytes_and_long_user_paths_without_mutation() {
        let localizer = Localizer::try_new("en-US").unwrap();
        assert_eq!(
            localizer.format(&LastUpdatedArgs::new(DateTime(0))),
            "Last updated 01/01/1970 00:00 UTC"
        );
        let path = "/Users/example/a very long directory/文件/שלום/project";
        let formatted = localizer.format(&PathUnavailableArgs::new(UserData::new(path)));
        assert!(formatted.contains(path));
    }

    #[test]
    fn localization_runtime_format_errors_use_visible_id_fallback() {
        struct Broken;
        impl MessageArguments for Broken {
            const ID: MessageId = MessageId::StatusConnecting;
            fn arguments(&self) -> Vec<Argument<'_>> {
                Vec::new()
            }
        }
        assert_eq!(
            Localizer::try_new("en-US").unwrap().format(&Broken),
            "[status-connecting]"
        );

        struct WrongType;
        impl MessageArguments for WrongType {
            const ID: MessageId = MessageId::StatusConnecting;
            fn arguments(&self) -> Vec<Argument<'_>> {
                vec![Argument::new("host", ArgumentValue::Text("not UserData"))]
            }
        }
        assert_eq!(
            Localizer::try_new("en-US").unwrap().format(&WrongType),
            "[status-connecting]"
        );
    }

    #[test]
    fn localization_dynamic_arguments_remain_schema_checked_and_isolated() {
        let localizer = Localizer::try_new("en-US").unwrap();
        let arguments = [
            Argument::new("value1", ArgumentValue::UserData("2")),
            Argument::new("value2", ArgumentValue::UserData("15")),
        ];
        assert_eq!(
            localizer
                .format_arguments(MessageId::AgentCanvasDynamicCurrentMatchMatches, &arguments,),
            format!("Match {FSI}2{PDI} of {FSI}15{PDI}")
        );
        assert_eq!(
            localizer.format_arguments(
                MessageId::AgentCanvasDynamicCurrentMatchMatches,
                &arguments[..1],
            ),
            "[agent-canvas-dynamic-current-match-matches]"
        );
    }

    #[test]
    fn localization_generated_pseudo_catalogs_meet_expansion_and_bidi_contracts() {
        let schema_source = include_bytes!("../../../locales/schema.toml");
        let english = include_bytes!("../../../locales/en-US.ftl");
        let schema = parse_message_schema(schema_source).unwrap();
        let artifacts = generate_message_artifacts(&schema, schema_source, english).unwrap();
        validate_catalog_set(
            &schema,
            english,
            artifacts.en_xa.as_bytes(),
            artifacts.ar_xb.as_bytes(),
        )
        .unwrap();
    }
}
