use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::browser::{
    SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::cdp::browser_protocol::page::NavigateParams;
use chromiumoxide::handler::HandlerConfig;
use chromiumoxide::page::{Page, ScreenshotParams};
use futures::StreamExt as _;

use crate::network::NetworkPolicy;
use crate::process::{OwnedBrowserProcess, discover_browser};

const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: usize = 25 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(20);
const DOCUMENT_READY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
pub struct BrowserCancellation {
    cancelled: Arc<AtomicBool>,
}

impl fmt::Debug for BrowserCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrowserCancellation")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl BrowserCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserArtifactKind {
    SemanticText,
    ScreenshotPng,
    Download,
}

#[derive(Clone, Eq, PartialEq)]
pub struct BrowserArtifact {
    pub kind: BrowserArtifactKind,
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub browser_version: String,
}

impl fmt::Debug for BrowserArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrowserArtifact")
            .field("kind", &self.kind)
            .field(
                "bytes",
                &format_args!("<redacted:{} bytes>", self.bytes.len()),
            )
            .field("truncated", &self.truncated)
            .field("browser_version", &self.browser_version)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserRuntimeStatus {
    Ready,
    Missing,
}

#[derive(Clone, Debug)]
pub struct BrowserRuntimeConfig {
    pub profile_parent: PathBuf,
    pub executable: Option<PathBuf>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct BrowserRequest {
    pub url: String,
    pub approved_origins: Vec<String>,
    pub kind: BrowserArtifactKind,
}

impl fmt::Debug for BrowserRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrowserRequest")
            .field("url", &"<redacted>")
            .field("approved_origin_count", &self.approved_origins.len())
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserError {
    BrowserMissing,
    InvalidPolicy,
    NetworkDenied,
    ResourceLimit,
    Cancelled,
    Timeout,
    Unavailable,
}

impl fmt::Display for BrowserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BrowserMissing => "a supported browser is not installed",
            Self::InvalidPolicy => "browser policy is invalid",
            Self::NetworkDenied => "browser network policy denied the request",
            Self::ResourceLimit => "browser operation exceeded a resource limit",
            Self::Cancelled => "browser operation was cancelled",
            Self::Timeout => "browser operation timed out",
            Self::Unavailable => "browser operation is unavailable",
        })
    }
}

impl std::error::Error for BrowserError {}

pub struct BrowserRuntime {
    config: BrowserRuntimeConfig,
}

impl fmt::Debug for BrowserRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrowserRuntime")
            .field("profile_parent", &"<redacted>")
            .field("executable_configured", &self.config.executable.is_some())
            .finish()
    }
}

impl BrowserRuntime {
    pub fn new(config: BrowserRuntimeConfig) -> Self {
        Self { config }
    }

    pub fn status(&self) -> BrowserRuntimeStatus {
        if discover_browser(self.config.executable.as_deref()).is_ok() {
            BrowserRuntimeStatus::Ready
        } else {
            BrowserRuntimeStatus::Missing
        }
    }

    pub fn capture(
        &self,
        request: BrowserRequest,
        cancellation: &BrowserCancellation,
    ) -> Result<BrowserArtifact, BrowserError> {
        let policy = NetworkPolicy::resolve(&request.approved_origins)?;
        if !policy.permits_url(&request.url) {
            return Err(BrowserError::NetworkDenied);
        }
        self.capture_with_policy(request, policy, cancellation)
    }

    pub fn download(
        &self,
        request: BrowserRequest,
        cancellation: &BrowserCancellation,
    ) -> Result<BrowserArtifact, BrowserError> {
        let policy = NetworkPolicy::resolve(&request.approved_origins)?;
        if request.kind != BrowserArtifactKind::Download || !policy.permits_url(&request.url) {
            return Err(BrowserError::NetworkDenied);
        }
        self.download_with_policy(request, policy, cancellation)
    }

