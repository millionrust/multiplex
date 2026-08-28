pub mod bridge {
    pub use termirust_accessibility_macos::*;
}

#[cfg(target_os = "macos")]
pub mod harness;
