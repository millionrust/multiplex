#[cfg(not(test))]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(test))]
use keyring::{Entry, Error as KeyringError};

/// Where this app's passwords live in the system credential store, and where an installed copy
/// put them before the rename. Both are needed: the store is keyed by this name, so a password
/// saved under the old one is invisible under the new.
const SERVICE_NAME: &str = "com.multiplex.password";
const LEGACY_SERVICE_NAME: &str = "com.termirust.password";

/// The system credential store, as this module uses it. One implementation talks to the real
/// store; the tests keep passwords in memory so they never read or write a developer's own, and
/// so a machine without a credential store still runs them. Both go through the same lookup
/// below, so the move from the old service name is exercised rather than assumed.
trait PasswordStore {
    fn get(&self, service: &str, credential_id: &str) -> Result<Option<String>>;
    fn set(&self, service: &str, credential_id: &str, password: &str) -> Result<()>;
    fn delete(&self, service: &str, credential_id: &str) -> Result<bool>;
}

/// Reads a password, bringing one saved under the old service name across as it goes.
///
/// The copy is written before the original is removed, so a failure in between leaves the
/// password readable under one name rather than neither. A failure to remove the old one is not
/// worth refusing the password over: the next read finds it under the new name and does not
/// come back here.
fn load_from(store: &impl PasswordStore, credential_id: &str) -> Result<String> {
    if let Some(password) = store.get(SERVICE_NAME, credential_id)? {
        return Ok(password);
    }
    let Some(password) = store.get(LEGACY_SERVICE_NAME, credential_id)? else {
        return Err(anyhow::anyhow!(
            "No stored password was found in {}",
            secure_store_label()
        ));
    };
    store.set(SERVICE_NAME, credential_id, &password)?;
    let _ = store.delete(LEGACY_SERVICE_NAME, credential_id);
    Ok(password)
}

/// Forgets a password under both names, so removing one does not leave the old copy behind.
fn delete_from(store: &impl PasswordStore, credential_id: &str) -> Result<bool> {
    let current = store.delete(SERVICE_NAME, credential_id)?;
    let legacy = store.delete(LEGACY_SERVICE_NAME, credential_id)?;
    Ok(current || legacy)
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

    /// Keyed by service as well as credential, like the real store, so the move from the old
    /// service name is exercised here rather than only in production.
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
        LEGACY_SERVICE_NAME, PasswordStore as _, SERVICE_NAME, delete_from, load_from,
        load_password, store_password,
    };

    /// A password saved before the rename still opens a host, and moves to this app's name as it
    /// is read: the store is keyed by that name, so one saved under the old one is otherwise
    /// invisible and the user is asked for a password they already gave.
    #[test]
    fn a_password_saved_under_the_previous_name_is_found_and_brought_across() {
        let id = "connection:migrated@example.test:22";
        passwords().insert(
            (LEGACY_SERVICE_NAME.to_owned(), id.to_owned()),
            "hunter2".to_owned(),
        );

        assert_eq!(
            load_from(&MemoryStore, id).expect("the password"),
            "hunter2"
        );
        assert_eq!(
            MemoryStore.get(SERVICE_NAME, id).expect("a read"),
            Some("hunter2".to_owned()),
            "it is stored under this app's name afterwards"
        );
        assert_eq!(
            MemoryStore.get(LEGACY_SERVICE_NAME, id).expect("a read"),
            None,
            "and not left behind under the old one"
        );
    }

    #[test]
    fn a_password_saved_now_is_read_without_touching_the_previous_name() {
        let id = "connection:current@example.test:22";
        store_password(id, "correct-horse").expect("the password is stored");
        assert_eq!(
            MemoryStore.get(LEGACY_SERVICE_NAME, id).expect("a read"),
            None
        );
        assert_eq!(load_password(id).expect("the password"), "correct-horse");
    }

    #[test]
    fn forgetting_a_password_forgets_the_one_saved_under_the_previous_name_too() {
        let id = "profile:both";
        passwords().insert(
            (LEGACY_SERVICE_NAME.to_owned(), id.to_owned()),
            "old".to_owned(),
        );
        store_password(id, "new").expect("the password is stored");

        assert!(delete_from(&MemoryStore, id).expect("the delete runs"));
        assert_eq!(MemoryStore.get(SERVICE_NAME, id).expect("a read"), None);
        assert_eq!(
            MemoryStore.get(LEGACY_SERVICE_NAME, id).expect("a read"),
            None
        );
        assert!(!delete_from(&MemoryStore, id).expect("the delete runs"));
    }
}
