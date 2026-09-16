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
//! | 0x15 | viewer → host | attached panes: terminal session ids the viewer draws as text |
//! | 0x20 | host → viewer | batch: one `TSB1` tile batch |
//! | 0x21 | host → viewer | motion region: surface, optional rectangle |
//! | 0x22 | host → viewer | control holder |
//! | 0x23 | host → viewer | pane placements: surface, terminal panes with rectangle and cell size |
//! | 0x24 | host → viewer | video config: surface, codec, rectangle, decoder parameter sets |
//! | 0x25 | host → viewer | video frame: surface, sequence, keyframe, optional LTR token, payload |
//! | 0x26 | host → viewer | parity: surface, group, shard index, data shard count, payload |
//! | 0x30 | viewer → host | request control |
//! | 0x31 | viewer → host | release control |
//! | 0x32 | viewer → host | pointer move: surface, x, y |
//! | 0x33 | viewer → host | pointer button: surface, x, y, button, pressed |
//! | 0x34 | viewer → host | scroll: surface, x, y, dx, dy |
//! | 0x35 | viewer → host | key: USB HID usage, modifiers, pressed |
//! | 0x36 | viewer → host | text: surface, UTF-8 up to 256 bytes |
//! | 0x37 | viewer → host | video acknowledge: surface, long-term reference tokens held |
//! | 0x38 | viewer → host | video lost: surface, the sequence that did not arrive whole |
//!
//! Version 2 adds a feature set to the hello and the welcome, and everything from 0x24 on. A
//! version 1 peer never sends those bytes and is never sent them.

use termirust_screen_codec::{
    Batch, Generation, MAX_SURFACE_DIMENSION, Rect, Size, TileHash, TileIndex,
};

use crate::ProtocolError;

/// The version this build speaks. Version 1 is Stage A: tiles only, no features field.
///
/// A version 2 peer sends what it can do in the hello and the welcome, and a version 1 peer, which
/// says nothing, is understood to do only Stage A. Both are accepted, so a phone built before the
/// motion path existed keeps working against a host that has it.
pub const PROTOCOL_VERSION: u16 = 2;
/// The oldest version this build still talks to.
pub const MINIMUM_PROTOCOL_VERSION: u16 = 1;
/// The first version whose hello and welcome carry a feature set.
pub const FEATURES_PROTOCOL_VERSION: u16 = 2;
const MAX_SURFACES: usize = 16;
const MAX_NAME_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 256;
const MAX_REASON_BYTES: usize = 64;
/// Most terminal panes one placements message or attachment list names.
pub const MAX_PANES: usize = 32;
/// The largest encoded video frame one message carries. A 4K keyframe measured 688 KB in spike
/// 0.3, so this leaves room for one without letting a peer claim an unbounded allocation.
pub const MAX_VIDEO_FRAME_BYTES: usize = 4 * 1024 * 1024;
/// The largest parity payload, which is one shard rather than a whole group.
pub const MAX_PARITY_BYTES: usize = MAX_VIDEO_FRAME_BYTES;
/// Most acknowledgement tokens one message carries.
pub const MAX_VIDEO_TOKENS: usize = 32;

/// What a peer can do beyond Stage A.
///
/// Sent by both sides, and only what both advertise may be used: a host never sends video to a
/// viewer that cannot decode it, and never waits for acknowledgements a viewer will not send.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FeatureSet(u32);

impl FeatureSet {
    /// Encoded video for the motion region, instead of tiles.
    pub const MOTION_VIDEO: u32 = 1;
    /// Forward error correction over the video packets.
    pub const VIDEO_PARITY: u32 = 1 << 1;
    /// Long-term reference acknowledgement, so loss recovers without a keyframe.
    pub const LONG_TERM_REFERENCES: u32 = 1 << 2;

    /// Every bit this build understands. An unknown bit is dropped rather than refused, so a
    /// later peer advertising more does not end the session.
    pub const KNOWN: u32 = Self::MOTION_VIDEO | Self::VIDEO_PARITY | Self::LONG_TERM_REFERENCES;

    pub const fn none() -> Self {
        Self(0)
    }

