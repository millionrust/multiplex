//! Moving a stored secret from the service names earlier versions used to this one's.
//!
//! The system credential store is keyed by service name, so a secret saved under an old name is
//! invisible under the new one, and the name is what the operating system shows the person when
//! it asks to unlock it. Every store here reads through [`password`] or [`secret`], which look
//! under the current name first, then under each name an earlier version used, and bring what
//! they find across.
//!
//! The copy is written before the original is removed, so a failure in between leaves the secret
//! readable under one name rather than neither, and a failure to remove the old one is not worth
//! refusing the secret over: the next read finds it under the new name and never comes back here.
//! Deleting removes every name, so an item left behind by a failed removal cannot come back.

use zeroize::Zeroize as _;

pub use keyring::Error as CredentialError;

/// The service name a secret is stored under now, and the names earlier versions used, newest
/// first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceNames {
    pub current: &'static str,
    pub legacy: &'static [&'static str],
}

impl ServiceNames {
    pub const fn new(current: &'static str, legacy: &'static [&'static str]) -> Self {
        Self { current, legacy }
    }

    fn all(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.current).chain(self.legacy.iter().copied())
    }
}

/// The system credential store, as this module uses it. The tests put secrets in memory instead,
/// so they never read or write a developer's own store and so the move between names is
/// exercised rather than assumed.
pub trait CredentialStore {
    fn password(&self, service: &str, account: &str) -> Result<Option<String>, CredentialError>;
    fn set_password(
        &self,
        service: &str,
        account: &str,
        value: &str,
    ) -> Result<(), CredentialError>;
    fn secret(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, CredentialError>;
    fn set_secret(&self, service: &str, account: &str, value: &[u8])
    -> Result<(), CredentialError>;
    /// `true` when there was one to remove.
    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialError>;
}

/// A text secret, brought across from an earlier name if that is where it still is.
pub fn password(
    store: &impl CredentialStore,
    names: ServiceNames,
    account: &str,
) -> Result<Option<String>, CredentialError> {
    if let Some(value) = store.password(names.current, account)? {
        return Ok(Some(value));
    }
    for legacy in names.legacy {
        let Some(value) = store.password(legacy, account)? else {
            continue;
        };
        if store.set_password(names.current, account, &value).is_ok() {
            let _ = store.delete(legacy, account);
        }
        return Ok(Some(value));
    }
    Ok(None)
}

/// The same for a secret held as bytes rather than text.
pub fn secret(
    store: &impl CredentialStore,
    names: ServiceNames,
    account: &str,
) -> Result<Option<Vec<u8>>, CredentialError> {
    if let Some(value) = store.secret(names.current, account)? {
        return Ok(Some(value));
    }
    for legacy in names.legacy {
        let Some(value) = store.secret(legacy, account)? else {
            continue;
        };
        if store.set_secret(names.current, account, &value).is_ok() {
            let _ = store.delete(legacy, account);
        }
        return Ok(Some(value));
    }
    Ok(None)
}

/// Whether a secret exists under any of the names, for a store that refuses to overwrite one.
pub fn exists(
    store: &impl CredentialStore,
    names: ServiceNames,
    account: &str,
) -> Result<bool, CredentialError> {
    for name in names.all() {
        match store.secret(name, account) {
            Ok(Some(mut found)) => {
                found.zeroize();
                return Ok(true);
            }
            Ok(None) => {}
            // A stored value that cannot be decoded is still a stored value.
            Err(CredentialError::BadEncoding(mut bytes)) => {
                bytes.zeroize();
                return Ok(true);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

/// Removes the secret under every name. `true` when any of them held one.
pub fn delete(
    store: &impl CredentialStore,
    names: ServiceNames,
    account: &str,
) -> Result<bool, CredentialError> {
    let mut removed = false;
    let mut failure = None;
    for name in names.all() {
        match store.delete(name, account) {
            Ok(found) => removed |= found,
            Err(error) => failure = Some(error),
        }
    }
    match failure {
        // Leaving one name behind would let a deleted secret come back on the next read, so a
        // removal that failed anywhere is a failure even when another name gave way.
        Some(error) => Err(error),
        None => Ok(removed),
    }
}

/// The real credential store.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCredentials;

impl SystemCredentials {
    fn entry(service: &str, account: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(service, account)
    }
}

impl CredentialStore for SystemCredentials {
    fn password(&self, service: &str, account: &str) -> Result<Option<String>, CredentialError> {
        match Self::entry(service, account)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(CredentialError::NoEntry) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn set_password(
        &self,
        service: &str,
        account: &str,
        value: &str,
    ) -> Result<(), CredentialError> {
        Self::entry(service, account)?.set_password(value)
    }

    fn secret(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, CredentialError> {
        match Self::entry(service, account)?.get_secret() {
            Ok(value) => Ok(Some(value)),
            Err(CredentialError::NoEntry) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn set_secret(
        &self,
        service: &str,
        account: &str,
        value: &[u8],
    ) -> Result<(), CredentialError> {
        Self::entry(service, account)?.set_secret(value)
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialError> {
        match Self::entry(service, account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(CredentialError::NoEntry) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    const NAMES: ServiceNames = ServiceNames::new(
        "com.millionrust.multiplex.test",
        &["com.multiplex.test", "com.termirust.test"],
    );

    #[derive(Default)]
    struct MemoryStore {
        values: RefCell<HashMap<(String, String), Vec<u8>>>,
        /// Service names whose writes fail, standing in for a store that refuses one.
        write_fails: RefCell<Vec<String>>,
        /// Service names whose removals fail, standing in for one left behind.
        delete_fails: RefCell<Vec<String>>,
    }

    impl MemoryStore {
        fn put(&self, service: &str, account: &str, value: &[u8]) {
            self.values
                .borrow_mut()
                .insert((service.to_owned(), account.to_owned()), value.to_vec());
        }

        fn has(&self, service: &str, account: &str) -> bool {
            self.values
                .borrow()
                .contains_key(&(service.to_owned(), account.to_owned()))
        }

        fn failure() -> CredentialError {
            CredentialError::Invalid("service".to_owned(), "refused".to_owned())
        }
    }

    impl CredentialStore for MemoryStore {
        fn password(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<String>, CredentialError> {
            match self.secret(service, account)? {
                Some(bytes) => String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|error| CredentialError::BadEncoding(error.into_bytes())),
                None => Ok(None),
            }
        }

        fn set_password(
            &self,
            service: &str,
            account: &str,
            value: &str,
        ) -> Result<(), CredentialError> {
            self.set_secret(service, account, value.as_bytes())
        }

        fn secret(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, CredentialError> {
            Ok(self
                .values
                .borrow()
                .get(&(service.to_owned(), account.to_owned()))
                .cloned())
        }

        fn set_secret(
            &self,
            service: &str,
            account: &str,
            value: &[u8],
        ) -> Result<(), CredentialError> {
            if self.write_fails.borrow().iter().any(|name| name == service) {
                return Err(Self::failure());
            }
            self.put(service, account, value);
            Ok(())
        }

        fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialError> {
            if self
                .delete_fails
                .borrow()
                .iter()
                .any(|name| name == service)
            {
                return Err(Self::failure());
            }
            Ok(self
                .values
                .borrow_mut()
                .remove(&(service.to_owned(), account.to_owned()))
                .is_some())
        }
    }

    #[test]
    fn a_secret_under_the_current_name_is_read_as_it_is() {
        let store = MemoryStore::default();
        store.put(NAMES.current, "account", b"here");
        assert_eq!(
            password(&store, NAMES, "account").unwrap().as_deref(),
            Some("here")
        );
        assert!(!store.has("com.termirust.test", "account"));
    }

    #[test]
    fn a_secret_under_an_old_name_is_copied_across_and_the_old_one_removed() {
        for legacy in NAMES.legacy {
            let store = MemoryStore::default();
            store.put(legacy, "account", b"moved");
            assert_eq!(
                password(&store, NAMES, "account").unwrap().as_deref(),
                Some("moved")
            );
            assert!(store.has(NAMES.current, "account"));
            assert!(!store.has(legacy, "account"));
            // The second read never looks at the old name again.
            assert_eq!(
                password(&store, NAMES, "account").unwrap().as_deref(),
                Some("moved")
            );
        }
    }

    #[test]
    fn bytes_move_across_the_same_way() {
        let store = MemoryStore::default();
        store.put("com.termirust.test", "account", &[0, 1, 2, 255]);
        assert_eq!(
            secret(&store, NAMES, "account").unwrap(),
            Some(vec![0, 1, 2, 255])
        );
        assert!(store.has(NAMES.current, "account"));
        assert!(!store.has("com.termirust.test", "account"));
    }

    #[test]
    fn a_copy_that_cannot_be_written_keeps_the_secret_under_the_old_name() {
        let store = MemoryStore::default();
        store.put("com.termirust.test", "account", b"kept");
        store
            .write_fails
            .borrow_mut()
            .push(NAMES.current.to_owned());
        assert_eq!(
            password(&store, NAMES, "account").unwrap().as_deref(),
            Some("kept"),
            "the secret is still returned"
        );
        assert!(
            store.has("com.termirust.test", "account"),
            "and is still readable next time"
        );
    }

    #[test]
    fn nothing_anywhere_reads_as_nothing() {
        let store = MemoryStore::default();
        assert_eq!(password(&store, NAMES, "account").unwrap(), None);
        assert_eq!(secret(&store, NAMES, "account").unwrap(), None);
        assert!(!exists(&store, NAMES, "account").unwrap());
    }

    #[test]
    fn a_secret_under_any_name_counts_as_existing() {
        for name in NAMES.all() {
            let store = MemoryStore::default();
            store.put(name, "account", b"there");
            assert!(exists(&store, NAMES, "account").unwrap());
        }
    }

    #[test]
    fn deleting_takes_every_name_and_reports_a_name_left_behind() {
        let store = MemoryStore::default();
        for name in NAMES.all() {
            store.put(name, "account", b"gone");
        }
        assert!(delete(&store, NAMES, "account").unwrap());
        for name in NAMES.all() {
            assert!(!store.has(name, "account"));
        }
        assert!(!delete(&store, NAMES, "account").unwrap());

        let store = MemoryStore::default();
        store.put(NAMES.current, "account", b"gone");
        store.put("com.termirust.test", "account", b"stuck");
        store
            .delete_fails
            .borrow_mut()
            .push("com.termirust.test".to_owned());
        assert!(delete(&store, NAMES, "account").is_err());
    }
}
