use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use url::{Host, Url};

use crate::{HostInstanceId, HostedSessionId, OutputSequence};

pub const MAX_DEV_URL_BYTES: usize = 2 * 1024;
pub const MAX_DEV_URL_CARRY_BYTES: usize = 4 * 1024;
pub const MAX_DEV_URL_CANDIDATES: usize = 64;
pub const MAX_DEV_URL_PATH_LABEL_BYTES: usize = 160;
const MAX_CONTROL_SEQUENCE_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DevUrlPolicy {
    pub maximum_url_bytes: usize,
    pub maximum_carry_bytes: usize,
    pub maximum_candidates: usize,
}

impl Default for DevUrlPolicy {
    fn default() -> Self {
        Self {
            maximum_url_bytes: MAX_DEV_URL_BYTES,
            maximum_carry_bytes: MAX_DEV_URL_CARRY_BYTES,
            maximum_candidates: MAX_DEV_URL_CANDIDATES,
        }
    }
}

impl DevUrlPolicy {
    pub fn validate(self) -> Result<(), DevUrlError> {
        if self.maximum_url_bytes == 0
            || self.maximum_url_bytes > MAX_DEV_URL_BYTES
            || self.maximum_carry_bytes < self.maximum_url_bytes
            || self.maximum_carry_bytes > MAX_DEV_URL_CARRY_BYTES
            || self.maximum_candidates == 0
            || self.maximum_candidates > MAX_DEV_URL_CANDIDATES
        {
            return Err(DevUrlError::InvalidPolicy);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DevUrlError {
    InvalidPolicy,
    InvalidUrl,
    UnsupportedScheme,
    CredentialsForbidden,
    NonLoopbackHost,
    AmbiguousHost,
    InvalidPort,
    TooLong,
    Cancelled,
}

impl fmt::Display for DevUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidPolicy => "invalid local development URL policy",
            Self::InvalidUrl => "invalid local development URL",
            Self::UnsupportedScheme => "unsupported local development URL scheme",
            Self::CredentialsForbidden => "local development URL credentials are forbidden",
            Self::NonLoopbackHost => "local development URL host is not loopback",
            Self::AmbiguousHost => "local development URL host is ambiguous",
            Self::InvalidPort => "local development URL port is invalid",
            Self::TooLong => "local development URL exceeds the byte limit",
            Self::Cancelled => "local development URL scan was cancelled",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for DevUrlError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenUrlError {
    Invalidated,
    StaleHost,
    SessionUnavailable,
    BrowserUnavailable,
    PermissionDenied,
    DispatchFailed,
}

impl fmt::Display for OpenUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Invalidated => "the local development URL is no longer available",
            Self::StaleHost => "the local development URL belongs to a replaced Host",
            Self::SessionUnavailable => "the local development URL session is unavailable",
            Self::BrowserUnavailable => "no browser is available",
            Self::PermissionDenied => "browser access was denied",
            Self::DispatchFailed => "the browser could not open the local development URL",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for OpenUrlError {}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct LocalDevUrl {
    normalized: Arc<str>,
    display_origin: Arc<str>,
    path_label: Arc<str>,
    has_hidden_query: bool,
    requires_confirmation: bool,
}

impl LocalDevUrl {
    pub fn parse(input: &str) -> Result<Self, DevUrlError> {
        Self::parse_with_policy(input, DevUrlPolicy::default())
    }

    pub fn parse_with_policy(input: &str, policy: DevUrlPolicy) -> Result<Self, DevUrlError> {
        policy.validate()?;
        if input.is_empty() || input.len() > policy.maximum_url_bytes {
            return Err(if input.len() > policy.maximum_url_bytes {
                DevUrlError::TooLong
            } else {
                DevUrlError::InvalidUrl
            });
        }
        if !input.is_ascii() || input.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(DevUrlError::InvalidUrl);
        }

        let (raw_scheme, raw_authority) = raw_scheme_and_authority(input)?;
        if !raw_scheme.eq_ignore_ascii_case("http") && !raw_scheme.eq_ignore_ascii_case("https") {
            return Err(DevUrlError::UnsupportedScheme);
        }
        let (raw_host, _) = split_raw_authority(raw_authority)?;
        if raw_authority.contains('@') {
            return Err(DevUrlError::CredentialsForbidden);
        }

        let mut parsed = Url::parse(input).map_err(|_| DevUrlError::InvalidUrl)?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(DevUrlError::UnsupportedScheme);
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(DevUrlError::CredentialsForbidden);
        }
        if parsed.port() == Some(0) {
            return Err(DevUrlError::InvalidPort);
        }

        let display_host = match parsed.host().ok_or(DevUrlError::InvalidUrl)? {
            Host::Domain(domain) => validate_loopback_domain(raw_host, domain)?,
            Host::Ipv4(address) => validate_loopback_ipv4(raw_host, address)?,
            Host::Ipv6(address) => validate_loopback_ipv6(raw_host, address)?,
        };
        let effective_port = parsed
            .port_or_known_default()
            .ok_or(DevUrlError::InvalidPort)?;
        let display_origin = if display_host.contains(':') {
            format!("[{display_host}]:{effective_port}")
        } else {
            format!("{display_host}:{effective_port}")
        };

        // `url` canonicalizes scheme, host, IPv6 spelling, percent escapes, and default ports.
        parsed
            .set_username("")
            .map_err(|_| DevUrlError::InvalidUrl)?;
        parsed
            .set_password(None)
            .map_err(|_| DevUrlError::InvalidUrl)?;
        let has_hidden_query = parsed.query().is_some() || parsed.fragment().is_some();
        let path = parsed.path();
        let requires_confirmation = has_hidden_query || (path != "/" && !path.is_empty());
        let path_label = bounded_path_label(path);

        Ok(Self {
            normalized: Arc::from(parsed.as_str()),
            display_origin: Arc::from(display_origin),
            path_label: Arc::from(path_label),
            has_hidden_query,
            requires_confirmation,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.normalized
    }

    pub fn display_origin(&self) -> &str {
        &self.display_origin
    }

    pub fn path_label(&self) -> &str {
        &self.path_label
    }

    pub const fn has_hidden_query(&self) -> bool {
        self.has_hidden_query
    }

    pub const fn requires_confirmation(&self) -> bool {
        self.requires_confirmation
    }

    pub fn revalidate(&self, policy: DevUrlPolicy) -> Result<Self, DevUrlError> {
        Self::parse_with_policy(self.as_str(), policy)
    }
}

impl fmt::Debug for LocalDevUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalDevUrl")
            .field("normalized", &"<redacted>")
            .field("display_origin", &self.display_origin)
            .field("path", &"<redacted>")
            .field("has_hidden_query", &self.has_hidden_query)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct DevUrlCandidate {
    pub id: u64,
    pub session_id: HostedSessionId,
    pub host_instance: HostInstanceId,
    pub output_sequence: OutputSequence,
    pub normalized_url: LocalDevUrl,
    pub display_origin: String,
    pub has_hidden_query: bool,
}

impl fmt::Debug for DevUrlCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevUrlCandidate")
            .field("id", &self.id)
            .field("session_id", &self.session_id)
            .field("host_instance", &self.host_instance)
            .field("output_sequence", &self.output_sequence)
            .field("normalized_url", &"<redacted>")
            .field("display_origin", &self.display_origin)
            .field("has_hidden_query", &self.has_hidden_query)
            .finish()
    }
}

#[derive(Clone, Default)]
pub struct DevUrlCancellation(Arc<AtomicBool>);

impl DevUrlCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl fmt::Debug for DevUrlCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevUrlCancellation")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DevUrlDetectorCounters {
    pub accepted: u64,
    pub rejected: u64,
    pub oversized: u64,
    pub control_boundaries: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParserState {
    Ground,
    Escape,
    Csi { bytes: usize, valid_sgr: bool },
    ControlString { bytes: usize, escape: bool },
}

#[derive(Clone)]
pub struct DevUrlDetector {
    policy: DevUrlPolicy,
    token: Vec<u8>,
    token_overflow: bool,
    state: ParserState,
    counters: DevUrlDetectorCounters,
}

impl fmt::Debug for DevUrlDetector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevUrlDetector")
            .field("policy", &self.policy)
            .field("carry_bytes", &self.token.len())
            .field("token_overflow", &self.token_overflow)
            .field("state", &self.state)
            .field("counters", &self.counters)
            .finish()
    }
}

