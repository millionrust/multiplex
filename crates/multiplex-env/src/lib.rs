//! Multiplex's own environment variables, read under both the name it uses now and the one it
//! used before the product was renamed.
//!
//! Every variable this application defines is spelled `MULTIPLEX_SOMETHING`. It used to be
//! spelled `TERMIRUST_SOMETHING`, and those names are in people's shell profiles, in CI
//! configuration, and in notes written before the rename. Reading both costs one lookup and
//! means none of that has to be found and changed on the same day.
//!
//! The old name is a fallback, never an override: when both are set the new one wins, so
//! putting the new name in front of a command does what it looks like it does.
//!
//! Call [`var`] or [`var_os`] with the new name. The legacy name is derived from it, so a
//! caller cannot accidentally read a pair that does not correspond.

use std::env::{self, VarError};
use std::ffi::OsString;

/// The prefix every variable this application defines carries.
pub const PREFIX: &str = "MULTIPLEX_";
/// The prefix those variables carried before the product was renamed.
pub const LEGACY_PREFIX: &str = "TERMIRUST_";

/// The legacy spelling of `name`, or `None` when `name` is not one of ours.
///
/// A name without the prefix has no legacy equivalent to guess at: `PATH` is `PATH`.
#[must_use]
pub fn legacy_name(name: &str) -> Option<String> {
    name.strip_prefix(PREFIX)
        .map(|rest| format!("{LEGACY_PREFIX}{rest}"))
}

/// Reads `name`, falling back to its legacy spelling.
///
/// Returns [`VarError::NotPresent`] only when neither name is set. A value that is not valid
/// Unicode is reported against whichever name held it, exactly as [`env::var`] would.
///
/// # Errors
///
/// Propagates [`env::var`]'s error for the name that was found.
pub fn var(name: &str) -> Result<String, VarError> {
    match env::var(name) {
        Err(VarError::NotPresent) => match legacy_name(name) {
            Some(legacy) => env::var(legacy),
            None => Err(VarError::NotPresent),
        },
        found => found,
    }
}

/// Reads `name` as an [`OsString`], falling back to its legacy spelling.
#[must_use]
pub fn var_os(name: &str) -> Option<OsString> {
    env::var_os(name).or_else(|| legacy_name(name).and_then(env::var_os))
}

/// Whether `name` is set, under either spelling, to anything but the empty string.
///
/// This is the shape most of the switches in this workspace want: present means on.
#[must_use]
pub fn is_set(name: &str) -> bool {
    var_os(name).is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests here must not read the ambient environment: another test in this process may be
    // setting variables at the same time, and `set_var` is unsafe for exactly that reason. The
    // name derivation is the part worth pinning, and it is pure.

    #[test]
    fn a_legacy_name_is_only_derived_for_our_own_variables() {
        assert_eq!(
            legacy_name("MULTIPLEX_CONFIG_DIR").as_deref(),
            Some("TERMIRUST_CONFIG_DIR")
        );
        assert_eq!(legacy_name("MULTIPLEX_").as_deref(), Some("TERMIRUST_"));
        assert_eq!(legacy_name("PATH"), None);
        assert_eq!(legacy_name("HOME"), None);
        // Not ours, and close enough to something of ours to be worth stating.
        assert_eq!(legacy_name("TERMIRUST_CONFIG_DIR"), None);
        assert_eq!(legacy_name("MY_MULTIPLEX_CONFIG_DIR"), None);
    }

    #[test]
    fn the_prefixes_are_what_the_rest_of_the_workspace_spells() {
        assert!(PREFIX.ends_with('_'));
        assert!(LEGACY_PREFIX.ends_with('_'));
        assert_ne!(PREFIX, LEGACY_PREFIX);
    }
}
