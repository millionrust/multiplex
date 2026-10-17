//! The process-wide half: the server's lifecycle, its token and discovery file, and the queue
//! each window answers calls from.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::http::HttpServer;
use super::protocol::McpProtocol;
use super::{ENDPOINT_PATH, OpError, OpResult, TerminalBackend, TerminalOp};

const TOKEN_FILE: &str = "terminal-mcp-token";
const DISCOVERY_FILE: &str = "terminal-mcp.json";

/// A call waiting for a window to answer it.
pub struct PendingCall {
    op: TerminalOp,
    reply: SyncSender<OpResult>,
}

impl PendingCall {
    pub fn op(&self) -> &TerminalOp {
        &self.op
    }

    pub fn respond(self, result: OpResult) {
        let _ = self.reply.send(result);
    }
}

struct WindowEndpoint {
    id: u64,
    calls: Sender<PendingCall>,
    wake: Arc<tokio::sync::Notify>,
}

/// One window's end of the queue. Dropping it, as a closing window does, takes the window out
/// of every later call.
pub struct WindowCalls {
    id: u64,
    calls: Receiver<PendingCall>,
    wake: Arc<tokio::sync::Notify>,
    hub: Arc<TerminalMcpHub>,
}

impl WindowCalls {
    #[cfg(test)]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Resolves once a call may be waiting.
    pub fn waker(&self) -> Arc<tokio::sync::Notify> {
        self.wake.clone()
    }

    pub fn take(&self) -> Vec<PendingCall> {
        self.calls.try_iter().collect()
    }
}

impl Drop for WindowCalls {
    fn drop(&mut self) {
        self.hub.unregister(self.id);
        // The window's loop is waiting on this; let it see the window has gone.
        self.wake.notify_one();
    }
}

/// Where a running server can be reached.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerInfo {
    pub url: String,
    pub token: String,
}

pub struct TerminalMcpHub {
    endpoints: Mutex<Vec<WindowEndpoint>>,
    next_window: AtomicU64,
    server: Mutex<Option<(HttpServer, ServerInfo)>>,
    running: AtomicBool,
    last_error: Mutex<Option<String>>,
}

static HUB: OnceLock<Arc<TerminalMcpHub>> = OnceLock::new();

/// The hub every window in this process shares.
pub fn hub() -> &'static Arc<TerminalMcpHub> {
    HUB.get_or_init(|| Arc::new(TerminalMcpHub::new()))
}

impl TerminalMcpHub {
    fn new() -> Self {
        Self {
            endpoints: Mutex::new(Vec::new()),
            next_window: AtomicU64::new(1),
            server: Mutex::new(None),
            running: AtomicBool::new(false),
            last_error: Mutex::new(None),
        }
    }

    /// A queue for a new window.
    pub fn register_window(self: &Arc<Self>) -> WindowCalls {
        let (calls_tx, calls) = mpsc::channel();
        let wake = Arc::new(tokio::sync::Notify::new());
        let id = self.next_window.fetch_add(1, Ordering::Relaxed);
        self.endpoints.lock().unwrap().push(WindowEndpoint {
            id,
            calls: calls_tx,
            wake: wake.clone(),
        });
        WindowCalls {
            id,
            calls,
            wake,
            hub: self.clone(),
        }
    }

    fn unregister(&self, id: u64) {
        self.endpoints
            .lock()
            .unwrap()
            .retain(|endpoint| endpoint.id != id);
    }

    /// Makes panes keep their output as if the server were listening.
    #[cfg(test)]
    pub fn set_running_for_tests(&self, running: bool) {
        self.running.store(running, Ordering::Relaxed);
    }