impl Default for DevUrlDetector {
    fn default() -> Self {
        Self::new(DevUrlPolicy::default()).expect("default URL policy must be valid")
    }
}

impl DevUrlDetector {
    pub fn new(policy: DevUrlPolicy) -> Result<Self, DevUrlError> {
        policy.validate()?;
        Ok(Self {
            policy,
            token: Vec::with_capacity(policy.maximum_url_bytes.min(256)),
            token_overflow: false,
            state: ParserState::Ground,
            counters: DevUrlDetectorCounters::default(),
        })
    }

    pub const fn counters(&self) -> DevUrlDetectorCounters {
        self.counters
    }

    pub fn reset_carry(&mut self) {
        self.token.clear();
        self.token_overflow = false;
        self.state = ParserState::Ground;
    }

    pub fn observe(
        &mut self,
        bytes: &[u8],
        cancellation: &DevUrlCancellation,
    ) -> Result<Vec<LocalDevUrl>, DevUrlError> {
        if cancellation.is_cancelled() {
            return Err(DevUrlError::Cancelled);
        }
        let mut urls = Vec::new();
        for (index, &byte) in bytes.iter().enumerate() {
            if index % 1024 == 0 && cancellation.is_cancelled() {
                return Err(DevUrlError::Cancelled);
            }
            self.observe_byte(byte, &mut urls);
        }
        Ok(urls)
    }

