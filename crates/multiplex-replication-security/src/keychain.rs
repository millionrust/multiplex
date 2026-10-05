//! Access to secrets under the current Multiplex service identifiers only.
//! Earlier applications' keychain entries are never read, migrated, or removed.

use zeroize::Zeroize as _;

pub use keyring::Error as CredentialError;

/// The system credential store, as this module uses it. Tests use memory instead.
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

/// Reads a text secret from the requested service only.
pub fn password(
    store: &impl CredentialStore,
    service: &str,
    account: &str,
) -> Result<Option<String>, CredentialError> {
    store.password(service, account)
}

/// Reads a byte secret from the requested service only.
pub fn secret(
    store: &impl CredentialStore,
    service: &str,
    account: &str,
) -> Result<Option<Vec<u8>>, CredentialError> {
    store.secret(service, account)
}

/// Whether a secret exists in the requested service, without retaining its contents.
pub fn exists(
    store: &impl CredentialStore,
    service: &str,
    account: &str,
) -> Result<bool, CredentialError> {
    match store.secret(service, account) {
        Ok(Some(mut found)) => {
            found.zeroize();
            Ok(true)
        }
        Ok(None) => Ok(false),
        // A stored value that cannot be decoded is still a stored value.
        Err(CredentialError::BadEncoding(mut bytes)) => {
            bytes.zeroize();
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

/// Removes a secret from the requested service only.
pub fn delete(
    store: &impl CredentialStore,
    service: &str,
    account: &str,
) -> Result<bool, CredentialError> {
    store.delete(service, account)
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

    const SERVICE: &str = "com.millionrust.multiplex.test";
    const LEGACY: [&str; 2] = ["com.multiplex.test", "com.termirust.test"];

    #[derive(Default)]
    struct MemoryStore {
        values: RefCell<HashMap<(String, String), Vec<u8>>>,
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
            assert_eq!(service, SERVICE, "must never read an old service");
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
            self.put(service, account, value);
            Ok(())
        }

        fn delete(&self, service: &str, account: &str) -> Result<bool, CredentialError> {
            assert_eq!(service, SERVICE, "must never remove an old service");
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
    fn legacy_entries_are_not_read_migrated_or_deleted() {
        let store = MemoryStore::default();
        for legacy in LEGACY {
            store.put(legacy, "account", b"old");
        }
        assert_eq!(password(&store, SERVICE, "account").unwrap(), None);
        assert_eq!(secret(&store, SERVICE, "account").unwrap(), None);
        assert!(!exists(&store, SERVICE, "account").unwrap());
        assert!(!delete(&store, SERVICE, "account").unwrap());
        assert!(!store.has(SERVICE, "account"));
        store.set_password(SERVICE, "account", "new").unwrap();
        assert_eq!(
            password(&store, SERVICE, "account").unwrap().as_deref(),
            Some("new")
        );
        assert!(delete(&store, SERVICE, "account").unwrap());
        for legacy in LEGACY {
            assert!(store.has(legacy, "account"));
        }
    }

    #[test]
    fn current_byte_secrets_can_be_stored_read_and_deleted() {
        let store = MemoryStore::default();
        store
            .set_secret(SERVICE, "account", &[0, 1, 2, 255])
            .unwrap();
        assert_eq!(
            secret(&store, SERVICE, "account").unwrap(),
            Some(vec![0, 1, 2, 255])
        );
        assert!(exists(&store, SERVICE, "account").unwrap());
        assert!(delete(&store, SERVICE, "account").unwrap());
        assert!(!delete(&store, SERVICE, "account").unwrap());
    }

    #[test]
    fn deletion_errors_are_returned() {
        let store = MemoryStore::default();
        store.put(SERVICE, "account", b"kept");
        store.delete_fails.borrow_mut().push(SERVICE.to_owned());
        assert!(delete(&store, SERVICE, "account").is_err());
        assert!(store.has(SERVICE, "account"));
    }
}
