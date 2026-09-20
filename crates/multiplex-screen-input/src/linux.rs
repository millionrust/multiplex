//! Injecting input on Linux, through `uinput`.
//!
//! `uinput` creates a virtual input device in the kernel. Events written to it arrive in whatever
//! is running the session — X11, a Wayland compositor, a bare console — as if a keyboard and
//! mouse had been plugged in, which is why it works the same under all of them when the X11 and
//! Wayland routes do not.
//!
//! **Text is the one thing this cannot do.** A `uinput` device reports key positions; the
//! compositor's keymap turns those into characters, and nothing here can reach that keymap. So
//! there is no way to inject an `é` or an emoji the way `KEYEVENTF_UNICODE` does on Windows or a
//! Unicode keyboard event does on macOS. Typing normal keys works, which is most of what a remote
//! screen needs; pasted text and accented characters return [`InputError::UnmappedKey`] rather
//! than silently arriving as the wrong letters. Doing better means uploading a custom XKB keymap
//! with the virtual device, which is a larger piece of work than this one.
//!
//! **Permission.** `/dev/uinput` is usually root-only. A desktop install that wants this should
//! ship a udev rule granting the `input` group access rather than running the host as root; the
//! error says so rather than leaving a blank refusal.

#![allow(unsafe_code)]

use std::ffi::{c_int, c_ulong};
use std::os::fd::{AsRawFd as _, OwnedFd};

use multiplex_screen_protocol::PointerButton;

use crate::keymap::linux_keycode;
use crate::{InputError, InputSink, Point, SinkEvent};

/// Where the kernel takes virtual devices.
const UINPUT_PATH: &str = "/dev/uinput";

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;

const SYN_REPORT: u16 = 0;
const REL_WHEEL: u16 = 0x08;
const REL_HWHEEL: u16 = 0x06;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;

const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

/// Absolute pointer coordinates are reported on this scale, and the compositor maps it onto
/// whatever the screen really is — the same trick Windows uses, for the same reason.
const ABS_RANGE: i32 = 65_535;

/// `ioctl` request numbers, built the way `linux/uinput.h` builds them.
///
/// The encoding is the kernel's `_IOW(UINPUT_IOCTL_BASE, number, type)`: direction, size, the
/// letter `U`, and the number. Spelling the arithmetic out is what makes it checkable against the
/// header rather than a set of magic constants copied from somewhere.
const fn iow(number: u32, size: u32) -> c_ulong {
    const WRITE: u32 = 1;
    const BASE: u32 = b'U' as u32;
    ((WRITE << 30) | (size << 16) | (BASE << 8) | number) as c_ulong
}

const fn io(number: u32) -> c_ulong {
    const BASE: u32 = b'U' as u32;
    ((BASE << 8) | number) as c_ulong
}

fn ui_set_evbit() -> c_ulong {
    iow(100, size_of::<c_int>() as u32)
}
fn ui_set_keybit() -> c_ulong {
    iow(101, size_of::<c_int>() as u32)
}
fn ui_set_relbit() -> c_ulong {
    iow(102, size_of::<c_int>() as u32)
}
fn ui_set_absbit() -> c_ulong {
    iow(103, size_of::<c_int>() as u32)
}
fn ui_dev_setup() -> c_ulong {
    iow(3, size_of::<UinputSetup>() as u32)
}
fn ui_abs_setup() -> c_ulong {
    iow(4, size_of::<UinputAbsSetup>() as u32)
}
fn ui_dev_create() -> c_ulong {
    io(1)
}
fn ui_dev_destroy() -> c_ulong {
    io(2)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    absinfo: AbsInfo,
}

/// `struct input_event`. The timestamp is left zero, which the kernel fills in.
#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    seconds: i64,
    microseconds: i64,
    kind: u16,
    code: u16,
    value: i32,
}

/// A virtual keyboard and pointer in the kernel.
#[derive(Debug)]
pub struct UinputSink {
    device: OwnedFd,
}

