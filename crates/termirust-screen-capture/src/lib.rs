//! Display capture for TermiRust Remote Screens.
//!
//! A [`FrameSource`] delivers opaque BGRA frames with the damage the operating system reported.
//! Frames that did not change are not delivered. When a frame had to be dropped because the
//! consumer fell behind, the next frame carries [`Damage::Unknown`] so no change is lost.
//!
//! Backends: ScreenCaptureKit on macOS; [`ReplaySource`] everywhere, for tests and recorded
//! workloads. Windows and Linux backends follow in milestone M6.

#![deny(unsafe_code)]

mod error;
#[cfg(target_os = "macos")]
mod macos;
mod replay;
mod source;

pub use error::CaptureError;
#[cfg(target_os = "macos")]
pub use macos::{ScreenCaptureKitSource, displays};
pub use replay::ReplaySource;
pub use source::{CaptureConfig, CapturedFrame, Damage, DisplayInfo, FrameSource};
