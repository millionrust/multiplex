use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use multiplex_screen_protocol::{Modifiers, PointerButton};

use crate::{InputError, InputSink, SinkEvent, mac_virtual_keycode};

/// Written to every injected event's `kCGEventSourceUserData` field, so the host can tell remote
/// input from local input. The bytes spell `TRSI`.
pub const INJECTED_EVENT_TAG: i64 = 0x5452_5349;

#[allow(unsafe_code)]
mod accessibility {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        pub safe fn AXIsProcessTrusted() -> bool;
    }
}

/// Whether this process may post input events. macOS drops posted events silently without it.
pub fn accessibility_trusted() -> bool {
    accessibility::AXIsProcessTrusted()
}

/// Posts input into the login session through Core Graphics.
pub struct CoreGraphicsSink {
    source: CGEventSource,
}

impl CoreGraphicsSink {
    /// Fails with [`InputError::NotPermitted`] when Accessibility is not allowed for this process.
    pub fn new() -> Result<Self, InputError> {
        if !accessibility_trusted() {
            return Err(InputError::NotPermitted);
        }
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| InputError::Unavailable)?;
        Ok(Self { source })
    }

    fn send(&self, event: Result<CGEvent, ()>, modifiers: Modifiers) -> Result<(), InputError> {
        let event = event.map_err(|()| InputError::Unavailable)?;
        event.set_flags(flags(modifiers));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INJECTED_EVENT_TAG);
        event.post(CGEventTapLocation::HID);
        Ok(())
    }
}

impl InputSink for CoreGraphicsSink {
    fn post(&mut self, event: SinkEvent) -> Result<(), InputError> {
        match event {
            SinkEvent::PointerMove {
                at,
                held,
                modifiers,
            } => {
                let (kind, button) = match held {
                    None => (CGEventType::MouseMoved, CGMouseButton::Left),
                    Some(PointerButton::Primary) => {
                        (CGEventType::LeftMouseDragged, CGMouseButton::Left)
                    }
                    Some(PointerButton::Secondary) => {
                        (CGEventType::RightMouseDragged, CGMouseButton::Right)
                    }
                    Some(PointerButton::Middle) => {
                        (CGEventType::OtherMouseDragged, CGMouseButton::Center)
                    }
                };
                let event = CGEvent::new_mouse_event(self.source.clone(), kind, point(at), button);
                self.send(event, modifiers)
            }
            SinkEvent::PointerButton {
                at,
                button,
                pressed,
                clicks,
                modifiers,
            } => {
                let (kind, button) = match (button, pressed) {
                    (PointerButton::Primary, true) => {
                        (CGEventType::LeftMouseDown, CGMouseButton::Left)
                    }
                    (PointerButton::Primary, false) => {
                        (CGEventType::LeftMouseUp, CGMouseButton::Left)
                    }
                    (PointerButton::Secondary, true) => {
                        (CGEventType::RightMouseDown, CGMouseButton::Right)
                    }
                    (PointerButton::Secondary, false) => {
                        (CGEventType::RightMouseUp, CGMouseButton::Right)
                    }
                    (PointerButton::Middle, true) => {
                        (CGEventType::OtherMouseDown, CGMouseButton::Center)
                    }
                    (PointerButton::Middle, false) => {
                        (CGEventType::OtherMouseUp, CGMouseButton::Center)
                    }
                };
                let event = CGEvent::new_mouse_event(self.source.clone(), kind, point(at), button)
                    .inspect(|event| {
                        event.set_integer_value_field(
                            EventField::MOUSE_EVENT_CLICK_STATE,
                            i64::from(clicks),
                        );
                    });
                self.send(event, modifiers)
            }
            // Scroll events go to the window under the pointer, which the injector has already
            // moved to `at`.
            SinkEvent::Scroll {
                at: _,
                dx,
                dy,
                modifiers,
            } => {
                let event = CGEvent::new_scroll_event(
                    self.source.clone(),
                    ScrollEventUnit::PIXEL,
                    2,
                    dy,
                    dx,
                    0,
                );
                self.send(event, modifiers)
            }
            SinkEvent::Key {
                usage,
                pressed,
                repeat,
                modifiers,
            } => {
                let keycode = mac_virtual_keycode(usage).ok_or(InputError::UnmappedKey)?;
                let event = CGEvent::new_keyboard_event(self.source.clone(), keycode, pressed)
                    .inspect(|event| {
                        event.set_integer_value_field(
                            EventField::KEYBOARD_EVENT_AUTOREPEAT,
                            i64::from(repeat),
                        );
                    });
                self.send(event, modifiers)
            }
            SinkEvent::Text(text) => {
                for pressed in [true, false] {
                    let event = CGEvent::new_keyboard_event(self.source.clone(), 0, pressed)
                        .inspect(|event| event.set_string(&text));
                    self.send(event, Modifiers::default())?;
                }
                Ok(())
            }
        }
    }
}

fn point(at: crate::Point) -> CGPoint {
    CGPoint::new(at.x, at.y)
}

fn flags(modifiers: Modifiers) -> CGEventFlags {
    let bits = modifiers.bits();
    let mut flags = CGEventFlags::CGEventFlagNull;
    for (bit, flag) in [
        (Modifiers::SHIFT, CGEventFlags::CGEventFlagShift),
        (Modifiers::CONTROL, CGEventFlags::CGEventFlagControl),
        (Modifiers::OPTION, CGEventFlags::CGEventFlagAlternate),
        (Modifiers::COMMAND, CGEventFlags::CGEventFlagCommand),
    ] {
        if bits & bit != 0 {
            flags |= flag;
        }
    }
    flags
}
