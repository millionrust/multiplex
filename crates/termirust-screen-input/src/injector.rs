use std::collections::BTreeSet;

use termirust_screen_protocol::{KeyEvent, Modifiers, PointerButton};
use termirust_screen_session::InputEvent;

use crate::{DisplayLayout, InputError, InputSink, Point, SinkEvent, TEXT_CHUNK_UTF16};

/// A second press of the same button within this time and distance continues a multi-click.
pub const DOUBLE_CLICK_MS: u64 = 500;
pub const DOUBLE_CLICK_DISTANCE_POINTS: f64 = 4.0;

/// The highest HID keyboard-page usage a viewer can send; higher values are reserved.
const MAX_KEY_USAGE: u16 = 0xFF;

const BUTTONS: [PointerButton; 3] = [
    PointerButton::Primary,
    PointerButton::Secondary,
    PointerButton::Middle,
];

struct LastPress {
    button: PointerButton,
    at: Point,
    at_ms: u64,
    clicks: u8,
}

/// Injects one writer's input and guarantees nothing stays pressed when the writer changes.
pub struct Injector<S> {
    sink: S,
    layout: DisplayLayout,
    holder: Option<u64>,
    pointer: Option<Point>,
    /// The click count of each button press still held, by [`BUTTONS`] order.
    buttons: [Option<u8>; 3],
    keys: BTreeSet<u16>,
    modifiers: Modifiers,
    last_press: Option<LastPress>,
    scroll_remainder: (f64, f64),
}

impl<S: InputSink> Injector<S> {
    /// An injector with no writer: all input is refused until [`Self::set_holder`] names one.
    pub fn new(sink: S, layout: DisplayLayout) -> Self {
        Self {
            sink,
            layout,
            holder: None,
            pointer: None,
            buttons: [None; 3],
            keys: BTreeSet::new(),
            modifiers: Modifiers::default(),
            last_press: None,
            scroll_remainder: (0.0, 0.0),
        }
    }

    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// Replaces the display arrangement, for example after a display was added.
    pub fn set_layout(&mut self, layout: DisplayLayout) {
        self.layout = layout;
    }

    /// The device holding the writer lease.
    pub const fn holder(&self) -> Option<u64> {
        self.holder
    }

    /// Moves the writer lease to `holder`, or to nobody. Keys and buttons the previous holder
    /// still had pressed are released first, even when posting one of the releases fails; the
    /// first failure is returned.
    pub fn set_holder(&mut self, holder: Option<u64>) -> Result<(), InputError> {
        if self.holder == holder {
            return Ok(());
        }
        let released = self.release_all();
        self.holder = holder;
        self.last_press = None;
        self.scroll_remainder = (0.0, 0.0);
        released
    }

    /// Releases every key, then every button, that is still pressed.
    pub fn release_all(&mut self) -> Result<(), InputError> {
        let mut result = Ok(());
        for usage in std::mem::take(&mut self.keys) {
            let posted = self.sink.post(SinkEvent::Key {
                usage,
                pressed: false,
                repeat: false,
                modifiers: Modifiers::default(),
            });
            result = result.and(posted);
        }
        self.modifiers = Modifiers::default();
        for (index, held) in self.buttons.iter_mut().enumerate() {
            if let (Some(clicks), Some(at)) = (held.take(), self.pointer) {
                let posted = self.sink.post(SinkEvent::PointerButton {
                    at,
                    button: BUTTONS[index],
                    pressed: false,
                    clicks,
                    modifiers: Modifiers::default(),
                });
                result = result.and(posted);
            }
        }
        result
    }

    /// Injects `event` from `device`, received at `now_ms`. A press of a button or key that is
    /// already down is treated as a repeat, and a release of one that is not down is ignored, so
    /// the operating system never sees an unbalanced pair.
    pub fn inject(
        &mut self,
        device: u64,
        event: &InputEvent,
        now_ms: u64,
    ) -> Result<(), InputError> {
        if self.holder != Some(device) {
            return Err(InputError::NotHolder);
        }
        match event {
            InputEvent::PointerMove { surface, x, y } => {
                let at = self.locate(*surface, *x, *y)?;
                self.move_to(at)
            }
            InputEvent::PointerButton {
                surface,
                x,
                y,
                button,
                pressed,
            } => {
                let at = self.locate(*surface, *x, *y)?;
                self.move_to(at)?;
                self.button(at, *button, *pressed, now_ms)
            }
            InputEvent::Scroll {
                surface,
                x,
                y,
                dx,
                dy,
            } => {
                let at = self.locate(*surface, *x, *y)?;
                self.move_to(at)?;
                self.scroll(*surface, at, *dx, *dy)
            }
            InputEvent::Key(key) => self.key(*key),
            InputEvent::Text { surface, text } => {
                self.layout
                    .placement(*surface)
                    .ok_or(InputError::UnknownSurface)?;
                for chunk in text_chunks(text) {
                    self.sink.post(SinkEvent::Text(chunk.to_owned()))?;
                }
                Ok(())
            }
        }
    }

