use anyhow::{Context, Result, bail};
use std::net::IpAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::models::OutboundProxy;

const PROXY_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;

pub async fn connect_first_hop(
    target_host: &str,
    target_port: u16,
    proxy: Option<&OutboundProxy>,
) -> Result<TcpStream> {
    connect_first_hop_with_timeout(target_host, target_port, proxy, PROXY_TIMEOUT).await
}

async fn connect_first_hop_with_timeout(
    target_host: &str,
    target_port: u16,
    proxy: Option<&OutboundProxy>,
    deadline: Duration,
) -> Result<TcpStream> {
    timeout(
        deadline,
        connect_first_hop_inner(target_host, target_port, proxy),
    )
    .await
    .context("Proxy or target connection timed out")?
}

async fn connect_first_hop_inner(
    target_host: &str,
    target_port: u16,
    proxy: Option<&OutboundProxy>,
) -> Result<TcpStream> {
    match proxy {
        None => TcpStream::connect((target_host, target_port))
            .await
            .context("Unable to reach the SSH target"),
        Some(OutboundProxy::Socks5 { host, port }) => {
            let mut stream = TcpStream::connect((host.as_str(), *port))
                .await
                .with_context(|| format!("Unable to reach SOCKS5 proxy {host}:{port}"))?;
            negotiate_socks5(&mut stream, target_host, target_port).await?;
            Ok(stream)
        }
        Some(OutboundProxy::HttpConnect { host, port }) => {
            let mut stream = TcpStream::connect((host.as_str(), *port))
                .await
                .with_context(|| format!("Unable to reach HTTP CONNECT proxy {host}:{port}"))?;
            negotiate_http_connect(&mut stream, target_host, target_port).await?;
            Ok(stream)
        }
    }
}

async fn negotiate_socks5(
    stream: &mut TcpStream,
    target_host: &str,
    target_port: u16,
) -> Result<()> {
    stream.write_all(&[5, 1, 0]).await?;
    let mut method = [0_u8; 2];
    stream
        .read_exact(&mut method)
        .await
        .context("SOCKS5 proxy closed during method negotiation")?;
    match method {
        [5, 0] => {}
        [5, 0xff] => bail!("SOCKS5 proxy rejected unauthenticated connections"),
        [5, _] => bail!("SOCKS5 proxy requires unsupported authentication"),
        _ => bail!("SOCKS5 proxy returned an invalid negotiation response"),
    }

    let mut request = vec![5, 1, 0];
    match target_host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => {
            request.push(1);
            request.extend_from_slice(&address.octets());
        }
        Ok(IpAddr::V6(address)) => {
            request.push(4);
            request.extend_from_slice(&address.octets());
        }
        Err(_) => {
            let host = target_host.as_bytes();
            if host.is_empty() || host.len() > u8::MAX as usize {
                bail!("SSH target host cannot be encoded for SOCKS5");
            }
            request.extend_from_slice(&[3, host.len() as u8]);
            request.extend_from_slice(host);
        }
    }
    request.extend_from_slice(&target_port.to_be_bytes());
    stream.write_all(&request).await?;

    let mut response = [0_u8; 4];
    stream
        .read_exact(&mut response)
        .await
        .context("SOCKS5 proxy closed during CONNECT")?;
    if response[0] != 5 || response[2] != 0 {
        bail!("SOCKS5 proxy returned an invalid CONNECT response");
    }
    if response[1] != 0 {
        bail!(
            "SOCKS5 proxy denied the SSH target (reply code {})",
            response[1]
        );
    }
    consume_socks_address(stream, response[3]).await?;
    Ok(())
}

async fn consume_socks_address(stream: &mut TcpStream, address_type: u8) -> Result<()> {
    let address_len = match address_type {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0_u8; 1];
            stream.read_exact(&mut len).await?;
            usize::from(len[0])
        }
        _ => bail!("SOCKS5 proxy returned an invalid address type"),
    };
    let mut remainder = vec![0_u8; address_len + 2];
    stream.read_exact(&mut remainder).await?;
    Ok(())
}

