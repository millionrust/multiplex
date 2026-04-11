use anyhow::{Context, Result, bail};
use std::process::Command;

const KEYCHAIN_SERVICE: &str = "com.termirust.password";

pub fn profile_password_credential_id(profile_id: &str) -> String {
    format!("profile:{profile_id}")
}

pub fn connection_password_credential_id(username: &str, host: &str, port: u16) -> String {
    format!(
        "connection:{}@{}:{}",
        normalize(username),
        normalize(host),
        port
    )
}

#[cfg(target_os = "macos")]
pub fn store_password(credential_id: &str, password: &str) -> Result<()> {
    let output = Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            credential_id,
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
            password,
        ])
        .output()
        .context("Unable to invoke macOS Keychain")?;

    if output.status.success() {
        Ok(())
    } else {
        bail!(stderr_message(&output.stderr, "Unable to store password in macOS Keychain"));
    }
}

#[cfg(not(target_os = "macos"))]
pub fn store_password(_credential_id: &str, _password: &str) -> Result<()> {
    bail!("Password storage is currently supported only on macOS")
}

#[cfg(target_os = "macos")]
pub fn load_password(credential_id: &str) -> Result<String> {
    let output = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-a",
            credential_id,
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
        ])
        .output()
        .context("Unable to invoke macOS Keychain")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        bail!(stderr_message(
            &output.stderr,
            "No stored password was found in macOS Keychain"
        ));
    }
}

#[cfg(not(target_os = "macos"))]
pub fn load_password(_credential_id: &str) -> Result<String> {
    bail!("Password storage is currently supported only on macOS")
}

#[cfg(target_os = "macos")]
pub fn delete_password(credential_id: &str) -> Result<bool> {
    let output = Command::new("/usr/bin/security")
        .args([
            "delete-generic-password",
            "-a",
            credential_id,
            "-s",
            KEYCHAIN_SERVICE,
        ])
        .output()
        .context("Unable to invoke macOS Keychain")?;

    if output.status.success() {
        Ok(true)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if stderr.contains("could not be found") || stderr.contains("item not found") {
            Ok(false)
        } else {
            bail!(stderr_message(
                &output.stderr,
                "Unable to delete password from macOS Keychain"
            ));
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn delete_password(_credential_id: &str) -> Result<bool> {
    bail!("Password storage is currently supported only on macOS")
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '@') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn stderr_message(stderr: &[u8], fallback: &str) -> String {
    let message = String::from_utf8_lossy(stderr).trim().to_string();
    if message.is_empty() {
        fallback.to_string()
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::{connection_password_credential_id, profile_password_credential_id};

    #[test]
    fn builds_profile_credential_ids() {
        assert_eq!(profile_password_credential_id("abc"), "profile:abc");
    }

    #[test]
    fn normalizes_connection_credential_ids() {
        assert_eq!(
            connection_password_credential_id("Root", "prod.example.com", 22),
            "connection:root@prod.example.com:22"
        );
    }
}