impl UinputSink {
    /// Creates the device.
    ///
    /// Fails with [`InputError::NotPermitted`] when `/dev/uinput` cannot be opened, which on a
    /// normal desktop means the user is not in a group a udev rule granted access to.
    pub fn open() -> Result<Self, InputError> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(UINPUT_PATH)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound => {
                    InputError::NotPermitted
                }
                _ => InputError::Unavailable,
            })?;
        let device = OwnedFd::from(file);
        let sink = Self { device };
        sink.configure()?;
        Ok(sink)
    }

    fn fd(&self) -> c_int {
        self.device.as_raw_fd()
    }

    /// Declares what the device can do, then asks the kernel to create it.
    ///
    /// Everything has to be declared before creation: a key not claimed here is silently dropped
    /// when it is later written, which is a failure mode with no error to catch.
    fn configure(&self) -> Result<(), InputError> {
        self.ioctl(ui_set_evbit(), EV_KEY as c_ulong)?;
        self.ioctl(ui_set_evbit(), EV_REL as c_ulong)?;
        self.ioctl(ui_set_evbit(), EV_ABS as c_ulong)?;
        self.ioctl(ui_set_evbit(), EV_SYN as c_ulong)?;

        // Every key the map can produce, plus the pointer buttons, which are keys to the kernel.
        for usage in 0..=0xFFu16 {
            if let Some(code) = linux_keycode(usage) {
                self.ioctl(ui_set_keybit(), code as c_ulong)?;
            }
        }
        for button in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
            self.ioctl(ui_set_keybit(), button as c_ulong)?;
        }
        for wheel in [REL_WHEEL, REL_HWHEEL] {
            self.ioctl(ui_set_relbit(), wheel as c_ulong)?;
        }
        for axis in [ABS_X, ABS_Y] {
            self.ioctl(ui_set_absbit(), axis as c_ulong)?;
            let setup = UinputAbsSetup {
                code: axis,
                absinfo: AbsInfo {
                    maximum: ABS_RANGE,
                    ..AbsInfo::default()
                },
            };
            // Safety: `setup` is a live, fully initialised value of the type this request takes.
            let result =
                unsafe { libc::ioctl(self.fd(), ui_abs_setup(), std::ptr::addr_of!(setup)) };
            if result < 0 {
                return Err(InputError::Unavailable);
            }
        }

        let mut name = [0_u8; 80];
        let label = b"TermiRust Remote Screen";
        name[..label.len()].copy_from_slice(label);
        let setup = UinputSetup {
            id: InputId {
                // A virtual bus, so nothing mistakes this for hardware that was plugged in.
                bustype: 0x06,
                ..InputId::default()
            },
            name,
            ff_effects_max: 0,
        };
        // Safety: as above.
        let result = unsafe { libc::ioctl(self.fd(), ui_dev_setup(), std::ptr::addr_of!(setup)) };
        if result < 0 {
            return Err(InputError::Unavailable);
        }
        self.ioctl(ui_dev_create(), 0)
    }

    fn ioctl(&self, request: c_ulong, argument: c_ulong) -> Result<(), InputError> {
        // Safety: every request used here takes an integer by value, not a pointer.
        let result = unsafe { libc::ioctl(self.fd(), request, argument) };
        if result < 0 {
            Err(InputError::Unavailable)
        } else {
            Ok(())
        }
    }

    /// Writes events and the synchronisation that makes them one report.
    ///
    /// The `SYN_REPORT` matters: without it the compositor sees a half-finished gesture, so a
    /// move and the click that belongs with it must go in one write or the click can land
    /// somewhere the viewer did not mean.
    fn emit(&self, events: &[(u16, u16, i32)]) -> Result<(), InputError> {
        if events.is_empty() {
            return Ok(());
        }
        let mut batch: Vec<InputEvent> = events
            .iter()
            .map(|(kind, code, value)| InputEvent {
                seconds: 0,
                microseconds: 0,
                kind: *kind,
                code: *code,
                value: *value,
            })
            .collect();
        batch.push(InputEvent {
            seconds: 0,
            microseconds: 0,
            kind: EV_SYN,
            code: SYN_REPORT,
            value: 0,
        });
        let bytes = size_of_val(batch.as_slice());
        // Safety: `batch` is a live slice of plain data, and `bytes` is exactly its length.
        let written = unsafe { libc::write(self.fd(), batch.as_ptr().cast(), bytes) };
        if written == bytes as isize {
            Ok(())
        } else {
            Err(InputError::Unavailable)
        }
    }
}