    /// Keeps only the bits this build knows about.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & Self::KNOWN)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn has(self, feature: u32) -> bool {
        self.0 & feature == feature
    }

    pub const fn with(self, feature: u32) -> Self {
        Self(self.0 | (feature & Self::KNOWN))
    }

    /// What both peers can do, which is the only thing either may use.
    pub const fn shared(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// How the motion region is encoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotionCodec {
    /// HEVC, which every target decodes in hardware.
    Hevc,
}

/// One encoded frame of a surface's motion region.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoFrame {
    pub surface: u32,
    /// Counts up per surface, so a viewer can name what it lost.
    pub sequence: u64,
    /// A frame everything after it can be decoded from.
    pub keyframe: bool,
    /// The long-term reference this frame may become, once the viewer acknowledges it. `None`
    /// when the encoder did not mark it as a candidate.
    pub token: Option<u32>,
    pub payload: Vec<u8>,
}

/// The decoder configuration a viewer needs before the first frame: parameter sets, out of band.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoConfig {
    pub surface: u32,
    pub codec: MotionCodec,
    /// The region of the surface the video covers, which the viewer draws it into.
    pub rect: Rect,
    pub payload: Vec<u8>,
}

/// One forward error correction shard for a group of video packets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parity {
    pub surface: u32,
    /// Which group of data shards this repairs.
    pub group: u64,
    /// Which parity shard within the group, so duplicates are recognised.
    pub index: u8,
    /// How many data shards the group had, which the decoder needs to rebuild it.
    pub data_shards: u8,
    pub payload: Vec<u8>,
}

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
    /// What this viewer can do beyond Stage A. Empty from a version 1 viewer.
    pub features: FeatureSet,
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
    /// What this host can do beyond Stage A. Empty from a version 1 host.
    pub features: FeatureSet,
}

/// The part of a surface a viewer shows, in surface pixels, and its zoom in thousandths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub surface: u32,
    pub rect: Rect,
    pub scale_milli: u16,
}

/// A TermiRust terminal session, by its hosted session id.
pub type PaneSession = [u8; 16];

/// Where a TermiRust terminal pane sits on a surface. A viewer attached to the pane's text stream
/// draws the rectangle from that stream; the host sends it no pixels there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanePlacement {
    pub session: PaneSession,
    /// In surface pixels.
    pub rect: Rect,
    /// One terminal cell, in surface pixels.
    pub cell_width: u16,
    pub cell_height: u16,
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
    /// Replaces the viewer's previous list.
    AttachedPanes {
        sessions: Vec<PaneSession>,
    },
    Batch(Batch),
    MotionRegion {
        surface: u32,
        rect: Option<Rect>,
    },
    Control(ControlHolder),
    /// Replaces the previous placements on `surface`.
    PanePlacements {
        surface: u32,
        panes: Vec<PanePlacement>,
    },
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
    /// Host to viewer: the decoder configuration for a surface's motion region.
    VideoConfig(VideoConfig),
    /// Host to viewer: one encoded frame of the motion region.
    VideoFrame(VideoFrame),
    /// Host to viewer: one forward error correction shard.
    Parity(Parity),
    /// Viewer to host: frames this viewer decoded, so the encoder may predict from them.
    VideoAcknowledge {
        surface: u32,
        tokens: Vec<u32>,
    },
    /// Viewer to host: this viewer could not decode from `sequence` onwards, so the next frame
    /// has to come from a reference it still holds.
    VideoLost {
        surface: u32,
        sequence: u64,
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
const ATTACHED_PANES: u8 = 0x15;
const BATCH: u8 = 0x20;
const MOTION_REGION: u8 = 0x21;
const CONTROL: u8 = 0x22;
const PANE_PLACEMENTS: u8 = 0x23;
const VIDEO_CONFIG: u8 = 0x24;
const VIDEO_FRAME: u8 = 0x25;
const PARITY: u8 = 0x26;
const REQUEST_CONTROL: u8 = 0x30;
const RELEASE_CONTROL: u8 = 0x31;
const POINTER_MOVE: u8 = 0x32;
const POINTER_BUTTON: u8 = 0x33;
const SCROLL: u8 = 0x34;
const KEY: u8 = 0x35;
const TEXT: u8 = 0x36;
const VIDEO_ACKNOWLEDGE: u8 = 0x37;
const VIDEO_LOST: u8 = 0x38;

/// Which kind of input a message carries, for hosts that grant pointer and keyboard separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputKind {
    Pointer,
    Keyboard,
}

