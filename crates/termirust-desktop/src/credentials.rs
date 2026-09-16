#[cfg(not(test))]
use anyhow::{Context, Result};
#[cfg(not(test))]
use keyring::{Entry, Error as KeyringError};

const SERVICE_NAME: &str = "com.termirust.password";

pub fn secure_store_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macOS Keychain"
    }
    #[cfg(target_os = "windows")]
    {
        "Windows Credential Manager"
    }
    #[cfg(target_os = "linux")]
    {
        "system credential store"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        "system credential store"
    }
}

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

/// Tests keep passwords in memory: they must not read or write a developer's own credential
/// store, and a machine without one, such as a CI runner with no Secret Service, still runs
/// them.
#[cfg(test)]
mod in_memory {
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use anyhow::{Result, anyhow};

    fn passwords() -> MutexGuard<'static, HashMap<String, String>> {
        static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        STORE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn store_password(credential_id: &str, password: &str) -> Result<()> {
        passwords().insert(credential_id.to_owned(), password.to_owned());
        Ok(())
    }

    pub fn load_password(credential_id: &str) -> Result<String> {
        passwords().get(credential_id).cloned().ok_or_else(|| {
            anyhow!(
                "No stored password was found in {}",
                super::secure_store_label()
            )
        })
    }

    pub fn delete_password(credential_id: &str) -> Result<bool> {
        Ok(passwords().remove(credential_id).is_some())
    }
}

#[cfg(test)]
pub use in_memory::{delete_password, load_password, store_password};

#[cfg(not(test))]
pub fn store_password(credential_id: &str, password: &str) -> Result<()> {
    entry(credential_id)?
        .set_password(password)
        .with_context(|| format!("Unable to store password in {}", secure_store_label()))?;
    Ok(())
}

#[cfg(not(test))]
pub fn load_password(credential_id: &str) -> Result<String> {
    entry(credential_id)?
        .get_password()
        .with_context(|| format!("No stored password was found in {}", secure_store_label()))
}

#[cfg(not(test))]
pub fn delete_password(credential_id: &str) -> Result<bool> {
    let entry = entry(credential_id)?;
    match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(KeyringError::NoEntry) => Ok(false),
        Err(error) => Err(anyhow::Error::new(error))
            .with_context(|| format!("Unable to delete password from {}", secure_store_label())),
    }
}

#[cfg(not(test))]
fn entry(credential_id: &str) -> Result<Entry> {
    Entry::new(SERVICE_NAME, credential_id).context("Unable to initialize credential entry")
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
