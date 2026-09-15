pub mod bridge {
    pub use termirust_accessibility_macos::*;
}

pub mod shell;

#[cfg(target_os = "macos")]
pub mod harness;
