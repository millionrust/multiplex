//! Display capture for TermiRust Remote Screens.
//!
//! A [`FrameSource`] delivers opaque BGRA frames with the damage the operating system reported.
//! Frames that did not change are not delivered. When a frame had to be dropped because the
//! consumer fell behind, the next frame carries [`Damage::Unknown`] so no change is lost.
//!
//! Backends: ScreenCaptureKit on macOS; Desktop Duplication on Windows; [`ReplaySource`]
//! everywhere, for tests and recorded workloads. The Linux backend follows in milestone M6.
//!
//! Each backend names its own source type and its own `displays`, because what they need to be
//! asked differs: macOS has a permission to request and Windows does not, and only macOS can
//! resample while capturing. A caller picks the backend for the platform it was compiled for.

#![deny(unsafe_code)]

mod error;
#[cfg(target_os = "macos")]
mod macos;
mod replay;
mod source;
#[cfg(target_os = "windows")]
mod windows;

pub use error::CaptureError;
#[cfg(target_os = "macos")]
pub use macos::{ScreenCaptureKitSource, displays, request_screen_capture, screen_capture_allowed};
pub use replay::ReplaySource;
pub use source::{CaptureConfig, CapturedFrame, Damage, DisplayInfo, FrameSource};
#[cfg(target_os = "windows")]
pub use windows::{DesktopDuplicationSource, displays, screen_capture_allowed};
