use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rand::RngCore as _;
use serde::Deserialize;
use serde_json::{Value, json};
use termirust_cli::Cancellation;
use termirust_domain::{HostedSessionId, ProjectId};

use crate::backend::{
    ActionRequest, InspectionPage, InspectionRequest, InspectionSource, SourceError,
};

pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const SERVER_NAME: &str = "termirust-readonly";
const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 100;
const DEFAULT_MAX_CONCURRENT_CALLS: usize = 8;
const DEFAULT_MAX_CALLS_PER_MINUTE: usize = 120;
const DEFAULT_MAX_CURSORS: usize = 256;
const MAX_TOOL_RESULT_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    InspectStatus,
    ReadProjects,
    ReadConnections,
    ReadSessions,
    ReadRuntime,
    ListArtifacts,
    ReadTranscripts,
    LaunchSessions,
    WaitSessions,
    AttachSessions,
    CancelSessions,
    InputSessions,
    ReviewResume,
    ResumeSessions,
    CreateArtifacts,
    CaptureBrowserText,
    CaptureBrowserScreenshot,
    DownloadBrowserArtifact,
}

impl Capability {
    pub const ALL: [Self; 18] = [
        Self::InspectStatus,
        Self::ReadProjects,
        Self::ReadConnections,
        Self::ReadSessions,
        Self::ReadRuntime,
        Self::ListArtifacts,
        Self::ReadTranscripts,
        Self::LaunchSessions,
        Self::WaitSessions,
        Self::AttachSessions,
        Self::CancelSessions,
        Self::InputSessions,
        Self::ReviewResume,
        Self::ResumeSessions,
        Self::CreateArtifacts,
        Self::CaptureBrowserText,
        Self::CaptureBrowserScreenshot,
        Self::DownloadBrowserArtifact,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InspectStatus => "status.read",
            Self::ReadProjects => "projects.read",
            Self::ReadConnections => "connections.read",
            Self::ReadSessions => "sessions.read",
            Self::ReadRuntime => "runtime.read",
            Self::ListArtifacts => "artifacts.list",
            Self::ReadTranscripts => "transcripts.read",
            Self::LaunchSessions => "sessions.launch",
            Self::WaitSessions => "sessions.wait",
            Self::AttachSessions => "sessions.attach",
            Self::CancelSessions => "sessions.cancel",
            Self::InputSessions => "sessions.input",
            Self::ReviewResume => "sessions.resume.review",
            Self::ResumeSessions => "sessions.resume",
            Self::CreateArtifacts => "artifacts.create",
            Self::CaptureBrowserText => "browser.text",
            Self::CaptureBrowserScreenshot => "browser.screenshot",
            Self::DownloadBrowserArtifact => "browser.download",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|item| item.as_str() == value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilitySet(BTreeSet<Capability>);

impl Default for CapabilitySet {
    fn default() -> Self {
        Self(BTreeSet::from([
            Capability::InspectStatus,
            Capability::ReadProjects,
            Capability::ReadConnections,
            Capability::ReadSessions,
            Capability::ReadRuntime,
        ]))
    }
}

impl CapabilitySet {
    pub fn all() -> Self {
        Self(Capability::ALL.into_iter().collect())
    }

    pub fn read_only_all() -> Self {
        Self(BTreeSet::from([
            Capability::InspectStatus,
            Capability::ReadProjects,
            Capability::ReadConnections,
            Capability::ReadSessions,
            Capability::ReadRuntime,
            Capability::ListArtifacts,
            Capability::ReadTranscripts,
        ]))
    }

    pub fn none() -> Self {
        Self(BTreeSet::new())
    }

    pub fn parse(value: &str) -> Result<Self, ConfigurationError> {
        if value.len() > 512 {
            return Err(ConfigurationError::InvalidCapabilities);
        }
        if value == "all" {
            return Ok(Self::read_only_all());
        }
        if value.is_empty() || value == "none" {
            return Ok(Self::none());
        }
        let mut capabilities = BTreeSet::new();
        for item in value.split(',') {
            let capability =
                Capability::parse(item.trim()).ok_or(ConfigurationError::InvalidCapabilities)?;
            capabilities.insert(capability);
        }
        Ok(Self(capabilities))
    }

    pub fn contains(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }

    pub fn display_names(&self) -> Vec<&'static str> {
        self.0.iter().map(|value| value.as_str()).collect()
    }
}

#[derive(Clone, Debug)]
pub struct ServerConfiguration {
    pub capabilities: CapabilitySet,
    pub max_concurrent_calls: usize,
    pub max_calls_per_minute: usize,
    pub max_cursors: usize,
}

impl Default for ServerConfiguration {
    fn default() -> Self {
        Self {
            capabilities: CapabilitySet::default(),
            max_concurrent_calls: DEFAULT_MAX_CONCURRENT_CALLS,
            max_calls_per_minute: DEFAULT_MAX_CALLS_PER_MINUTE,
            max_cursors: DEFAULT_MAX_CURSORS,
        }
    }
}

impl ServerConfiguration {
    pub fn from_environment() -> Result<Self, ConfigurationError> {
        let capabilities = std::env::var("TERMIRUST_MCP_CAPABILITIES")
            .ok()
            .map(|value| CapabilitySet::parse(&value))
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            capabilities,
            ..Self::default()
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigurationError {
    InvalidCapabilities,
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TERMIRUST_MCP_CAPABILITIES contains an unsupported capability")
    }
}

impl std::error::Error for ConfigurationError {}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn capability_configuration_is_an_exact_bounded_allowlist() {
        assert_eq!(CapabilitySet::parse(""), Ok(CapabilitySet::none()));
        assert_eq!(CapabilitySet::parse("none"), Ok(CapabilitySet::none()));
        assert_eq!(
            CapabilitySet::parse("all"),
            Ok(CapabilitySet::read_only_all())
        );
        assert!(
            !CapabilitySet::parse("all")
                .expect("read-only all")
                .contains(Capability::InputSessions)
        );
        assert_eq!(
            CapabilitySet::parse("status.read, transcripts.read")
                .expect("known capability list")
                .display_names(),
            vec!["status.read", "transcripts.read"]
        );
        assert_eq!(
            CapabilitySet::parse("sessions.write"),
            Err(ConfigurationError::InvalidCapabilities)
        );
        assert_eq!(
            CapabilitySet::parse(&"x".repeat(513)),
            Err(ConfigurationError::InvalidCapabilities)
        );
    }
}

#[derive(Clone)]
pub struct McpServer {
    inner: Arc<ServerInner>,
}

struct ServerInner {
    source: Arc<dyn InspectionSource>,
    configuration: ServerConfiguration,
    lifecycle: Mutex<Lifecycle>,
    active: Mutex<HashMap<String, ActiveCall>>,
    cursors: Mutex<CursorStore>,
    rate_limit: Mutex<VecDeque<Instant>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lifecycle {
    AwaitingInitialize,
    AwaitingInitialized,
    Ready,
}

#[derive(Clone)]
enum ActiveCall {
    Reserved(Cancellation),
    Running(Cancellation),
}

impl ActiveCall {
    fn cancellation(&self) -> &Cancellation {
        match self {
            Self::Reserved(value) | Self::Running(value) => value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CursorState {
    tool: String,
    request_fingerprint: String,
    offset: usize,
}

struct CursorStore {
    maximum: usize,
    values: VecDeque<(String, CursorState)>,
}

impl CursorStore {
    fn new(maximum: usize) -> Self {
        Self {
            maximum,
            values: VecDeque::with_capacity(maximum),
        }
    }

    fn insert(&mut self, state: CursorState) -> Result<String, ProtocolError> {
        if self.maximum == 0 {
            return Err(ProtocolError::internal());
        }
        while self.values.len() >= self.maximum {
            self.values.pop_front();
        }
        let mut bytes = [0_u8; 18];
        rand::thread_rng().fill_bytes(&mut bytes);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        self.values.push_back((token.clone(), state));
        Ok(token)
    }

    fn get(&self, token: &str) -> Option<&CursorState> {
        self.values
            .iter()
            .find_map(|(candidate, state)| (candidate == token).then_some(state))
    }
}

impl McpServer {
    pub fn new(source: Arc<dyn InspectionSource>, configuration: ServerConfiguration) -> Self {
        let maximum = configuration.max_cursors;
        Self {
            inner: Arc::new(ServerInner {
                source,
                configuration,
                lifecycle: Mutex::new(Lifecycle::AwaitingInitialize),
                active: Mutex::new(HashMap::new()),
                cursors: Mutex::new(CursorStore::new(maximum)),
                rate_limit: Mutex::new(VecDeque::new()),
            }),
        }
    }

    pub fn is_tool_call(message: &Value) -> bool {
        message.get("method").and_then(Value::as_str) == Some("tools/call")
    }

    pub fn process(&self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let pending_key = message
            .get("method")
            .and_then(Value::as_str)
            .filter(|method| *method == "tools/call")
            .and(id.as_ref())
            .and_then(request_key);
        let response = match self.process_inner(message) {
            Ok(result) => id.map(|id| success(id, result)),
            Err(error) => error
                .respond
                .then(|| failure(id.unwrap_or(Value::Null), error)),
        };
        if let Some(key) = pending_key
            && let Ok(mut active) = self.inner.active.lock()
            && matches!(active.get(&key), Some(ActiveCall::Reserved(_)))
        {
            active.remove(&key);
        }
        response
    }

    pub fn reserve_tool_call(&self, message: &Value) -> bool {
        if !Self::is_tool_call(message) {
            return true;
        }
        let Some(key) = message.get("id").and_then(request_key) else {
            return true;
        };
        let Ok(mut active) = self.inner.active.lock() else {
            return false;
        };
        if active.len() >= self.inner.configuration.max_concurrent_calls
            || active.contains_key(&key)
        {
            return false;
        }
        active.insert(key, ActiveCall::Reserved(Cancellation::default()));
        true
    }

    pub fn cancel(&self, request_id: &Value) {
        let Some(key) = request_key(request_id) else {
            return;
        };
        if let Ok(active) = self.inner.active.lock()
            && let Some(cancellation) = active.get(&key)
        {
            cancellation.cancellation().cancel();
        }
    }

    fn process_inner(&self, message: Value) -> Result<Value, ProtocolError> {
        let object = message
            .as_object()
            .ok_or_else(ProtocolError::invalid_request)?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(ProtocolError::invalid_request());
        }
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(ProtocolError::invalid_request)?;
        let id = object.get("id");
        if id.is_some_and(|value| request_key(value).is_none()) {
            return Err(ProtocolError::invalid_request());
        }
        match method {
            "initialize" => self.initialize(object.get("params"), id),
            "notifications/initialized" => {
                require_notification(id)?;
                self.mark_initialized()?;
                Err(ProtocolError::notification())
            }
            "notifications/cancelled" => {
                require_notification(id)?;
                let params = parse_params::<CancelledParams>(object.get("params"))?;
                self.cancel(&params.request_id);
                Err(ProtocolError::notification())
            }
            "ping" => {
                require_request(id)?;
                Ok(json!({}))
            }
            "tools/list" => {
                require_request(id)?;
                self.require_ready()?;
                parse_optional_empty_params(object.get("params"))?;
                Ok(json!({ "tools": self.tools() }))
            }
            "tools/call" => {
                let id = require_request(id)?.clone();
                self.require_ready()?;
                let params = parse_params::<CallToolParams>(object.get("params"))?;
                self.call_tool(id, params)
            }
            _ => Err(ProtocolError::method_not_found()),
        }
    }

    fn initialize(
        &self,
        params: Option<&Value>,
        id: Option<&Value>,
    ) -> Result<Value, ProtocolError> {
        require_request(id)?;
        let params = parse_params::<InitializeParams>(params)?;
        if params.protocol_version.len() > 32
            || !params._capabilities.is_object()
            || params.client_info.name.is_empty()
            || params.client_info.name.len() > 128
            || params.client_info.version.len() > 64
        {
            return Err(ProtocolError::invalid_params(
                "invalid initialization metadata",
            ));
        }
        let mut lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| ProtocolError::internal())?;
        if *lifecycle != Lifecycle::AwaitingInitialize {
            return Err(ProtocolError::invalid_request());
        }
        *lifecycle = Lifecycle::AwaitingInitialized;
        Ok(json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": SERVER_NAME,
                "title": "TermiRust Local Control",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": "Bounded TermiRust inspection is read-only by default. Terminal byte streams are never exposed as inspection output. Every mutation requires an explicit startup capability plus a current, scoped, local approval policy.",
        }))
    }

