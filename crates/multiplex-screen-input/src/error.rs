use std::fmt;

/// Why input was not injected. Messages never include typed text or key identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputError {
    /// The sending device does not hold the writer lease.
    NotHolder,
    /// The event names a surface with no display placement, such as a preview.
    UnknownSurface,
    /// The key has no equivalent on this host.
    UnmappedKey,
    /// The operating system refused injection, usually because Accessibility is not allowed.
    NotPermitted,
    /// The operating system could not create or post the event.
    Unavailable,
}

impl InputError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotHolder => "input_not_holder",
            Self::UnknownSurface => "input_unknown_surface",
            Self::UnmappedKey => "input_unmapped_key",
            Self::NotPermitted => "input_not_permitted",
            Self::Unavailable => "input_unavailable",
        }
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for InputError {}
