use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const MAX_TRANSCRIPT_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_TRANSCRIPT_SCANNED_RECORDS: usize = 100_000;
pub const MAX_TRANSCRIPT_EXPORTED_ENTRIES: usize = 10_000;
pub const MAX_TRANSCRIPT_OUTPUT_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_TRANSCRIPT_PAGE_ENTRIES: usize = 256;
pub const MAX_PROVIDER_RECORD_REF_BYTES: usize = 128;
pub const MAX_PROVIDER_CONTRACT_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptKind {
    User,
    Assistant,
    Reasoning,
    ToolCall,
    ToolResult,
    Diff,
    Plan,
    Metadata,
}

impl TranscriptKind {
    pub const ALL: [Self; 8] = [
        Self::User,
        Self::Assistant,
        Self::Reasoning,
        Self::ToolCall,
        Self::ToolResult,
        Self::Diff,
        Self::Plan,
        Self::Metadata,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Assistant => "Assistant",
            Self::Reasoning => "Reasoning",
            Self::ToolCall => "Tool call",
            Self::ToolResult => "Tool result",
            Self::Diff => "Diff",
            Self::Plan => "Plan",
            Self::Metadata => "Metadata",
        }
    }

    pub const fn sensitive(self) -> bool {
        !matches!(self, Self::User | Self::Assistant)
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TranscriptContent(String);

impl TranscriptContent {
    pub fn new(value: String) -> Result<Self, TranscriptError> {
        if value.len() > MAX_TRANSCRIPT_RECORD_BYTES {
            return Err(TranscriptError::RecordTooLarge);
        }
        if value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err(TranscriptError::InvalidContent);
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for TranscriptContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TranscriptContent(<redacted>)")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProviderRecordRef(String);

impl ProviderRecordRef {
    pub fn new(value: impl Into<String>) -> Result<Self, TranscriptError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PROVIDER_RECORD_REF_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(TranscriptError::InvalidProviderReference);
        }
        Ok(Self(value))
    }
}

impl fmt::Debug for ProviderRecordRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderRecordRef(<redacted>)")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptEntry {
    pub sequence: u64,
    pub occurred_at: Option<i64>,
    pub kind: TranscriptKind,
    pub content: TranscriptContent,
    pub provenance: ProviderRecordRef,
}