    pub fn finish(
        &mut self,
        cancellation: &DevUrlCancellation,
    ) -> Result<Vec<LocalDevUrl>, DevUrlError> {
        if cancellation.is_cancelled() {
            return Err(DevUrlError::Cancelled);
        }
        let mut urls = Vec::new();
        self.flush_token(&mut urls);
        self.state = ParserState::Ground;
        Ok(urls)
    }

    fn observe_byte(&mut self, byte: u8, urls: &mut Vec<LocalDevUrl>) {
        match self.state {
            ParserState::Ground => self.observe_ground(byte, urls),
            ParserState::Escape => match byte {
                b'[' => {
                    self.state = ParserState::Csi {
                        bytes: 0,
                        valid_sgr: true,
                    };
                }
                b']' | b'P' | b'X' | b'^' | b'_' => {
                    self.control_boundary(urls);
                    self.state = ParserState::ControlString {
                        bytes: 0,
                        escape: false,
                    };
                }
                _ => {
                    self.control_boundary(urls);
                    self.state = ParserState::Ground;
                }
            },
            ParserState::Csi {
                mut bytes,
                mut valid_sgr,
            } => {
                bytes = bytes.saturating_add(1);
                if bytes > MAX_CONTROL_SEQUENCE_BYTES {
                    self.control_boundary(urls);
                    self.state = ParserState::Ground;
                } else if (0x40..=0x7e).contains(&byte) {
                    if byte != b'm' || !valid_sgr {
                        self.control_boundary(urls);
                    }
                    self.state = ParserState::Ground;
                } else {
                    valid_sgr &= (0x20..=0x3f).contains(&byte);
                    self.state = ParserState::Csi { bytes, valid_sgr };
                }
            }
            ParserState::ControlString {
                mut bytes,
                mut escape,
            } => {
                bytes = bytes.saturating_add(1);
                if byte == 0x07 || (escape && byte == b'\\') {
                    self.state = ParserState::Ground;
                } else {
                    escape = byte == 0x1b;
                    self.state = ParserState::ControlString { bytes, escape };
                }
            }
        }
    }

    fn observe_ground(&mut self, byte: u8, urls: &mut Vec<LocalDevUrl>) {
        match byte {
            0x1b => self.state = ParserState::Escape,
            0x21..=0x7e if is_url_token_byte(byte) => {
                if !self.token_overflow {
                    if self.token.len() < self.policy.maximum_carry_bytes {
                        self.token.push(byte);
                    } else {
                        self.token.clear();
                        self.token_overflow = true;
                        self.counters.oversized = self.counters.oversized.saturating_add(1);
                    }
                }
            }
            _ => self.flush_token(urls),
        }
    }

    fn control_boundary(&mut self, urls: &mut Vec<LocalDevUrl>) {
        self.counters.control_boundaries = self.counters.control_boundaries.saturating_add(1);
        self.flush_token(urls);
    }