    fn capture_with_policy(
        &self,
        request: BrowserRequest,
        policy: NetworkPolicy,
        cancellation: &BrowserCancellation,
    ) -> Result<BrowserArtifact, BrowserError> {
        if request.kind == BrowserArtifactKind::Download {
            return Err(BrowserError::InvalidPolicy);
        }
        if cancellation.is_cancelled() {
            return Err(BrowserError::Cancelled);
        }
        let executable = discover_browser(self.config.executable.as_deref())?;
        let proxy = policy.start_proxy()?;
        let mut process = OwnedBrowserProcess::launch(
            &executable,
            &self.config.profile_parent,
            proxy.address(),
            cancellation,
        )?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| BrowserError::Unavailable)?;
        let result = runtime.block_on(capture_async(
            process.debug_url(),
            request,
            policy,
            cancellation,
        ));
        process.stop();
        drop(proxy);
        if cancellation.is_cancelled() {
            Err(BrowserError::Cancelled)
        } else {
            result
        }
    }

    fn download_with_policy(
        &self,
        request: BrowserRequest,
        policy: NetworkPolicy,
        cancellation: &BrowserCancellation,
    ) -> Result<BrowserArtifact, BrowserError> {
        if cancellation.is_cancelled() {
            return Err(BrowserError::Cancelled);
        }
        let proxy = policy.start_proxy()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| BrowserError::Unavailable)?;
        let result = runtime.block_on(download_async(
            request.url,
            policy,
            proxy.address(),
            cancellation,
        ));
        drop(proxy);
        if cancellation.is_cancelled() {
            Err(BrowserError::Cancelled)
        } else {
            result
        }
    }
}

async fn download_async(
    initial_url: String,
    policy: NetworkPolicy,
    proxy_address: std::net::SocketAddr,
    cancellation: &BrowserCancellation,
) -> Result<BrowserArtifact, BrowserError> {
    let proxy = reqwest::Proxy::all(format!("http://{proxy_address}"))
        .map_err(|_| BrowserError::Unavailable)?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(OPERATION_TIMEOUT)
        .user_agent("TermiRust isolated browser")
        .build()
        .map_err(|_| BrowserError::Unavailable)?;
    let mut current = url::Url::parse(&initial_url).map_err(|_| BrowserError::InvalidPolicy)?;
    let started = tokio::time::Instant::now();
    for redirect_count in 0..=MAX_REDIRECTS {
        if cancellation.is_cancelled() {
            return Err(BrowserError::Cancelled);
        }
        if !policy.permits_url(current.as_str()) {
            return Err(BrowserError::NetworkDenied);
        }
        let mut response =
            await_reqwest(client.get(current.clone()).send(), cancellation, started).await?;
        if response.status().is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err(BrowserError::ResourceLimit);
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(BrowserError::NetworkDenied)?;
            current = current
                .join(location)
                .map_err(|_| BrowserError::NetworkDenied)?;
            continue;
        }
        if !response.status().is_success() {
            return Err(BrowserError::Unavailable);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
        {
            return Err(BrowserError::ResourceLimit);
        }
        let mut bytes = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(0)
                .min(MAX_DOWNLOAD_BYTES as u64) as usize,
        );
        while let Some(chunk) = await_reqwest(response.chunk(), cancellation, started).await? {
            if cancellation.is_cancelled() {
                return Err(BrowserError::Cancelled);
            }
            if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
                return Err(BrowserError::ResourceLimit);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(BrowserError::Unavailable);
        }
        return Ok(BrowserArtifact {
            kind: BrowserArtifactKind::Download,
            bytes,
            truncated: false,
            browser_version: "bounded-downloader".to_string(),
        });
    }
    Err(BrowserError::ResourceLimit)
}

async fn await_reqwest<T>(
    future: impl Future<Output = Result<T, reqwest::Error>>,
    cancellation: &BrowserCancellation,
    started: tokio::time::Instant,
) -> Result<T, BrowserError> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return result.map_err(map_reqwest_error),
            _ = tokio::time::sleep(Duration::from_millis(25)) => {
                if cancellation.is_cancelled() {
                    return Err(BrowserError::Cancelled);
                }
                if started.elapsed() >= OPERATION_TIMEOUT {
                    return Err(BrowserError::Timeout);
                }
            }
        }
    }
}

