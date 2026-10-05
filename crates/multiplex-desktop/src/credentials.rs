#[cfg(not(test))]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(test))]
use keyring::{Entry, Error as KeyringError};

/// This app's passwords are stored only under its current identifier.
const SERVICE_NAME: &str = "com.millionrust.multiplex.password";

/// Tests use memory instead of the system credential store.
trait PasswordStore {
    fn get(&self, service: &str, credential_id: &str) -> Result<Option<String>>;
    fn set(&self, service: &str, credential_id: &str, password: &str) -> Result<()>;
    fn delete(&self, service: &str, credential_id: &str) -> Result<bool>;
}

/// Reads a password from this app's current service only.
fn load_from(store: &impl PasswordStore, credential_id: &str) -> Result<String> {
    store
        .get(SERVICE_NAME, credential_id)?
        .ok_or_else(|| anyhow::anyhow!("No stored password was found in {}", secure_store_label()))
}

/// Forgets a password from this app's current service only.
fn delete_from(store: &impl PasswordStore, credential_id: &str) -> Result<bool> {
    store.delete(SERVICE_NAME, credential_id)
}

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

    use anyhow::Result;

    /// Keyed by service as well as credential, like the real store.
    pub(super) fn passwords() -> MutexGuard<'static, HashMap<(String, String), String>> {
        static STORE: OnceLock<Mutex<HashMap<(String, String), String>>> = OnceLock::new();
        STORE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) struct MemoryStore;

    impl super::PasswordStore for MemoryStore {
        fn get(&self, service: &str, credential_id: &str) -> Result<Option<String>> {
            Ok(passwords()
                .get(&(service.to_owned(), credential_id.to_owned()))
                .cloned())
        }

        fn set(&self, service: &str, credential_id: &str, password: &str) -> Result<()> {
            passwords().insert(
                (service.to_owned(), credential_id.to_owned()),
                password.to_owned(),
            );
            Ok(())
        }

        fn delete(&self, service: &str, credential_id: &str) -> Result<bool> {
            Ok(passwords()
                .remove(&(service.to_owned(), credential_id.to_owned()))
                .is_some())
        }
    }

    pub fn store_password(credential_id: &str, password: &str) -> Result<()> {
        use super::PasswordStore as _;
        MemoryStore.set(super::SERVICE_NAME, credential_id, password)
    }

    pub fn load_password(credential_id: &str) -> Result<String> {
        super::load_from(&MemoryStore, credential_id)
    }

    pub fn delete_password(credential_id: &str) -> Result<bool> {
        super::delete_from(&MemoryStore, credential_id)
    }
}

#[cfg(test)]
pub use in_memory::{delete_password, load_password, store_password};

#[cfg(not(test))]
pub fn store_password(credential_id: &str, password: &str) -> Result<()> {
    SystemStore.set(SERVICE_NAME, credential_id, password)
}

#[cfg(not(test))]
pub fn load_password(credential_id: &str) -> Result<String> {
    load_from(&SystemStore, credential_id)
}

#[cfg(not(test))]
pub fn delete_password(credential_id: &str) -> Result<bool> {
    delete_from(&SystemStore, credential_id)
}

#[cfg(not(test))]
struct SystemStore;

#[cfg(not(test))]
impl PasswordStore for SystemStore {
    fn get(&self, service: &str, credential_id: &str) -> Result<Option<String>> {
        match entry(service, credential_id)?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => Err(anyhow::Error::new(error)).with_context(|| {
                format!("Unable to read a password from {}", secure_store_label())
            }),
        }
    }

    fn set(&self, service: &str, credential_id: &str, password: &str) -> Result<()> {
        entry(service, credential_id)?
            .set_password(password)
            .with_context(|| format!("Unable to store password in {}", secure_store_label()))
    }

    fn delete(&self, service: &str, credential_id: &str) -> Result<bool> {
        match entry(service, credential_id)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(error) => Err(anyhow::Error::new(error)).with_context(|| {
                format!("Unable to delete password from {}", secure_store_label())
            }),
        }
    }
}

#[cfg(not(test))]
fn entry(service: &str, credential_id: &str) -> Result<Entry> {
    Entry::new(service, credential_id).context("Unable to initialize credential entry")
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

#[cfg(test)]
mod credential_store_tests {
    use super::in_memory::{MemoryStore, passwords};
    use super::{
        PasswordStore as _, SERVICE_NAME, delete_from, load_from, load_password, store_password,
    };

    #[test]
    fn legacy_passwords_are_not_read_migrated_or_deleted() {
        for (index, legacy) in ["com.multiplex.password", "com.termirust.password"]
            .into_iter()
            .enumerate()
        {
            let id = format!("connection:legacy{index}@example.test:22");
            passwords().insert((legacy.to_owned(), id.clone()), "old".to_owned());
            assert!(load_from(&MemoryStore, &id).is_err());
            assert_eq!(MemoryStore.get(SERVICE_NAME, &id).unwrap(), None);
            assert!(!delete_from(&MemoryStore, &id).unwrap());
            store_password(&id, "new").unwrap();
            assert_eq!(load_password(&id).unwrap(), "new");
            assert!(delete_from(&MemoryStore, &id).unwrap());
            assert_eq!(
                MemoryStore.get(legacy, &id).unwrap().as_deref(),
                Some("old")
            );
        }
    }

    #[test]
    fn current_passwords_can_be_stored_read_and_deleted() {
        let id = "connection:current@example.test:22";
        store_password(id, "correct-horse").unwrap();
        assert_eq!(load_password(id).unwrap(), "correct-horse");
        assert!(delete_from(&MemoryStore, id).unwrap());
        assert!(!delete_from(&MemoryStore, id).unwrap());
        assert!(load_password(id).is_err());
    }
}