    fn flush_token(&mut self, urls: &mut Vec<LocalDevUrl>) {
        if self.token_overflow {
            self.token_overflow = false;
            self.token.clear();
            return;
        }
        if self.token.is_empty() {
            return;
        }
        let token = std::mem::take(&mut self.token);
        let mut cursor = 0;
        while let Some(start) = find_http_start(&token[cursor..]).map(|offset| cursor + offset) {
            let candidate = trim_candidate_end(&token[start..]);
            if candidate.len() > self.policy.maximum_url_bytes {
                self.counters.oversized = self.counters.oversized.saturating_add(1);
            } else if let Ok(text) = std::str::from_utf8(candidate) {
                match LocalDevUrl::parse_with_policy(text, self.policy) {
                    Ok(url) => {
                        self.counters.accepted = self.counters.accepted.saturating_add(1);
                        urls.push(url);
                    }
                    Err(_) => {
                        self.counters.rejected = self.counters.rejected.saturating_add(1);
                    }
                }
            }
            cursor = start.saturating_add(candidate.len().max(1));
            if cursor >= token.len() {
                break;
            }
        }
    }
}

fn raw_scheme_and_authority(input: &str) -> Result<(&str, &str), DevUrlError> {
    let scheme_end = input.find("://").ok_or(DevUrlError::InvalidUrl)?;
    let scheme = &input[..scheme_end];
    if scheme.is_empty() || !scheme.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(DevUrlError::UnsupportedScheme);
    }
    let after_scheme = &input[scheme_end + 3..];
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    if authority.is_empty() {
        return Err(DevUrlError::InvalidUrl);
    }
    Ok((scheme, authority))
}

fn split_raw_authority(authority: &str) -> Result<(&str, Option<u16>), DevUrlError> {
    if authority.contains('@') {
        return Err(DevUrlError::CredentialsForbidden);
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let close = rest.find(']').ok_or(DevUrlError::InvalidUrl)?;
        let host = &rest[..close];
        let suffix = &rest[close + 1..];
        let port = if suffix.is_empty() {
            None
        } else {
            let value = suffix.strip_prefix(':').ok_or(DevUrlError::InvalidPort)?;
            Some(parse_port(value)?)
        };
        return Ok((host, port));
    }
    if authority.matches(':').count() > 1 {
        return Err(DevUrlError::AmbiguousHost);
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() {
            return Err(DevUrlError::InvalidUrl);
        }
        Ok((host, Some(parse_port(port)?)))
    } else {
        Ok((authority, None))
    }
}

fn parse_port(value: &str) -> Result<u16, DevUrlError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DevUrlError::InvalidPort);
    }
    let port = value.parse::<u16>().map_err(|_| DevUrlError::InvalidPort)?;
    if port == 0 {
        return Err(DevUrlError::InvalidPort);
    }
    Ok(port)
}

fn validate_loopback_domain(raw: &str, parsed: &str) -> Result<String, DevUrlError> {
    if !raw.is_ascii()
        || raw.ends_with('.')
        || raw.contains('%')
        || !raw.eq_ignore_ascii_case(parsed)
        || parsed.split('.').any(str::is_empty)
    {
        return Err(DevUrlError::AmbiguousHost);
    }
    let normalized = parsed.to_ascii_lowercase();
    if normalized != "localhost" && !normalized.ends_with(".localhost") {
        return Err(DevUrlError::NonLoopbackHost);
    }
    Ok(normalized)
}

fn validate_loopback_ipv4(raw: &str, parsed: Ipv4Addr) -> Result<String, DevUrlError> {
    let components = raw.split('.').collect::<Vec<_>>();
    if components.len() != 4
        || components.iter().any(|component| {
            component.is_empty()
                || !component.bytes().all(|byte| byte.is_ascii_digit())
                || (component.len() > 1 && component.starts_with('0'))
        })
    {
        return Err(DevUrlError::AmbiguousHost);
    }
    let reparsed = Ipv4Addr::from_str(raw).map_err(|_| DevUrlError::AmbiguousHost)?;
    if reparsed != parsed {
        return Err(DevUrlError::AmbiguousHost);
    }
    if !parsed.is_loopback() {
        return Err(DevUrlError::NonLoopbackHost);
    }
    Ok(parsed.to_string())
}

