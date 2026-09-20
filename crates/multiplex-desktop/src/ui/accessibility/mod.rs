#[cfg(target_os = "macos")]
pub mod bridge {
    pub use multiplex_accessibility_macos::*;
}

pub mod shell;

#[cfg(target_os = "macos")]
pub mod harness;