fn map_reqwest_error(error: reqwest::Error) -> BrowserError {
    if error.is_timeout() {
        BrowserError::Timeout
    } else {
        BrowserError::Unavailable
    }
}

async fn capture_async(
    debug_url: &str,
    request: BrowserRequest,
    policy: NetworkPolicy,
    cancellation: &BrowserCancellation,
) -> Result<BrowserArtifact, BrowserError> {
    let handler_config = HandlerConfig {
        ignore_https_errors: false,
        ignore_invalid_messages: false,
        viewport: Some(Default::default()),
        context_ids: Vec::new(),
        request_timeout: OPERATION_TIMEOUT,
        request_intercept: true,
        cache_enabled: false,
    };
    let (mut browser, mut handler) = Browser::connect_with_config(debug_url, handler_config)
        .await
        .map_err(|_| BrowserError::Unavailable)?;
    let handler_task = tokio::spawn(async move {
        // A denied child target can report a target-local protocol error. Keep the
        // browser event pump alive so the approved top-level page remains usable.
        while handler.next().await.is_some() {}
    });
    let version = browser
        .version()
        .await
        .map(|value| value.product)
        .unwrap_or_else(|_| "unknown".to_string());
    browser
        .execute(SetDownloadBehaviorParams::new(
            SetDownloadBehaviorBehavior::Deny,
        ))
        .await
        .map_err(|_| BrowserError::Unavailable)?;
    let page = browser
        .new_page("about:blank")
        .await
        .map_err(|_| BrowserError::Unavailable)?;
    page.add_init_script(
        "Object.defineProperty(window, 'open', { value: () => null, configurable: false });",
    )
    .await
    .map_err(|_| BrowserError::Unavailable)?;
    let mut events = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(|_| BrowserError::Unavailable)?;
    let event_page = page.clone();
    let event_policy = policy.clone();
    let interception = tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let method = event.request.method.as_str();
            let permitted = event_policy.permits_url(&event.request.url)
                && matches!(method, "GET" | "HEAD" | "OPTIONS");
            if permitted {
                let _ = event_page
                    .execute(ContinueRequestParams::new(event.request_id.clone()))
                    .await;
            } else {
                let _ = event_page
                    .execute(FailRequestParams::new(
                        event.request_id.clone(),
                        ErrorReason::BlockedByClient,
                    ))
                    .await;
            }
        }
    });
    let operation = async {
        let navigation = page
            .execute(NavigateParams::new(request.url.as_str()))
            .await
            .map_err(|_| BrowserError::NetworkDenied)?;
        if navigation.result.error_text.is_some() {
            return Err(BrowserError::NetworkDenied);
        }
        wait_for_document_body(&page, request.url.as_str(), cancellation).await?;
        match request.kind {
            BrowserArtifactKind::SemanticText => {
                let text: String = page
                    .evaluate("() => document.body ? document.body.innerText : ''")
                    .await
                    .map_err(|_| BrowserError::Unavailable)?
                    .into_value()
                    .map_err(|_| BrowserError::Unavailable)?;
                let mut bytes = text.into_bytes();
                let truncated = bytes.len() > MAX_TEXT_BYTES;
                bytes.truncate(MAX_TEXT_BYTES);
                Ok(BrowserArtifact {
                    kind: BrowserArtifactKind::SemanticText,
                    bytes,
                    truncated,
                    browser_version: version,
                })
            }
            BrowserArtifactKind::ScreenshotPng => {
                let bytes = page
                    .screenshot(ScreenshotParams::builder().full_page(false).build())
                    .await
                    .map_err(|_| BrowserError::Unavailable)?;
                if bytes.len() > MAX_SCREENSHOT_BYTES {
                    return Err(BrowserError::ResourceLimit);
                }
                Ok(BrowserArtifact {
                    kind: BrowserArtifactKind::ScreenshotPng,
                    bytes,
                    truncated: false,
                    browser_version: version,
                })
            }
            BrowserArtifactKind::Download => Err(BrowserError::InvalidPolicy),
        }
    };
    tokio::pin!(operation);
    let started = tokio::time::Instant::now();
    let result = loop {
        tokio::select! {
            value = &mut operation => break value,
            _ = tokio::time::sleep(Duration::from_millis(25)) => {
                if cancellation.is_cancelled() {
                    break Err(BrowserError::Cancelled);
                }
                if started.elapsed() >= OPERATION_TIMEOUT {
                    break Err(BrowserError::Timeout);
                }
            }
        }
    };
    interception.abort();
    let _ = browser.close().await;
    handler_task.abort();
    result
}

