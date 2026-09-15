//! The phone or computer looking at a remote screen.

use std::collections::{HashMap, VecDeque};

use termirust_screen_codec::{Decoder, FrameBuffer, Rect};
use termirust_screen_protocol::{
    ControlHolder, Hello, MAX_PANES, Message, PROTOCOL_VERSION, PanePlacement, PaneSession,
    Profile, ResumeOutcome, ResumeRequest, SurfaceInfo, Viewport,
};

use crate::{InputEvent, SessionError, THUMBNAIL_CACHE_BYTES, THUMBNAIL_SURFACE_BIT};

/// Something the viewer's interface should show.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewerEvent {
    Welcomed {
        surfaces: Vec<SurfaceInfo>,
        resume: ResumeOutcome,
    },
    /// Part of a surface or its preview changed; repaint `damaged`.
    Updated {
        surface: u32,
        preview: bool,
        damaged: Vec<Rect>,
        reset: bool,
    },
    Control(ControlHolder),
    MotionRegion {
        surface: u32,
        rect: Option<Rect>,
    },
    /// Terminal panes on `surface` moved, appeared, or closed. Draw attached ones from their text.
    Panes {
        surface: u32,
        panes: Vec<PanePlacement>,
    },
    Closed {
        reason: String,
    },
}

enum State {
    Disconnected,
    AwaitingWelcome,
    Open,
    Closed,
}

/// A viewer's side of a session. Decoders survive reconnects so the host can resume.
pub struct ViewerSession {
    cache_bytes: usize,
    state: State,
    surfaces: Vec<SurfaceInfo>,
    decoders: HashMap<u32, Decoder>,
    control: ControlHolder,
    panes: HashMap<u32, Vec<PanePlacement>>,
    attached: Vec<PaneSession>,
    outbox: VecDeque<Message>,
}

impl ViewerSession {
    pub fn new(cache_bytes: usize) -> Self {
        Self {
            cache_bytes,
            state: State::Disconnected,
            surfaces: Vec::new(),
            decoders: HashMap::new(),
            control: ControlHolder::Nobody,
            panes: HashMap::new(),
            attached: Vec::new(),
            outbox: VecDeque::new(),
        }
    }

    /// Starts a connection with the ticket proof from the Controller channel. When a full view
    /// was open before, the hello asks to resume it.
    pub fn connect(&mut self, ticket_proof: [u8; 32]) {
        self.outbox.clear();
        let resume = self
            .decoders
            .iter()
            .filter(|(id, decoder)| *id & THUMBNAIL_SURFACE_BIT == 0 && decoder.last_sequence() > 0)
            .filter_map(|(id, decoder)| {
                decoder.generation().map(|generation| ResumeRequest {
                    surface: *id,
                    generation,
                    sequence: decoder.last_sequence(),
                })
            })
            .min_by_key(|request| request.surface);
        self.outbox.push_back(Message::Hello(Hello {
            version: PROTOCOL_VERSION,
            ticket_proof,
            cache_bytes: self.cache_bytes as u64,
            resume,
        }));
        if !self.attached.is_empty() {
            self.outbox.push_back(Message::AttachedPanes {
                sessions: self.attached.clone(),
            });
        }
        self.panes.clear();
        self.state = State::AwaitingWelcome;
    }

