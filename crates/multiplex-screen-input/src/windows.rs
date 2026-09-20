//! Injecting input on Windows, with `SendInput`.
//!
//! The one module in this crate that touches raw pointers. Windows has no safe wrapper here short
//! of the `windows` crate, which would be a large dependency for four functions and a struct, so
//! the slice used is declared by hand — the same choice `multiplex-screen-video` made for
//! VideoToolbox, and for the same reason.
//!
//! Two decisions worth knowing before reading the code.
//!
//! **Keys go as scancodes, not virtual keys.** A virtual key means "the key that types this
//! character on this machine's layout"; a scancode means "this physical position". A viewer on a
//! French keyboard driving a US host should get what that host's layout produces from the
//! position it pressed, exactly as an attached keyboard would, so positions are what travel.
//!
//! **Text goes as Unicode, not as keys.** Anything typed rather than pressed — a pasted line, an
//! accented character, an emoji — has no position to send. `KEYEVENTF_UNICODE` delivers the
//! character itself, which is the only way a host with a different layout receives what was meant.
//!
//! The `modifiers` on each event are ignored here, and deliberately. The injector already presses
//! and releases modifier keys as the physical keys they are, so applying them again would double
//! them; `SendInput` has no field for "this happened while shift was down" the way Core Graphics
//! does.

#![allow(unsafe_code)]

use std::ffi::c_int;

use multiplex_screen_protocol::PointerButton;

use crate::keymap::windows_scancode;
use crate::{InputError, InputSink, Point, SinkEvent};

/// One wheel notch, as Windows counts them.
const WHEEL_DELTA: i32 = 120;
/// Windows takes absolute pointer positions on this scale, whatever the screen really is.
const ABSOLUTE_RANGE: f64 = 65_535.0;

const INPUT_MOUSE: u32 = 0;
const INPUT_KEYBOARD: u32 = 1;

const MOUSEEVENTF_MOVE: u32 = 0x0001;
const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
const MOUSEEVENTF_WHEEL: u32 = 0x0800;
const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;

const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
const KEYEVENTF_KEYUP: u32 = 0x0002;
const KEYEVENTF_UNICODE: u32 = 0x0004;
const KEYEVENTF_SCANCODE: u32 = 0x0008;

/// `GetSystemMetrics` indices for the bounding box of every monitor together.
const SM_XVIRTUALSCREEN: c_int = 76;
const SM_YVIRTUALSCREEN: c_int = 77;
const SM_CXVIRTUALSCREEN: c_int = 78;
const SM_CYVIRTUALSCREEN: c_int = 79;