impl Message {
    pub const fn is_batch(&self) -> bool {
        matches!(self, Self::Batch(_))
    }

    /// The input this message would inject, if any.
    pub const fn input_kind(&self) -> Option<InputKind> {
        match self {
            Self::PointerMove { .. } | Self::PointerButton { .. } | Self::Scroll { .. } => {
                Some(InputKind::Pointer)
            }
            Self::Key(_) | Self::Text { .. } => Some(InputKind::Keyboard),
            _ => None,
        }
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
                // A version 1 hello has no room for a feature set, so a peer speaking it is
                // Stage A whatever this build can do.
                if hello.version >= FEATURES_PROTOCOL_VERSION {
                    w.u32(hello.features.bits());
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
                if welcome.version >= FEATURES_PROTOCOL_VERSION {
                    w.u32(welcome.features.bits());
                }
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
            Self::AttachedPanes { sessions } => {
                w.u8(ATTACHED_PANES);
                if sessions.len() > MAX_PANES {
                    return Err(ProtocolError::Malformed);
                }
                w.u8(sessions.len() as u8);
                sessions.iter().for_each(|session| w.bytes(session));
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
            Self::PanePlacements { surface, panes } => {
                w.u8(PANE_PLACEMENTS);
                w.u32(*surface);
                if panes.len() > MAX_PANES {
                    return Err(ProtocolError::Malformed);
                }
                w.u8(panes.len() as u8);
                for pane in panes {
                    if pane.cell_width == 0 || pane.cell_height == 0 {
                        return Err(ProtocolError::Malformed);
                    }
                    w.bytes(&pane.session);
                    w.rect(pane.rect);
                    w.u16(pane.cell_width);
                    w.u16(pane.cell_height);
                }
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
            Self::VideoConfig(config) => {
                if config.payload.is_empty() || config.payload.len() > MAX_VIDEO_FRAME_BYTES {
                    return Err(ProtocolError::Malformed);
                }
                w.u8(VIDEO_CONFIG);
                w.u32(config.surface);
                w.u8(match config.codec {
                    MotionCodec::Hevc => 0,
                });
                w.rect(config.rect);
                w.u32(config.payload.len() as u32);
                w.bytes(&config.payload);
            }
            Self::VideoFrame(frame) => {
                if frame.payload.is_empty() || frame.payload.len() > MAX_VIDEO_FRAME_BYTES {
                    return Err(ProtocolError::Malformed);
                }
                w.u8(VIDEO_FRAME);
                w.u32(frame.surface);
                w.u64(frame.sequence);
                w.u8(u8::from(frame.keyframe));
                match frame.token {
                    None => w.u8(0),
                    Some(token) => {
                        w.u8(1);
                        w.u32(token);
                    }
                }
                w.u32(frame.payload.len() as u32);
                w.bytes(&frame.payload);
            }
            Self::Parity(parity) => {
                if parity.payload.is_empty()
                    || parity.payload.len() > MAX_PARITY_BYTES
                    || parity.data_shards == 0
                {
                    return Err(ProtocolError::Malformed);
                }
                w.u8(PARITY);
                w.u32(parity.surface);
                w.u64(parity.group);
                w.u8(parity.index);
                w.u8(parity.data_shards);
                w.u32(parity.payload.len() as u32);
                w.bytes(&parity.payload);
            }
            Self::VideoAcknowledge { surface, tokens } => {
                if tokens.len() > MAX_VIDEO_TOKENS {
                    return Err(ProtocolError::Malformed);
                }
                w.u8(VIDEO_ACKNOWLEDGE);
                w.u32(*surface);
                w.u8(tokens.len() as u8);
                for token in tokens {
                    w.u32(*token);
                }
            }
            Self::VideoLost { surface, sequence } => {
                w.u8(VIDEO_LOST);
                w.u32(*surface);
                w.u64(*sequence);
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
                if !(MINIMUM_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version) {
                    return Err(ProtocolError::UnsupportedVersion);
                }
                let ticket_proof = r.take(32)?.try_into().expect("32 bytes");
                let cache_bytes = r.u64()?;
                let resume = match r.u8()? {
                    0 => None,
                    1 => Some(r.resume()?),
                    _ => return Err(ProtocolError::Malformed),
                };
                // A version 1 viewer says nothing about features, which means Stage A only.
                let features = if version >= FEATURES_PROTOCOL_VERSION {
                    FeatureSet::from_bits(r.u32()?)
                } else {
                    FeatureSet::none()
                };
                Self::Hello(Hello {
                    version,
                    ticket_proof,
                    cache_bytes,
                    resume,
                    features,
                })
            }
            WELCOME => {
                let version = r.u16()?;
                if !(MINIMUM_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version) {
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
                let features = if version >= FEATURES_PROTOCOL_VERSION {
                    FeatureSet::from_bits(r.u32()?)
                } else {
                    FeatureSet::none()
                };
                Self::Welcome(Welcome {
                    version,
                    surfaces,
                    resume,
                    features,
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
            ATTACHED_PANES => {
                let count = r.pane_count()?;
                let mut sessions = Vec::with_capacity(count);
                for _ in 0..count {
                    sessions.push(r.session()?);
                }
                Self::AttachedPanes { sessions }
            }
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
            PANE_PLACEMENTS => {
                let surface = r.u32()?;
                let count = r.pane_count()?;
                let mut panes = Vec::with_capacity(count);
                for _ in 0..count {
                    let session = r.session()?;
                    let rect = r.rect()?;
                    let (cell_width, cell_height) = (r.u16()?, r.u16()?);
                    if cell_width == 0 || cell_height == 0 {
                        return Err(ProtocolError::Malformed);
                    }
                    panes.push(PanePlacement {
                        session,
                        rect,
                        cell_width,
                        cell_height,
                    });
                }
                Self::PanePlacements { surface, panes }
            }
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
            VIDEO_CONFIG => {
                let surface = r.u32()?;
                let codec = match r.u8()? {
                    0 => MotionCodec::Hevc,
                    _ => return Err(ProtocolError::Malformed),
                };
                let rect = r.rect()?;
                let payload = r.sized_bytes(MAX_VIDEO_FRAME_BYTES)?;
                Self::VideoConfig(VideoConfig {
                    surface,
                    codec,
                    rect,
                    payload,
                })
            }
            VIDEO_FRAME => {
                let surface = r.u32()?;
                let sequence = r.u64()?;
                let keyframe = match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(ProtocolError::Malformed),
                };
                let token = match r.u8()? {
                    0 => None,
                    1 => Some(r.u32()?),
                    _ => return Err(ProtocolError::Malformed),
                };
                let payload = r.sized_bytes(MAX_VIDEO_FRAME_BYTES)?;
                Self::VideoFrame(VideoFrame {
                    surface,
                    sequence,
                    keyframe,
                    token,
                    payload,
                })
            }
            PARITY => {
                let surface = r.u32()?;
                let group = r.u64()?;
                let index = r.u8()?;
                let data_shards = r.u8()?;
                if data_shards == 0 {
                    return Err(ProtocolError::Malformed);
                }
                let payload = r.sized_bytes(MAX_PARITY_BYTES)?;
                Self::Parity(Parity {
                    surface,
                    group,
                    index,
                    data_shards,
                    payload,
                })
            }
            VIDEO_ACKNOWLEDGE => {
                let surface = r.u32()?;
                let count = r.u8()? as usize;
                if count > MAX_VIDEO_TOKENS {
                    return Err(ProtocolError::Malformed);
                }
                let mut tokens = Vec::with_capacity(count);
                for _ in 0..count {
                    tokens.push(r.u32()?);
                }
                Self::VideoAcknowledge { surface, tokens }
            }
            VIDEO_LOST => Self::VideoLost {
                surface: r.u32()?,
                sequence: r.u64()?,
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
    fn pane_count(&mut self) -> Result<usize, ProtocolError> {
        let count = self.u8()? as usize;
        if count > MAX_PANES {
            return Err(ProtocolError::Malformed);
        }
        Ok(count)
    }
    fn session(&mut self) -> Result<PaneSession, ProtocolError> {
        Ok(self.take(16)?.try_into().expect("16 bytes"))
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

    /// A 32-bit length and that many bytes. The length is checked against `max` before anything
    /// is allocated, so a peer cannot ask for memory by claiming a huge frame.
    fn sized_bytes(&mut self, max: usize) -> Result<Vec<u8>, ProtocolError> {
        let length = self.u32()? as usize;
        if length == 0 || length > max {
            return Err(ProtocolError::Malformed);
        }
        Ok(self.take(length)?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use termirust_screen_codec::{SurfaceId, TileOp};

    /// Keep in step with `all_messages`; the mutation property indexes into it.
    const ALL_MESSAGE_COUNT: usize = 30;

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
                features: FeatureSet::from_bits(FeatureSet::KNOWN),
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
                features: FeatureSet::none().with(FeatureSet::MOTION_VIDEO),
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
            Message::AttachedPanes {
                sessions: vec![[0x11; 16], [0x22; 16]],
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
            Message::PanePlacements {
                surface: 1,
                panes: vec![PanePlacement {
                    session: [0x11; 16],
                    rect: Rect::new(240, 96, 1200, 720),
                    cell_width: 16,
                    cell_height: 34,
                }],
            },
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
            Message::VideoConfig(VideoConfig {
                surface: 1,
                codec: MotionCodec::Hevc,
                rect: Rect::new(896, 512, 384, 256),
                payload: vec![0x40, 0x01, 0x0C, 0x01],
            }),
            Message::VideoFrame(VideoFrame {
                surface: 1,
                sequence: 4096,
                keyframe: true,
                token: Some(7),
                payload: vec![0x26, 0x01, 0xAF],
            }),
            Message::VideoFrame(VideoFrame {
                surface: 1,
                sequence: 4097,
                keyframe: false,
                token: None,
                payload: vec![0x02, 0x01],
            }),
            Message::Parity(Parity {
                surface: 1,
                group: 12,
                index: 1,
                data_shards: 8,
                payload: vec![0xFF; 16],
            }),
            Message::VideoAcknowledge {
                surface: 1,
                tokens: vec![7, 9],
            },
            Message::VideoAcknowledge {
                surface: 1,
                tokens: Vec::new(),
            },
            Message::VideoLost {
                surface: 1,
                sequence: 4097,
            },
            // The two hellos a host must tell apart: a Stage A viewer that has no feature field
            // at all, and a Stage B one that asks for the motion path.
            Message::Hello(Hello {
                version: MINIMUM_PROTOCOL_VERSION,
                ticket_proof: [3; 32],
                cache_bytes: 1 << 20,
                resume: None,
                features: FeatureSet::none(),
            }),
            Message::Welcome(Welcome {
                version: MINIMUM_PROTOCOL_VERSION,
                surfaces: Vec::new(),
                resume: ResumeOutcome::NotRequested,
                features: FeatureSet::none(),
            }),
        ]
    }

    #[test]
    fn every_message_roundtrips() {
        let messages = all_messages();
        assert_eq!(messages.len(), ALL_MESSAGE_COUNT);
        for message in messages {
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
            features: FeatureSet::none(),
        });
        let mut expected = vec![0x01, 0x00, 0x01];
        expected.extend([0xAB; 32]);
        expected.extend([0, 0, 0, 0, 0x04, 0, 0, 0, 0x00]);
        assert_eq!(
            hello.encode().unwrap(),
            expected,
            "a version 1 hello ends at the resume flag, with no feature word"
        );

        let hello = Message::Hello(Hello {
            version: 2,
            ticket_proof: [0xAB; 32],
            cache_bytes: 0x0400_0000,
            resume: None,
            features: FeatureSet::none()
                .with(FeatureSet::MOTION_VIDEO)
                .with(FeatureSet::LONG_TERM_REFERENCES),
        });
        expected[2] = 0x02;
        expected.extend([0, 0, 0, 0b101]);
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
    fn golden_video_layouts_are_stable() {
        let config = Message::VideoConfig(VideoConfig {
            surface: 1,
            codec: MotionCodec::Hevc,
            rect: Rect::new(16, 32, 640, 480),
            payload: vec![0x40, 0x01],
        });
        assert_eq!(
            config.encode().unwrap(),
            vec![
                0x24, 0, 0, 0, 1, // kind, surface
                0, // HEVC
                0, 0, 0, 16, 0, 0, 0, 32, 0, 0, 2, 128, 0, 0, 1, 224, // x, y, width, height
                0, 0, 0, 2, 0x40, 0x01, // payload length and bytes
            ]
        );

        let frame = Message::VideoFrame(VideoFrame {
            surface: 1,
            sequence: 258,
            keyframe: true,
            token: Some(9),
            payload: vec![0xAA],
        });
        assert_eq!(
            frame.encode().unwrap(),
            vec![
                0x25, 0, 0, 0, 1, // kind, surface
                0, 0, 0, 0, 0, 0, 1, 2, // sequence
                1, // keyframe
                1, 0, 0, 0, 9, // a token is present, and is 9
                0, 0, 0, 1, 0xAA,
            ]
        );
        let untagged = Message::VideoFrame(VideoFrame {
            surface: 1,
            sequence: 258,
            keyframe: false,
            token: None,
            payload: vec![0xAA],
        });
        assert_eq!(
            untagged.encode().unwrap(),
            vec![
                0x25, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 0, 1, 0xAA
            ],
        );

        let parity = Message::Parity(Parity {
            surface: 1,
            group: 3,
            index: 1,
            data_shards: 8,
            payload: vec![0x5A, 0x5A],
        });
        assert_eq!(
            parity.encode().unwrap(),
            vec![
                0x26, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 1, 8, 0, 0, 0, 2, 0x5A, 0x5A
            ],
        );

        let acknowledge = Message::VideoAcknowledge {
            surface: 1,
            tokens: vec![9, 11],
        };
        assert_eq!(
            acknowledge.encode().unwrap(),
            vec![0x37, 0, 0, 0, 1, 2, 0, 0, 0, 9, 0, 0, 0, 11],
        );

        let lost = Message::VideoLost {
            surface: 1,
            sequence: 258,
        };
        assert_eq!(
            lost.encode().unwrap(),
            vec![0x38, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, 2],
        );
    }

    #[test]
    fn a_version_1_peer_never_carries_features() {
        let stage_a = Message::Hello(Hello {
            version: MINIMUM_PROTOCOL_VERSION,
            ticket_proof: [0; 32],
            cache_bytes: 1,
            // Whatever this build can do, a version 1 hello has nowhere to say it, so the bits
            // must not survive the round trip and tempt a host into sending video.
            features: FeatureSet::from_bits(FeatureSet::KNOWN),
            resume: None,
        });
        let decoded = Message::decode(&stage_a.encode().unwrap()).unwrap();
        assert_eq!(
            decoded,
            Message::Hello(Hello {
                features: FeatureSet::none(),
                ..match stage_a {
                    Message::Hello(hello) => hello,
                    _ => unreachable!(),
                }
            })
        );

        let welcome = Message::Welcome(Welcome {
            version: MINIMUM_PROTOCOL_VERSION,
            surfaces: Vec::new(),
            resume: ResumeOutcome::NotRequested,
            features: FeatureSet::from_bits(FeatureSet::KNOWN),
        });
        assert_eq!(
            Message::decode(&welcome.encode().unwrap()).unwrap(),
            Message::Welcome(Welcome {
                version: MINIMUM_PROTOCOL_VERSION,
                surfaces: Vec::new(),
                resume: ResumeOutcome::NotRequested,
                features: FeatureSet::none(),
            })
        );
    }

    #[test]
    fn features_negotiate_down_to_what_both_peers_have() {
        let host = FeatureSet::none()
            .with(FeatureSet::MOTION_VIDEO)
            .with(FeatureSet::LONG_TERM_REFERENCES);
        let viewer = FeatureSet::none()
            .with(FeatureSet::MOTION_VIDEO)
            .with(FeatureSet::VIDEO_PARITY);
        let shared = host.shared(viewer);
        assert!(shared.has(FeatureSet::MOTION_VIDEO));
        assert!(!shared.has(FeatureSet::VIDEO_PARITY), "the host cannot");
        assert!(
            !shared.has(FeatureSet::LONG_TERM_REFERENCES),
            "the viewer cannot"
        );
        // A later peer advertising something this build has never heard of keeps the bits it
        // shares, rather than ending the session.
        let newer = FeatureSet::from_bits(FeatureSet::MOTION_VIDEO | 0x8000_0000);
        assert_eq!(newer.bits(), FeatureSet::MOTION_VIDEO);
    }

    #[test]
    fn video_payloads_are_bounded_and_never_empty() {
        let empty = Message::VideoFrame(VideoFrame {
            surface: 1,
            sequence: 0,
            keyframe: true,
            token: None,
            payload: Vec::new(),
        });
        assert_eq!(empty.encode(), Err(ProtocolError::Malformed));
        assert_eq!(
            Message::VideoConfig(VideoConfig {
                surface: 1,
                codec: MotionCodec::Hevc,
                rect: Rect::new(0, 0, 16, 16),
                payload: vec![0; MAX_VIDEO_FRAME_BYTES + 1],
            })
            .encode(),
            Err(ProtocolError::Malformed)
        );
        assert_eq!(
            Message::Parity(Parity {
                surface: 1,
                group: 0,
                index: 0,
                data_shards: 0,
                payload: vec![1],
            })
            .encode(),
            Err(ProtocolError::Malformed),
            "a group with no data shards repairs nothing"
        );
        assert_eq!(
            Message::VideoAcknowledge {
                surface: 1,
                tokens: vec![0; MAX_VIDEO_TOKENS + 1],
            }
            .encode(),
            Err(ProtocolError::Malformed)
        );
        // A length larger than the message is refused before anything is allocated for it.
        let claimed = vec![
            VIDEO_FRAME,
            0,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            0,
            0,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
        ];
        assert_eq!(Message::decode(&claimed), Err(ProtocolError::Malformed));
        assert_eq!(
            Message::decode(&[VIDEO_CONFIG, 0, 0, 0, 1, 9]),
            Err(ProtocolError::Malformed),
            "unknown codec"
        );
    }

    #[test]
    fn invalid_fields_are_rejected() {
        let mut other_version = Message::Hello(Hello {
            version: 1,
            ticket_proof: [0; 32],
            cache_bytes: 1,
            resume: None,
            features: FeatureSet::none(),
        })
        .encode()
        .unwrap();
        // One past what this build speaks; version 2 is now understood, so refusal needs a 3.
        other_version[2] = PROTOCOL_VERSION as u8 + 1;
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

    #[test]
    fn pane_lists_are_bounded_and_cells_are_never_empty() {
        let too_many = Message::AttachedPanes {
            sessions: vec![[0; 16]; MAX_PANES + 1],
        };
        assert_eq!(too_many.encode(), Err(ProtocolError::Malformed));
        let mut claimed = vec![ATTACHED_PANES, MAX_PANES as u8 + 1];
        claimed.extend([0; 16 * (MAX_PANES + 1)]);
        assert_eq!(Message::decode(&claimed), Err(ProtocolError::Malformed));

        let pane = PanePlacement {
            session: [1; 16],
            rect: Rect::new(0, 0, 100, 100),
            cell_width: 8,
            cell_height: 16,
        };
        let mut bytes = Message::PanePlacements {
            surface: 1,
            panes: vec![pane],
        }
        .encode()
        .unwrap();
        assert_eq!(bytes.len(), 1 + 4 + 1 + 16 + 16 + 4);
        let height_at = bytes.len() - 2;
        bytes[height_at..].copy_from_slice(&[0, 0]);
        assert_eq!(Message::decode(&bytes), Err(ProtocolError::Malformed));
        assert_eq!(
            Message::PanePlacements {
                surface: 1,
                panes: vec![PanePlacement {
                    cell_width: 0,
                    ..pane
                }],
            }
            .encode(),
            Err(ProtocolError::Malformed)
        );
    }

    proptest! {
        #[test]
        fn random_bodies_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = Message::decode(&bytes);
        }

        #[test]
        fn mutated_messages_reencode_to_themselves(index in 0usize..ALL_MESSAGE_COUNT, at in 0usize..64, value in any::<u8>()) {
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
