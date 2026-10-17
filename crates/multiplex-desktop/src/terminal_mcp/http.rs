//! MCP's Streamable HTTP transport, the part of it this server needs: JSON-RPC posted to
//! `/mcp` and answered with one JSON body. No event stream is offered, so `GET` is refused as
//! the specification allows.
//!
//! It listens on 127.0.0.1 only. Every request carries the bearer token, compared in constant
//! time, and a request from a web page (any `Origin` other than this server's own) is refused,
//! so a site the user visits cannot reach the terminals through DNS rebinding.

use std::io::{self, BufRead as _, BufReader, Read as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

use super::ENDPOINT_PATH;
use super::protocol::{McpProtocol, error_response};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 32;
/// Long enough for the longest wait_for_output and a client that keeps its connection open.
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const ACCEPT_POLL: Duration = Duration::from_millis(50);

pub struct HttpServer {
    port: u16,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl HttpServer {
    /// Listens on `port`, or on any free port when that one is taken.
    pub fn start(port: u16, token: String, protocol: Arc<McpProtocol>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .or_else(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let context = Arc::new(Context {
            token,
            port,
            protocol,
            stop: stop.clone(),
            connections: AtomicUsize::new(0),
        });
        let accept = thread::Builder::new()
            .name("terminal-mcp-accept".to_owned())
            .spawn(move || accept_loop(listener, context))?;
        Ok(Self {
            port,
            stop,
            accept: Some(accept),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stops accepting; open connections end with their next request.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct Context {
    token: String,
    port: u16,
    protocol: Arc<McpProtocol>,
    stop: Arc<AtomicBool>,
    connections: AtomicUsize,
}

fn accept_loop(listener: TcpListener, context: Arc<Context>) {
    while !context.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                if context.connections.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                    context.connections.fetch_sub(1, Ordering::AcqRel);
                    let _ = respond(
                        &mut &stream,
                        503,
                        "Service Unavailable",
                        &[],
                        b"too many connections",
                        true,
                    );
                    continue;
                }
                let context = context.clone();
                let spawned = thread::Builder::new()
                    .name("terminal-mcp-connection".to_owned())
                    .spawn({
                        let context = context.clone();
                        move || {
                            let _ = serve_connection(stream, peer, &context);
                            context.connections.fetch_sub(1, Ordering::AcqRel);
                        }
                    });
                if spawned.is_err() {
                    context.connections.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn wants_close(&self) -> bool {
        self.header("connection")
            .is_some_and(|value| value.eq_ignore_ascii_case("close"))
    }
}

enum ReadOutcome {
    Request(Request),
    Closed,
    Refused(u16, &'static str, &'static str),
}

fn serve_connection(stream: TcpStream, _peer: SocketAddr, context: &Context) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = &stream;
    loop {
        if context.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let request = match read_request(&mut reader)? {
            ReadOutcome::Request(request) => request,
            ReadOutcome::Closed => return Ok(()),
            ReadOutcome::Refused(status, reason, message) => {
                return respond(&mut writer, status, reason, &[], message.as_bytes(), true);
            }
        };
        let close = request.wants_close();
        handle(&request, context, &mut writer, close)?;
        if close {
            return Ok(());
        }
    }
}

fn handle(
    request: &Request,
    context: &Context,
    writer: &mut impl io::Write,
    close: bool,
) -> io::Result<()> {
    let path = request.path.split('?').next().unwrap_or_default();
    if path != ENDPOINT_PATH {
        return respond(writer, 404, "Not Found", &[], b"not found", close);
    }
    if !origin_allowed(request.header("origin"), context.port) {
        return respond(writer, 403, "Forbidden", &[], b"origin not allowed", close);
    }
    if !authorized(request.header("authorization"), &context.token) {
        return respond(
            writer,
            401,
            "Unauthorized",
            &[("WWW-Authenticate", "Bearer")],
            b"a bearer token is required",
            close,
        );
    }
    if request.method != "POST" {
        return respond(
            writer,
            405,
            "Method Not Allowed",
            &[("Allow", "POST")],
            b"",
            close,
        );
    }
    let body = match serde_json::from_slice::<Value>(&request.body) {
        Ok(body) => body,
        Err(_) => {
            let error = error_response(Value::Null, -32700, "parse error");
            return respond_json(writer, &error, close);
        }
    };
    match context.protocol.handle_body(body) {
        Some(response) => respond_json(writer, &response, close),
        None => respond(writer, 202, "Accepted", &[], b"", close),
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> io::Result<ReadOutcome> {
    let mut head = Vec::new();
    loop {
        let before = head.len();
        let read = match reader
            .by_ref()
            .take((MAX_HEADER_BYTES + 1 - head.len()) as u64)
            .read_until(b'\n', &mut head)
        {
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(ReadOutcome::Closed);
            }
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(ReadOutcome::Closed);
        }
        if head.len() > MAX_HEADER_BYTES {
            return Ok(ReadOutcome::Refused(
                431,
                "Request Header Fields Too Large",
                "headers too large",
            ));
        }
        let line = &head[before..];
        // Blank lines before a request line are allowed and skipped.
        if (line == b"\r\n" || line == b"\n") && before == 0 {
            head.clear();
            continue;
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let Ok(head) = String::from_utf8(head) else {
        return Ok(ReadOutcome::Refused(400, "Bad Request", "bad request"));
    };
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let (Some(method), Some(path)) = (request_line.next(), request_line.next()) else {
        return Ok(ReadOutcome::Refused(400, "Bad Request", "bad request"));
    };
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    let mut request = Request {
        method: method.to_owned(),
        path: path.to_owned(),
        headers,
        body: Vec::new(),
    };
    if request
        .header("transfer-encoding")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Ok(ReadOutcome::Refused(
            411,
            "Length Required",
            "send Content-Length",
        ));
    }
    let length = match request.header("content-length") {
        None => 0,
        Some(value) => match value.parse::<usize>() {
            Ok(length) if length <= MAX_BODY_BYTES => length,
            Ok(_) => {
                return Ok(ReadOutcome::Refused(
                    413,
                    "Content Too Large",
                    "request body too large",
                ));
            }
            Err(_) => return Ok(ReadOutcome::Refused(400, "Bad Request", "bad length")),
        },
    };
    request.body.resize(length, 0);
    reader.read_exact(&mut request.body)?;
    Ok(ReadOutcome::Request(request))
}

/// No `Origin` (a program, not a browser) or this server's own.
fn origin_allowed(origin: Option<&str>, port: u16) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ]
    .iter()
    .any(|allowed| origin.eq_ignore_ascii_case(allowed))
}

fn authorized(header: Option<&str>, token: &str) -> bool {
    let Some(presented) = header.and_then(|value| {
        let (scheme, credentials) = value.trim().split_once(' ')?;
        scheme
            .eq_ignore_ascii_case("bearer")
            .then_some(credentials.trim())
    }) else {
        return false;
    };
    let (presented, expected) = (presented.as_bytes(), token.as_bytes());
    if presented.len() != expected.len() {
        return false;
    }
    presented
        .iter()
        .zip(expected)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn respond_json(writer: &mut impl io::Write, value: &Value, close: bool) -> io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    respond(
        writer,
        200,
        "OK",
        &[("Content-Type", "application/json")],
        &body,
        close,
    )
}

fn respond(
    writer: &mut impl io::Write,
    status: u16,
    reason: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    close: bool,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    if close {
        head.push_str("Connection: close\r\n");
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_mcp::{OpError, OpResult, TerminalBackend, TerminalOp};
    use serde_json::json;
    use std::io::Write as _;

    struct Empty;

    impl TerminalBackend for Empty {
        fn call(&self, op: TerminalOp, _timeout: Duration) -> OpResult {
            match op {
                TerminalOp::List => Ok(json!([])),
                _ => Err(OpError::NotFound),
            }
        }
    }

    fn server() -> HttpServer {
        HttpServer::start(
            0,
            "secret-token".to_owned(),
            Arc::new(McpProtocol::new(Arc::new(Empty))),
        )
        .unwrap()
    }

    /// Sends one raw request and returns the status line and body.
    fn exchange(port: u16, request: &str) -> (String, String) {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let status = response.lines().next().unwrap_or_default().to_owned();
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_owned())
            .unwrap_or_default();
        (status, body)
    }

    fn post(port: u16, extra_headers: &str, body: &str) -> (String, String) {
        exchange(
            port,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra_headers}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    const AUTH: &str = "Authorization: Bearer secret-token\r\n";

    #[test]
    fn an_authorized_post_is_answered_with_json() {
        let server = server();
        let (status, body) = post(
            server.port(),
            AUTH,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_terminals"}}"#,
        );
        assert_eq!(status, "HTTP/1.1 200 OK");
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["result"]["structuredContent"]["terminals"], json!([]));

        let (status, body) = post(
            server.port(),
            AUTH,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        assert_eq!(status, "HTTP/1.1 202 Accepted");
        assert!(body.is_empty());
        server.stop();
    }

    #[test]
    fn missing_or_wrong_tokens_and_web_origins_are_refused() {
        let server = server();
        let port = server.port();
        let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        assert_eq!(post(port, "", ping).0, "HTTP/1.1 401 Unauthorized");
        assert_eq!(
            post(port, "Authorization: Bearer secret-tokem\r\n", ping).0,
            "HTTP/1.1 401 Unauthorized"
        );
        assert_eq!(
            post(
                port,
                &format!("{AUTH}Origin: https://evil.example\r\n"),
                ping
            )
            .0,
            "HTTP/1.1 403 Forbidden"
        );
        assert_eq!(
            post(
                port,
                &format!("{AUTH}Origin: http://127.0.0.1:{port}\r\n"),
                ping
            )
            .0,
            "HTTP/1.1 200 OK"
        );
        assert_eq!(
            exchange(
                port,
                &format!("GET /mcp HTTP/1.1\r\nConnection: close\r\n{AUTH}\r\n")
            )
            .0,
            "HTTP/1.1 405 Method Not Allowed"
        );
        assert_eq!(
            exchange(port, "GET /other HTTP/1.1\r\nConnection: close\r\n\r\n").0,
            "HTTP/1.1 404 Not Found"
        );
        assert_eq!(
            exchange(
                port,
                &format!(
                    "POST /mcp HTTP/1.1\r\n{AUTH}Content-Length: {}\r\n\r\n",
                    MAX_BODY_BYTES + 1
                )
            )
            .0,
            "HTTP/1.1 413 Content Too Large"
        );
        server.stop();
    }

    #[test]
    fn one_connection_carries_several_requests() {
        let server = server();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port())).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\n{AUTH}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        for _ in 0..2 {
            stream.write_all(request.as_bytes()).unwrap();
            let mut status = String::new();
            reader.read_line(&mut status).unwrap();
            assert_eq!(status.trim_end(), "HTTP/1.1 200 OK");
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length: ") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            assert!(String::from_utf8(body).unwrap().contains("\"result\":{}"));
        }
        server.stop();
    }
}
