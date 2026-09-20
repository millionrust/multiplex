//! The phone or computer looking at a remote screen.

use std::collections::{HashMap, VecDeque};

use multiplex_screen_codec::{Decoder, FrameBuffer, Rect};
use multiplex_screen_protocol::{
    ControlHolder, FeatureSet, Hello, MAX_PANES, MAX_VIDEO_TOKENS, Message, PROTOCOL_VERSION,
    PanePlacement, PaneSession, Profile, ResumeOutcome, ResumeRequest, SurfaceInfo, VideoConfig,
    VideoFrame, Viewport,
};

use crate::motion::MotionView;
use crate::video::{Repaired, VideoRepair};
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
    /// The decoder configuration for a surface's motion region; hand it to the decoder before
    /// the first frame. Only ever sent when both sides agreed to the motion path.
    VideoConfig(VideoConfig),
    /// One encoded frame of the motion region, to decode and draw into its rectangle.
    VideoFrame(VideoFrame),
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

/// What arrived since the last burst mark.
///
/// Only the receiver can measure a link. The host can time its own sending, but that measures how
/// fast it chose to send, which is exactly the thing being questioned. So the host closes each
/// burst with a mark, and this times how long the bytes before it took to arrive.
#[derive(Debug, Default)]
struct BurstArrival {
    /// When the first byte of this burst arrived. Absent until a transport says so, which is how
    /// a caller that never reports arrivals ends up never claiming a measurement.
    first_at_micros: Option<u64>,
    /// When the most recent chunk arrived. The mark that closes a burst is in the last chunk, so
    /// this is when the burst finished arriving, and nothing needs a clock of its own.
    last_at_micros: u64,
    bytes: u64,
}

impl BurstArrival {
    /// Closes the burst and returns the bytes that arrived and how long they took, if that was
    /// measured at all.
    fn close(&mut self) -> Option<(u64, u64)> {
        let first = self.first_at_micros.take()?;
        let bytes = std::mem::take(&mut self.bytes);
        // A burst that arrived inside one clock tick says nothing about the link's rate beyond
        // "faster than this clock can see", and dividing by zero would claim an absurd figure.
        let spread = self
            .last_at_micros
            .checked_sub(first)
            .filter(|spread| *spread > 0)?;
        (bytes > 0).then_some((bytes, spread))
    }
}

/// A viewer's side of a session. Decoders survive reconnects so the host can resume.
pub struct ViewerSession {
    /// What this viewer can do beyond Stage A.
    features: FeatureSet,
    /// What the host agreed to, once the welcome arrives.
    agreed: FeatureSet,
    cache_bytes: usize,
    state: State,
    surfaces: Vec<SurfaceInfo>,
    decoders: HashMap<u32, Decoder>,
    control: ControlHolder,
    panes: HashMap<u32, Vec<PanePlacement>>,
    attached: Vec<PaneSession>,
    outbox: VecDeque<Message>,
    /// What has arrived since the last burst mark, for measuring the link.
    burst: BurstArrival,
    /// Rebuilds video frames the link dropped, from the parity the host sends with each group.
    repair: VideoRepair,
    /// Decodes the motion region and draws it into the surface it belongs to.
    motion: MotionView,
}

impl ViewerSession {
    pub fn new(cache_bytes: usize) -> Self {
        Self::with_features(cache_bytes, FeatureSet::none())
    }

    /// A viewer that tells the host what more it can do than Stage A.
    pub fn with_features(cache_bytes: usize, features: FeatureSet) -> Self {
        Self {
            features,
            agreed: FeatureSet::none(),
            cache_bytes,
            state: State::Disconnected,
            surfaces: Vec::new(),
            decoders: HashMap::new(),
            control: ControlHolder::Nobody,
            panes: HashMap::new(),
            attached: Vec::new(),
            outbox: VecDeque::new(),
            burst: BurstArrival::default(),
            repair: VideoRepair::default(),
            motion: MotionView::default(),
        }
    }