fn validate_loopback_ipv6(raw: &str, parsed: Ipv6Addr) -> Result<String, DevUrlError> {
    if raw.contains('%')
        || Ipv6Addr::from_str(raw).map_err(|_| DevUrlError::AmbiguousHost)? != parsed
    {
        return Err(DevUrlError::AmbiguousHost);
    }
    if !parsed.is_loopback() {
        return Err(DevUrlError::NonLoopbackHost);
    }
    Ok(parsed.to_string())
}

fn bounded_path_label(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }
    if path.len() <= MAX_DEV_URL_PATH_LABEL_BYTES {
        return path.to_string();
    }
    let mut end = MAX_DEV_URL_PATH_LABEL_BYTES
        .saturating_sub(3)
        .min(path.len());
    while !path.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}...", &path[..end])
}

fn is_url_token_byte(byte: u8) -> bool {
    !matches!(
        byte,
        b'"' | b'\'' | b'<' | b'>' | b'{' | b'}' | b'|' | b'\\' | b'^' | b'`'
    )
}

fn find_http_start(bytes: &[u8]) -> Option<usize> {
    const HTTP: &[u8] = b"http://";
    const HTTPS: &[u8] = b"https://";
    (0..bytes.len()).find(|&index| {
        let has_scheme_boundary = index == 0
            || !matches!(
                bytes[index - 1],
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            );
        has_scheme_boundary
            && (starts_ascii_case_insensitive(&bytes[index..], HTTP)
                || starts_ascii_case_insensitive(&bytes[index..], HTTPS))
    })
}