async fn wait_for_document_body(
    page: &Page,
    expected_url: &str,
    cancellation: &BrowserCancellation,
) -> Result<(), BrowserError> {
    let expected_url = url::Url::parse(expected_url).map_err(|_| BrowserError::InvalidPolicy)?;
    let started = tokio::time::Instant::now();
    loop {
        if cancellation.is_cancelled() {
            return Err(BrowserError::Cancelled);
        }
        if started.elapsed() >= DOCUMENT_READY_TIMEOUT {
            return Err(BrowserError::Timeout);
        }
        let current_url = tokio::time::timeout(
            Duration::from_millis(500),
            page.evaluate("() => document.body ? location.href : ''"),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|result| result.into_value::<String>().ok())
        .and_then(|current| url::Url::parse(&current).ok());
        let ready = current_url.is_some_and(|current| current == expected_url);
        if ready {
            // Give already-parsed scripts one bounded turn to issue requests. The proxy
            // remains the hard network boundary for child targets and subresources.
            tokio::time::sleep(Duration::from_millis(100)).await;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static LIVE_BROWSER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn read_request_head(stream: &mut std::net::TcpStream) -> std::io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut request = Vec::with_capacity(1024);
        let mut chunk = [0_u8; 1024];
        while request.len() < 16 * 1024 {
            let count = std::io::Read::read(stream, &mut chunk)?;
            if count == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bounded HTTP request header was incomplete",
        ))
    }

    #[test]
    fn debug_output_redacts_urls_and_payloads() {
        let request = BrowserRequest {
            url: "https://secret.example/private".to_string(),
            approved_origins: vec!["https://secret.example".to_string()],
            kind: BrowserArtifactKind::SemanticText,
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains("secret.example"));
        let artifact = BrowserArtifact {
            kind: BrowserArtifactKind::SemanticText,
            bytes: b"secret body".to_vec(),
            truncated: false,
            browser_version: "test".to_string(),
        };
        assert!(!format!("{artifact:?}").contains("secret body"));
    }

    #[test]
    fn browser_status_is_non_panicking() {
        let runtime = BrowserRuntime::new(BrowserRuntimeConfig {
            profile_parent: std::env::temp_dir(),
            executable: None,
        });
        assert!(matches!(
            runtime.status(),
            BrowserRuntimeStatus::Ready | BrowserRuntimeStatus::Missing
        ));
    }

    #[test]
    fn loopback_download_is_bounded_and_inert() {
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                read_request_head(&mut stream).expect("request header");
                let body = b"inert download";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
                let _ = std::io::Write::write_all(&mut stream, body);
            }
        });
        let temp = tempfile::tempdir().expect("temp");
        let origin = format!("http://{address}");
        let policy =
            NetworkPolicy::resolve_loopback(std::slice::from_ref(&origin)).expect("policy");
        let artifact = BrowserRuntime::new(BrowserRuntimeConfig {
            profile_parent: temp.path().to_path_buf(),
            executable: None,
        })
        .download_with_policy(
            BrowserRequest {
                url: origin,
                approved_origins: Vec::new(),
                kind: BrowserArtifactKind::Download,
            },
            policy,
            &BrowserCancellation::default(),
        )
        .expect("download");
        assert_eq!(artifact.bytes, b"inert download");
        server.join().expect("server");
    }

    #[test]
    fn oversized_download_is_rejected_before_body_allocation() {
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                read_request_head(&mut stream).expect("request header");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    MAX_DOWNLOAD_BYTES + 1
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        let temp = tempfile::tempdir().expect("temp");
        let origin = format!("http://{address}");
        let policy =
            NetworkPolicy::resolve_loopback(std::slice::from_ref(&origin)).expect("policy");
        let result = BrowserRuntime::new(BrowserRuntimeConfig {
            profile_parent: temp.path().join("browser-profiles"),
            executable: None,
        })
        .download_with_policy(
            BrowserRequest {
                url: origin,
                approved_origins: Vec::new(),
                kind: BrowserArtifactKind::Download,
            },
            policy,
            &BrowserCancellation::default(),
        );
        assert_eq!(result, Err(BrowserError::ResourceLimit));
        server.join().expect("server");
    }

    #[test]
    fn stalled_download_cancels_promptly() {
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let (headers_tx, headers_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                read_request_head(&mut stream).expect("request header");
                let _ = std::io::Write::write_all(
                    &mut stream,
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nx",
                );
                let _ = headers_tx.send(());
                std::thread::sleep(Duration::from_secs(3));
            }
        });
        let temp = tempfile::tempdir().expect("temp");
        let origin = format!("http://{address}");
        let policy =
            NetworkPolicy::resolve_loopback(std::slice::from_ref(&origin)).expect("policy");
        let cancellation = BrowserCancellation::default();
        let worker_cancellation = cancellation.clone();
        let started = std::time::Instant::now();
        let worker = std::thread::spawn(move || {
            BrowserRuntime::new(BrowserRuntimeConfig {
                profile_parent: temp.path().join("browser-profiles"),
                executable: None,
            })
            .download_with_policy(
                BrowserRequest {
                    url: origin,
                    approved_origins: Vec::new(),
                    kind: BrowserArtifactKind::Download,
                },
                policy,
                &worker_cancellation,
            )
        });
        headers_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("response headers sent");
        cancellation.cancel();
        assert_eq!(
            worker.join().expect("download worker"),
            Err(BrowserError::Cancelled)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        server.join().expect("server");
    }

    #[test]
    fn redirect_to_unapproved_origin_is_denied_before_connection() {
        let blocked = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("blocked listener");
        blocked.set_nonblocking(true).expect("blocked nonblocking");
        let blocked_address = blocked.local_addr().expect("blocked address");
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                read_request_head(&mut stream).expect("request header");
                let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{blocked_address}/denied\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        let temp = tempfile::tempdir().expect("temp");
        let origin = format!("http://{address}");
        let policy =
            NetworkPolicy::resolve_loopback(std::slice::from_ref(&origin)).expect("policy");
        let result = BrowserRuntime::new(BrowserRuntimeConfig {
            profile_parent: temp.path().join("browser-profiles"),
            executable: None,
        })
        .download_with_policy(
            BrowserRequest {
                url: origin,
                approved_origins: Vec::new(),
                kind: BrowserArtifactKind::Download,
            },
            policy,
            &BrowserCancellation::default(),
        );
        assert_eq!(result, Err(BrowserError::NetworkDenied));
        server.join().expect("server");
        assert!(
            blocked.accept().is_err(),
            "redirect reached an unapproved origin"
        );
    }

    #[test]
    #[ignore = "requires an installed Chromium-family browser"]
    fn live_loopback_capture_uses_ephemeral_profile() {
        let _live = LIVE_BROWSER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let blocked = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("blocked listener");
        blocked.set_nonblocking(true).expect("blocked nonblocking");
        let blocked_address = blocked.local_addr().expect("blocked address");
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("listener");
        listener.set_nonblocking(true).expect("nonblocking");
        let address = listener.local_addr().expect("address");
        let server_stop = Arc::new(AtomicBool::new(false));
        let worker_stop = server_stop.clone();
        let server = std::thread::spawn(move || {
            let body = format!(
                "<html><body>N13 LIVE MARKER\
                 <img src=\"http://{blocked_address}/image\">\
                 <iframe src=\"http://{blocked_address}/frame\"></iframe>\
                 <script>fetch('http://{blocked_address}/fetch').catch(() => {{}});\
                 new WebSocket('ws://{blocked_address}/socket');\
                 window.open('http://{blocked_address}/popup');</script>\
                 </body></html>"
            );
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        read_request_head(&mut stream).expect("request header");
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
                        let _ = std::io::Write::write_all(&mut stream, body.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        let temp = tempfile::tempdir().expect("temp");
        let profile_parent = temp.path().join("browser-profiles");
        let runtime = BrowserRuntime::new(BrowserRuntimeConfig {
            profile_parent: profile_parent.clone(),
            executable: None,
        });
        if runtime.status() == BrowserRuntimeStatus::Missing {
            panic!("explicit live browser test requires an installed browser");
        }
        let origin = format!("http://{address}");
        let request = BrowserRequest {
            url: origin.clone(),
            approved_origins: vec![origin.clone()],
            kind: BrowserArtifactKind::SemanticText,
        };
        let policy = NetworkPolicy::resolve_loopback(std::slice::from_ref(&origin))
            .expect("loopback policy");
        let capture = runtime.capture_with_policy(request, policy, &BrowserCancellation::default());
        let screenshot = runtime.capture_with_policy(
            BrowserRequest {
                url: origin.clone(),
                approved_origins: Vec::new(),
                kind: BrowserArtifactKind::ScreenshotPng,
            },
            NetworkPolicy::resolve_loopback(std::slice::from_ref(&origin))
                .expect("screenshot policy"),
            &BrowserCancellation::default(),
        );
        server_stop.store(true, Ordering::Release);
        server.join().expect("server");
        let artifact = capture.expect("capture");
        assert_eq!(artifact.bytes, b"N13 LIVE MARKER");
        let screenshot = screenshot.expect("screenshot");
        assert_eq!(screenshot.kind, BrowserArtifactKind::ScreenshotPng);
        assert!(screenshot.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(screenshot.bytes.len() <= MAX_SCREENSHOT_BYTES);
        assert!(
            blocked.accept().is_err(),
            "unapproved subresource reached its destination"
        );
        assert_eq!(
            std::fs::read_dir(profile_parent)
                .expect("profile root")
                .count(),
            0
        );
    }

    #[test]
    #[ignore = "requires an installed Chromium-family browser"]
    fn live_stalled_navigation_cancels_and_removes_profile() {
        let _live = LIVE_BROWSER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 4096];
                read_request_head(&mut stream).expect("request header");
                let _ = accepted_tx.send(());
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let started = std::time::Instant::now();
                while started.elapsed() < Duration::from_secs(10) {
                    match std::io::Read::read(&mut stream, &mut request) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Err(_) => break,
                    }
                }
            }
        });
        let temp = tempfile::tempdir().expect("temp");
        let profile_parent = temp.path().join("browser-profiles");
        let profile_check = profile_parent.clone();
        let origin = format!("http://{address}");
        let policy =
            NetworkPolicy::resolve_loopback(std::slice::from_ref(&origin)).expect("policy");
        let cancellation = BrowserCancellation::default();
        let worker_cancellation = cancellation.clone();
        let worker = std::thread::spawn(move || {
            BrowserRuntime::new(BrowserRuntimeConfig {
                profile_parent,
                executable: None,
            })
            .capture_with_policy(
                BrowserRequest {
                    url: origin,
                    approved_origins: Vec::new(),
                    kind: BrowserArtifactKind::SemanticText,
                },
                policy,
                &worker_cancellation,
            )
        });
        accepted_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("navigation reached fixture");
        let started = std::time::Instant::now();
        cancellation.cancel();
        assert_eq!(
            worker.join().expect("capture worker"),
            Err(BrowserError::Cancelled)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        server.join().expect("server");
        assert_eq!(
            std::fs::read_dir(profile_check)
                .expect("profile root")
                .count(),
            0
        );
    }
}
