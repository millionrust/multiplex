//! Session messages. All integers are big-endian; strings are length-prefixed UTF-8.
//!
//! | Kind | Direction | Message |
//! |---:|---|---|
//! | 0x01 | viewer → host | hello: version, ticket proof, cache budget, optional resume |
//! | 0x02 | host → viewer | welcome: version, surfaces, resume outcome |
//! | 0x03 | host → viewer | goodbye: reason code |
//! | 0x10 | viewer → host | subscribe: surface, profile |
//! | 0x11 | viewer → host | unsubscribe: surface |
//! | 0x12 | viewer → host | viewport: surface, rectangle, scale |
//! | 0x13 | viewer → host | acknowledge: surface, generation, sequence |
//! | 0x14 | viewer → host | cache miss: surface, tile, hash |
//! | 0x20 | host → viewer | batch: one `TSB1` tile batch |
//! | 0x21 | host → viewer | motion region: surface, optional rectangle |
//! | 0x22 | host → viewer | control holder |
//! | 0x30 | viewer → host | request control |
//! | 0x31 | viewer → host | release control |
//! | 0x32 | viewer → host | pointer move: surface, x, y |
//! | 0x33 | viewer → host | pointer button: surface, x, y, button, pressed |
//! | 0x34 | viewer → host | scroll: surface, x, y, dx, dy |
//! | 0x35 | viewer → host | key: USB HID usage, modifiers, pressed |
//! | 0x36 | viewer → host | text: surface, UTF-8 up to 256 bytes |

use termirust_screen_codec::{
    Batch, Generation, MAX_SURFACE_DIMENSION, Rect, Size, TileHash, TileIndex,
};

use crate::ProtocolError;

pub const PROTOCOL_VERSION: u16 = 1;
const MAX_SURFACES: usize = 16;
const MAX_NAME_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 256;
const MAX_REASON_BYTES: usize = 64;

/// How much detail a subscription wants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    /// Full detail for an open remote screen.
    Interactive,
    /// A small preview for the Devices list, about one image per second.
    Thumbnail,
}

/// Where a viewer resumes after reconnecting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResumeRequest {
    pub surface: u32,
    pub generation: Generation,
    pub sequence: u64,
}

/// The first message a viewer sends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hello {
    pub version: u16,
    /// Proof of the screen ticket issued over the Controller channel.
    pub ticket_proof: [u8; 32],
    /// The viewer's tile cache budget; the host's shadow must use the same.
    pub cache_bytes: u64,
    pub resume: Option<ResumeRequest>,
}

/// A surface the host can share.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceInfo {
    pub id: u32,
    pub size: Size,
    /// Pixels per point, in thousandths.
    pub scale_milli: u16,
    pub name: String,
}

/// Whether a resume request could be honoured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeOutcome {
    NotRequested,
    Partial,
    Full,
}

/// The host's answer to a valid hello.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Welcome {
    pub version: u16,
    pub surfaces: Vec<SurfaceInfo>,
    pub resume: ResumeOutcome,
}

/// The part of a surface a viewer shows, in surface pixels, and its zoom in thousandths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub surface: u32,
    pub rect: Rect,
    pub scale_milli: u16,
}

/// Who may click and type on the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlHolder {
    Nobody,
    You,
    AnotherDevice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

