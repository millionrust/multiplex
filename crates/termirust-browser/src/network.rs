use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs as _,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use url::Url;

use crate::runtime::BrowserError;

const MAX_ORIGINS: usize = 32;
const MAX_PROXY_CONNECTIONS: usize = 32;
const MAX_PROXY_HEADER_BYTES: usize = 16 * 1024;
const MAX_NETWORK_BYTES: u64 = 32 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct ApprovedOrigin {
    scheme: String,
    host: String,
    port: u16,
}

impl fmt::Debug for ApprovedOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApprovedOrigin")
            .field("scheme", &self.scheme)
            .field("host", &"<redacted>")
            .field("port", &self.port)
            .finish()
    }
}

impl ApprovedOrigin {
    pub fn parse(value: &str) -> Result<Self, BrowserError> {
        let url = parse_network_url(value)?;
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(BrowserError::InvalidPolicy);
        }
        Ok(Self {
            scheme: canonical_scheme(url.scheme())?.to_string(),
            host: url
                .host_str()
                .ok_or(BrowserError::InvalidPolicy)?
                .to_ascii_lowercase(),
            port: url
                .port_or_known_default()
                .ok_or(BrowserError::InvalidPolicy)?,
        })
    }

    pub fn as_string(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default = (self.scheme == "http" && self.port == 80)
            || (self.scheme == "https" && self.port == 443);
        if default {
            format!("{}://{host}", self.scheme)
        } else {
            format!("{}://{host}:{}", self.scheme, self.port)
        }
    }

    pub fn permits_url(&self, value: &str) -> bool {
        let Ok(url) = parse_network_url(value) else {
            return false;
        };
        let Ok(scheme) = canonical_scheme(url.scheme()) else {
            return false;
        };
        url.host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(&self.host))
            && url.port_or_known_default() == Some(self.port)
            && scheme == self.scheme
    }
}

#[derive(Clone)]
pub struct NetworkPolicy {
    origins: BTreeSet<ApprovedOrigin>,
    pinned: Arc<BTreeMap<String, Vec<IpAddr>>>,
}

impl fmt::Debug for NetworkPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkPolicy")
            .field("origin_count", &self.origins.len())
            .finish_non_exhaustive()
    }
}

impl NetworkPolicy {
    pub fn resolve(origins: &[String]) -> Result<Self, BrowserError> {
        Self::resolve_inner(origins, false)
    }

    #[cfg(test)]
    pub(crate) fn resolve_loopback(origins: &[String]) -> Result<Self, BrowserError> {
        Self::resolve_inner(origins, true)
    }