impl TranscriptEntry {
    pub fn validate(&self) -> Result<(), TranscriptError> {
        if self.sequence == 0 {
            return Err(TranscriptError::InvalidSequence);
        }
        if self.content.expose().len() > MAX_TRANSCRIPT_RECORD_BYTES {
            return Err(TranscriptError::RecordTooLarge);
        }
        if self
            .content
            .expose()
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err(TranscriptError::InvalidContent);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TranscriptCategorySet(BTreeSet<TranscriptKind>);

impl Default for TranscriptCategorySet {
    fn default() -> Self {
        Self(BTreeSet::from([
            TranscriptKind::User,
            TranscriptKind::Assistant,
        ]))
    }
}

impl TranscriptCategorySet {
    pub fn new(values: impl IntoIterator<Item = TranscriptKind>) -> Self {
        Self(values.into_iter().collect())
    }

    pub fn contains(&self, kind: TranscriptKind) -> bool {
        self.0.contains(&kind)
    }

    pub fn iter(&self) -> impl Iterator<Item = TranscriptKind> + '_ {
        self.0.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptRange {
    pub start_inclusive: Option<u64>,
    pub end_inclusive: Option<u64>,
}

impl TranscriptRange {
    pub fn includes(self, sequence: u64) -> bool {
        self.start_inclusive.is_none_or(|start| sequence >= start)
            && self.end_inclusive.is_none_or(|end| sequence <= end)
    }

    pub fn validate(self) -> Result<(), TranscriptError> {
        if self
            .start_inclusive
            .zip(self.end_inclusive)
            .is_some_and(|(start, end)| start == 0 || start > end)
            || self.start_inclusive == Some(0)
            || self.end_inclusive == Some(0)
        {
            return Err(TranscriptError::InvalidRange);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptLimits {
    pub record_bytes: usize,
    pub scanned_records: usize,
    pub exported_entries: usize,
    pub output_bytes: usize,
    pub page_entries: usize,
}

impl Default for TranscriptLimits {
    fn default() -> Self {
        Self {
            record_bytes: MAX_TRANSCRIPT_RECORD_BYTES,
            scanned_records: MAX_TRANSCRIPT_SCANNED_RECORDS,
            exported_entries: MAX_TRANSCRIPT_EXPORTED_ENTRIES,
            output_bytes: MAX_TRANSCRIPT_OUTPUT_BYTES,
            page_entries: MAX_TRANSCRIPT_PAGE_ENTRIES,
        }
    }
}

impl TranscriptLimits {
    pub fn validate(self) -> Result<(), TranscriptError> {
        if self.record_bytes == 0
            || self.record_bytes > MAX_TRANSCRIPT_RECORD_BYTES
            || self.scanned_records == 0
            || self.scanned_records > MAX_TRANSCRIPT_SCANNED_RECORDS
            || self.exported_entries == 0
            || self.exported_entries > MAX_TRANSCRIPT_EXPORTED_ENTRIES
            || self.output_bytes == 0
            || self.output_bytes > MAX_TRANSCRIPT_OUTPUT_BYTES
            || self.page_entries == 0
            || self.page_entries > MAX_TRANSCRIPT_PAGE_ENTRIES
        {
            return Err(TranscriptError::ResourceLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptRequest {
    pub categories: TranscriptCategorySet,
    pub range: TranscriptRange,
    pub limits: TranscriptLimits,
}

impl TranscriptRequest {
    pub fn validate(&self) -> Result<(), TranscriptError> {
        if self.categories.is_empty() {
            return Err(TranscriptError::EmptyCategories);
        }
        self.range.validate()?;
        self.limits.validate()
    }

    pub fn includes(&self, entry: &TranscriptEntry) -> bool {
        self.categories.contains(entry.kind) && self.range.includes(entry.sequence)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptPage {
    pub entries: Vec<TranscriptEntry>,
    pub next_record: Option<u64>,
    pub scanned_count: u64,
    pub skipped_count: u64,
    pub redaction_count: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportManifest {
    pub provider_contract: String,
    pub categories: Vec<TranscriptKind>,
    pub entry_count: u64,
    pub skipped_count: u64,
    pub redaction_count: u64,
    pub deterministic_content_hash: String,
}

impl ExportManifest {
    pub fn validate(&self) -> Result<(), TranscriptError> {
        if self.provider_contract.is_empty()
            || self.provider_contract.len() > MAX_PROVIDER_CONTRACT_BYTES
            || !self
                .provider_contract
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(TranscriptError::InvalidProviderContract);
        }
        if self.categories.is_empty() || self.categories.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(TranscriptError::EmptyCategories);
        }
        if self.entry_count > MAX_TRANSCRIPT_EXPORTED_ENTRIES as u64
            || self.deterministic_content_hash.len() != 64
            || !self
                .deterministic_content_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(TranscriptError::ResourceLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedTranscript {
    pub content: TranscriptContent,
    pub redaction_count: u64,
}

#[derive(Clone, Debug, Default)]
pub struct TranscriptCancellation(Arc<AtomicBool>);

impl TranscriptCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn check(&self) -> Result<(), TranscriptError> {
        if self.is_cancelled() {
            Err(TranscriptError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptError {
    Cancelled,
    EmptyCategories,
    InvalidContent,
    InvalidProviderContract,
    InvalidProviderReference,
    InvalidRange,
    InvalidSequence,
    RecordTooLarge,
    ResourceLimit,
}

pub fn normalize_transcript_content(
    input: &str,
    cancellation: &TranscriptCancellation,
) -> Result<NormalizedTranscript, TranscriptError> {
    if input.len() > MAX_TRANSCRIPT_RECORD_BYTES {
        return Err(TranscriptError::RecordTooLarge);
    }
    cancellation.check()?;
    let mut normalized = String::with_capacity(input.len());
    let mut characters = input.chars().peekable();
    let mut processed = 0usize;
    while let Some(character) = characters.next() {
        if processed.is_multiple_of(4096) {
            cancellation.check()?;
        }
        processed = processed.saturating_add(character.len_utf8());
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                normalized.push('\n');
            }
            '\n' | '\t' => normalized.push(character),
            value if value.is_control() => normalized.push(' '),
            value => normalized.push(value),
        }
    }
    let (content, redaction_count) = redact_common_secrets(&normalized, cancellation)?;
    Ok(NormalizedTranscript {
        content: TranscriptContent::new(content)?,
        redaction_count,
    })
}

pub fn escape_markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '('
                | ')'
                | '<'
                | '>'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub fn render_transcript_entry_markdown(entry: &TranscriptEntry) -> String {
    render_transcript_entry_markdown_with_label(entry, entry.kind.label())
}

pub fn render_transcript_entry_markdown_with_label(entry: &TranscriptEntry, label: &str) -> String {
    format!(
        "## {}\n\n{}\n\n",
        escape_markdown_text(label),
        escape_markdown_text(entry.content.expose())
    )
}

pub fn deterministic_content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn redact_common_secrets(
    value: &str,
    cancellation: &TranscriptCancellation,
) -> Result<(String, u64), TranscriptError> {
    let mut output = Vec::new();
    let mut count = 0u64;
    let mut private_key = false;
    for (index, line) in value.lines().enumerate() {
        if index % 128 == 0 {
            cancellation.check()?;
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains("-----begin ") && lower.contains("private key-----") {
            private_key = true;
            count = count.saturating_add(1);
            output.push("[REDACTED PRIVATE KEY]".to_string());
            continue;
        }
        if private_key {
            if lower.contains("-----end ") && lower.contains("private key-----") {
                private_key = false;
            }
            continue;
        }
        if sensitive_assignment(&lower)
            && let Some(separator) = line.find('=').or_else(|| line.find(':'))
        {
            count = count.saturating_add(1);
            output.push(format!("{}=[REDACTED]", line[..separator].trim_end()));
            continue;
        }
        let (redacted, replacements) = redact_token_shapes(line);
        count = count.saturating_add(replacements);
        output.push(redacted);
    }
    Ok((output.join("\n"), count))
}

fn sensitive_assignment(lower: &str) -> bool {
    let key = lower
        .split(['=', ':'])
        .next()
        .unwrap_or(lower)
        .trim()
        .trim_start_matches("export ");
    [
        "api_key",
        "apikey",
        "access_token",
        "auth_token",
        "authorization",
        "password",
        "passwd",
        "client_secret",
        "private_key",
    ]
    .iter()
    .any(|candidate| key.contains(candidate))
}

fn redact_token_shapes(line: &str) -> (String, u64) {
    let mut result = line.to_string();
    let mut replacements = 0u64;
    for prefix in ["sk-", "ghp_", "github_pat_", "xoxb-", "xoxp-"] {
        let mut cursor = 0usize;
        while let Some(offset) = result[cursor..].find(prefix) {
            let start = cursor + offset;
            let end = result[start..]
                .find(|character: char| character.is_whitespace() || "'\";,)]}".contains(character))
                .map(|offset| start + offset)
                .unwrap_or(result.len());
            if end.saturating_sub(start) < prefix.len() + 8 {
                cursor = start + prefix.len();
                continue;
            }
            result.replace_range(start..end, "[REDACTED TOKEN]");
            replacements = replacements.saturating_add(1);
            cursor = start + "[REDACTED TOKEN]".len();
        }
    }
    (result, replacements)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(sequence: u64, kind: TranscriptKind, content: &str) -> TranscriptEntry {
        TranscriptEntry {
            sequence,
            occurred_at: None,
            kind,
            content: TranscriptContent::new(content.to_string()).unwrap(),
            provenance: ProviderRecordRef::new(format!("record-{sequence}")).unwrap(),
        }
    }

    #[test]
    fn transcript_default_categories_are_exactly_user_and_assistant() {
        let categories = TranscriptCategorySet::default();
        assert_eq!(
            categories.iter().collect::<Vec<_>>(),
            vec![TranscriptKind::User, TranscriptKind::Assistant]
        );
        assert!(!categories.contains(TranscriptKind::Reasoning));
        assert!(!categories.contains(TranscriptKind::ToolResult));
    }

    #[test]
    fn transcript_normalization_redacts_secrets_and_escapes_markdown() {
        let normalized = normalize_transcript_content(
            "# heading\r\nAPI_KEY=canary-secret\rsk-12345678901234567890\u{1b}",
            &TranscriptCancellation::default(),
        )
        .unwrap();
        assert_eq!(normalized.redaction_count, 2);
        assert!(!normalized.content.expose().contains("canary-secret"));
        assert!(!normalized.content.expose().contains("12345678901234567890"));
        assert!(!normalized.content.expose().contains('\u{1b}'));
        let rendered = render_transcript_entry_markdown(&TranscriptEntry {
            content: normalized.content,
            ..entry(1, TranscriptKind::User, "unused")
        });
        assert!(rendered.contains("\\# heading"));
        assert!(!rendered.contains("\n# heading"));
    }

    #[test]
    fn transcript_redaction_scans_past_short_prefixes_and_normalization_is_deterministic() {
        let input = "short sk-x then sk-12345678901234567890\n";
        let first =
            normalize_transcript_content(input, &TranscriptCancellation::default()).unwrap();
        let second =
            normalize_transcript_content(input, &TranscriptCancellation::default()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.redaction_count, 1);
        assert!(first.content.expose().contains("short sk-x"));
        assert!(!first.content.expose().contains("12345678901234567890"));

        let controls = (0..=127).filter_map(char::from_u32).collect::<String>();
        let normalized =
            normalize_transcript_content(&controls, &TranscriptCancellation::default()).unwrap();
        assert!(
            !normalized
                .content
                .expose()
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        );
    }

    #[test]
    fn transcript_sequence_and_filtering_ignore_timestamps() {
        let request = TranscriptRequest {
            range: TranscriptRange {
                start_inclusive: Some(2),
                end_inclusive: Some(3),
            },
            ..TranscriptRequest::default()
        };
        let first = TranscriptEntry {
            occurred_at: Some(100),
            ..entry(2, TranscriptKind::User, "first")
        };
        let second = TranscriptEntry {
            occurred_at: Some(1),
            ..entry(3, TranscriptKind::Assistant, "second")
        };
        assert!(request.includes(&first));
        assert!(request.includes(&second));
        assert!(!request.includes(&entry(4, TranscriptKind::Reasoning, "hidden")));
    }

    #[test]
    fn transcript_limits_and_cancellation_fail_closed() {
        assert_eq!(
            TranscriptLimits {
                record_bytes: MAX_TRANSCRIPT_RECORD_BYTES + 1,
                ..TranscriptLimits::default()
            }
            .validate(),
            Err(TranscriptError::ResourceLimit)
        );
        assert_eq!(
            TranscriptContent::new("x".repeat(MAX_TRANSCRIPT_RECORD_BYTES + 1)),
            Err(TranscriptError::RecordTooLarge)
        );
        let cancellation = TranscriptCancellation::default();
        cancellation.cancel();
        assert_eq!(
            normalize_transcript_content("safe", &cancellation),
            Err(TranscriptError::Cancelled)
        );
    }

    #[test]
    fn transcript_sensitive_debug_and_manifest_are_content_free() {
        let secret = "opaque-provider-record-secret";
        let content = TranscriptContent::new(secret.to_string()).unwrap();
        let reference = ProviderRecordRef::new("record-1").unwrap();
        assert!(!format!("{content:?}").contains(secret));
        assert!(!format!("{reference:?}").contains("record-1"));
        let manifest = ExportManifest {
            provider_contract: "fixture-v1".to_string(),
            categories: vec![TranscriptKind::User, TranscriptKind::Assistant],
            entry_count: 1,
            skipped_count: 2,
            redaction_count: 3,
            deterministic_content_hash: deterministic_content_hash(b"safe export"),
        };
        assert_eq!(manifest.validate(), Ok(()));
        let serialized = serde_json::to_string(&manifest).unwrap();
        assert!(!serialized.contains(secret));
    }

    #[test]
    fn transcript_validation_rejects_deserialized_control_content() {
        let entry: TranscriptEntry = serde_json::from_str(
            r#"{"sequence":1,"occurred_at":null,"kind":"user","content":"bad\u0000value","provenance":"record-1"}"#,
        )
        .unwrap();
        assert_eq!(entry.validate(), Err(TranscriptError::InvalidContent));
    }

    #[test]
    fn transcript_fixture_manifest_declares_hostile_cases_without_real_provider_data() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/runtimes/contract-manifest.json");
        let manifest = std::fs::read_to_string(path).unwrap();
        for scenario in [
            "equal_times",
            "missing_times",
            "unicode",
            "markdown_injection",
            "canary_secrets",
            "malformed",
            "oversize",
            "escaping_symlink",
            "permission_denied",
            "source_changed",
            "cancelled",
        ] {
            assert!(manifest.contains(scenario), "missing scenario {scenario}");
        }
        assert!(!manifest.contains("release_enabled\": true"));
    }
}