    fn mark_initialized(&self) -> Result<(), ProtocolError> {
        let mut lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| ProtocolError::internal())?;
        if *lifecycle != Lifecycle::AwaitingInitialized {
            return Err(ProtocolError::invalid_request());
        }
        *lifecycle = Lifecycle::Ready;
        Ok(())
    }

    fn require_ready(&self) -> Result<(), ProtocolError> {
        let lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| ProtocolError::internal())?;
        if *lifecycle == Lifecycle::Ready {
            Ok(())
        } else {
            Err(ProtocolError::invalid_request())
        }
    }

    fn tools(&self) -> Vec<Value> {
        TOOL_DEFINITIONS
            .iter()
            .filter(|tool| {
                self.inner
                    .configuration
                    .capabilities
                    .contains(tool.capability)
            })
            .map(ToolDefinition::schema)
            .collect()
    }

    fn call_tool(&self, id: Value, params: CallToolParams) -> Result<Value, ProtocolError> {
        self.check_rate_limit()?;
        let tool = TOOL_DEFINITIONS
            .iter()
            .find(|tool| tool.name == params.name)
            .filter(|tool| {
                self.inner
                    .configuration
                    .capabilities
                    .contains(tool.capability)
            })
            .ok_or_else(ProtocolError::unknown_tool)?;
        let parsed = tool.parse_arguments(params.arguments)?;
        let request = parsed.request;
        let request_fingerprint = request.fingerprint()?;
        let (offset, page_size) = self.resolve_page(
            tool.name,
            &request_fingerprint,
            parsed.cursor.as_deref(),
            parsed.page_size,
        )?;
        let key = request_key(&id).ok_or_else(ProtocolError::invalid_request)?;
        let cancellation = {
            let mut active = self
                .inner
                .active
                .lock()
                .map_err(|_| ProtocolError::internal())?;
            let at_capacity = active.len() >= self.inner.configuration.max_concurrent_calls;
            match active.entry(key.clone()) {
                Entry::Occupied(mut entry) => match entry.get() {
                    ActiveCall::Reserved(value) => {
                        let cancellation = value.clone();
                        entry.insert(ActiveCall::Running(cancellation.clone()));
                        cancellation
                    }
                    ActiveCall::Running(_) => {
                        return Ok(tool_error("request capacity is currently exhausted"));
                    }
                },
                Entry::Vacant(entry) if !at_capacity => {
                    let cancellation = Cancellation::default();
                    entry.insert(ActiveCall::Running(cancellation.clone()));
                    cancellation
                }
                Entry::Vacant(_) => {
                    return Ok(tool_error("request capacity is currently exhausted"));
                }
            }
        };
        let result = match request {
            ToolRequest::Inspection(request) => {
                self.inner
                    .source
                    .inspect(request, offset, page_size, &cancellation)
            }
            ToolRequest::Action(request) => {
                self.inner
                    .source
                    .act(request, &cancellation)
                    .map(|data| InspectionPage {
                        data,
                        next_offset: None,
                    })
            }
        };
        if let Ok(mut active) = self.inner.active.lock() {
            active.remove(&key);
        }
        if cancellation.is_cancelled() {
            return Err(ProtocolError::cancelled());
        }
        match result {
            Ok(page) => {
                let next_cursor = page
                    .next_offset
                    .map(|offset| {
                        self.inner
                            .cursors
                            .lock()
                            .map_err(|_| ProtocolError::internal())?
                            .insert(CursorState {
                                tool: tool.name.to_string(),
                                request_fingerprint: request_fingerprint.clone(),
                                offset,
                            })
                    })
                    .transpose()?;
                let structured = json!({
                    "schemaVersion": 1,
                    "data": page.data,
                    "nextCursor": next_cursor,
                });
                let text =
                    serde_json::to_string(&structured).map_err(|_| ProtocolError::internal())?;
                if text.len() > MAX_TOOL_RESULT_BYTES {
                    return Ok(tool_error(
                        "inspection result exceeded the MCP response limit",
                    ));
                }
                Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "structuredContent": structured,
                    "isError": false,
                }))
            }
            Err(SourceError::Cancelled) => Err(ProtocolError::cancelled()),
            Err(error) => Ok(tool_error(error.to_string())),
        }
    }

    fn resolve_page(
        &self,
        tool: &str,
        request_fingerprint: &str,
        cursor: Option<&str>,
        page_size: Option<usize>,
    ) -> Result<(usize, usize), ProtocolError> {
        let page_size = page_size.unwrap_or(DEFAULT_PAGE_SIZE);
        if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
            return Err(ProtocolError::invalid_params(
                "pageSize must be between 1 and 100",
            ));
        }
        let Some(cursor) = cursor else {
            return Ok((0, page_size));
        };
        if cursor.len() > 128 {
            return Err(ProtocolError::invalid_params("cursor is invalid"));
        }
        let cursors = self
            .inner
            .cursors
            .lock()
            .map_err(|_| ProtocolError::internal())?;
        let state = cursors
            .get(cursor)
            .filter(|state| state.tool == tool && state.request_fingerprint == request_fingerprint)
            .ok_or_else(|| ProtocolError::invalid_params("cursor is invalid or expired"))?;
        Ok((state.offset, page_size))
    }

    fn check_rate_limit(&self) -> Result<(), ProtocolError> {
        let maximum = self.inner.configuration.max_calls_per_minute;
        if maximum == 0 {
            return Err(ProtocolError::resource_limit());
        }
        let now = Instant::now();
        let mut calls = self
            .inner
            .rate_limit
            .lock()
            .map_err(|_| ProtocolError::internal())?;
        while calls
            .front()
            .is_some_and(|instant| now.duration_since(*instant) >= Duration::from_secs(60))
        {
            calls.pop_front();
        }
        if calls.len() >= maximum {
            return Err(ProtocolError::resource_limit());
        }
        calls.push_back(now);
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InitializeParams {
    protocol_version: String,
    #[serde(default, rename = "capabilities")]
    _capabilities: Value,
    client_info: ImplementationInfo,
    #[serde(default, rename = "_meta")]
    _meta: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImplementationInfo {
    name: String,
    version: String,
    #[serde(default, rename = "title")]
    _title: Option<String>,
    #[serde(default, rename = "description")]
    _description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CallToolParams {
    name: String,
    #[serde(default = "empty_object")]
    arguments: Value,
    #[serde(default, rename = "_meta")]
    _meta: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancelledParams {
    request_id: Value,
    #[serde(default, rename = "reason")]
    _reason: Option<String>,
    #[serde(default, rename = "_meta")]
    _meta: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PageArguments {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    page_size: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectArguments {
    project_id: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    page_size: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionArguments {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionPageArguments {
    session_id: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    page_size: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SessionListArguments {
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    include_archived: bool,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    page_size: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchArguments {
    command_id: String,
    project_id: String,
    preset_id: String,
    #[serde(default)]
    group_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArguments {
    session_id: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    activity: Option<String>,
    #[serde(default = "default_wait_timeout")]
    timeout_ms: u64,
}

const fn default_wait_timeout() -> u64 {
    30_000
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachArguments {
    session_id: String,
    #[serde(default)]
    from_sequence: u64,
    #[serde(default = "default_columns")]
    columns: u16,
    #[serde(default = "default_rows")]
    rows: u16,
}

const fn default_columns() -> u16 {
    80
}

const fn default_rows() -> u16 {
    24
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelArguments {
    command_id: String,
    session_id: String,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputArguments {
    command_id: String,
    session_id: String,
    input: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeArguments {
    command_id: String,
    session_id: String,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArtifactArguments {
    command_id: String,
    session_id: String,
    display_name: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserCaptureArguments {
    command_id: String,
    session_id: String,
    display_name: String,
    url: String,
}

enum ToolRequest {
    Inspection(InspectionRequest),
    Action(ActionRequest),
}

impl ToolRequest {
    fn fingerprint(&self) -> Result<String, ProtocolError> {
        match self {
            Self::Inspection(request) => {
                serde_json::to_string(request).map_err(|_| ProtocolError::internal())
            }
            Self::Action(request) => Ok(request.fingerprint()),
        }
    }
}

struct ParsedToolArguments {
    request: ToolRequest,
    cursor: Option<String>,
    page_size: Option<usize>,
}

struct ToolDefinition {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    capability: Capability,
    arguments: ToolArguments,
}

#[derive(Clone, Copy)]
enum ToolArguments {
    Empty,
    Project,
    Session,
    SessionList,
    Launch,
    Wait,
    Attach,
    Cancel,
    Input,
    Resume,
    CreateArtifact,
    BrowserCapture,
}

impl ToolDefinition {
    fn schema(&self) -> Value {
        let (mut properties, required, paginated) = match self.arguments {
            ToolArguments::Empty => (json!({}), json!([]), self.name == "termirust_list_projects"),
            ToolArguments::Project => (
                json!({ "project_id": { "type": "string", "format": "uuid" } }),
                json!(["project_id"]),
                true,
            ),
            ToolArguments::Session => (
                json!({ "session_id": { "type": "string", "format": "uuid" } }),
                json!(["session_id"]),
                matches!(
                    self.name,
                    "termirust_list_artifacts" | "termirust_read_transcript"
                ),
            ),
            ToolArguments::SessionList => (
                json!({
                    "project_id": { "type": "string", "format": "uuid" },
                    "state": { "type": "string" },
                    "include_archived": { "type": "boolean", "default": false }
                }),
                json!([]),
                true,
            ),
            ToolArguments::Launch => (
                json!({
                    "command_id": uuid_schema(),
                    "project_id": uuid_schema(),
                    "preset_id": uuid_schema(),
                    "group_id": uuid_schema()
                }),
                json!(["command_id", "project_id", "preset_id"]),
                false,
            ),
            ToolArguments::Wait => (
                json!({
                    "session_id": uuid_schema(),
                    "state": { "type": "string" },
                    "activity": { "type": "string", "enum": ["idle", "busy", "needs_input", "done"] },
                    "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 300000, "default": 30000 }
                }),
                json!(["session_id"]),
                false,
            ),
            ToolArguments::Attach => (
                json!({
                    "session_id": uuid_schema(),
                    "from_sequence": { "type": "integer", "minimum": 0 },
                    "columns": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 80 },
                    "rows": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 24 }
                }),
                json!(["session_id"]),
                false,
            ),
            ToolArguments::Cancel => (
                json!({
                    "command_id": uuid_schema(),
                    "session_id": uuid_schema(),
                    "expected_revision": { "type": "integer", "minimum": 0 }
                }),
                json!(["command_id", "session_id", "expected_revision"]),
                false,
            ),
            ToolArguments::Input => (
                json!({
                    "command_id": uuid_schema(),
                    "session_id": uuid_schema(),
                    "input": { "type": "string", "minLength": 1, "maxLength": 65536 }
                }),
                json!(["command_id", "session_id", "input"]),
                false,
            ),
            ToolArguments::Resume => (
                json!({
                    "command_id": uuid_schema(),
                    "session_id": uuid_schema(),
                    "expected_revision": { "type": "integer", "minimum": 0 }
                }),
                json!(["command_id", "session_id", "expected_revision"]),
                false,
            ),
            ToolArguments::CreateArtifact => (
                json!({
                    "command_id": uuid_schema(),
                    "session_id": uuid_schema(),
                    "display_name": { "type": "string", "minLength": 1, "maxLength": 255 },
                    "content": { "type": "string", "minLength": 1, "maxLength": 65536 }
                }),
                json!(["command_id", "session_id", "display_name", "content"]),
                false,
            ),
            ToolArguments::BrowserCapture => (
                json!({
                    "command_id": uuid_schema(),
                    "session_id": uuid_schema(),
                    "display_name": { "type": "string", "minLength": 1, "maxLength": 255 },
                    "url": { "type": "string", "format": "uri", "minLength": 8, "maxLength": 2048 }
                }),
                json!(["command_id", "session_id", "display_name", "url"]),
                false,
            ),
        };
        if paginated {
            let Some(object) = properties.as_object_mut() else {
                return json!({});
            };
            object.insert(
                "cursor".to_string(),
                json!({ "type": "string", "maxLength": 128 }),
            );
            object.insert(
                "page_size".to_string(),
                json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 50 }),
            );
        }
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "schemaVersion": { "type": "integer", "const": 1 },
                    "data": { "type": "object" },
                    "nextCursor": { "type": ["string", "null"] }
                },
                "required": ["schemaVersion", "data", "nextCursor"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": self.is_read_only(),
                "destructiveHint": self.is_destructive(),
                "idempotentHint": true,
                "openWorldHint": false,
            },
            "execution": { "taskSupport": "forbidden" },
        })
    }

    fn is_read_only(&self) -> bool {
        !matches!(
            self.name,
            "termirust_launch_session"
                | "termirust_cancel_session"
                | "termirust_send_input"
                | "termirust_resume_session"
                | "termirust_create_artifact"
                | "termirust_capture_page_text"
                | "termirust_capture_page_screenshot"
                | "termirust_download_browser_artifact"
        )
    }

    fn is_destructive(&self) -> bool {
        self.name == "termirust_cancel_session"
    }

    fn parse_arguments(&self, value: Value) -> Result<ParsedToolArguments, ProtocolError> {
        match self.name {
            "termirust_status" => {
                parse_value::<EmptyArguments>(value)?;
                Ok(parsed(InspectionRequest::Status, None, None))
            }
            "termirust_list_projects" => {
                let value = parse_value::<PageArguments>(value)?;
                Ok(parsed(
                    InspectionRequest::Projects,
                    value.cursor,
                    value.page_size,
                ))
            }
            "termirust_list_connections" => {
                let value = parse_value::<ProjectArguments>(value)?;
                Ok(parsed(
                    InspectionRequest::Connections {
                        project_id: canonical_project_id(value.project_id)?,
                    },
                    value.cursor,
                    value.page_size,
                ))
            }
            "termirust_list_sessions" => {
                let value = parse_value::<SessionListArguments>(value)?;
                Ok(parsed(
                    InspectionRequest::Sessions {
                        project_id: value.project_id.map(canonical_project_id).transpose()?,
                        state: value.state,
                        include_archived: value.include_archived,
                    },
                    value.cursor,
                    value.page_size,
                ))
            }
            "termirust_get_session" => {
                let value = parse_value::<SessionArguments>(value)?;
                Ok(parsed(
                    InspectionRequest::Session {
                        session_id: canonical_session_id(value.session_id)?,
                    },
                    None,
                    None,
                ))
            }
            "termirust_runtime_status" => {
                let value = parse_value::<SessionArguments>(value)?;
                Ok(parsed(
                    InspectionRequest::RuntimeStatus {
                        session_id: canonical_session_id(value.session_id)?,
                    },
                    None,
                    None,
                ))
            }
            "termirust_list_artifacts" => {
                let value = parse_value::<SessionPageArguments>(value)?;
                Ok(parsed(
                    InspectionRequest::Artifacts {
                        session_id: canonical_session_id(value.session_id)?,
                    },
                    value.cursor,
                    value.page_size,
                ))
            }
            "termirust_read_transcript" => {
                let value = parse_value::<SessionPageArguments>(value)?;
                Ok(parsed(
                    InspectionRequest::Transcript {
                        session_id: canonical_session_id(value.session_id)?,
                    },
                    value.cursor,
                    value.page_size,
                ))
            }
            "termirust_launch_session" => {
                let value = parse_value::<LaunchArguments>(value)?;
                Ok(action(ActionRequest::Launch {
                    command_id: canonical_command_id(value.command_id)?,
                    project_id: canonical_project_id(value.project_id)?,
                    preset_id: canonical_uuid(value.preset_id, "preset_id must be a UUID")?,
                    group_id: value
                        .group_id
                        .map(|id| canonical_uuid(id, "group_id must be a UUID"))
                        .transpose()?,
                }))
            }
            "termirust_wait_session" => {
                let value = parse_value::<WaitArguments>(value)?;
                if value.state.is_some() == value.activity.is_some()
                    || !(1..=300_000).contains(&value.timeout_ms)
                {
                    return Err(ProtocolError::invalid_params(
                        "provide exactly one bounded wait condition",
                    ));
                }
                Ok(action(ActionRequest::Wait {
                    session_id: canonical_session_id(value.session_id)?,
                    state: value.state,
                    activity: value.activity,
                    timeout_ms: value.timeout_ms,
                }))
            }
            "termirust_attach_session" => {
                let value = parse_value::<AttachArguments>(value)?;
                if !(1..=1_000).contains(&value.columns) || !(1..=1_000).contains(&value.rows) {
                    return Err(ProtocolError::invalid_params(
                        "terminal dimensions must be between 1 and 1000",
                    ));
                }
                Ok(action(ActionRequest::Attach {
                    session_id: canonical_session_id(value.session_id)?,
                    from_sequence: value.from_sequence,
                    columns: value.columns,
                    rows: value.rows,
                }))
            }
            "termirust_cancel_session" => {
                let value = parse_value::<CancelArguments>(value)?;
                Ok(action(ActionRequest::Cancel {
                    command_id: canonical_command_id(value.command_id)?,
                    session_id: canonical_session_id(value.session_id)?,
                    expected_revision: value.expected_revision,
                }))
            }
            "termirust_send_input" => {
                let value = parse_value::<InputArguments>(value)?;
                if value.input.is_empty() || value.input.len() > 65_536 {
                    return Err(ProtocolError::invalid_params(
                        "input must contain between 1 and 65536 UTF-8 bytes",
                    ));
                }
                Ok(action(ActionRequest::Input {
                    command_id: canonical_command_id(value.command_id)?,
                    session_id: canonical_session_id(value.session_id)?,
                    input: value.input,
                }))
            }
            "termirust_review_resume" => {
                let value = parse_value::<SessionArguments>(value)?;
                Ok(action(ActionRequest::ResumeReview {
                    session_id: canonical_session_id(value.session_id)?,
                }))
            }
            "termirust_resume_session" => {
                let value = parse_value::<ResumeArguments>(value)?;
                Ok(action(ActionRequest::Resume {
                    command_id: canonical_command_id(value.command_id)?,
                    session_id: canonical_session_id(value.session_id)?,
                    expected_revision: value.expected_revision,
                }))
            }
            "termirust_create_artifact" => {
                let value = parse_value::<CreateArtifactArguments>(value)?;
                if value.content.is_empty()
                    || value.content.len() > 65_536
                    || value.display_name.is_empty()
                    || value.display_name.len() > 255
                {
                    return Err(ProtocolError::invalid_params(
                        "artifact name or content is outside the supported bounds",
                    ));
                }
                Ok(action(ActionRequest::CreateArtifact {
                    command_id: canonical_command_id(value.command_id)?,
                    session_id: canonical_session_id(value.session_id)?,
                    display_name: value.display_name,
                    content: value.content,
                }))
            }
            "termirust_capture_page_text"
            | "termirust_capture_page_screenshot"
            | "termirust_download_browser_artifact" => {
                let value = parse_value::<BrowserCaptureArguments>(value)?;
                if value.url.len() > 2_048
                    || !(value.url.starts_with("https://") || value.url.starts_with("http://"))
                    || value.display_name.is_empty()
                    || value.display_name.len() > 255
                {
                    return Err(ProtocolError::invalid_params(
                        "browser URL or artifact name is outside the supported bounds",
                    ));
                }
                let common = (
                    canonical_command_id(value.command_id)?,
                    canonical_session_id(value.session_id)?,
                    value.display_name,
                    value.url,
                );
                if self.name == "termirust_capture_page_text" {
                    Ok(action(ActionRequest::BrowserText {
                        command_id: common.0,
                        session_id: common.1,
                        display_name: common.2,
                        url: common.3,
                    }))
                } else if self.name == "termirust_capture_page_screenshot" {
                    Ok(action(ActionRequest::BrowserScreenshot {
                        command_id: common.0,
                        session_id: common.1,
                        display_name: common.2,
                        url: common.3,
                    }))
                } else {
                    Ok(action(ActionRequest::BrowserDownload {
                        command_id: common.0,
                        session_id: common.1,
                        display_name: common.2,
                        url: common.3,
                    }))
                }
            }
            _ => Err(ProtocolError::unknown_tool()),
        }
    }
}

const TOOL_DEFINITIONS: [ToolDefinition; 19] = [
    ToolDefinition {
        name: "termirust_status",
        title: "Inspect TermiRust status",
        description: "Read bounded store and Host-control availability metadata.",
        capability: Capability::InspectStatus,
        arguments: ToolArguments::Empty,
    },
    ToolDefinition {
        name: "termirust_list_projects",
        title: "List TermiRust projects",
        description: "List a bounded page of Project metadata without exposing filesystem paths.",
        capability: Capability::ReadProjects,
        arguments: ToolArguments::Empty,
    },
    ToolDefinition {
        name: "termirust_list_connections",
        title: "List Project connections",
        description: "List a bounded page of typed launch presets for one Project; executable details are omitted.",
        capability: Capability::ReadConnections,
        arguments: ToolArguments::Project,
    },
    ToolDefinition {
        name: "termirust_list_sessions",
        title: "List TermiRust sessions",
        description: "List bounded Session metadata and activity, with optional Project and lifecycle filters.",
        capability: Capability::ReadSessions,
        arguments: ToolArguments::SessionList,
    },
    ToolDefinition {
        name: "termirust_get_session",
        title: "Inspect one TermiRust session",
        description: "Read one Session metadata record. Terminal output is never included.",
        capability: Capability::ReadSessions,
        arguments: ToolArguments::Session,
    },
    ToolDefinition {
        name: "termirust_runtime_status",
        title: "Inspect Session runtime status",
        description: "Read the Host-projected lifecycle and activity state for one Session.",
        capability: Capability::ReadRuntime,
        arguments: ToolArguments::Session,
    },
    ToolDefinition {
        name: "termirust_list_artifacts",
        title: "List Session artifacts",
        description: "List bounded inert artifact metadata. Artifact payload bytes are never returned.",
        capability: Capability::ListArtifacts,
        arguments: ToolArguments::Session,
    },
    ToolDefinition {
        name: "termirust_read_transcript",
        title: "Read a semantic Session transcript",
        description: "Read a bounded, secret-redacted page containing only User and Assistant semantic records. Raw terminal output, tool calls, reasoning, and diffs are excluded.",
        capability: Capability::ReadTranscripts,
        arguments: ToolArguments::Session,
    },
    ToolDefinition {
        name: "termirust_launch_session",
        title: "Launch a TermiRust Session",
        description: "Launch one reviewed Project preset with a stable command ID. Requires a current Project-scoped local approval.",
        capability: Capability::LaunchSessions,
        arguments: ToolArguments::Launch,
    },
    ToolDefinition {
        name: "termirust_wait_session",
        title: "Wait for Session state",
        description: "Wait for one bounded lifecycle or semantic activity condition. Requires a current Session-scoped local approval.",
        capability: Capability::WaitSessions,
        arguments: ToolArguments::Wait,
    },
    ToolDefinition {
        name: "termirust_attach_session",
        title: "Inspect Session replay availability",
        description: "Attach read-only, return bounded replay metadata without terminal bytes, then detach.",
        capability: Capability::AttachSessions,
        arguments: ToolArguments::Attach,
    },
    ToolDefinition {
        name: "termirust_cancel_session",
        title: "Cancel a running Session",
        description: "Gracefully stop one Session using an exact revision and stable command ID.",
        capability: Capability::CancelSessions,
        arguments: ToolArguments::Cancel,
    },
    ToolDefinition {
        name: "termirust_send_input",
        title: "Send Session input",
        description: "Send one bounded UTF-8 terminal payload under the current Host writer lease and a stable command ID.",
        capability: Capability::InputSessions,
        arguments: ToolArguments::Input,
    },
    ToolDefinition {
        name: "termirust_review_resume",
        title: "Review Session resume",
        description: "Validate a semantic agent resume and return the exact source revision required for approval.",
        capability: Capability::ReviewResume,
        arguments: ToolArguments::Session,
    },
    ToolDefinition {
        name: "termirust_resume_session",
        title: "Resume a semantic agent Session",
        description: "Create one read-only successor from an exact reviewed source revision and stable command ID.",
        capability: Capability::ResumeSessions,
        arguments: ToolArguments::Resume,
    },
    ToolDefinition {
        name: "termirust_create_artifact",
        title: "Create a Session artifact",
        description: "Store bounded UTF-8 content as inert Session-owned artifact data under a stable command ID.",
        capability: Capability::CreateArtifacts,
        arguments: ToolArguments::CreateArtifact,
    },
    ToolDefinition {
        name: "termirust_capture_page_text",
        title: "Capture semantic page text",
        description: "Open one exact-origin-approved URL in an isolated ephemeral browser and store bounded visible text as an inert Session artifact.",
        capability: Capability::CaptureBrowserText,
        arguments: ToolArguments::BrowserCapture,
    },
    ToolDefinition {
        name: "termirust_capture_page_screenshot",
        title: "Capture a page screenshot",
        description: "Open one exact-origin-approved URL in an isolated ephemeral browser and store a bounded PNG viewport as an inert Session artifact.",
        capability: Capability::CaptureBrowserScreenshot,
        arguments: ToolArguments::BrowserCapture,
    },
    ToolDefinition {
        name: "termirust_download_browser_artifact",
        title: "Download a browser artifact",
        description: "Stream one exact-origin-approved URL through the isolated network boundary into a bounded inert Session artifact without cookies or credentials.",
        capability: Capability::DownloadBrowserArtifact,
        arguments: ToolArguments::BrowserCapture,
    },
];

#[derive(Debug)]
struct ProtocolError {
    code: i32,
    message: &'static str,
    respond: bool,
}

impl ProtocolError {
    fn invalid_request() -> Self {
        Self::new(-32600, "Invalid Request")
    }

    fn method_not_found() -> Self {
        Self::new(-32601, "Method not found")
    }

    fn unknown_tool() -> Self {
        Self::new(-32602, "Unknown or unauthorized tool")
    }

    fn invalid_params(message: &'static str) -> Self {
        Self::new(-32602, message)
    }

    fn internal() -> Self {
        Self::new(-32603, "Internal error")
    }

    fn resource_limit() -> Self {
        Self::new(-32001, "Request rate limit exceeded")
    }

    fn cancelled() -> Self {
        Self {
            code: -32800,
            message: "Request cancelled",
            respond: false,
        }
    }

    fn notification() -> Self {
        Self {
            code: 0,
            message: "notification",
            respond: false,
        }
    }

    const fn new(code: i32, message: &'static str) -> Self {
        Self {
            code,
            message,
            respond: true,
        }
    }
}

fn parse_params<T: for<'de> Deserialize<'de>>(params: Option<&Value>) -> Result<T, ProtocolError> {
    let params = params.cloned().unwrap_or_else(|| json!({}));
    parse_value(params)
}

fn parse_value<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ProtocolError> {
    serde_json::from_value(value).map_err(|_| ProtocolError::invalid_params("Invalid params"))
}

fn parse_optional_empty_params(params: Option<&Value>) -> Result<(), ProtocolError> {
    if let Some(value) = params {
        parse_value::<EmptyArguments>(value.clone())?;
    }
    Ok(())
}

fn require_request(id: Option<&Value>) -> Result<&Value, ProtocolError> {
    id.filter(|value| request_key(value).is_some())
        .ok_or_else(ProtocolError::invalid_request)
}

fn require_notification(id: Option<&Value>) -> Result<(), ProtocolError> {
    if id.is_none() {
        Ok(())
    } else {
        Err(ProtocolError::invalid_request())
    }
}

fn request_key(id: &Value) -> Option<String> {
    match id {
        Value::String(value) if value.len() <= 128 => Some(format!("s:{value}")),
        Value::Number(value) => Some(format!("n:{value}")),
        _ => None,
    }
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: Value, error: ProtocolError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": error.code, "message": error.message },
    })
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true,
    })
}

fn parsed(
    request: InspectionRequest,
    cursor: Option<String>,
    page_size: Option<usize>,
) -> ParsedToolArguments {
    ParsedToolArguments {
        request: ToolRequest::Inspection(request),
        cursor,
        page_size,
    }
}

fn action(request: ActionRequest) -> ParsedToolArguments {
    ParsedToolArguments {
        request: ToolRequest::Action(request),
        cursor: None,
        page_size: None,
    }
}

fn empty_object() -> Value {
    json!({})
}

fn canonical_project_id(value: String) -> Result<String, ProtocolError> {
    value
        .parse::<ProjectId>()
        .map(|id| id.to_string())
        .map_err(|_| ProtocolError::invalid_params("project_id must be a UUID"))
}

fn canonical_session_id(value: String) -> Result<String, ProtocolError> {
    value
        .parse::<HostedSessionId>()
        .map(|id| id.to_string())
        .map_err(|_| ProtocolError::invalid_params("session_id must be a UUID"))
}

fn canonical_command_id(value: String) -> Result<String, ProtocolError> {
    value
        .parse::<termirust_domain::CommandId>()
        .map(|id| id.to_string())
        .map_err(|_| ProtocolError::invalid_params("command_id must be a UUID"))
}

fn canonical_uuid(value: String, message: &'static str) -> Result<String, ProtocolError> {
    value
        .parse::<termirust_domain::CommandId>()
        .map(|id| id.to_string())
        .map_err(|_| ProtocolError::invalid_params(message))
}

fn uuid_schema() -> Value {
    json!({ "type": "string", "format": "uuid" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::InspectionPage;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    #[derive(Default)]
    struct FakeSource;

    impl InspectionSource for FakeSource {
        fn inspect(
            &self,
            request: InspectionRequest,
            offset: usize,
            page_size: usize,
            cancellation: &Cancellation,
        ) -> Result<InspectionPage, SourceError> {
            if cancellation.is_cancelled() {
                return Err(SourceError::Cancelled);
            }
            Ok(InspectionPage {
                data: json!({
                    "request": request,
                    "offset": offset,
                    "page_size": page_size,
                    "secret": "[REDACTED]"
                }),
                next_offset: (offset == 0).then_some(page_size),
            })
        }

        fn act(
            &self,
            request: ActionRequest,
            cancellation: &Cancellation,
        ) -> Result<Value, SourceError> {
            if cancellation.is_cancelled() {
                return Err(SourceError::Cancelled);
            }
            Ok(json!({
                "action": request.kind(),
                "command_id": request.command_id(),
                "accepted": true,
            }))
        }
    }

    fn ready(capabilities: CapabilitySet) -> McpServer {
        let server = McpServer::new(
            Arc::new(FakeSource),
            ServerConfiguration {
                capabilities,
                ..ServerConfiguration::default()
            },
        );
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        });
        assert!(server.process(initialize).is_some());
        assert!(
            server
                .process(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }))
                .is_none()
        );
        server
    }

    fn call(name: &str, arguments: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        })
    }

    #[test]
    fn initialization_advertises_only_tools_and_read_only_instructions() {
        let server = McpServer::new(Arc::new(FakeSource), ServerConfiguration::default());
        let response = server
            .process(json!({
                "jsonrpc": "2.0",
                "id": "init",
                "method": "initialize",
                "params": {
                    "protocolVersion": "2099-01-01",
                    "capabilities": {},
                    "clientInfo": { "name": "host", "version": "1" }
                }
            }))
            .expect("initialize responds");
        assert_eq!(response["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(
            response["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
        assert!(
            response["result"]["capabilities"]
                .get("resources")
                .is_none()
        );
        assert!(
            response["result"]["instructions"]
                .as_str()
                .is_some_and(
                    |value| value.contains("Terminal byte streams") && value.contains("mutation")
                )
        );
    }

    #[test]
    fn default_permissions_hide_transcript_and_artifact_tools() {
        let server = ready(CapabilitySet::default());
        let response = server
            .process(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .expect("tools list responds");
        let names = response["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(!names.contains(&"termirust_read_transcript"));
        assert!(!names.contains(&"termirust_list_artifacts"));
        assert!(!names.contains(&"termirust_launch_session"));
        assert!(!names.contains(&"termirust_send_input"));
        assert!(!names.contains(&"termirust_capture_page_text"));
        assert!(!names.contains(&"termirust_capture_page_screenshot"));
        assert!(names.contains(&"termirust_list_sessions"));
        assert!(response.to_string().contains("readOnlyHint"));
    }

    #[test]
    fn action_tools_are_explicitly_capability_scoped_and_payload_safe() {
        let server = ready(CapabilitySet::all());
        let tools = server
            .process(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .expect("tools list");
        let tools = tools["result"]["tools"].as_array().expect("tools array");
        let input = tools
            .iter()
            .find(|tool| tool["name"] == "termirust_send_input")
            .expect("input tool");
        assert_eq!(input["annotations"]["readOnlyHint"], false);
        assert_eq!(input["annotations"]["destructiveHint"], false);
        assert_eq!(input["inputSchema"]["additionalProperties"], false);
        let cancel = tools
            .iter()
            .find(|tool| tool["name"] == "termirust_cancel_session")
            .expect("cancel tool");
        assert_eq!(cancel["annotations"]["destructiveHint"], true);
        let attach = tools
            .iter()
            .find(|tool| tool["name"] == "termirust_attach_session")
            .expect("attach tool");
        assert_eq!(attach["annotations"]["readOnlyHint"], true);
        let browser = tools
            .iter()
            .find(|tool| tool["name"] == "termirust_capture_page_text")
            .expect("browser text tool");
        assert_eq!(browser["annotations"]["readOnlyHint"], false);
        assert_eq!(browser["inputSchema"]["additionalProperties"], false);

        let browser_response = server
            .process(call(
                "termirust_capture_page_text",
                json!({
                    "command_id": "00000000-0000-0000-0000-000000000007",
                    "session_id": "00000000-0000-0000-0000-000000000002",
                    "display_name": "page.txt",
                    "url": "https://private.example/reviewed"
                }),
            ))
            .expect("browser response");
        assert_eq!(
            browser_response["result"]["structuredContent"]["data"]["accepted"],
            true
        );
        assert!(!browser_response.to_string().contains("private.example"));

        let response = server
            .process(call(
                "termirust_send_input",
                json!({
                    "command_id": "00000000-0000-0000-0000-000000000001",
                    "session_id": "00000000-0000-0000-0000-000000000002",
                    "input": "secret-input-canary\n"
                }),
            ))
            .expect("action response");
        assert_eq!(
            response["result"]["structuredContent"]["data"]["accepted"],
            true
        );
        assert!(!response.to_string().contains("secret-input-canary"));

        let invalid = server
            .process(call(
                "termirust_launch_session",
                json!({
                    "command_id": "not-a-uuid",
                    "project_id": "00000000-0000-0000-0000-000000000002",
                    "preset_id": "00000000-0000-0000-0000-000000000003"
                }),
            ))
            .expect("invalid action response");
        assert_eq!(invalid["error"]["code"], -32602);
    }

    #[test]
    fn unauthorized_tool_call_is_indistinguishable_from_unknown_tool() {
        let server = ready(CapabilitySet::default());
        let response = server
            .process(call(
                "termirust_read_transcript",
                json!({ "session_id": "00000000-0000-0000-0000-000000000001" }),
            ))
            .expect("tool error responds");
        assert_eq!(response["error"]["code"], -32602);
        assert_eq!(response["error"]["message"], "Unknown or unauthorized tool");
    }

    #[test]
    fn cursor_is_opaque_scoped_and_paginated() {
        let server = ready(CapabilitySet::all());
        let first = server
            .process(call("termirust_list_projects", json!({})))
            .expect("first page responds");
        let cursor = first["result"]["structuredContent"]["nextCursor"]
            .as_str()
            .expect("next cursor");
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(cursor)
            .expect("cursor is URL-safe base64");
        assert_eq!(decoded.len(), 18);

        let forged_offset = server
            .process(json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/call",
                "params": {
                    "name": "termirust_list_projects",
                    "arguments": { "cursor": "50" }
                }
            }))
            .expect("forged cursor responds");
        assert_eq!(forged_offset["error"]["code"], -32602);
        let second = server
            .process(json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/call",
                "params": {
                    "name": "termirust_list_projects",
                    "arguments": { "cursor": cursor, "page_size": 10 }
                }
            }))
            .expect("second page responds");
        assert_eq!(second["result"]["structuredContent"]["data"]["offset"], 50);
        let wrong_tool = server
            .process(json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": {
                    "name": "termirust_list_sessions",
                    "arguments": { "cursor": cursor }
                }
            }))
            .expect("wrong cursor responds");
        assert_eq!(wrong_tool["error"]["code"], -32602);
    }

    #[test]
    fn expired_cursor_and_page_limits_fail_closed() {
        let server = McpServer::new(
            Arc::new(FakeSource),
            ServerConfiguration {
                capabilities: CapabilitySet::all(),
                max_cursors: 1,
                ..ServerConfiguration::default()
            },
        );
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        });
        assert!(server.process(initialize).is_some());
        assert!(
            server
                .process(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }))
                .is_none()
        );
        let first = server
            .process(call("termirust_list_projects", json!({})))
            .expect("first cursor");
        let expired = first["result"]["structuredContent"]["nextCursor"]
            .as_str()
            .expect("first token")
            .to_string();
        let _replacement = server
            .process(call("termirust_list_projects", json!({})))
            .expect("replacement cursor");
        let response = server
            .process(call(
                "termirust_list_projects",
                json!({ "cursor": expired }),
            ))
            .expect("expired cursor responds");
        assert_eq!(response["error"]["code"], -32602);
        let response = server
            .process(call("termirust_list_projects", json!({ "page_size": 101 })))
            .expect("oversized page responds");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn rolling_rate_limit_rejects_excess_calls() {
        let server = McpServer::new(
            Arc::new(FakeSource),
            ServerConfiguration {
                capabilities: CapabilitySet::all(),
                max_calls_per_minute: 1,
                ..ServerConfiguration::default()
            },
        );
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        });
        assert!(server.process(initialize).is_some());
        assert!(
            server
                .process(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }))
                .is_none()
        );
        assert!(
            server
                .process(call("termirust_status", json!({})))
                .expect("first call")["result"]
                .is_object()
        );
        let limited = server
            .process(json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/call",
                "params": { "name": "termirust_status" }
            }))
            .expect("rate error");
        assert_eq!(limited["error"]["code"], -32001);
    }

    #[test]
    fn hostile_and_mutating_requests_fail_closed() {
        let server = ready(CapabilitySet::default());
        for message in [
            json!([]),
            json!({ "jsonrpc": "1.0", "id": 1, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": null, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "sessions/launch" }),
            call("termirust_list_projects", json!({ "unexpected": true })),
            call(
                "termirust_get_session",
                json!({ "session_id": "../../private" }),
            ),
        ] {
            let response = server.process(message).expect("request responds");
            assert!(response.get("error").is_some() || response["result"]["isError"] == true);
        }
        let tools = server
            .process(json!({ "jsonrpc": "2.0", "id": 12, "method": "tools/list" }))
            .expect("tools respond");
        let names = tools["result"]["tools"]
            .as_array()
            .expect("tool list")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(names.iter().all(|name| !name.contains("terminal_output")));
        assert!(names.iter().all(|name| !name.contains("send_input")));
        assert!(names.iter().all(|name| !name.contains("launch")));
    }

    struct BlockingSource {
        started: Arc<AtomicBool>,
    }

    impl InspectionSource for BlockingSource {
        fn inspect(
            &self,
            _: InspectionRequest,
            _: usize,
            _: usize,
            cancellation: &Cancellation,
        ) -> Result<InspectionPage, SourceError> {
            self.started.store(true, Ordering::Release);
            while !cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            Err(SourceError::Cancelled)
        }
    }

    #[test]
    fn cancellation_stops_an_in_flight_inspection_and_suppresses_its_response() {
        let started = Arc::new(AtomicBool::new(false));
        let server = McpServer::new(
            Arc::new(BlockingSource {
                started: Arc::clone(&started),
            }),
            ServerConfiguration::default(),
        );
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        });
        assert!(server.process(initialize).is_some());
        assert!(
            server
                .process(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }))
                .is_none()
        );
        let worker_server = server.clone();
        let worker = thread::spawn(move || {
            worker_server.process(json!({
                "jsonrpc": "2.0",
                "id": 77,
                "method": "tools/call",
                "params": { "name": "termirust_status", "arguments": {} }
            }))
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !started.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(started.load(Ordering::Acquire));
        server.cancel(&json!(77));
        assert_eq!(worker.join().expect("worker exits"), None);
    }

    #[test]
    fn cancellation_reserved_before_worker_start_is_not_lost() {
        let server = ready(CapabilitySet::default());
        let message = json!({
            "jsonrpc": "2.0",
            "id": 77,
            "method": "tools/call",
            "params": { "name": "termirust_status", "arguments": {} }
        });
        assert!(server.reserve_tool_call(&message));
        server.cancel(&json!(77));
        assert!(server.process(message).is_none());
        assert!(server.inner.active.lock().expect("active calls").is_empty());
    }

    #[test]
    fn concurrency_limit_rejects_a_second_call_without_disturbing_the_first() {
        let started = Arc::new(AtomicBool::new(false));
        let server = McpServer::new(
            Arc::new(BlockingSource {
                started: Arc::clone(&started),
            }),
            ServerConfiguration {
                max_concurrent_calls: 1,
                ..ServerConfiguration::default()
            },
        );
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        });
        assert!(server.process(initialize).is_some());
        assert!(
            server
                .process(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }))
                .is_none()
        );
        let worker_server = server.clone();
        let worker = thread::spawn(move || {
            worker_server.process(json!({
                "jsonrpc": "2.0",
                "id": 81,
                "method": "tools/call",
                "params": { "name": "termirust_status", "arguments": {} }
            }))
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !started.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(started.load(Ordering::Acquire));
        let second = server
            .process(json!({
                "jsonrpc": "2.0",
                "id": 82,
                "method": "tools/call",
                "params": { "name": "termirust_status", "arguments": {} }
            }))
            .expect("capacity result");
        assert_eq!(second["result"]["isError"], true);
        assert!(
            second["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|value| value.contains("capacity"))
        );
        server.cancel(&json!(81));
        assert_eq!(worker.join().expect("first worker exits"), None);
    }
}
