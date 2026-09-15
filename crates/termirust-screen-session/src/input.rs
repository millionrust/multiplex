use termirust_screen_protocol::{KeyEvent, Message, PointerButton};

/// Input a viewer sends while it holds control.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputEvent {
    PointerMove {
        surface: u32,
        x: u32,
        y: u32,
    },
    PointerButton {
        surface: u32,
        x: u32,
        y: u32,
        button: PointerButton,
        pressed: bool,
    },
    Scroll {
        surface: u32,
        x: u32,
        y: u32,
        dx: i32,
        dy: i32,
    },
    Key(KeyEvent),
    Text {
        surface: u32,
        text: String,
    },
}

impl InputEvent {
    pub(crate) fn into_message(self) -> Message {
        match self {
            Self::PointerMove { surface, x, y } => Message::PointerMove { surface, x, y },
            Self::PointerButton {
                surface,
                x,
                y,
                button,
                pressed,
            } => Message::PointerButton {
                surface,
                x,
                y,
                button,
                pressed,
            },
            Self::Scroll {
                surface,
                x,
                y,
                dx,
                dy,
            } => Message::Scroll {
                surface,
                x,
                y,
                dx,
                dy,
            },
            Self::Key(key) => Message::Key(key),
            Self::Text { surface, text } => Message::Text { surface, text },
        }
    }

    pub(crate) fn from_message(message: Message) -> Option<Self> {
        Some(match message {
            Message::PointerMove { surface, x, y } => Self::PointerMove { surface, x, y },
            Message::PointerButton {
                surface,
                x,
                y,
                button,
                pressed,
            } => Self::PointerButton {
                surface,
                x,
                y,
                button,
                pressed,
            },
            Message::Scroll {
                surface,
                x,
                y,
                dx,
                dy,
            } => Self::Scroll {
                surface,
                x,
                y,
                dx,
                dy,
            },
            Message::Key(key) => Self::Key(key),
            Message::Text { surface, text } => Self::Text { surface, text },
            _ => return None,
        })
    }
}