impl Drop for UinputSink {
    fn drop(&mut self) {
        // Removes the device from the kernel. Without it a crashed host leaves a phantom keyboard
        // behind until the file descriptor is reaped.
        let _ = self.ioctl(ui_dev_destroy(), 0);
    }
}

impl InputSink for UinputSink {
    fn post(&mut self, event: SinkEvent) -> Result<(), InputError> {
        match event {
            SinkEvent::PointerMove { at, .. } => self.emit(&absolute(at)),
            SinkEvent::PointerButton {
                at,
                button,
                pressed,
                ..
            } => {
                let mut events = absolute(at);
                events.push((EV_KEY, button_code(button), i32::from(pressed)));
                self.emit(&events)
            }
            SinkEvent::Scroll { at, dx, dy, .. } => {
                let mut events = absolute(at);
                if dy != 0 {
                    events.push((EV_REL, REL_WHEEL, dy.signum()));
                }
                if dx != 0 {
                    // Positive `dx` in the protocol shows content to the left; a positive
                    // `REL_HWHEEL` scrolls right, so the sign flips here rather than somewhere a
                    // reader would have to hunt for it.
                    events.push((EV_REL, REL_HWHEEL, -dx.signum()));
                }
                self.emit(&events)
            }
            SinkEvent::Key { usage, pressed, .. } => {
                let code = linux_keycode(usage).ok_or(InputError::UnmappedKey)?;
                self.emit(&[(EV_KEY, code, i32::from(pressed))])
            }
            // See the module note: a uinput device reports positions, and nothing here can reach
            // the keymap that turns positions into characters.
            SinkEvent::Text(_) => Err(InputError::UnmappedKey),
        }
    }
}

/// A point as the absolute pair the device was set up for.
///
/// The compositor scales this onto the real screen, so the host's display arrangement is its own
/// business — which is what makes this work with monitors added or removed mid-session.
fn absolute(at: Point) -> Vec<(u16, u16, i32)> {
    let clamp = |value: f64| (value.round() as i64).clamp(0, i64::from(ABS_RANGE)) as i32;
    vec![(EV_ABS, ABS_X, clamp(at.x)), (EV_ABS, ABS_Y, clamp(at.y))]
}

const fn button_code(button: PointerButton) -> u16 {
    match button {
        PointerButton::Primary => BTN_LEFT,
        PointerButton::Secondary => BTN_RIGHT,
        PointerButton::Middle => BTN_MIDDLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_numbers_match_the_kernel_header() {
        // `_IOW('U', 100, int)` and friends, worked out by hand from `linux/uinput.h`. These are
        // the one thing here that cannot be caught by the compiler: a wrong number is a valid
        // ioctl that does something else.
        assert_eq!(ui_set_evbit(), 0x4004_5564);
        assert_eq!(ui_set_keybit(), 0x4004_5565);
        assert_eq!(ui_set_relbit(), 0x4004_5566);
        assert_eq!(ui_set_absbit(), 0x4004_5567);
        assert_eq!(ui_dev_create(), 0x5501);
        assert_eq!(ui_dev_destroy(), 0x5502);
    }

    #[test]
    fn a_point_lands_on_the_scale_the_device_declared() {
        let events = absolute(Point { x: 0.0, y: 0.0 });
        assert_eq!(events, vec![(EV_ABS, ABS_X, 0), (EV_ABS, ABS_Y, 0)]);
        let far = absolute(Point {
            x: 1_000_000.0,
            y: -50.0,
        });
        assert_eq!(
            far,
            vec![(EV_ABS, ABS_X, ABS_RANGE), (EV_ABS, ABS_Y, 0)],
            "a point off the screen is clamped rather than wrapped"
        );
    }

    #[test]
    fn an_input_event_is_the_size_the_kernel_reads() {
        // 64-bit `struct input_event`: two 64-bit times, two 16-bit fields, one 32-bit value.
        assert_eq!(size_of::<InputEvent>(), 24);
        assert_eq!(size_of::<InputId>(), 8);
    }
}