async fn negotiate_http_connect(
    stream: &mut TcpStream,
    target_host: &str,
    target_port: u16,
) -> Result<()> {
    if target_host
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        bail!("SSH target host cannot be encoded for HTTP CONNECT");
    }
    let authority = match target_host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{target_host}]:{target_port}"),
        _ => format!("{target_host}:{target_port}"),
    };
    let request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;

    let mut header = Vec::with_capacity(256);
    let mut byte = [0_u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= MAX_HTTP_HEADER_BYTES {
            bail!("HTTP CONNECT proxy response headers exceeded 16 KiB");
        }
        stream
            .read_exact(&mut byte)
            .await
            .context("HTTP CONNECT proxy closed before completing its response")?;
        if byte[0] == 0 {
            bail!("HTTP CONNECT proxy returned an invalid response");
        }
        header.push(byte[0]);
    }
    let header = std::str::from_utf8(&header)
        .context("HTTP CONNECT proxy returned non-UTF-8 response headers")?;
    let status_line = header
        .split("\r\n")
        .next()
        .context("HTTP CONNECT proxy returned an empty response")?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || code.len() != 3
        || !code.bytes().all(|byte| byte.is_ascii_digit())
    {
        bail!("HTTP CONNECT proxy returned an invalid status line");
    }
    let code = code.parse::<u16>()?;
    match code {
        200..=299 => Ok(()),
        407 => bail!("HTTP CONNECT proxy requires unsupported authentication"),
        _ => bail!("HTTP CONNECT proxy denied the SSH target (status {code})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use tokio::net::TcpListener;

    fn run_async_test(test: impl std::future::Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(test);
    }

    #[test]
    fn socks5_preserves_domain_dns_and_tunnel_bytes() {
        run_async_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut greeting = [0; 3];
                stream.read_exact(&mut greeting).await.unwrap();
                assert_eq!(greeting, [5, 1, 0]);
                stream.write_all(&[5, 0]).await.unwrap();
                let mut prefix = [0; 5];
                stream.read_exact(&mut prefix).await.unwrap();
                assert_eq!(&prefix[..4], &[5, 1, 0, 3]);
                let mut rest = vec![0; usize::from(prefix[4]) + 2];
                stream.read_exact(&mut rest).await.unwrap();
                assert_eq!(&rest[..rest.len() - 2], b"ssh.internal");
                stream
                    .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 22])
                    .await
                    .unwrap();
                stream.write_all(b"SSH-BANNER").await.unwrap();
            });
            let proxy = OutboundProxy::Socks5 {
                host: address.ip().to_string(),
                port: address.port(),
            };
            let mut stream = connect_first_hop("ssh.internal", 22, Some(&proxy))
                .await
                .unwrap();
            let mut banner = [0; 10];
            stream.read_exact(&mut banner).await.unwrap();
            assert_eq!(&banner, b"SSH-BANNER");
            server.await.unwrap();
        });
    }

    #[test]
    fn http_connect_does_not_consume_tunnel_bytes() {
        run_async_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("CONNECT ssh.internal:22 HTTP/1.1\r\n"));
                stream
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\nSSH-BANNER")
                    .await
                    .unwrap();
            });
            let proxy = OutboundProxy::HttpConnect {
                host: address.ip().to_string(),
                port: address.port(),
            };
            let mut stream = connect_first_hop("ssh.internal", 22, Some(&proxy))
                .await
                .unwrap();
            let mut banner = [0; 10];
            stream.read_exact(&mut banner).await.unwrap();
            assert_eq!(&banner, b"SSH-BANNER");
            server.await.unwrap();
        });
    }

    #[test]
    fn authentication_challenges_are_explicit_and_contain_no_secret() {
        let (tx, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut greeting = [0; 3];
                stream.read_exact(&mut greeting).await.unwrap();
                stream.write_all(&[5, 2]).await.unwrap();
            });
        });
        let address = rx.recv().unwrap();
        run_async_test(async {
            let proxy = OutboundProxy::Socks5 {
                host: address.ip().to_string(),
                port: address.port(),
            };
            let error = connect_first_hop("secret-target", 22, Some(&proxy))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("unsupported authentication"));
            assert!(!error.contains("secret-target"));
        });
        thread.join().unwrap();
    }

    #[test]
    fn malformed_denied_and_timed_out_proxy_responses_are_distinct() {
        run_async_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut greeting = [0_u8; 3];
                stream.read_exact(&mut greeting).await.unwrap();
                stream.write_all(&[5, 0]).await.unwrap();
                let mut request = [0_u8; 10];
                stream.read_exact(&mut request).await.unwrap();
                stream
                    .write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0])
                    .await
                    .unwrap();
            });
            let proxy = OutboundProxy::Socks5 {
                host: address.ip().to_string(),
                port: address.port(),
            };
            let error = connect_first_hop("127.0.0.1", 22, Some(&proxy))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("denied"));
            server.await.unwrap();

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut byte = [0_u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                }
                stream.write_all(b"NOT HTTP\r\n\r\n").await.unwrap();
            });
            let proxy = OutboundProxy::HttpConnect {
                host: address.ip().to_string(),
                port: address.port(),
            };
            let error = connect_first_hop("127.0.0.1", 22, Some(&proxy))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("invalid status line"));
            server.await.unwrap();

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (_stream, _) = listener.accept().await.unwrap();
                tokio::time::sleep(Duration::from_millis(200)).await;
            });
            let proxy = OutboundProxy::HttpConnect {
                host: address.ip().to_string(),
                port: address.port(),
            };
            let error = connect_first_hop_with_timeout(
                "127.0.0.1",
                22,
                Some(&proxy),
                Duration::from_millis(20),
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(error.contains("timed out"));
            server.abort();
        });
    }

    #[test]
    fn cancelling_proxy_handshake_closes_the_socket() {
        run_async_test(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut byte = [0_u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                }
                ready_tx.send(()).unwrap();
                let read = timeout(Duration::from_secs(1), stream.read(&mut byte))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(read, 0);
            });
            let proxy = OutboundProxy::HttpConnect {
                host: address.ip().to_string(),
                port: address.port(),
            };
            let client =
                tokio::spawn(async move { connect_first_hop("127.0.0.1", 22, Some(&proxy)).await });
            ready_rx.await.unwrap();
            client.abort();
            server.await.unwrap();
        });
    }
}