    /// Whether the server is listening, so panes know to keep their recent output.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn info(&self) -> Option<ServerInfo> {
        self.server
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, info)| info.clone())
    }

    /// Why the server last failed to start, until it starts or is turned off.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    /// Starts or stops the server. Starting keeps the token from earlier runs in `app_dir` so
    /// clients stay configured, and writes the discovery file the stdio bridge reads.
    pub fn set_enabled(
        self: &Arc<Self>,
        enabled: bool,
        app_dir: &Path,
        port: u16,
    ) -> Result<Option<ServerInfo>, String> {
        let result = self.apply(enabled, app_dir, port);
        *self.last_error.lock().unwrap() = result.as_ref().err().cloned();
        result
    }

    fn apply(
        self: &Arc<Self>,
        enabled: bool,
        app_dir: &Path,
        port: u16,
    ) -> Result<Option<ServerInfo>, String> {
        let mut server = self.server.lock().unwrap();
        if !enabled {
            if let Some((running, _)) = server.take() {
                running.stop();
            }
            self.running.store(false, Ordering::Relaxed);
            let _ = std::fs::remove_file(discovery_path(app_dir));
            return Ok(None);
        }
        if let Some((_, info)) = server.as_ref() {
            return Ok(Some(info.clone()));
        }
        let token = load_or_create_token(app_dir)?;
        let protocol = Arc::new(McpProtocol::new(Arc::new(HubBackend(self.clone()))));
        let http = HttpServer::start(port, token.clone(), protocol)
            .map_err(|error| format!("the terminal MCP server could not listen: {error}"))?;
        let info = ServerInfo {
            url: format!("http://127.0.0.1:{}{ENDPOINT_PATH}", http.port()),
            token,
        };
        write_private(
            &discovery_path(app_dir),
            serde_json::to_vec_pretty(&info)
                .unwrap_or_default()
                .as_slice(),
        )?;
        self.running.store(true, Ordering::Relaxed);
        *server = Some((http, info.clone()));
        Ok(Some(info))
    }

    /// Replaces the token, so clients holding the old one are refused from now on.
    pub fn rotate_token(self: &Arc<Self>, app_dir: &Path, port: u16) -> Result<(), String> {
        let was_running = self.info().is_some();
        self.set_enabled(false, app_dir, port)?;
        let _ = std::fs::remove_file(app_dir.join(TOKEN_FILE));
        if was_running {
            self.set_enabled(true, app_dir, port)?;
        }
        Ok(())
    }

    fn dispatch(&self, op: TerminalOp, timeout: Duration) -> OpResult {
        let endpoints: Vec<(u64, Sender<PendingCall>, Arc<tokio::sync::Notify>)> = self
            .endpoints
            .lock()
            .unwrap()
            .iter()
            .map(|endpoint| (endpoint.id, endpoint.calls.clone(), endpoint.wake.clone()))
            .collect();
        // A new terminal opens in the first window; everything else asks them all.
        let targets = if matches!(op, TerminalOp::Open { .. }) {
            &endpoints[..endpoints.len().min(1)]
        } else {
            &endpoints[..]
        };
        let mut waiting = Vec::new();
        for (id, calls, wake) in targets {
            let (reply, answer) = mpsc::sync_channel(1);
            if calls
                .send(PendingCall {
                    op: op.clone(),
                    reply,
                })
                .is_ok()
            {
                wake.notify_one();
                waiting.push((*id, answer));
            }
        }
        if waiting.is_empty() {
            return Err(OpError::NotFound);
        }
        let deadline = Instant::now() + timeout;
        let mut listed = Vec::new();
        let mut found = None;
        for (window, answer) in waiting {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = match answer.recv_timeout(remaining) {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    Err(OpError::Failed("the app did not answer in time".to_owned()))
                }
                // The window closed before answering.
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(OpError::NotFound),
            };
            if matches!(op, TerminalOp::List) {
                if let Ok(Value::Array(items)) = result {
                    listed.extend(items.into_iter().map(|mut item| {
                        item["window"] = json!(window);
                        item
                    }));
                }
                continue;
            }
            if found.is_none() && result != Err(OpError::NotFound) {
                found = Some(result);
            }
        }
        if matches!(op, TerminalOp::List) {
            return Ok(Value::Array(listed));
        }
        found.unwrap_or(Err(OpError::NotFound))
    }
}

