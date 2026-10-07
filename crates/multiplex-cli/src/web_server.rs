//! Opt-in, private-network browser terminal server. Static assets and HTTP run inside Rust.
use crate::CliPaths;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, StatusCode, body::Bytes, service::service_fn};
use hyper_util::rt::TokioIo;
use rand::RngCore as _;
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    net::{SocketAddr, TcpListener},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));
pub const DEFAULT_PORT: u16 = 7417;
const MAX_BODY: usize = 1024 * 1024 + 8192;
type Reply = Response<Full<Bytes>>;

pub struct BrowserServer {
    pub address: SocketAddr,
    pub addresses: Vec<SocketAddr>,
    pub access_code: String,
    shutdown: CancellationToken,
}
impl Drop for BrowserServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}
struct Access {
    paths: CliPaths,
    addresses: Vec<SocketAddr>,
    code: String,
    cookie: String,
    attempts: Mutex<(u8, Instant)>,
    jobs: Semaphore,
}
pub fn start(paths: CliPaths) -> std::io::Result<BrowserServer> {
    let listener = match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, DEFAULT_PORT)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?
        }
        Err(error) => return Err(error),
    };
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let mut listeners = vec![listener];
    let mut addresses = vec![address];
    use multiplex_controller_listener::InterfaceProvider as _;
    if let Ok(interfaces) =
        multiplex_controller_listener::SystemInterfaceProvider.eligible_interfaces()
    {
        for interface in interfaces {
            let target = SocketAddr::new(interface.address, address.port());
            if addresses.contains(&target) {
                continue;
            }
            if let Ok(listener) = TcpListener::bind(target) {
                listener.set_nonblocking(true)?;
                addresses.push(target);
                listeners.push(listener);
            }
        }
    }
    let code = secret();
    let cookie = secret();
    let shutdown = CancellationToken::new();
    let handle = BrowserServer {
        address,
        addresses: addresses.clone(),
        access_code: code.clone(),
        shutdown: shutdown.clone(),
    };
    let access = Arc::new(Access {
        paths,
        addresses,
        code,
        cookie,
        attempts: Mutex::new((0, Instant::now())),
        jobs: Semaphore::new(12),
    });
    let connections = Arc::new(Semaphore::new(64));
    std::thread::Builder::new().name("multiplex-browser-server".into()).spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return; };
        runtime.block_on(async move {
            for listener in listeners {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else { continue; };
                let access = access.clone(); let stop = shutdown.clone();
                let connections = connections.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = stop.cancelled() => break,
                            accepted = listener.accept() => {
                                let Ok((stream,_)) = accepted else { break; };
                                let Ok(permit) = connections.clone().try_acquire_owned() else { continue; };
                                let access = access.clone(); let stop = stop.clone();
                                tokio::spawn(async move {
                                    let _permit = permit;
                                    let mut http = hyper::server::conn::http1::Builder::new();
                                    http.max_buf_size(32 * 1024);
                                    let connection = http.serve_connection(TokioIo::new(stream),service_fn(move |request| {
                                        let access = access.clone(); async move { Ok::<_,Infallible>(handle_request(access,request).await) }
                                    }));
                                    tokio::select! { _ = stop.cancelled() => {}, _ = tokio::time::timeout(Duration::from_secs(30),connection) => {} }
                                });
                            }
                        }
                    }
                });
            }
            shutdown.cancelled().await;
        });
    })?;
    Ok(handle)
}
fn secret() -> String {
    let mut bytes = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn same_secret(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
fn respond(code: StatusCode, body: impl Into<Bytes>, mime: &str) -> Reply {
    Response::builder()
        .status(code)
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .header("X-Frame-Options", "DENY")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .header(
            "Content-Security-Policy",
            "frame-ancestors 'none'; object-src 'none'; base-uri 'self'",
        )
        .body(Full::new(body.into()))
        .unwrap()
}
fn json_response(code: StatusCode, mut value: Value) -> Reply {
    if value.is_object() {
        value["updatedAt"] = json!(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        );
    }
    respond(
        code,
        serde_json::to_vec(&value).unwrap(),
        "application/json",
    )
}
fn error(code: StatusCode, message: &str) -> Reply {
    json_response(code, json!({"message":message,"statusMessage":message}))
}
fn trusted_headers(headers: &hyper::HeaderMap, addresses: &[SocketAddr], mutation: bool) -> bool {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let allowed = addresses
        .iter()
        .map(ToString::to_string)
        .chain(
            addresses
                .first()
                .map(|address| format!("localhost:{}", address.port())),
        )
        .collect::<Vec<_>>();
    if !allowed.contains(&host.to_owned()) {
        return false;
    }
    if matches!(
        headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()),
        Some("cross-site" | "same-site")
    ) {
        return false;
    }
    match headers.get("origin").and_then(|v| v.to_str().ok()) {
        Some(origin) => origin == format!("http://{host}"),
        None => !mutation,
    }
}
fn has_access(headers: &hyper::HeaderMap, cookie: &str) -> bool {
    headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .any(|part| {
            part.trim()
                .strip_prefix("multiplex_browser_access=")
                .is_some_and(|value| same_secret(value, cookie))
        })
}
async fn read_body<B: hyper::body::Body<Data = Bytes> + Unpin>(
    request: Request<B>,
) -> Result<Value, (StatusCode, &'static str)> {
    if !request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
    {
        return Err((StatusCode::UNSUPPORTED_MEDIA_TYPE, "JSON required"));
    }
    let mut body = request.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| (StatusCode::BAD_REQUEST, "Invalid body"))?;
        if let Some(data) = frame.data_ref() {
            if bytes.len() + data.len() > MAX_BODY {
                return Err((StatusCode::PAYLOAD_TOO_LARGE, "Request too large"));
            }
            bytes.extend_from_slice(data);
        }
    }
    serde_json::from_slice(&bytes).map_err(|_| (StatusCode::BAD_REQUEST, "Invalid JSON"))
}
async fn handle_request<B: hyper::body::Body<Data = Bytes> + Unpin>(
    access: Arc<Access>,
    request: Request<B>,
) -> Reply {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    if !trusted_headers(
        request.headers(),
        &access.addresses,
        method != hyper::Method::GET,
    ) {
        return error(StatusCode::FORBIDDEN, "Cross-origin or unknown host denied");
    }
    let authorized = has_access(request.headers(), &access.cookie);
    if path == "/api/auth/status" && method == hyper::Method::GET {
        return json_response(StatusCode::OK, json!({"authorized":authorized}));
    }
    if path == "/api/auth/login" && method == hyper::Method::POST {
        let body = match read_body(request).await {
            Ok(body) => body,
            Err((status, message)) => return error(status, message),
        };
        let mut attempts = access.attempts.lock().await;
        if attempts.1.elapsed() > Duration::from_secs(300) {
            *attempts = (0, Instant::now());
        }
        if attempts.0 >= 5 {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "Wait five minutes before trying again",
            );
        }
        attempts.0 += 1;
        if !body
            .get("code")
            .and_then(Value::as_str)
            .is_some_and(|code| same_secret(code, &access.code))
        {
            return error(StatusCode::UNAUTHORIZED, "Invalid access code");
        }
        *attempts = (0, Instant::now());
        let mut reply = json_response(StatusCode::OK, json!({"ok":true}));
        reply.headers_mut().insert(
            "Set-Cookie",
            format!(
                "multiplex_browser_access={}; HttpOnly; SameSite=Strict; Path=/",
                access.cookie
            )
            .parse()
            .unwrap(),
        );
        return reply;
    }
    if path.starts_with("/api/") && !authorized {
        return error(StatusCode::UNAUTHORIZED, "Browser access code required");
    }
    if path == "/api/auth/logout" && method == hyper::Method::POST {
        let mut reply = json_response(StatusCode::OK, json!({"ok":true}));
        reply.headers_mut().insert(
            "Set-Cookie",
            "multiplex_browser_access=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"
                .parse()
                .unwrap(),
        );
        return reply;
    }
    if !path.starts_with("/api/") {
        if method != hyper::Method::GET {
            return error(StatusCode::METHOD_NOT_ALLOWED, "GET required");
        }
        let asset = if path == "/" || path == "/login" {
            "/index.html"
        } else {
            path.as_str()
        };
        return match embedded(asset) {
            Some((bytes, mime)) => {
                let mut response = respond(StatusCode::OK, Bytes::from_static(bytes), mime);
                if asset.starts_with("/_nuxt/") && !asset.ends_with(".json") {
                    response.headers_mut().insert(
                        "Cache-Control",
                        "public, max-age=31536000, immutable".parse().unwrap(),
                    );
                }
                response
            }
            None => error(StatusCode::NOT_FOUND, "Not found"),
        };
    }
    let Ok(_permit) = access.jobs.try_acquire() else {
        return error(StatusCode::TOO_MANY_REQUESTS, "Viewer busy; retry shortly");
    };
    let mut body = json!({});
    let action;
    if path == "/api/sessions" && method == hyper::Method::GET {
        action = "list";
    } else if let Some(route) = path.strip_prefix("/api/panes/") {
        let parts = route.split('/').collect::<Vec<_>>();
        let Ok(id) = parts[0].parse::<uuid::Uuid>() else {
            return error(StatusCode::BAD_REQUEST, "Invalid terminal id");
        };
        action = match (parts.get(1).copied(), method.as_str()) {
            (None, "GET") => "snapshot",
            (None, "DELETE") => "kill",
            (Some("input"), "POST") => "input",
            (Some("pin"), "POST") => "pin",
            (Some("resize"), "POST") => "resize",
            (Some("rename"), "POST") => "rename",
            _ => return error(StatusCode::NOT_FOUND, "Unknown terminal action"),
        };
        if method == hyper::Method::POST {
            body = match read_body(request).await {
                Ok(body) if body.is_object() => body,
                Ok(_) => return error(StatusCode::BAD_REQUEST, "JSON object required"),
                Err((status, message)) => return error(status, message),
            };
        } else if action == "snapshot" {
            body["plain"] = json!(request.uri().query() == Some("plain=1"));
        }
        body["id"] = json!(id.to_string());
    } else {
        return error(StatusCode::NOT_FOUND, "Not found");
    }
    body["action"] = json!(action);
    let request = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(_) => return error(StatusCode::BAD_REQUEST, "Invalid terminal request"),
    };
    match tokio::time::timeout(
        Duration::from_secs(7),
        crate::browser_api::execute(&access.paths, request),
    )
    .await
    {
        Ok(Ok(value)) => json_response(StatusCode::OK, value),
        Ok(Err(message)) => error(StatusCode::CONFLICT, message),
        Err(_) => error(StatusCode::GATEWAY_TIMEOUT, "Terminal request timed out"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_and_host_validation_reject_cross_site_access() {
        let address = "127.0.0.1:4317".parse().unwrap();
        let mut headers = hyper::HeaderMap::new();
        headers.insert("host", "127.0.0.1:4317".parse().unwrap());
        assert!(trusted_headers(&headers, &[address], false));
        assert!(!trusted_headers(&headers, &[address], true));
        headers.insert("origin", "http://127.0.0.1:4317".parse().unwrap());
        assert!(trusted_headers(&headers, &[address], true));
        headers.insert("origin", "https://attacker.test".parse().unwrap());
        assert!(!trusted_headers(&headers, &[address], false));
        headers.insert("host", "attacker.test:4317".parse().unwrap());
        assert!(!trusted_headers(&headers, &[address], false));
    }
    #[test]
    fn cookies_require_the_full_secret() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("cookie", "multiplex_browser_access=abcdef".parse().unwrap());
        assert!(has_access(&headers, "abcdef"));
        assert!(!has_access(&headers, "abc"));
        assert!(!has_access(&headers, "xbcdef"));
    }
    #[test]
    fn embedded_view_is_available_without_a_javascript_server() {
        assert!(embedded("/index.html").is_some());
        assert!(embedded("/../../Cargo.toml").is_none());
    }
    fn test_access(root: &std::path::Path) -> Arc<Access> {
        Arc::new(Access {
            paths: CliPaths::new(root, root.join("host")),
            addresses: vec!["127.0.0.1:4317".parse().unwrap()],
            code: "test-code".into(),
            cookie: "test-cookie".into(),
            attempts: Mutex::new((0, Instant::now())),
            jobs: Semaphore::new(12),
        })
    }
    fn request(method: &str, path: &str, body: &str, cookie: Option<&str>) -> Request<Full<Bytes>> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("Host", "127.0.0.1:4317")
            .header("Origin", "http://127.0.0.1:4317")
            .header("Content-Type", "application/json");
        if let Some(cookie) = cookie {
            builder = builder.header("Cookie", cookie);
        }
        builder
            .body(Full::new(Bytes::from(body.to_owned())))
            .unwrap()
    }
    #[tokio::test]
    async fn login_gates_terminal_api_and_logout_clears_the_browser_cookie() {
        let root = tempfile::tempdir().unwrap();
        let access = test_access(root.path());
        let reply = handle_request(access.clone(), request("GET", "/api/sessions", "", None)).await;
        assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
        let reply = handle_request(
            access.clone(),
            request("POST", "/api/auth/login", r#"{"code":"wrong"}"#, None),
        )
        .await;
        assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
        let reply = handle_request(
            access.clone(),
            request("POST", "/api/auth/login", r#"{"code":"test-code"}"#, None),
        )
        .await;
        assert_eq!(reply.status(), StatusCode::OK);
        let cookie = reply.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        let reply = handle_request(
            access.clone(),
            request("GET", "/api/sessions", "", Some(cookie)),
        )
        .await;
        assert_eq!(reply.status(), StatusCode::OK);
        let bytes = reply.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["panes"],
            json!([])
        );
        let reply = handle_request(
            access,
            request("POST", "/api/auth/logout", "", Some(cookie)),
        )
        .await;
        assert!(
            reply.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
    }
    #[tokio::test]
    async fn oversized_login_body_and_repeated_failed_logins_are_bounded() {
        let root = tempfile::tempdir().unwrap();
        let access = test_access(root.path());
        let reply = handle_request(
            access.clone(),
            request("POST", "/api/auth/login", &"x".repeat(MAX_BODY + 1), None),
        )
        .await;
        assert_eq!(reply.status(), StatusCode::PAYLOAD_TOO_LARGE);
        for _ in 0..5 {
            assert_eq!(
                handle_request(
                    access.clone(),
                    request("POST", "/api/auth/login", r#"{"code":"wrong"}"#, None)
                )
                .await
                .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            handle_request(
                access,
                request("POST", "/api/auth/login", r#"{"code":"test-code"}"#, None)
            )
            .await
            .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    #[tokio::test]
    async fn authenticated_requests_cannot_use_traversal_or_execute_unknown_actions() {
        let root = tempfile::tempdir().unwrap();
        let access = test_access(root.path());
        let cookie = "multiplex_browser_access=test-cookie";
        assert_eq!(
            handle_request(
                access.clone(),
                request("GET", "/api/panes/not-a-uuid", "", Some(cookie))
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            handle_request(
                access.clone(),
                request("GET", "/../../Cargo.toml", "", None)
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        let request = request(
            "DELETE",
            "/api/panes/00000000-0000-0000-0000-000000000000",
            "",
            Some(cookie),
        );
        let mut request = request;
        request
            .headers_mut()
            .insert("Origin", "https://attacker.test".parse().unwrap());
        assert_eq!(
            handle_request(access, request).await.status(),
            StatusCode::FORBIDDEN
        );
    }
    #[test]
    fn private_network_links_use_exact_bound_hosts_and_same_origin() {
        let addresses: [SocketAddr; 3] = [
            "127.0.0.1:4317".parse().unwrap(),
            "192.168.1.20:4317".parse().unwrap(),
            "100.100.0.20:4317".parse().unwrap(),
        ];
        for address in addresses.iter().skip(1) {
            let mut headers = hyper::HeaderMap::new();
            headers.insert("Host", address.to_string().parse().unwrap());
            headers.insert("Origin", format!("http://{address}").parse().unwrap());
            assert!(trusted_headers(&headers, &addresses, true));
            headers.insert("Origin", "http://127.0.0.1:4317".parse().unwrap());
            assert!(!trusted_headers(&headers, &addresses, true));
        }
        let mut headers = hyper::HeaderMap::new();
        headers.insert("Host", "198.51.100.20:4317".parse().unwrap());
        assert!(!trusted_headers(&headers, &addresses, false));
    }
}
