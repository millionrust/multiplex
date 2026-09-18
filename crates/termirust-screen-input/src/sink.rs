use termirust_screen_protocol::{Modifiers, PointerButton};

use crate::{InputError, Point};

/// The most UTF-16 units one [`SinkEvent::Text`] carries. Core Graphics keyboard events take at
/// most 20.
pub const TEXT_CHUNK_UTF16: usize = 20;

/// One operating-system input event, already mapped to the global display arrangement.
#[derive(Clone, Debug, PartialEq)]
pub enum SinkEvent {
    /// The pointer moved to `at`. `held` is the button being dragged, if any.
    PointerMove {
        at: Point,
        held: Option<PointerButton>,
        modifiers: Modifiers,
    },
    /// `clicks` is 1 for a single click, 2 for the second press of a double click, and so on.
    PointerButton {
        at: Point,
        button: PointerButton,
        pressed: bool,
        clicks: u8,
        modifiers: Modifiers,
    },
    /// Scroll by whole points. Positive `dy` shows content above, positive `dx` content to the left.
    Scroll {
        at: Point,
        dx: i32,
        dy: i32,
        modifiers: Modifiers,
    },
    /// A physical key by USB HID usage. The injector only emits keys the host can map.
    Key {
        usage: u16,
        pressed: bool,
        repeat: bool,
        modifiers: Modifiers,
    },
    /// Typed text of at most [`TEXT_CHUNK_UTF16`] UTF-16 units, never splitting a character.
    Text(String),
}

/// Where the injector sends events.
pub trait InputSink {
    fn post(&mut self, event: SinkEvent) -> Result<(), InputError>;
}

/// Keeps every event in order, for tests and dry runs.
#[derive(Debug, Default)]
pub struct RecordingSink {
    pub events: Vec<SinkEvent>,
}

impl InputSink for RecordingSink {
    fn post(&mut self, event: SinkEvent) -> Result<(), InputError> {
        self.events.push(event);
        Ok(())
    }
}