#[repr(C)]
#[derive(Clone, Copy)]
struct MouseInput {
    dx: i32,
    dy: i32,
    mouse_data: i32,
    flags: u32,
    time: u32,
    extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KeyboardInput {
    virtual_key: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
union InputUnion {
    mouse: MouseInput,
    keyboard: KeyboardInput,
    /// Never used, and present only so the union is the size Windows expects. `SendInput` is
    /// given `size_of::<Input>()` and refuses anything else, so this is load-bearing.
    _hardware: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Input {
    kind: u32,
    value: InputUnion,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SendInput(count: u32, inputs: *const Input, size: c_int) -> u32;
    fn GetSystemMetrics(index: c_int) -> c_int;
}

/// The bounding box of every monitor, in pixels: left, top, width, height.
///
/// Separated from the sink so the arithmetic that depends on it can be tested without a Windows
/// machine to ask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtualScreen {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl VirtualScreen {
    fn current() -> Self {
        // Safety: `GetSystemMetrics` reads a system value and touches no memory of ours.
        unsafe {
            Self {
                left: GetSystemMetrics(SM_XVIRTUALSCREEN),
                top: GetSystemMetrics(SM_YVIRTUALSCREEN),
                width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
                height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
            }
        }
    }

    /// A point on the desktop as the 0..65535 pair `SendInput` wants.
    ///
    /// Returns `None` for a screen of no size, which is what a machine with no monitor attached
    /// reports: sending a pointer somewhere on a desktop that does not exist is not better than
    /// sending nothing.
    pub fn absolute(self, at: Point) -> Option<(i32, i32)> {
        if self.width <= 0 || self.height <= 0 {
            return None;
        }
        // Windows maps the range onto the screen's *last* pixel, not one past it, so the divisor
        // is the span rather than the count. Getting this wrong puts the pointer a pixel short of
        // the right edge, which nobody notices until they try to hit a scrollbar.
        let span = |value: f64, origin: i32, size: i32| {
            let offset = (value - f64::from(origin)) / f64::from(size);
            (offset * ABSOLUTE_RANGE).round().clamp(0.0, ABSOLUTE_RANGE) as i32
        };
        Some((
            span(at.x, self.left, self.width),
            span(at.y, self.top, self.height),
        ))
    }
}

/// Injects through `SendInput`.
#[derive(Debug)]
pub struct SendInputSink {
    /// Read once: a monitor added mid-session changes it, and [`Self::rescan`] is how a caller
    /// that noticed says so.
    screen: VirtualScreen,
}

impl Default for SendInputSink {
    fn default() -> Self {
        Self::new()
    }
}

impl SendInputSink {
    pub fn new() -> Self {
        Self {
            screen: VirtualScreen::current(),
        }
    }

    /// Re-reads the desktop's bounds, after a display was added or removed.
    pub fn rescan(&mut self) {
        self.screen = VirtualScreen::current();
    }

    fn send(&self, inputs: &[Input]) -> Result<(), InputError> {
        if inputs.is_empty() {
            return Ok(());
        }
        let size = c_int::try_from(size_of::<Input>()).map_err(|_| InputError::NotPermitted)?;
        // Safety: `inputs` is a live slice of exactly `count` `Input` values, and `size` is the
        // size of one, which is what `SendInput` requires of them.
        let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size) };
        // A short count means the input was blocked, usually by a more privileged window holding
        // the foreground. It is a refusal rather than a failure of ours.
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            Err(InputError::NotPermitted)
        }
    }

    fn mouse(&self, flags: u32, at: Option<Point>, data: i32) -> Option<Input> {
        let (dx, dy, absolute) = match at {
            Some(at) => {
                let (x, y) = self.screen.absolute(at)?;
                (x, y, MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)
            }
            None => (0, 0, 0),
        };
        Some(Input {
            kind: INPUT_MOUSE,
            value: InputUnion {
                mouse: MouseInput {
                    dx,
                    dy,
                    mouse_data: data,
                    flags: flags | absolute,
                    time: 0,
                    extra_info: 0,
                },
            },
        })
    }

    fn key(usage: u16, pressed: bool) -> Option<Input> {
        let (scan, extended) = windows_scancode(usage)?;
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !pressed {
            flags |= KEYEVENTF_KEYUP;
        }
        Some(Input {
            kind: INPUT_KEYBOARD,
            value: InputUnion {
                keyboard: KeyboardInput {
                    virtual_key: 0,
                    scan,
                    flags,
                    time: 0,
                    extra_info: 0,
                },
            },
        })
    }

    /// One UTF-16 unit as a press and a release. Characters outside the basic plane arrive as
    /// their two surrogates, which is what Windows expects rather than a defect.
    fn unicode(unit: u16) -> [Input; 2] {
        let make = |flags: u32| Input {
            kind: INPUT_KEYBOARD,
            value: InputUnion {
                keyboard: KeyboardInput {
                    virtual_key: 0,
                    scan: unit,
                    flags: KEYEVENTF_UNICODE | flags,
                    time: 0,
                    extra_info: 0,
                },
            },
        };
        [make(0), make(KEYEVENTF_KEYUP)]
    }
}

impl InputSink for SendInputSink {
    fn post(&mut self, event: SinkEvent) -> Result<(), InputError> {
        let inputs = match event {
            SinkEvent::PointerMove { at, .. } => self
                .mouse(MOUSEEVENTF_MOVE, Some(at), 0)
                .into_iter()
                .collect::<Vec<_>>(),
            SinkEvent::PointerButton {
                at,
                button,
                pressed,
                ..
            } => {
                // The move and the button go in one call, so nothing can land between them and
                // click somewhere the viewer did not mean.
                let flags = button_flags(button, pressed);
                let mut inputs = Vec::with_capacity(2);
                inputs.extend(self.mouse(MOUSEEVENTF_MOVE, Some(at), 0));
                inputs.extend(self.mouse(flags, Some(at), 0));
                inputs
            }
            SinkEvent::Scroll { at, dx, dy, .. } => {
                let mut inputs = Vec::with_capacity(2);
                if dy != 0 {
                    inputs.extend(self.mouse(MOUSEEVENTF_WHEEL, Some(at), dy * WHEEL_DELTA));
                }
                if dx != 0 {
                    // Windows counts a positive horizontal wheel as scrolling to the right, and
                    // the protocol counts positive `dx` as showing content to the left, so the
                    // sign flips here rather than anywhere a reader would have to hunt for it.
                    inputs.extend(self.mouse(MOUSEEVENTF_HWHEEL, Some(at), -dx * WHEEL_DELTA));
                }
                inputs
            }
            SinkEvent::Key { usage, pressed, .. } => {
                Self::key(usage, pressed).into_iter().collect::<Vec<_>>()
            }
            SinkEvent::Text(text) => text
                .encode_utf16()
                .flat_map(Self::unicode)
                .collect::<Vec<_>>(),
        };
        self.send(&inputs)
    }
}