fn starts_ascii_case_insensitive(value: &[u8], prefix: &[u8]) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn trim_candidate_end(mut bytes: &[u8]) -> &[u8] {
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'.' | b',' | b';' | b'!' | b')'))
    {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn scan(chunks: &[&[u8]]) -> Vec<LocalDevUrl> {
        let mut detector = DevUrlDetector::default();
        let cancellation = DevUrlCancellation::default();
        let mut urls = Vec::new();
        for chunk in chunks {
            urls.extend(detector.observe(chunk, &cancellation).unwrap());
        }
        urls.extend(detector.finish(&cancellation).unwrap());
        urls
    }

    fn decode_fixture_bytes(encoded: &str) -> Vec<u8> {
        let bytes = encoded.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut cursor = 0;
        while cursor < bytes.len() {
            if bytes.get(cursor..cursor + 2) == Some(b"\\x") {
                let hex = std::str::from_utf8(&bytes[cursor + 2..cursor + 4]).unwrap();
                decoded.push(u8::from_str_radix(hex, 16).unwrap());
                cursor += 4;
            } else if bytes.get(cursor..cursor + 2) == Some(b"\\\\") {
                decoded.push(b'\\');
                cursor += 2;
            } else {
                decoded.push(bytes[cursor]);
                cursor += 1;
            }
        }
        decoded
    }

    #[test]
    fn dev_url_fixture_streams_are_invariant_at_every_chunk_boundary() {
        let fixtures = include_str!("../../../tests/fixtures/dev_urls/streams.txt");
        for line in fixtures.lines().filter(|line| !line.starts_with('#')) {
            let mut fields = line.splitn(3, '|');
            let name = fields.next().unwrap();
            let expected = fields.next().unwrap();
            let stream = decode_fixture_bytes(fields.next().unwrap());
            let expected = if expected == "-" {
                Vec::new()
            } else {
                expected.split(',').map(str::to_string).collect::<Vec<_>>()
            };
            let whole = scan(&[&stream])
                .iter()
                .map(|url| url.as_str().to_string())
                .collect::<Vec<_>>();
            assert_eq!(whole, expected, "fixture {name}");
            for split in 0..=stream.len() {
                let chunked = scan(&[&stream[..split], &stream[split..]])
                    .iter()
                    .map(|url| url.as_str().to_string())
                    .collect::<Vec<_>>();
                assert_eq!(chunked, expected, "fixture {name}, split {split}");
            }
        }
    }

    #[test]
    fn dev_url_accepts_only_canonical_loopback_web_origins() {
        let cases = [
            (
                "HTTP://LOCALHOST:3000/a?q=secret#part",
                "http://localhost:3000/a?q=secret#part",
                "localhost:3000",
            ),
            (
                "https://api.localhost/",
                "https://api.localhost/",
                "api.localhost:443",
            ),
            (
                "http://127.8.9.10:8080/",
                "http://127.8.9.10:8080/",
                "127.8.9.10:8080",
            ),
            (
                "http://[0:0:0:0:0:0:0:1]:9000/",
                "http://[::1]:9000/",
                "[::1]:9000",
            ),
        ];
        for (input, normalized, origin) in cases {
            let url = LocalDevUrl::parse(input).unwrap();
            assert_eq!(url.as_str(), normalized);
            assert_eq!(url.display_origin(), origin);
        }
    }

    #[test]
    fn dev_url_rejects_external_private_credentialed_ambiguous_and_non_web_inputs() {
        for input in [
            "http://example.com/",
            "http://10.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[fe80::1]/",
            "http://user:pass@localhost:3000/",
            "http://2130706433:3000/",
            "http://0x7f000001:3000/",
            "http://127.000.0.1:3000/",
            "http://localhost.:3000/",
            "http://localhost:0/",
            "ftp://localhost/",
            "file:///tmp/a",
            "http://locálhost:3000/",
        ] {
            assert!(LocalDevUrl::parse(input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn dev_url_streaming_is_split_invariant_and_sgr_only_is_ignorable() {
        let stream = b"ready http://local\x1b[31mhost:3000/path?q=x\x1b[0m\n";
        let whole = scan(&[stream]);
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].as_str(), "http://localhost:3000/path?q=x");
        for split in 0..=stream.len() {
            let split_scan = scan(&[&stream[..split], &stream[split..]]);
            assert_eq!(split_scan, whole, "split {split}");
        }
    }

    #[test]
    fn dev_url_controls_osc_dcs_and_invalid_bytes_cannot_splice_or_create_actions() {
        let inputs: [&[u8]; 7] = [
            b"http://local\x1b[2Chost:3000\n",
            b"http://local\x1b]0;title\x07host:3000\n",
            b"http://local\x1bPignored\x1b\\host:3000\n",
            b"\x1b]8;;http://localhost:3000\x07label\x1b]8;;\x07\n",
            b"http://local\xffhost:3000\n",
            b"\x1b]52;c;aHR0cDovL2xvY2FsaG9zdDozMDAw\x07\n",
            b"prefixhttp://localhost:3000\n",
        ];
        for input in inputs {
            assert!(scan(&[input]).is_empty());
        }
    }

    #[test]
    fn dev_url_bounds_cancel_and_redaction_are_enforced() {
        let prefix = "http://localhost/";
        let maximum = format!("{prefix}{}", "a".repeat(MAX_DEV_URL_BYTES - prefix.len()));
        assert_eq!(maximum.len(), MAX_DEV_URL_BYTES);
        assert!(LocalDevUrl::parse(&maximum).is_ok());
        let oversized = format!("http://localhost/{}\n", "a".repeat(MAX_DEV_URL_BYTES));
        assert!(scan(&[oversized.as_bytes()]).is_empty());

        let cancellation = DevUrlCancellation::default();
        cancellation.cancel();
        assert_eq!(
            DevUrlDetector::default().observe(b"http://localhost\n", &cancellation),
            Err(DevUrlError::Cancelled)
        );

        let url = LocalDevUrl::parse("http://localhost:3000/path?canary-secret").unwrap();
        let candidate = DevUrlCandidate {
            id: 1,
            session_id: HostedSessionId::from_uuid(Uuid::from_u128(1)),
            host_instance: HostInstanceId::from_uuid(Uuid::from_u128(2)),
            output_sequence: OutputSequence::new(3),
            display_origin: url.display_origin().to_string(),
            has_hidden_query: url.has_hidden_query(),
            normalized_url: url,
        };
        let debug = format!("{candidate:?}");
        assert!(!debug.contains("canary-secret"));
        assert!(!debug.contains("/path"));
    }

    #[test]
    fn dev_url_confirmation_and_path_labels_never_expose_query_by_default() {
        let plain = LocalDevUrl::parse("http://localhost:3000/").unwrap();
        assert!(!plain.requires_confirmation());
        let sensitive =
            LocalDevUrl::parse("http://localhost:3000/api/token?key=x#fragment").unwrap();
        assert!(sensitive.requires_confirmation());
        assert!(sensitive.has_hidden_query());
        assert_eq!(sensitive.path_label(), "/api/token");
        assert!(!sensitive.path_label().contains("key"));
    }
}
