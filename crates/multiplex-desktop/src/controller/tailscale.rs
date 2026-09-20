//! This computer's Tailscale name, so pairing can tell the user what to type on a phone.
//!
//! Bonjour does not cross Tailscale, so a phone on the tailnet needs the address typed once.
//! The MagicDNS name survives Tailscale address changes, which makes it the better thing to
//! show; without the `tailscale` command the desktop shows the 100.x address instead.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_STATUS_BYTES: usize = 4 * 1024 * 1024;
const MAX_DNS_NAME_BYTES: usize = 253;

const CANDIDATES: &[&str] = &[
    "tailscale",
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

/// The MagicDNS name of this computer, without the trailing dot, when Tailscale reports one.
pub fn magic_dns_name() -> Option<String> {
    CANDIDATES
        .iter()
        .find_map(|program| status_json(program))
        .and_then(|status| parse_self_dns_name(&status))
}

fn status_json(program: &str) -> Option<Vec<u8>> {
    let mut child = Command::new(program)
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if started.elapsed() > STATUS_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut output = Vec::new();
    use std::io::Read as _;
    child
        .stdout
        .take()?
        .take(MAX_STATUS_BYTES as u64)
        .read_to_end(&mut output)
        .ok()?;
    Some(output)
}

fn parse_self_dns_name(status: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(status).ok()?;
    let name = value
        .get("Self")?
        .get("DNSName")?
        .as_str()?
        .trim_end_matches('.');
    let valid = !name.is_empty()
        && name.len() <= MAX_DNS_NAME_BYTES
        && name.contains('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    valid.then(|| name.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::parse_self_dns_name;

    #[test]
    fn reads_only_a_plain_magic_dns_name() {
        assert_eq!(
            parse_self_dns_name(
                br#"{"Self":{"DNSName":"Jacobs-MacBook.tail1234.ts.net.","HostName":"x"}}"#
            ),
            Some("jacobs-macbook.tail1234.ts.net".into())
        );
        for hostile in [
            br#"{"Self":{"DNSName":""}}"#.as_slice(),
            br#"{"Self":{"DNSName":"localhost"}}"#,
            "{\"Self\":{\"DNSName\":\"evil.ts.net\u{2068}\"}}".as_bytes(),
            br#"{"Self":{"DNSName":"a b.ts.net"}}"#,
            br#"{"Peer":{}}"#,
            b"not json",
        ] {
            assert_eq!(parse_self_dns_name(hostile), None);
        }
    }
}
