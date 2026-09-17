//! Display capture for TermiRust Remote Screens.
//!
//! A [`FrameSource`] delivers opaque BGRA frames with the damage the operating system reported.
//! Frames that did not change are not delivered. When a frame had to be dropped because the
//! consumer fell behind, the next frame carries [`Damage::Unknown`] so no change is lost.
//!
//! Backends: ScreenCaptureKit on macOS; Desktop Duplication on Windows; the xdg-desktop-portal
//! ScreenCast on Linux; [`ReplaySource`] everywhere, for tests and recorded workloads.
//!
//! Each backend names its own source type, and only two of them offer `displays`, because what
//! each platform lets an application ask differs too much to paper over. macOS has a permission to
//! request and Windows does not; only macOS can resample while capturing; and on Linux an
//! application cannot enumerate or choose a screen at all — the portal's picker does, so there is
//! nothing to list before the user has answered. A caller picks the backend for the platform it
//! was compiled for and handles that platform's shape.

#![deny(unsafe_code)]

mod cursor;
mod error;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod replay;
mod source;
#[cfg(target_os = "windows")]
mod windows;

pub use cursor::{CursorKind, CursorShape, composite};
pub use error::CaptureError;
#[cfg(target_os = "linux")]
pub use linux::PortalScreenCastSource;
#[cfg(target_os = "macos")]
pub use macos::{ScreenCaptureKitSource, displays, request_screen_capture, screen_capture_allowed};
pub use replay::ReplaySource;
pub use source::{CaptureConfig, CapturedFrame, Damage, DisplayInfo, FrameSource};
#[cfg(target_os = "windows")]
pub use windows::{DesktopDuplicationSource, displays, screen_capture_allowed};