struct HubBackend(Arc<TerminalMcpHub>);

impl TerminalBackend for HubBackend {
    fn call(&self, op: TerminalOp, timeout: Duration) -> OpResult {
        self.0.dispatch(op, timeout)
    }
}

/// Where a running server writes its address and token for the stdio bridge.
pub fn discovery_path(app_dir: &Path) -> PathBuf {
    app_dir.join(DISCOVERY_FILE)
}

pub fn read_discovery(app_dir: &Path) -> Option<ServerInfo> {
    serde_json::from_slice(&std::fs::read(discovery_path(app_dir)).ok()?).ok()
}

fn load_or_create_token(app_dir: &Path) -> Result<String, String> {
    let path = app_dir.join(TOKEN_FILE);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim();
        if existing.len() >= 32 {
            return Ok(existing.to_owned());
        }
    }
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    write_private(&path, token.as_bytes())?;
    Ok(token)
}

/// Writes a file only this user can read.
fn write_private(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(contents)
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use multiplex_domain::HostedSessionId;

    /// Answers every call on its own thread, as a window's loop would.
    fn answer_with(calls: WindowCalls, answer: impl Fn(&TerminalOp) -> OpResult + Send + 'static) {
        std::thread::spawn(move || {
            loop {
                for call in calls.take() {
                    let result = answer(call.op());
                    call.respond(result);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
    }

    #[test]
    fn calls_reach_the_window_that_holds_the_terminal_and_lists_are_merged() {
        let hub = Arc::new(TerminalMcpHub::new());
        let owned = HostedSessionId::new();
        let first = hub.register_window();
        let second = hub.register_window();
        let second_id = second.id();
        answer_with(first, |op| match op {
            TerminalOp::List => Ok(json!([{ "terminal_id": "a" }])),
            _ => Err(OpError::NotFound),
        });
        answer_with(second, move |op| match op {
            TerminalOp::List => Ok(json!([{ "terminal_id": "b" }])),
            TerminalOp::Focus { terminal_id } if *terminal_id == owned => {
                Ok(json!({ "focused": true }))
            }
            _ => Err(OpError::NotFound),
        });

        let listed = hub
            .dispatch(TerminalOp::List, Duration::from_secs(2))
            .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 2);
        assert_eq!(listed[1]["window"], second_id);
        assert_eq!(
            hub.dispatch(
                TerminalOp::Focus { terminal_id: owned },
                Duration::from_secs(2)
            ),
            Ok(json!({ "focused": true }))
        );
        assert_eq!(
            hub.dispatch(
                TerminalOp::Focus {
                    terminal_id: HostedSessionId::new()
                },
                Duration::from_secs(2)
            ),
            Err(OpError::NotFound)
        );
    }

    #[test]
    fn a_closed_window_is_left_out() {
        let hub = Arc::new(TerminalMcpHub::new());
        drop(hub.register_window());
        assert!(hub.endpoints.lock().unwrap().is_empty());
        assert_eq!(
            hub.dispatch(TerminalOp::List, Duration::from_millis(50)),
            Err(OpError::NotFound)
        );
    }

    #[test]
    fn the_token_survives_restarts_and_rotation_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Arc::new(TerminalMcpHub::new());
        let first = hub.set_enabled(true, dir.path(), 0).unwrap().unwrap();
        assert!(hub.is_running());
        assert_eq!(read_discovery(dir.path()), Some(first.clone()));
        hub.set_enabled(false, dir.path(), 0).unwrap();
        assert!(!hub.is_running());
        assert_eq!(read_discovery(dir.path()), None);

        let again = hub.set_enabled(true, dir.path(), 0).unwrap().unwrap();
        assert_eq!(again.token, first.token);
        hub.rotate_token(dir.path(), 0).unwrap();
        let rotated = hub.info().unwrap();
        assert_ne!(rotated.token, first.token);
        hub.set_enabled(false, dir.path(), 0).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join(TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