    fn locate(&self, surface: u32, x: u32, y: u32) -> Result<Point, InputError> {
        self.layout
            .placement(surface)
            .map(|placement| placement.to_global(x, y))
            .ok_or(InputError::UnknownSurface)
    }

    fn held_button(&self) -> Option<PointerButton> {
        self.buttons
            .iter()
            .position(Option::is_some)
            .map(|index| BUTTONS[index])
    }

    fn move_to(&mut self, at: Point) -> Result<(), InputError> {
        if self.pointer != Some(at) {
            self.sink.post(SinkEvent::PointerMove {
                at,
                held: self.held_button(),
                modifiers: self.modifiers,
            })?;
            self.pointer = Some(at);
        }
        Ok(())
    }

    fn button(
        &mut self,
        at: Point,
        button: PointerButton,
        pressed: bool,
        now_ms: u64,
    ) -> Result<(), InputError> {
        let index = BUTTONS
            .iter()
            .position(|candidate| *candidate == button)
            .unwrap_or_default();
        if pressed {
            if self.buttons[index].is_some() {
                return Ok(());
            }
            let clicks = match &self.last_press {
                Some(last)
                    if last.button == button
                        && now_ms.saturating_sub(last.at_ms) <= DOUBLE_CLICK_MS
                        && (last.at.x - at.x).hypot(last.at.y - at.y)
                            <= DOUBLE_CLICK_DISTANCE_POINTS =>
                {
                    last.clicks.saturating_add(1)
                }
                _ => 1,
            };
            self.sink.post(SinkEvent::PointerButton {
                at,
                button,
                pressed: true,
                clicks,
                modifiers: self.modifiers,
            })?;
            self.buttons[index] = Some(clicks);
            self.last_press = Some(LastPress {
                button,
                at,
                at_ms: now_ms,
                clicks,
            });
            Ok(())
        } else {
            let Some(clicks) = self.buttons[index].take() else {
                return Ok(());
            };
            self.sink.post(SinkEvent::PointerButton {
                at,
                button,
                pressed: false,
                clicks,
                modifiers: self.modifiers,
            })
        }
    }

    fn scroll(&mut self, surface: u32, at: Point, dx: i32, dy: i32) -> Result<(), InputError> {
        let placement = self
            .layout
            .placement(surface)
            .ok_or(InputError::UnknownSurface)?;
        let (points_x, points_y) = placement.to_points(f64::from(dx), f64::from(dy));
        let total_x = points_x + self.scroll_remainder.0;
        let total_y = points_y + self.scroll_remainder.1;
        let (whole_x, whole_y) = (total_x.trunc(), total_y.trunc());
        self.scroll_remainder = (total_x - whole_x, total_y - whole_y);
        if whole_x == 0.0 && whole_y == 0.0 {
            return Ok(());
        }
        self.sink.post(SinkEvent::Scroll {
            at,
            dx: whole_x as i32,
            dy: whole_y as i32,
            modifiers: self.modifiers,
        })
    }

    fn key(&mut self, key: KeyEvent) -> Result<(), InputError> {
        if key.usage > MAX_KEY_USAGE {
            return Err(InputError::UnmappedKey);
        }
        self.modifiers = key.modifiers;
        if key.pressed {
            let repeat = self.keys.contains(&key.usage);
            self.sink.post(SinkEvent::Key {
                usage: key.usage,
                pressed: true,
                repeat,
                modifiers: key.modifiers,
            })?;
            self.keys.insert(key.usage);
            Ok(())
        } else {
            if !self.keys.remove(&key.usage) {
                return Ok(());
            }
            self.sink.post(SinkEvent::Key {
                usage: key.usage,
                pressed: false,
                repeat: false,
                modifiers: key.modifiers,
            })
        }
    }
}

/// Splits `text` into pieces of at most [`TEXT_CHUNK_UTF16`] UTF-16 units on character boundaries.
fn text_chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (index, character) in text.char_indices() {
        if units + character.len_utf16() > TEXT_CHUNK_UTF16 {
            chunks.push(&text[start..index]);
            start = index;
            units = 0;
        }
        units += character.len_utf16();
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_chunks_respect_the_limit_and_characters() {
        assert!(text_chunks("").is_empty());
        let ascii = "a".repeat(45);
        let chunks = text_chunks(&ascii);
        assert_eq!(
            chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
            [20, 20, 5]
        );

        // 19 ASCII units, then a character that needs a surrogate pair.
        let text = format!("{}😀b", "a".repeat(19));
        let chunks = text_chunks(&text);
        assert_eq!(chunks, [&"a".repeat(19)[..], "😀b"]);
        assert_eq!(chunks.concat(), text);
    }
}