    /// What both sides settled on, which is all this viewer may expect to receive. Empty until
    /// the welcome arrives, and empty against a host that only speaks Stage A.
    pub const fn agreed_features(&self) -> FeatureSet {
        self.agreed
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
            features: self.features,
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

    /// Tells the session that `bytes` of the screen stream arrived at `now_micros`.
    ///
    /// Call it from the transport, with every chunk, before feeding the messages it decodes to
    /// [`Self::receive`]. This is the only place a measurement of the link can come from: the
    /// host times its own sending, which measures its own pacing and nothing else.
    ///
    /// A caller that never calls it simply never reports, and the host falls back to whatever its
    /// transport knows. Nothing breaks; the estimate is just coarser.
    pub fn observed_bytes(&mut self, bytes: usize, now_micros: u64) {
        if bytes == 0 {
            return;
        }
        self.burst.last_at_micros = now_micros;
        if self.burst.first_at_micros.is_none() {
            // The first chunk starts the clock; it does not count toward what the clock measures.
            // Its own journey happened before the interval opened, so counting its bytes against
            // a spread that excludes its transit time reads the link as faster than it is — by
            // a factor of n/(n-1) over a burst of n chunks, which on a short burst is enormous.
            self.burst.first_at_micros = Some(now_micros);
            return;
        }
        self.burst.bytes = self.burst.bytes.saturating_add(bytes as u64);
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

    /// Tells the host what this viewer's decoder holds and what it could not rebuild.
    ///
    /// `held` names every long-term reference still in the decoder — the host may predict from
    /// those and nothing else. `lost` names a frame that did not arrive whole, which the host
    /// answers by predicting from an older held reference rather than by sending a keyframe.
    ///
    /// Refused when this viewer never negotiated long-term references, because a host that did
    /// not agree to them would close the session on receiving one.
    pub fn report_video(
        &mut self,
        surface: u32,
        held: &[u32],
        lost: Option<u64>,
    ) -> Result<(), SessionError> {
        if !self.agreed.has(FeatureSet::LONG_TERM_REFERENCES) {
            return Err(SessionError::ProtocolViolation);
        }
        let mut tokens = held.to_vec();
        tokens.truncate(MAX_VIDEO_TOKENS);
        self.outbox
            .push_back(Message::VideoAcknowledge { surface, tokens });
        if let Some(sequence) = lost {
            self.outbox
                .push_back(Message::VideoLost { surface, sequence });
        }
        Ok(())
    }

    /// Decodes one video frame and draws it into its surface.
    ///
    /// The picture goes into the same framebuffer the tiles do, so everything above this reads
    /// one complete screen and never has to know the motion path exists. A viewer that cannot
    /// decode hands the frame up instead.
    fn draw(&mut self, frame: VideoFrame) -> Vec<ViewerEvent> {
        let surface = frame.surface;
        let decoded = self.motion.decode(&frame);
        if decoded.undecoded {
            return vec![ViewerEvent::VideoFrame(frame)];
        }
        // Acknowledging is what lets the host recover from the next loss without a keyframe, so
        // it goes out as soon as the decoder actually holds the reference.
        if let Some(holding) = decoded.holding
            && self.agreed.has(FeatureSet::LONG_TERM_REFERENCES)
        {
            self.outbox.push_back(Message::VideoAcknowledge {
                surface,
                tokens: holding,
            });
        }
        let Some((rect, picture)) = decoded.picture else {
            // Nothing to draw: the frame predicted from one that never arrived. What is on the
            // screen is still the last good picture, which is better than a torn one.
            return Vec::new();
        };
        let Some(framebuffer) = self
            .decoders
            .get_mut(&surface)
            .and_then(Decoder::framebuffer_mut)
        else {
            return Vec::new();
        };
        // The region can outlive the size the decoder agreed to, so it is clipped rather than
        // trusted; a picture that does not fit is not drawn at all.
        let rect = Rect::new(rect.x, rect.y, picture.width, picture.height)
            .intersect(framebuffer.size().bounds());
        if rect.width != picture.width
            || rect.height != picture.height
            || framebuffer.write_rect(rect, &picture.bgra).is_err()
        {
            return Vec::new();
        }
        vec![ViewerEvent::Updated {
            surface,
            preview: false,
            damaged: vec![rect],
            reset: false,
        }]
    }

    /// Turns a repair into events, and tells the host about anything parity could not rebuild.
    ///
    /// The report goes out on its own: the viewer knows a group failed before the application
    /// does, and the sooner the host hears, the sooner it predicts from a reference this viewer
    /// still holds instead of the one that never arrived.
    fn deliver(&mut self, repaired: Repaired) -> Vec<ViewerEvent> {
        for sequence in repaired.lost {
            if self.agreed.has(FeatureSet::LONG_TERM_REFERENCES) {
                self.outbox.push_back(Message::VideoLost {
                    surface: repaired.surface,
                    sequence,
                });
            }
        }
        repaired
            .frames
            .into_iter()
            .map(ViewerEvent::VideoFrame)
            .collect()
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
                self.motion.clear_all();
                self.state = State::Closed;
                Ok(vec![ViewerEvent::Closed { reason }])
            }
            (State::AwaitingWelcome, Message::Welcome(welcome)) => {
                // Only what both sides advertised may be used from here on.
                self.agreed = self.features.shared(welcome.features);
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
                if rect.is_none() {
                    // Demoted: the tile path has the rectangle back, and this decoder describes a
                    // stream that has ended. Holding it would leave stale references acknowledged.
                    self.motion.clear(surface);
                }
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
            // The motion path, but only if this viewer asked for it and the host agreed. A host
            // sending video to a viewer that never advertised it is talking to the wrong peer.
            (State::Open, Message::VideoConfig(config))
                if self.agreed.has(FeatureSet::MOTION_VIDEO) =>
            {
                // A new configuration is a new encoder, whose sequences and groups start again,
                // so nothing kept for repair still describes this stream.
                self.repair.restart(config.surface);
                self.motion.configure(&config);
                // Handed up only when this build cannot decode it, so an application on a
                // platform without a hardware decoder can do it itself.
                if self.motion.decoding(config.surface) {
                    Ok(Vec::new())
                } else {
                    Ok(vec![ViewerEvent::VideoConfig(config)])
                }
            }
            (State::Open, Message::VideoFrame(frame))
                if self.agreed.has(FeatureSet::MOTION_VIDEO) =>
            {
                // Decoded first and repaired second: the code is systematic, so a frame that
                // arrived is usable now and waiting for its group would be pure latency.
                let mut events = self.draw(frame.clone());
                if self.agreed.has(FeatureSet::VIDEO_PARITY) {
                    let repaired = self.repair.frame(&frame);
                    events.extend(self.deliver(repaired));
                }
                Ok(events)
            }
            // The host closed a burst. Answer with what arrived and how long it took, which is
            // the one measurement of the link neither side can take alone.
            (State::Open, Message::BurstMark { burst, bytes: _ })
                if self.agreed.has(FeatureSet::BANDWIDTH_REPORTS) =>
            {
                if let Some((bytes, spread_micros)) = self.burst.close() {
                    self.outbox.push_back(Message::BurstReport {
                        burst,
                        bytes,
                        spread_micros,
                    });
                }
                Ok(Vec::new())
            }
            (State::Open, Message::Parity(parity)) if self.agreed.has(FeatureSet::VIDEO_PARITY) => {
                let repaired = self.repair.parity(&parity);
                Ok(self.deliver(repaired))
            }
            (State::Open, _) => {
                self.state = State::Closed;
                Err(SessionError::ProtocolViolation)
            }
        }
    }
}
