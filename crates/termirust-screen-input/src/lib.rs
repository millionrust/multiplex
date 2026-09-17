//! Pointer and keyboard injection for TermiRust Remote Screens.
//!
//! An [`Injector`] turns the input a viewer sends into operating-system events. It holds the
//! single writer lease: only the device that holds control can inject, and when control moves to
//! another device or to nobody, every key and button the previous holder left pressed is released
//! so nothing stays stuck down on the host. Surface pixel coordinates are mapped onto the global
//! display arrangement with a [`DisplayLayout`].
//!
//! Backends: Core Graphics on macOS, which needs the Accessibility permission; [`RecordingSink`]
//! everywhere, for tests. Windows and Linux backends follow in milestone M6. See
//! `docs/remote-screens-implementation-plan.md`, section 4.7.

#![deny(unsafe_code)]

mod error;
mod injector;
mod keymap;
mod layout;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod sink;
#[cfg(target_os = "windows")]
mod windows;

pub use error::InputError;
pub use injector::{DOUBLE_CLICK_DISTANCE_POINTS, DOUBLE_CLICK_MS, Injector};
pub use keymap::{linux_keycode, mac_virtual_keycode, windows_scancode};
pub use layout::{DisplayLayout, DisplayPlacement, Point};
#[cfg(target_os = "linux")]
pub use linux::UinputSink;
#[cfg(target_os = "macos")]
pub use macos::{CoreGraphicsSink, INJECTED_EVENT_TAG, accessibility_trusted};
pub use sink::{InputSink, RecordingSink, SinkEvent, TEXT_CHUNK_UTF16};
#[cfg(target_os = "windows")]
pub use windows::{SendInputSink, VirtualScreen};