const fn button_flags(button: PointerButton, pressed: bool) -> u32 {
    match (button, pressed) {
        (PointerButton::Primary, true) => MOUSEEVENTF_LEFTDOWN,
        (PointerButton::Primary, false) => MOUSEEVENTF_LEFTUP,
        (PointerButton::Secondary, true) => MOUSEEVENTF_RIGHTDOWN,
        (PointerButton::Secondary, false) => MOUSEEVENTF_RIGHTUP,
        (PointerButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (PointerButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: VirtualScreen = VirtualScreen {
        left: 0,
        top: 0,
        width: 1920,
        height: 1080,
    };

    #[test]
    fn the_corners_of_the_desktop_map_to_the_ends_of_the_range() {
        assert_eq!(SCREEN.absolute(Point { x: 0.0, y: 0.0 }), Some((0, 0)));
        assert_eq!(
            SCREEN.absolute(Point {
                x: 1920.0,
                y: 1080.0
            }),
            Some((65_535, 65_535)),
            "the far corner is the end of the range, not one short of it"
        );
        let middle = SCREEN.absolute(Point { x: 960.0, y: 540.0 }).unwrap();
        assert!((32_000..=33_500).contains(&middle.0), "{middle:?}");
    }

    #[test]
    fn a_desktop_that_does_not_start_at_the_origin_is_still_mapped_from_its_own_corner() {
        // A second monitor to the left of the primary puts the origin negative, which is the
        // case that catches an implementation assuming the desktop starts at zero.
        let screen = VirtualScreen {
            left: -1920,
            top: -200,
            width: 3840,
            height: 1280,
        };
        assert_eq!(
            screen.absolute(Point {
                x: -1920.0,
                y: -200.0
            }),
            Some((0, 0))
        );
        assert_eq!(
            screen.absolute(Point {
                x: 1920.0,
                y: 1080.0
            }),
            Some((65_535, 65_535))
        );
    }

    #[test]
    fn a_point_outside_the_desktop_is_clamped_rather_than_wrapped() {
        assert_eq!(
            SCREEN.absolute(Point {
                x: -5_000.0,
                y: 9_000.0
            }),
            Some((0, 65_535)),
            "a wrapped coordinate would put the pointer on the opposite edge"
        );
    }

    #[test]
    fn a_machine_with_no_screen_is_not_given_a_position() {
        let none = VirtualScreen {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
        };
        assert_eq!(none.absolute(Point { x: 0.0, y: 0.0 }), None);
    }

    #[test]
    fn keys_travel_as_positions_and_carry_their_extended_flag() {
        // A letter: a plain scancode, no prefix.
        let (scan, extended) = windows_scancode(0x04).expect("A is a key");
        assert_eq!((scan, extended), (0x1E, false));
        // An arrow: the same code as a keypad key, told apart only by the prefix. Dropping the
        // prefix would turn every Right into a keypad 6.
        assert_eq!(windows_scancode(0x4F), Some((0x4D, true)));
        assert_eq!(windows_scancode(0x5E), Some((0x4D, false)));
    }

    #[test]
    fn the_pause_key_is_left_unmapped_rather_than_sent_wrong() {
        // Its scancode is a three-byte sequence rather than a code with a prefix, so there is no
        // honest way to express it here.
        assert_eq!(windows_scancode(0x48), None);
    }

    #[test]
    fn an_input_is_the_size_windows_will_accept() {
        // `SendInput` refuses a size it does not recognise, and the union's largest member is
        // what sets it. On 64-bit Windows that is 40 bytes; the assertion is that the union is
        // laid out from the mouse variant, which is the largest.
        assert_eq!(
            size_of::<Input>(),
            size_of::<u32>() * 2 + size_of::<MouseInput>()
        );
        assert!(size_of::<MouseInput>() >= size_of::<KeyboardInput>());
    }

    #[test]
    fn text_becomes_one_press_and_release_per_utf16_unit() {
        let units: Vec<u16> = "hé🙂".encode_utf16().collect();
        assert_eq!(units.len(), 4, "the emoji is a surrogate pair");
        let inputs: Vec<Input> = units
            .iter()
            .copied()
            .flat_map(SendInputSink::unicode)
            .collect();
        assert_eq!(inputs.len(), 8);
        // Safety: every one of these was built from the keyboard variant just above.
        let flags: Vec<u32> = inputs
            .iter()
            .map(|input| unsafe { input.value.keyboard.flags })
            .collect();
        assert!(flags.iter().all(|flags| flags & KEYEVENTF_UNICODE != 0));
        assert_eq!(flags[0] & KEYEVENTF_KEYUP, 0);
        assert_ne!(flags[1] & KEYEVENTF_KEYUP, 0);
    }
}