    fn resolve_inner(origins: &[String], permit_local: bool) -> Result<Self, BrowserError> {
        if origins.is_empty() || origins.len() > MAX_ORIGINS {
            return Err(BrowserError::InvalidPolicy);
        }
        let requested_count = origins.len();
        let origins = origins
            .iter()
            .map(|value| ApprovedOrigin::parse(value))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if origins.len() != requested_count || origins.is_empty() {
            return Err(BrowserError::InvalidPolicy);
        }
        let mut pinned = BTreeMap::<String, Vec<IpAddr>>::new();
        for origin in &origins {
            let addresses = (origin.host.as_str(), origin.port)
                .to_socket_addrs()
                .map_err(|_| BrowserError::NetworkDenied)?
                .map(|address| address.ip())
                .collect::<BTreeSet<_>>();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|address| !permit_local && is_non_public(*address))
            {
                return Err(BrowserError::NetworkDenied);
            }
            pinned.insert(origin.host.clone(), addresses.into_iter().collect());
        }
        Ok(Self {
            origins,
            pinned: Arc::new(pinned),
        })
    }

    pub fn permits_url(&self, value: &str) -> bool {
        if matches!(value, "about:blank")
            || value.starts_with("data:")
            || value.starts_with("blob:")
        {
            return true;
        }
        let Ok(url) = parse_network_url(value) else {
            return false;
        };
        let Ok(scheme) = canonical_scheme(url.scheme()) else {
            return false;
        };
        let Some(host) = url.host_str() else {
            return false;
        };
        let Some(port) = url.port_or_known_default() else {
            return false;
        };
        self.origins.contains(&ApprovedOrigin {
            scheme: scheme.to_string(),
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    fn permits_endpoint(&self, scheme: &str, host: &str, port: u16) -> bool {
        self.origins.contains(&ApprovedOrigin {
            scheme: scheme.to_string(),
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    fn endpoint(&self, host: &str, port: u16) -> Result<SocketAddr, BrowserError> {
        let addresses = self
            .pinned
            .get(&host.to_ascii_lowercase())
            .ok_or(BrowserError::NetworkDenied)?;
        addresses
            .first()
            .copied()
            .map(|ip| SocketAddr::new(ip, port))
            .ok_or(BrowserError::NetworkDenied)
    }

    pub(crate) fn start_proxy(&self) -> Result<FilteringProxy, BrowserError> {
        FilteringProxy::start(self.clone())
    }
}

pub(crate) struct FilteringProxy {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FilteringProxy {
    fn start(policy: NetworkPolicy) -> Result<Self, BrowserError> {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| BrowserError::Unavailable)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| BrowserError::Unavailable)?;
        let address = listener
            .local_addr()
            .map_err(|_| BrowserError::Unavailable)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let thread = thread::spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            let transferred = Arc::new(AtomicU64::new(0));
            let mut workers: Vec<thread::JoinHandle<()>> = Vec::new();
            while !worker_stop.load(Ordering::Acquire) {
                let mut index = 0;
                while index < workers.len() {
                    if workers[index].is_finished() {
                        let worker = workers.swap_remove(index);
                        let _ = worker.join();
                    } else {
                        index += 1;
                    }
                }
                match listener.accept() {
                    Ok((stream, _)) if active.load(Ordering::Acquire) < MAX_PROXY_CONNECTIONS => {
                        if stream.set_nonblocking(false).is_err() {
                            let _ = stream.shutdown(Shutdown::Both);
                            continue;
                        }
                        active.fetch_add(1, Ordering::AcqRel);
                        let policy = policy.clone();
                        let stop = worker_stop.clone();
                        let active = active.clone();
                        let transferred = transferred.clone();
                        workers.push(thread::spawn(move || {
                            let _guard = ActiveConnection(active);
                            let _ = handle_proxy_connection(stream, &policy, stop, transferred);
                        }));
                    }
                    Ok((stream, _)) => {
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Ok(Self {
            address,
            stop,
            thread: Some(thread),
        })
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for FilteringProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct ActiveConnection(Arc<AtomicUsize>);

impl Drop for ActiveConnection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn handle_proxy_connection(
    mut client: TcpStream,
    policy: &NetworkPolicy,
    stop: Arc<AtomicBool>,
    transferred: Arc<AtomicU64>,
) -> Result<(), BrowserError> {
    client.set_read_timeout(Some(IO_TIMEOUT)).map_err(map_io)?;
    client.set_write_timeout(Some(IO_TIMEOUT)).map_err(map_io)?;
    let header = read_header(&mut client)?;
    let first_line = header
        .split(|byte| *byte == b'\n')
        .next()
        .ok_or(BrowserError::NetworkDenied)?;
    let first_line = std::str::from_utf8(first_line).map_err(|_| BrowserError::NetworkDenied)?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().ok_or(BrowserError::NetworkDenied)?;
    let target = parts.next().ok_or(BrowserError::NetworkDenied)?;
    if method.eq_ignore_ascii_case("CONNECT") {
        let url =
            Url::parse(&format!("https://{target}/")).map_err(|_| BrowserError::NetworkDenied)?;
        let host = url.host_str().ok_or(BrowserError::NetworkDenied)?;
        let port = url
            .port_or_known_default()
            .ok_or(BrowserError::NetworkDenied)?;
        if !policy.permits_endpoint("https", host, port) {
            return deny(client);
        }
        let upstream = TcpStream::connect_timeout(&policy.endpoint(host, port)?, CONNECT_TIMEOUT)
            .map_err(map_io)?;
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .map_err(map_io)?;
        tunnel(client, upstream, stop, transferred)
    } else {
        let url = Url::parse(target).map_err(|_| BrowserError::NetworkDenied)?;
        let scheme = canonical_scheme(url.scheme())?;
        let host = url.host_str().ok_or(BrowserError::NetworkDenied)?;
        let port = url
            .port_or_known_default()
            .ok_or(BrowserError::NetworkDenied)?;
        if scheme != "http" || !policy.permits_endpoint(scheme, host, port) {
            return deny(client);
        }
        let mut upstream =
            TcpStream::connect_timeout(&policy.endpoint(host, port)?, CONNECT_TIMEOUT)
                .map_err(map_io)?;
        upstream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(map_io)?;
        upstream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(map_io)?;
        let version = parts.next().ok_or(BrowserError::NetworkDenied)?;
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        let first_line_end = header
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or(BrowserError::NetworkDenied)?;
        write!(upstream, "{method} {path} {version}\r\n").map_err(map_io)?;
        upstream
            .write_all(&header[first_line_end.saturating_add(1)..])
            .map_err(map_io)?;
        tunnel(client, upstream, stop, transferred)
    }
}

fn read_header(stream: &mut TcpStream) -> Result<Vec<u8>, BrowserError> {
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    let started = Instant::now();
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) && started.elapsed() < CONNECT_TIMEOUT =>
            {
                continue;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(BrowserError::Timeout);
            }
            Err(error) => return Err(map_io(error)),
        };
        if available.is_empty() {
            return Err(BrowserError::NetworkDenied);
        }
        let take = available
            .len()
            .min(MAX_PROXY_HEADER_BYTES.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(bytes);
        }
        if bytes.len() >= MAX_PROXY_HEADER_BYTES {
            return Err(BrowserError::ResourceLimit);
        }
    }
}

fn deny(mut stream: TcpStream) -> Result<(), BrowserError> {
    let _ = stream
        .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    Err(BrowserError::NetworkDenied)
}

fn tunnel(
    mut client: TcpStream,
    mut upstream: TcpStream,
    stop: Arc<AtomicBool>,
    transferred: Arc<AtomicU64>,
) -> Result<(), BrowserError> {
    let mut upstream_reader = upstream.try_clone().map_err(map_io)?;
    let mut client_writer = client.try_clone().map_err(map_io)?;
    let stop_read = stop.clone();
    let reverse_transferred = transferred.clone();
    let reverse = thread::spawn(move || {
        let result = copy_bounded(
            &mut upstream_reader,
            &mut client_writer,
            &stop_read,
            &reverse_transferred,
        );
        let _ = client_writer.shutdown(Shutdown::Write);
        result
    });
    let forward = copy_bounded(&mut client, &mut upstream, &stop, &transferred);
    let _ = upstream.shutdown(Shutdown::Write);
    let reverse = reverse.join().unwrap_or(Err(BrowserError::Unavailable));
    forward.and(reverse)
}

fn copy_bounded(
    reader: &mut TcpStream,
    writer: &mut TcpStream,
    global_stop: &AtomicBool,
    transferred: &AtomicU64,
) -> Result<(), BrowserError> {
    let mut buffer = [0_u8; 16 * 1024];
    while !global_stop.load(Ordering::Acquire) {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                let total = transferred.fetch_add(count as u64, Ordering::AcqRel) + count as u64;
                if total > MAX_NETWORK_BYTES {
                    return Err(BrowserError::ResourceLimit);
                }
                writer.write_all(&buffer[..count]).map_err(map_io)?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(map_io(error)),
        }
    }
    if global_stop.load(Ordering::Acquire) {
        Err(BrowserError::Cancelled)
    } else {
        Ok(())
    }
}

fn parse_network_url(value: &str) -> Result<Url, BrowserError> {
    if value.len() > 2_048 {
        return Err(BrowserError::InvalidPolicy);
    }
    let url = Url::parse(value).map_err(|_| BrowserError::InvalidPolicy)?;
    canonical_scheme(url.scheme())?;
    if !url.username().is_empty() || url.password().is_some() || url.host_str().is_none() {
        return Err(BrowserError::InvalidPolicy);
    }
    Ok(url)
}

fn canonical_scheme(scheme: &str) -> Result<&str, BrowserError> {
    match scheme {
        "http" | "ws" => Ok("http"),
        "https" | "wss" => Ok("https"),
        _ => Err(BrowserError::InvalidPolicy),
    }
}

fn is_non_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(value) => is_non_public_v4(value),
        IpAddr::V6(value) => is_non_public_v6(value),
    }
}

fn is_non_public_v4(value: Ipv4Addr) -> bool {
    let [a, b, c, _] = value.octets();
    a == 0
        || a == 10
        || (a == 100 && (64..=127).contains(&b))
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
}

fn is_non_public_v6(value: Ipv6Addr) -> bool {
    let segments = value.segments();
    let octets = value.octets();
    let compatible_v4 = (octets[..12] == [0; 12])
        .then(|| Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]));
    value.to_ipv4_mapped().is_some_and(is_non_public_v4)
        || compatible_v4.is_some_and(is_non_public_v4)
        || value.is_unspecified()
        || value.is_loopback()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
        || segments[0..4] == [0x0100, 0, 0, 0]
        || segments[0..2] == [0x0064, 0xff9b]
        || segments[0..2] == [0x2001, 0]
        || segments[0..2] == [0x2001, 0x0db8]
        || segments[0..3] == [0x2001, 0x0002, 0]
        || (segments[0] == 0x2001 && (segments[1] & 0xfff0) == 0x0020)
        || segments[0] == 0x2002
        || value.is_multicast()
}

fn map_io(error: io::Error) -> BrowserError {
    if matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ) {
        BrowserError::Cancelled
    } else {
        BrowserError::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_is_exact_and_credentials_are_rejected() {
        let origin = ApprovedOrigin::parse("https://example.com").expect("valid origin");
        assert_eq!(origin.as_string(), "https://example.com");
        assert!(ApprovedOrigin::parse("https://user@example.com").is_err());
        assert!(ApprovedOrigin::parse("https://example.com/path").is_err());
    }

    #[test]
    fn production_policy_rejects_private_destinations() {
        assert!(NetworkPolicy::resolve(&["http://127.0.0.1:8080".to_string()]).is_err());
        assert!(NetworkPolicy::resolve(&["http://169.254.169.254".to_string()]).is_err());
        for address in [
            "::ffff:127.0.0.1",
            "::192.168.1.1",
            "64:ff9b::7f00:1",
            "2001:0000:4136:e378:8000:63bf:3fff:fdd2",
            "2002:7f00:1::",
        ] {
            let address = address.parse::<Ipv6Addr>().expect("test IPv6 address");
            assert!(is_non_public_v6(address));
        }
    }
}