    /// The transport dropped. Framebuffers and caches are kept for the next [`Self::connect`].
    pub fn disconnected(&mut self) {
        self.outbox.clear();
        self.control = ControlHolder::Nobody;
        self.state = State::Disconnected;
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Open)
    }

    pub fn surfaces(&self) -> &[SurfaceInfo] {
        &self.surfaces
    }

    pub const fn control(&self) -> ControlHolder {
        self.control
    }

    pub fn poll_outgoing(&mut self) -> Option<Message> {
        self.outbox.pop_front()
    }

    /// The full view of `surface`, once any batch arrived.
    pub fn framebuffer(&self, surface: u32) -> Option<&FrameBuffer> {
        self.decoders.get(&surface).and_then(Decoder::framebuffer)
    }

    /// The preview of `surface`, once any preview arrived.
    pub fn preview(&self, surface: u32) -> Option<&FrameBuffer> {
        self.decoders
            .get(&(surface | THUMBNAIL_SURFACE_BIT))
            .and_then(Decoder::framebuffer)
    }

    pub fn subscribe(&mut self, surface: u32, profile: Profile) {
        self.outbox
            .push_back(Message::Subscribe { surface, profile });
    }

    pub fn unsubscribe(&mut self, surface: u32) {
        self.outbox.push_back(Message::Unsubscribe { surface });
    }

    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.outbox.push_back(Message::Viewport(viewport));
    }

    pub fn request_control(&mut self) {
        self.outbox.push_back(Message::RequestControl);
    }

    pub fn release_control(&mut self) {
        self.outbox.push_back(Message::ReleaseControl);
    }

    /// Names the terminal sessions whose text this viewer draws itself, replacing the previous
    /// list. The host stops sending pixels where those panes sit. Kept across reconnects; at most
    /// [`MAX_PANES`] are sent.
    pub fn attach_panes(&mut self, mut sessions: Vec<PaneSession>) {
        sessions.truncate(MAX_PANES);
        if sessions != self.attached {
            self.attached = sessions;
            if !matches!(self.state, State::Disconnected | State::Closed) {
                self.outbox.push_back(Message::AttachedPanes {
                    sessions: self.attached.clone(),
                });
            }
        }
    }

    /// Terminal panes on `surface`, as the host last published them.
    pub fn panes(&self, surface: u32) -> &[PanePlacement] {
        self.panes.get(&surface).map_or(&[], Vec::as_slice)
    }

    /// Sends input. The host injects it only while this viewer holds control.
    pub fn send_input(&mut self, input: InputEvent) {
        self.outbox.push_back(input.into_message());
    }

    /// Handles one message from the host.
    pub fn receive(&mut self, message: Message) -> Result<Vec<ViewerEvent>, SessionError> {
        match (&self.state, message) {
            (State::Closed | State::Disconnected, _) => Err(SessionError::Closed),
            (_, Message::Goodbye { reason }) => {
                self.state = State::Closed;
                Ok(vec![ViewerEvent::Closed { reason }])
            }
            (State::AwaitingWelcome, Message::Welcome(welcome)) => {
                self.surfaces = welcome.surfaces.clone();
                if welcome.resume != ResumeOutcome::Partial {
                    self.decoders
                        .retain(|id, _| id & THUMBNAIL_SURFACE_BIT != 0);
                }
                self.state = State::Open;
                Ok(vec![ViewerEvent::Welcomed {
                    surfaces: welcome.surfaces,
                    resume: welcome.resume,
                }])
            }
            (State::AwaitingWelcome, _) => {
                self.state = State::Closed;
                Err(SessionError::ProtocolViolation)
            }
            (State::Open, Message::Batch(batch)) => {
                let id = batch.surface.0;
                let cache = if id & THUMBNAIL_SURFACE_BIT != 0 {
                    THUMBNAIL_CACHE_BYTES
                } else {
                    self.cache_bytes
                };
                let decoder = self
                    .decoders
                    .entry(id)
                    .or_insert_with(|| Decoder::new(cache));
                let applied = decoder.apply(&batch)?;
                for miss in &applied.misses {
                    self.outbox.push_back(Message::CacheMiss {
                        surface: id,
                        tile: miss.tile,
                        hash: miss.hash,
                    });
                }
                self.outbox.push_back(Message::Acknowledge(ResumeRequest {
                    surface: id,
                    generation: batch.generation,
                    sequence: applied.sequence,
                }));
                Ok(vec![ViewerEvent::Updated {
                    surface: id & !THUMBNAIL_SURFACE_BIT,
                    preview: id & THUMBNAIL_SURFACE_BIT != 0,
                    damaged: applied.damaged,
                    reset: applied.reset,
                }])
            }
            (State::Open, Message::Control(holder)) => {
                self.control = holder;
                Ok(vec![ViewerEvent::Control(holder)])
            }
            (State::Open, Message::MotionRegion { surface, rect }) => {
                Ok(vec![ViewerEvent::MotionRegion { surface, rect }])
            }
            (State::Open, Message::PanePlacements { surface, panes }) => {
                if panes.is_empty() {
                    self.panes.remove(&surface);
                } else {
                    self.panes.insert(surface, panes.clone());
                }
                Ok(vec![ViewerEvent::Panes { surface, panes }])
            }
            (State::Open, _) => {
                self.state = State::Closed;
                Err(SessionError::ProtocolViolation)
            }
        }
    }
}