/// Modifier keys held during a key event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const SHIFT: u8 = 1;
    pub const CONTROL: u8 = 2;
    pub const OPTION: u8 = 4;
    pub const COMMAND: u8 = 8;

    pub const fn new(bits: u8) -> Option<Self> {
        if bits & !0x0F == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

/// A physical key, as a USB HID usage on the keyboard page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyEvent {
    pub usage: u16,
    pub modifiers: Modifiers,
    pub pressed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Hello(Hello),
    Welcome(Welcome),
    Goodbye {
        reason: String,
    },
    Subscribe {
        surface: u32,
        profile: Profile,
    },
    Unsubscribe {
        surface: u32,
    },
    Viewport(Viewport),
    Acknowledge(ResumeRequest),
    CacheMiss {
        surface: u32,
        tile: TileIndex,
        hash: TileHash,
    },
    Batch(Batch),
    MotionRegion {
        surface: u32,
        rect: Option<Rect>,
    },
    Control(ControlHolder),
    RequestControl,
    ReleaseControl,
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

const HELLO: u8 = 0x01;
const WELCOME: u8 = 0x02;
const GOODBYE: u8 = 0x03;
const SUBSCRIBE: u8 = 0x10;
const UNSUBSCRIBE: u8 = 0x11;
const VIEWPORT: u8 = 0x12;
const ACKNOWLEDGE: u8 = 0x13;
const CACHE_MISS: u8 = 0x14;
const BATCH: u8 = 0x20;
const MOTION_REGION: u8 = 0x21;
const CONTROL: u8 = 0x22;
const REQUEST_CONTROL: u8 = 0x30;
const RELEASE_CONTROL: u8 = 0x31;
const POINTER_MOVE: u8 = 0x32;
const POINTER_BUTTON: u8 = 0x33;
const SCROLL: u8 = 0x34;
const KEY: u8 = 0x35;
const TEXT: u8 = 0x36;

impl Message {
    pub const fn is_batch(&self) -> bool {
        matches!(self, Self::Batch(_))
    }

    pub(crate) const fn kind_is_batch(kind: u8) -> bool {
        kind == BATCH
    }

    /// Kind byte and body, without the frame length.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut w = Writer(Vec::with_capacity(32));
        match self {
            Self::Hello(hello) => {
                w.u8(HELLO);
                w.u16(hello.version);
                w.bytes(&hello.ticket_proof);
                w.u64(hello.cache_bytes);
                match hello.resume {
                    None => w.u8(0),
                    Some(resume) => {
                        w.u8(1);
                        w.resume(resume);
                    }
                }
            }
            Self::Welcome(welcome) => {
                w.u8(WELCOME);
                w.u16(welcome.version);
                if welcome.surfaces.len() > MAX_SURFACES {
                    return Err(ProtocolError::Malformed);
                }
                w.u8(welcome.surfaces.len() as u8);
                for surface in &welcome.surfaces {
                    w.u32(surface.id);
                    w.u32(surface.size.width());
                    w.u32(surface.size.height());
                    w.u16(surface.scale_milli);
                    w.string(&surface.name, MAX_NAME_BYTES)?;
                }
                w.u8(match welcome.resume {
                    ResumeOutcome::NotRequested => 0,
                    ResumeOutcome::Partial => 1,
                    ResumeOutcome::Full => 2,
                });
            }
            Self::Goodbye { reason } => {
                w.u8(GOODBYE);
                w.string(reason, MAX_REASON_BYTES)?;
            }
            Self::Subscribe { surface, profile } => {
                w.u8(SUBSCRIBE);
                w.u32(*surface);
                w.u8(match profile {
                    Profile::Interactive => 0,
                    Profile::Thumbnail => 1,
                });
            }
            Self::Unsubscribe { surface } => {
                w.u8(UNSUBSCRIBE);
                w.u32(*surface);
            }
            Self::Viewport(viewport) => {
                w.u8(VIEWPORT);
                w.u32(viewport.surface);
                w.rect(viewport.rect);
                w.u16(viewport.scale_milli);
            }
            Self::Acknowledge(ack) => {
                w.u8(ACKNOWLEDGE);
                w.resume(*ack);
            }
            Self::CacheMiss {
                surface,
                tile,
                hash,
            } => {
                w.u8(CACHE_MISS);
                w.u32(*surface);
                w.u32(tile.0);
                w.u64(hash.0);
            }
            Self::Batch(batch) => {
                w.u8(BATCH);
                w.bytes(&batch.encode().map_err(|_| ProtocolError::InvalidBatch)?);
            }
            Self::MotionRegion { surface, rect } => {
                w.u8(MOTION_REGION);
                w.u32(*surface);
                match rect {
                    None => w.u8(0),
                    Some(rect) => {
                        w.u8(1);
                        w.rect(*rect);
                    }
                }
            }
            Self::Control(holder) => {
                w.u8(CONTROL);
                w.u8(match holder {
                    ControlHolder::Nobody => 0,
                    ControlHolder::You => 1,
                    ControlHolder::AnotherDevice => 2,
                });
            }
            Self::RequestControl => w.u8(REQUEST_CONTROL),
            Self::ReleaseControl => w.u8(RELEASE_CONTROL),
            Self::PointerMove { surface, x, y } => {
                w.u8(POINTER_MOVE);
                w.u32(*surface);
                w.u32(*x);
                w.u32(*y);
            }
            Self::PointerButton {
                surface,
                x,
                y,
                button,
                pressed,
            } => {
                w.u8(POINTER_BUTTON);
                w.u32(*surface);
                w.u32(*x);
                w.u32(*y);
                w.u8(match button {
                    PointerButton::Primary => 0,
                    PointerButton::Secondary => 1,
                    PointerButton::Middle => 2,
                });
                w.u8(u8::from(*pressed));
            }
            Self::Scroll {
                surface,
                x,
                y,
                dx,
                dy,
            } => {
                w.u8(SCROLL);
                w.u32(*surface);
                w.u32(*x);
                w.u32(*y);
                w.u32(*dx as u32);
                w.u32(*dy as u32);
            }
            Self::Key(key) => {
                w.u8(KEY);
                w.u16(key.usage);
                w.u8(key.modifiers.bits());
                w.u8(u8::from(key.pressed));
            }
            Self::Text { surface, text } => {
                w.u8(TEXT);
                w.u32(*surface);
                if text.is_empty() {
                    return Err(ProtocolError::Malformed);
                }
                w.string(text, MAX_TEXT_BYTES)?;
            }
        }
        Ok(w.0)
    }

    /// Parses a kind byte and body. Every field is checked; trailing bytes are rejected.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut r = Reader { bytes, at: 0 };
        let message = match r.u8()? {
            HELLO => {
                let version = r.u16()?;
                if version != PROTOCOL_VERSION {
                    return Err(ProtocolError::UnsupportedVersion);
                }
                let ticket_proof = r.take(32)?.try_into().expect("32 bytes");
                let cache_bytes = r.u64()?;
                let resume = match r.u8()? {
                    0 => None,
                    1 => Some(r.resume()?),
                    _ => return Err(ProtocolError::Malformed),
                };
                Self::Hello(Hello {
                    version,
                    ticket_proof,
                    cache_bytes,
                    resume,
                })
            }
            WELCOME => {
                let version = r.u16()?;
                if version != PROTOCOL_VERSION {
                    return Err(ProtocolError::UnsupportedVersion);
                }
                let count = r.u8()? as usize;
                if count > MAX_SURFACES {
                    return Err(ProtocolError::Malformed);
                }
                let mut surfaces = Vec::with_capacity(count);
                for _ in 0..count {
                    let id = r.u32()?;
                    let size = r.size()?;
                    let scale_milli = r.u16()?;
                    let name = r.string(MAX_NAME_BYTES, true)?;
                    surfaces.push(SurfaceInfo {
                        id,
                        size,
                        scale_milli,
                        name,
                    });
                }
                let resume = match r.u8()? {
                    0 => ResumeOutcome::NotRequested,
                    1 => ResumeOutcome::Partial,
                    2 => ResumeOutcome::Full,
                    _ => return Err(ProtocolError::Malformed),
                };
                Self::Welcome(Welcome {
                    version,
                    surfaces,
                    resume,
                })
            }
            GOODBYE => Self::Goodbye {
                reason: r.string(MAX_REASON_BYTES, false)?,
            },
            SUBSCRIBE => Self::Subscribe {
                surface: r.u32()?,
                profile: match r.u8()? {
                    0 => Profile::Interactive,
                    1 => Profile::Thumbnail,
                    _ => return Err(ProtocolError::Malformed),
                },
            },
            UNSUBSCRIBE => Self::Unsubscribe { surface: r.u32()? },
            VIEWPORT => {
                let surface = r.u32()?;
                let rect = r.rect()?;
                let scale_milli = r.u16()?;
                if scale_milli == 0 {
                    return Err(ProtocolError::Malformed);
                }
                Self::Viewport(Viewport {
                    surface,
                    rect,
                    scale_milli,
                })
            }
            ACKNOWLEDGE => Self::Acknowledge(r.resume()?),
            CACHE_MISS => Self::CacheMiss {
                surface: r.u32()?,
                tile: TileIndex(r.u32()?),
                hash: TileHash(r.u64()?),
            },
            BATCH => {
                let rest = r.take(r.remaining())?;
                Self::Batch(Batch::decode(rest).map_err(|_| ProtocolError::InvalidBatch)?)
            }
            MOTION_REGION => Self::MotionRegion {
                surface: r.u32()?,
                rect: match r.u8()? {
                    0 => None,
                    1 => Some(r.rect()?),
                    _ => return Err(ProtocolError::Malformed),
                },
            },
            CONTROL => Self::Control(match r.u8()? {
                0 => ControlHolder::Nobody,
                1 => ControlHolder::You,
                2 => ControlHolder::AnotherDevice,
                _ => return Err(ProtocolError::Malformed),
            }),
            REQUEST_CONTROL => Self::RequestControl,
            RELEASE_CONTROL => Self::ReleaseControl,
            POINTER_MOVE => Self::PointerMove {
                surface: r.u32()?,
                x: r.coordinate()?,
                y: r.coordinate()?,
            },
            POINTER_BUTTON => Self::PointerButton {
                surface: r.u32()?,
                x: r.coordinate()?,
                y: r.coordinate()?,
                button: match r.u8()? {
                    0 => PointerButton::Primary,
                    1 => PointerButton::Secondary,
                    2 => PointerButton::Middle,
                    _ => return Err(ProtocolError::Malformed),
                },
                pressed: r.bool()?,
            },
            SCROLL => Self::Scroll {
                surface: r.u32()?,
                x: r.coordinate()?,
                y: r.coordinate()?,
                dx: r.u32()? as i32,
                dy: r.u32()? as i32,
            },
            KEY => Self::Key(KeyEvent {
                usage: r.u16()?,
                modifiers: Modifiers::new(r.u8()?).ok_or(ProtocolError::Malformed)?,
                pressed: r.bool()?,
            }),
            TEXT => Self::Text {
                surface: r.u32()?,
                text: r.string(MAX_TEXT_BYTES, false)?,
            },
            _ => return Err(ProtocolError::Malformed),
        };
        if r.remaining() != 0 {
            return Err(ProtocolError::Malformed);
        }
        Ok(message)
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.0.extend(value.to_be_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.0.extend(value.to_be_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend(value.to_be_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.0.extend_from_slice(value);
    }
    fn rect(&mut self, rect: Rect) {
        [rect.x, rect.y, rect.width, rect.height]
            .into_iter()
            .for_each(|v| self.u32(v));
    }
    fn resume(&mut self, resume: ResumeRequest) {
        self.u32(resume.surface);
        self.u32(resume.generation.0);
        self.u64(resume.sequence);
    }
    fn string(&mut self, value: &str, max: usize) -> Result<(), ProtocolError> {
        if value.len() > max {
            return Err(ProtocolError::Malformed);
        }
        self.u16(value.len() as u16);
        self.bytes(value.as_bytes());
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], ProtocolError> {
        if self.remaining() < count {
            return Err(ProtocolError::Malformed);
        }
        let slice = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("two bytes"),
        ))
    }
    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }
    fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }
    fn bool(&mut self) -> Result<bool, ProtocolError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ProtocolError::Malformed),
        }
    }
    fn coordinate(&mut self) -> Result<u32, ProtocolError> {
        let value = self.u32()?;
        if value >= MAX_SURFACE_DIMENSION {
            return Err(ProtocolError::Malformed);
        }
        Ok(value)
    }
    fn size(&mut self) -> Result<Size, ProtocolError> {
        Size::new(self.u32()?, self.u32()?).map_err(|_| ProtocolError::Malformed)
    }
    fn rect(&mut self) -> Result<Rect, ProtocolError> {
        let rect = Rect::new(self.u32()?, self.u32()?, self.u32()?, self.u32()?);
        let bound = Rect::new(0, 0, MAX_SURFACE_DIMENSION, MAX_SURFACE_DIMENSION);
        if rect.is_empty() || !bound.contains_rect(rect) {
            return Err(ProtocolError::Malformed);
        }
        Ok(rect)
    }
    fn resume(&mut self) -> Result<ResumeRequest, ProtocolError> {
        Ok(ResumeRequest {
            surface: self.u32()?,
            generation: Generation(self.u32()?),
            sequence: self.u64()?,
        })
    }
    fn string(&mut self, max: usize, allow_empty: bool) -> Result<String, ProtocolError> {
        let length = self.u16()? as usize;
        if length > max || (length == 0 && !allow_empty) {
            return Err(ProtocolError::Malformed);
        }
        let bytes = self.take(length)?;
        let text = std::str::from_utf8(bytes).map_err(|_| ProtocolError::Malformed)?;
        if text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(ProtocolError::Malformed);
        }
        Ok(text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use termirust_screen_codec::{SurfaceId, TileOp};

    fn all_messages() -> Vec<Message> {
        vec![
            Message::Hello(Hello {
                version: PROTOCOL_VERSION,
                ticket_proof: [7; 32],
                cache_bytes: 64 << 20,
                resume: Some(ResumeRequest {
                    surface: 1,
                    generation: Generation(2),
                    sequence: 99,
                }),
            }),
            Message::Welcome(Welcome {
                version: PROTOCOL_VERSION,
                surfaces: vec![
                    SurfaceInfo {
                        id: 1,
                        size: Size::new(3024, 1964).unwrap(),
                        scale_milli: 2000,
                        name: "Built-in Display".to_owned(),
                    },
                    SurfaceInfo {
                        id: 2,
                        size: Size::new(5120, 2880).unwrap(),
                        scale_milli: 2000,
                        name: String::new(),
                    },
                ],
                resume: ResumeOutcome::Partial,
            }),
            Message::Goodbye {
                reason: "sharing_stopped".to_owned(),
            },
            Message::Subscribe {
                surface: 1,
                profile: Profile::Thumbnail,
            },
            Message::Unsubscribe { surface: 1 },
            Message::Viewport(Viewport {
                surface: 1,
                rect: Rect::new(0, 0, 1512, 982),
                scale_milli: 500,
            }),
            Message::Acknowledge(ResumeRequest {
                surface: 1,
                generation: Generation(2),
                sequence: 100,
            }),
            Message::CacheMiss {
                surface: 1,
                tile: TileIndex(42),
                hash: TileHash(0xDEAD_BEEF),
            },
            Message::Batch(Batch {
                surface: SurfaceId(1),
                generation: Generation(2),
                sequence: 101,
                size: Size::new(64, 64).unwrap(),
                ops: vec![TileOp::Solid {
                    tile: TileIndex(0),
                    color: [1, 2, 3],
                }],
            }),
            Message::MotionRegion {
                surface: 1,
                rect: Some(Rect::new(896, 512, 384, 256)),
            },
            Message::MotionRegion {
                surface: 1,
                rect: None,
            },
            Message::Control(ControlHolder::AnotherDevice),
            Message::RequestControl,
            Message::ReleaseControl,
            Message::PointerMove {
                surface: 1,
                x: 100,
                y: 200,
            },
            Message::PointerButton {
                surface: 1,
                x: 100,
                y: 200,
                button: PointerButton::Secondary,
                pressed: false,
            },
            Message::Scroll {
                surface: 1,
                x: 100,
                y: 200,
                dx: -3,
                dy: 120,
            },
            Message::Key(KeyEvent {
                usage: 0x28,
                modifiers: Modifiers::new(Modifiers::COMMAND).unwrap(),
                pressed: true,
            }),
            Message::Text {
                surface: 1,
                text: "cargo test\n".to_owned(),
            },
        ]
    }

    #[test]
    fn every_message_roundtrips() {
        for message in all_messages() {
            let bytes = message.encode().unwrap();
            assert_eq!(Message::decode(&bytes).unwrap(), message);
        }
    }

    #[test]
    fn golden_hello_and_pointer_layouts_are_stable() {
        let hello = Message::Hello(Hello {
            version: 1,
            ticket_proof: [0xAB; 32],
            cache_bytes: 0x0400_0000,
            resume: None,
        });
        let mut expected = vec![0x01, 0x00, 0x01];
        expected.extend([0xAB; 32]);
        expected.extend([0, 0, 0, 0, 0x04, 0, 0, 0, 0x00]);
        assert_eq!(hello.encode().unwrap(), expected);

        let button = Message::PointerButton {
            surface: 2,
            x: 0x0102,
            y: 3,
            button: PointerButton::Primary,
            pressed: true,
        };
        assert_eq!(
            button.encode().unwrap(),
            vec![0x33, 0, 0, 0, 2, 0, 0, 1, 2, 0, 0, 0, 3, 0, 1]
        );
    }

    #[test]
    fn invalid_fields_are_rejected() {
        let mut other_version = Message::Hello(Hello {
            version: 1,
            ticket_proof: [0; 32],
            cache_bytes: 1,
            resume: None,
        })
        .encode()
        .unwrap();
        other_version[2] = 2;
        assert_eq!(
            Message::decode(&other_version),
            Err(ProtocolError::UnsupportedVersion)
        );

        assert_eq!(
            Message::decode(&[0x7F]),
            Err(ProtocolError::Malformed),
            "unknown kind"
        );
        assert_eq!(
            Message::decode(&[REQUEST_CONTROL, 0]),
            Err(ProtocolError::Malformed),
            "trailing byte"
        );
        assert_eq!(
            Message::decode(&[KEY, 0, 4, 0x10, 1]),
            Err(ProtocolError::Malformed),
            "unknown modifier"
        );
        assert_eq!(
            Message::decode(&[POINTER_BUTTON, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 2]),
            Err(ProtocolError::Malformed),
            "bool of 2"
        );
        let far = Message::PointerMove {
            surface: 1,
            x: MAX_SURFACE_DIMENSION,
            y: 0,
        }
        .encode()
        .unwrap();
        assert_eq!(
            Message::decode(&far),
            Err(ProtocolError::Malformed),
            "coordinate outside any surface"
        );
        let mut control_text = Message::Text {
            surface: 1,
            text: "a".to_owned(),
        }
        .encode()
        .unwrap();
        *control_text.last_mut().unwrap() = 0x1B;
        assert_eq!(
            Message::decode(&control_text),
            Err(ProtocolError::Malformed),
            "escape characters are not text"
        );
        assert_eq!(
            Message::Text {
                surface: 1,
                text: "x".repeat(MAX_TEXT_BYTES + 1)
            }
            .encode(),
            Err(ProtocolError::Malformed)
        );
        assert_eq!(
            Message::decode(&[BATCH, 0, 1]),
            Err(ProtocolError::InvalidBatch)
        );
    }

    proptest! {
        #[test]
        fn random_bodies_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = Message::decode(&bytes);
        }

        #[test]
        fn mutated_messages_reencode_to_themselves(index in 0usize..19, at in 0usize..64, value in any::<u8>()) {
            let message = &all_messages()[index];
            let mut bytes = message.encode().unwrap();
            let at = at % bytes.len();
            bytes[at] = value;
            if let Ok(decoded) = Message::decode(&bytes) {
                prop_assert_eq!(Message::decode(&decoded.encode().unwrap()).unwrap(), decoded);
            }
        }
    }
}
