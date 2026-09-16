//! The computer being viewed.

use std::collections::{HashMap, VecDeque};

use termirust_screen_codec::{
    Encoder, EncoderConfig, Frame, FrameBuffer, Generation, MotionEvent, Rect, Size, SurfaceId,
    downscale, preview_factor,
};
use termirust_screen_protocol::{
    ControlHolder, FeatureSet, Hello, MAX_PANES, Message, PROTOCOL_VERSION, PanePlacement,
    PaneSession, Profile, ResumeOutcome, SurfaceInfo, Viewport, Welcome,
};

use crate::{InputEvent, SessionError};

/// Set on the surface id of preview batches, so a viewer keeps previews and full views apart.
pub const THUMBNAIL_SURFACE_BIT: u32 = 0x8000_0000;
/// What a masked terminal pane holds in the pixel stream (BGRA). Viewers draw the pane's text on
/// top, so this is only seen for a moment while a pane's text is still loading.
pub const PANE_MASK_BGRA: [u8; 4] = [0x1C, 0x1E, 0x23, 0xFF];
/// Tile cache for previews, the same on both sides.
pub const THUMBNAIL_CACHE_BYTES: usize = 4 << 20;
/// Largest cache a viewer may ask the host to model.
const MAX_VIEWER_CACHE_BYTES: u64 = 1 << 30;

/// What a verified ticket allows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Grants {
    /// Stable identity of the paired device, for resume.
    pub device: u64,
    pub can_view: bool,
    pub can_control: bool,
}

/// Checks the ticket proof in a hello. Implemented over the Controller channel's ticket store.
pub trait TicketVerifier {
    fn verify(&mut self, proof: &[u8; 32]) -> Option<Grants>;
}

/// Host tunables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostConfig {
    /// Batches a viewer may leave unacknowledged before new frames wait.
    pub max_unacked_batches: u64,
    pub thumbnail_longest_side: u32,
    pub thumbnail_interval_ms: u64,
    /// Exact-pixel refinement sent per idle frame, in bytes.
    pub refine_budget_bytes: usize,
    /// What this host can do beyond Stage A. Only what the viewer also advertises is used.
    pub features: FeatureSet,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            max_unacked_batches: 4,
            thumbnail_longest_side: 320,
            thumbnail_interval_ms: 1_000,
            refine_budget_bytes: 2_000,
            features: FeatureSet::none(),
        }
    }
}

/// Something the host application must act on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostEvent {
    Opened {
        device: u64,
        resume: ResumeOutcome,
    },
    Subscribed {
        surface: u32,
        profile: Profile,
    },
    Unsubscribed {
        surface: u32,
        profile: Profile,
    },
    /// Permitted input to inject.
    Input(InputEvent),
    /// Input from a viewer that does not hold control; nothing was injected.
    InputRefused,
    /// The viewer asked for control and is allowed to have it; call [`HostSession::set_control`].
    ControlRequested,
    ControlReleased,
}

/// Encoders kept after a viewer leaves, so it can resume on its next connection.
#[derive(Debug, Default)]
pub struct ResumeStore {
    encoders: HashMap<(u64, u32), Encoder>,
}

impl ResumeStore {
    pub fn len(&self) -> usize {
        self.encoders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.encoders.is_empty()
    }
}

enum Pending {
    Nothing,
    Rects(Vec<Rect>),
    Everything,
}

impl Pending {
    fn add(&mut self, damage: Option<&[Rect]>) {
        *self = match (std::mem::replace(self, Pending::Nothing), damage) {
            (Pending::Everything, _) | (_, None) => Pending::Everything,
            (Pending::Nothing, Some(rects)) => Pending::Rects(rects.to_vec()),
            (Pending::Rects(mut existing), Some(rects)) => {
                existing.extend_from_slice(rects);
                Pending::Rects(existing)
            }
        };
    }
}

struct Subscription {
    profile: Profile,
    encoder: Encoder,
    factor: u32,
    acked: u64,
    pending: Pending,
    last_sent_ms: Option<u64>,
    viewport: Option<Viewport>,
    /// The captured frame with attached panes masked out, reused between frames.
    masked: Option<FrameBuffer>,
}

enum State {
    AwaitingHello,
    Open {
        grants: Grants,
        cache_bytes: usize,
        /// What both sides advertised, which is the only thing either may send.
        features: FeatureSet,
    },
    Closed,
}

/// One viewer's session on the host.
pub struct HostSession<V: TicketVerifier> {
    verifier: V,
    config: HostConfig,
    surfaces: Vec<SurfaceInfo>,
    generations: HashMap<u32, Generation>,
    state: State,
    subscriptions: HashMap<u32, Subscription>,
    resumable: HashMap<u32, Encoder>,
    control: ControlHolder,
    panes: HashMap<u32, Vec<PanePlacement>>,
    attached: Vec<PaneSession>,
    outbox: VecDeque<Message>,
}

impl<V: TicketVerifier> HostSession<V> {
    pub fn new(surfaces: Vec<SurfaceInfo>, verifier: V, config: HostConfig) -> Self {
        let generations = surfaces
            .iter()
            .map(|surface| (surface.id, Generation(1)))
            .collect();
        Self {
            verifier,
            config,
            surfaces,
            generations,
            state: State::AwaitingHello,
            subscriptions: HashMap::new(),
            resumable: HashMap::new(),
            control: ControlHolder::Nobody,
            panes: HashMap::new(),
            attached: Vec::new(),
            outbox: VecDeque::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Open { .. })
    }

    /// What both sides settled on, which is all the encoder above this session may use. Empty
    /// until a viewer is open, and empty for every Stage A viewer.
    pub fn agreed_features(&self) -> FeatureSet {
        match self.state {
            State::Open { features, .. } => features,
            _ => FeatureSet::none(),
        }
    }

    /// The ticket verifier, for hosts that learn what a ticket allows outside this session.
    pub const fn verifier_mut(&mut self) -> &mut V {
        &mut self.verifier
    }

    /// The next message to send, oldest first.
    pub fn poll_outgoing(&mut self) -> Option<Message> {
        self.outbox.pop_front()
    }

    /// Handles one message from the viewer.
    pub fn receive(
        &mut self,
        message: Message,
        store: &mut ResumeStore,
    ) -> Result<Vec<HostEvent>, SessionError> {
        match &self.state {
            State::Closed => Err(SessionError::Closed),
            State::AwaitingHello => match message {
                Message::Hello(hello) => self.open(hello, store),
                _ => self.fail(SessionError::ProtocolViolation, "hello_expected", store),
            },
            State::Open {
                grants,
                cache_bytes,
                features,
            } => {
                let (grants, cache_bytes, features) = (*grants, *cache_bytes, *features);
                self.handle_open(message, grants, cache_bytes, features, store)
            }
        }
    }

    fn open(
        &mut self,
        hello: Hello,
        store: &mut ResumeStore,
    ) -> Result<Vec<HostEvent>, SessionError> {
        let Some(grants) = self.verifier.verify(&hello.ticket_proof) else {
            return self.fail(SessionError::NotAuthorized, "ticket_rejected", store);
        };
        if !grants.can_view {
            return self.fail(SessionError::NotAuthorized, "view_not_allowed", store);
        }
        let mut resume = ResumeOutcome::NotRequested;
        if let Some(request) = hello.resume {
            resume = ResumeOutcome::Full;
            if let Some(mut encoder) = store.encoders.remove(&(grants.device, request.surface))
                && encoder.generation() == request.generation
                && self.generations.get(&request.surface) == Some(&request.generation)
            {
                if encoder.resume(request.sequence) == termirust_screen_codec::Resume::Partial {
                    resume = ResumeOutcome::Partial;
                }
                self.resumable.insert(request.surface, encoder);
            }
        }
        let cache_bytes = hello.cache_bytes.min(MAX_VIEWER_CACHE_BYTES) as usize;
        // Only what both sides can do: never send a viewer something it cannot decode.
        let features = self.config.features.shared(hello.features);
        self.state = State::Open {
            grants,
            cache_bytes,
            features,
        };
        self.outbox.push_back(Message::Welcome(Welcome {
            // Answer in the version the viewer spoke. A Stage A build refuses a welcome that
            // claims a version it has never heard of, so replying at 2 would lock out every
            // phone shipped before the motion path.
            version: hello.version.min(PROTOCOL_VERSION),
            surfaces: self.surfaces.clone(),
            resume,
            // The intersection, not this host's whole set: what the welcome names is exactly
            // what may be sent, so a viewer cannot be talked into expecting more.
            features,
        }));
        Ok(vec![HostEvent::Opened {
            device: grants.device,
            resume,
        }])
    }

    fn handle_open(
        &mut self,
        message: Message,
        grants: Grants,
        cache_bytes: usize,
        features: FeatureSet,
        store: &mut ResumeStore,
    ) -> Result<Vec<HostEvent>, SessionError> {
        match message {
            Message::Subscribe { surface, profile } => {
                let Some(info) = self.surfaces.iter().find(|s| s.id == surface).cloned() else {
                    return self.fail(SessionError::UnknownSurface, "unknown_surface", store);
                };
                let generation = self.generations[&surface];
                let subscription = self.subscription_for(&info, generation, profile, cache_bytes);
                let key = key_for(surface, profile);
                if let Some(previous) = self.subscriptions.insert(key, subscription) {
                    self.keep_for_resume(grants.device, key, previous, store);
                }
                if profile == Profile::Interactive
                    && let Some(panes) = self.panes.get(&surface)
                {
                    self.outbox.push_back(Message::PanePlacements {
                        surface,
                        panes: panes.clone(),
                    });
                }
                Ok(vec![HostEvent::Subscribed { surface, profile }])
            }
            Message::Unsubscribe { surface } => {
                let mut events = Vec::new();
                for profile in [Profile::Interactive, Profile::Thumbnail] {
                    let key = key_for(surface, profile);
                    if let Some(subscription) = self.subscriptions.remove(&key) {
                        self.keep_for_resume(grants.device, key, subscription, store);
                        events.push(HostEvent::Unsubscribed { surface, profile });
                    }
                }
                Ok(events)
            }
            Message::Viewport(viewport) => {
                if let Some(subscription) = self.subscriptions.get_mut(&viewport.surface) {
                    subscription.viewport = Some(viewport);
                }
                Ok(Vec::new())
            }
            Message::Acknowledge(ack) => {
                if let Some(subscription) = self.subscriptions.get_mut(&ack.surface)
                    && subscription.encoder.generation() == ack.generation
                {
                    subscription.encoder.acknowledge(ack.sequence);
                    subscription.acked = subscription
                        .acked
                        .max(ack.sequence.min(subscription.encoder.next_sequence() - 1));
                }
                Ok(Vec::new())
            }
            Message::CacheMiss {
                surface,
                tile,
                hash,
            } => {
                if let Some(subscription) = self.subscriptions.get_mut(&surface) {
                    subscription
                        .encoder
                        .cache_miss(termirust_screen_codec::CacheMiss { tile, hash });
                }
                Ok(Vec::new())
            }
            Message::AttachedPanes { sessions } => {
                if sessions != self.attached {
                    self.attached = sessions;
                    self.recheck_interactive(None);
                }
                Ok(Vec::new())
            }
            Message::RequestControl => {
                if grants.can_control {
                    Ok(vec![HostEvent::ControlRequested])
                } else {
                    self.outbox.push_back(Message::Control(self.control));
                    Ok(vec![HostEvent::InputRefused])
                }
            }
            Message::ReleaseControl => {
                if self.control == ControlHolder::You {
                    self.set_control(ControlHolder::Nobody);
                }
                Ok(vec![HostEvent::ControlReleased])
            }
            message @ (Message::PointerMove { .. }
            | Message::PointerButton { .. }
            | Message::Scroll { .. }
            | Message::Key(_)
            | Message::Text { .. }) => {
                if grants.can_control && self.control == ControlHolder::You {
                    Ok(InputEvent::from_message(message)
                        .map(HostEvent::Input)
                        .into_iter()
                        .collect())
                } else {
                    Ok(vec![HostEvent::InputRefused])
                }
            }
            // A viewer only reports what it decoded if both sides agreed to the motion path.
            // Reporting it otherwise is a viewer talking about a stream that was never sent.
            Message::VideoAcknowledge { .. } | Message::VideoLost { .. }
                if !features.has(FeatureSet::LONG_TERM_REFERENCES) =>
            {
                self.fail(SessionError::ProtocolViolation, "video_not_agreed", store)
            }
            Message::VideoAcknowledge { .. } | Message::VideoLost { .. } => {
                // M4 wires these to the encoder; until then they are accepted and ignored, so a
                // viewer that advertises the feature is not disconnected for using it.
                Ok(Vec::new())
            }
            Message::Hello(_)
            | Message::Welcome(_)
            | Message::Goodbye { .. }
            | Message::Batch(_)
            | Message::MotionRegion { .. }
            | Message::Control(_)
            | Message::PanePlacements { .. }
            | Message::VideoConfig(_)
            | Message::VideoFrame(_)
            | Message::Parity(_) => {
                self.fail(SessionError::ProtocolViolation, "unexpected_message", store)
            }
        }
    }

    /// Tells the viewer who holds control. The host application decides, using its writer lease.
    pub fn set_control(&mut self, holder: ControlHolder) {
        if self.control != holder {
            self.control = holder;
            self.outbox.push_back(Message::Control(holder));
        }
    }

    pub const fn control(&self) -> ControlHolder {
        self.control
    }

    /// Publishes where TermiRust terminal panes sit on `surface`, replacing the previous list.
    /// Panes the viewer has attached to are masked out of its pixel stream from the next frame.
    ///
    /// Publish only panes no other window covers, since a covered part would be hidden from the
    /// viewer, and only panes this device may watch. At most [`MAX_PANES`] are kept.
    pub fn set_panes(&mut self, surface: u32, mut panes: Vec<PanePlacement>) {
        panes.truncate(MAX_PANES);
        panes.retain(|pane| pane.cell_width > 0 && pane.cell_height > 0 && !pane.rect.is_empty());
        let previous = self.panes.get(&surface).map_or(&[][..], Vec::as_slice);
        if previous == panes.as_slice() {
            return;
        }
        if self.is_open() && self.subscriptions.contains_key(&surface) {
            self.outbox.push_back(Message::PanePlacements {
                surface,
                panes: panes.clone(),
            });
        }
        if panes.is_empty() {
            self.panes.remove(&surface);
        } else {
            self.panes.insert(surface, panes);
        }
        self.recheck_interactive(Some(surface));
    }

    /// The rectangles of `surface` currently kept out of the pixel stream.
    pub fn masked_rects(&self, surface: u32) -> Vec<Rect> {
        let Some(info) = self.surfaces.iter().find(|info| info.id == surface) else {
            return Vec::new();
        };
        self.panes
            .get(&surface)
            .into_iter()
            .flatten()
            .filter(|pane| self.attached.contains(&pane.session))
            .map(|pane| pane.rect.intersect(info.size.bounds()))
            .filter(|rect| !rect.is_empty())
            .collect()
    }

    /// Makes the next frame compare every tile of the interactive view of `surface`, or of every
    /// surface, because what is masked changed.
    fn recheck_interactive(&mut self, surface: Option<u32>) {
        for (key, subscription) in &mut self.subscriptions {
            if subscription.profile == Profile::Interactive && surface.is_none_or(|s| s == *key) {
                subscription.pending.add(None);
            }
        }
    }

    /// The part of `surface` the viewer last reported showing.
    pub fn viewport(&self, surface: u32) -> Option<Viewport> {
        self.subscriptions
            .get(&surface)
            .and_then(|subscription| subscription.viewport)
    }

    /// Feeds a captured frame of `surface`, taken at `now_ms`. Encodes it for every subscription
    /// to that surface that is not waiting for acknowledgements or for its preview interval.
    pub fn frame(
        &mut self,
        surface: u32,
        frame: &Frame<'_>,
        damage: Option<&[Rect]>,
        now_ms: u64,
    ) -> Result<(), SessionError> {
        if !self.is_open() {
            return Ok(());
        }
        let config = self.config;
        let masks = self.masked_rects(surface);
        for profile in [Profile::Interactive, Profile::Thumbnail] {
            let key = key_for(surface, profile);
            let Some(subscription) = self.subscriptions.get_mut(&key) else {
                continue;
            };
            subscription.pending.add(damage);
            let unacked = subscription.encoder.next_sequence() - 1 - subscription.acked;
            if unacked >= config.max_unacked_batches {
                continue;
            }
            let batch = match profile {
                Profile::Interactive => {
                    let pending = std::mem::replace(&mut subscription.pending, Pending::Nothing);
                    let rects = match &pending {
                        Pending::Rects(rects) => Some(rects.as_slice()),
                        _ => None,
                    };
                    if matches!(pending, Pending::Nothing) {
                        None
                    } else {
                        let batch = if masks.is_empty() {
                            subscription.masked = None;
                            subscription.encoder.encode_at(frame, rects, now_ms)?
                        } else {
                            let masked = subscription
                                .masked
                                .get_or_insert_with(|| FrameBuffer::new(frame.size()));
                            masked.copy_from_frame(frame)?;
                            for rect in &masks {
                                masked.fill_rect(*rect, PANE_MASK_BGRA)?;
                            }
                            subscription
                                .encoder
                                .encode_at(&masked.as_frame(), rects, now_ms)?
                        };
                        match subscription.encoder.motion_event() {
                            Some(MotionEvent::Promoted(rect)) => {
                                self.outbox.push_back(Message::MotionRegion {
                                    surface,
                                    rect: Some(rect),
                                })
                            }
                            Some(MotionEvent::Demoted(_)) => {
                                self.outbox.push_back(Message::MotionRegion {
                                    surface,
                                    rect: None,
                                })
                            }
                            None => {}
                        }
                        if batch.ops.is_empty() {
                            subscription
                                .encoder
                                .refine_at(now_ms, config.refine_budget_bytes)?
                        } else {
                            Some(batch)
                        }
                    }
                }
                Profile::Thumbnail => {
                    let due = subscription.last_sent_ms.is_none_or(|last| {
                        now_ms.saturating_sub(last) >= config.thumbnail_interval_ms
                    });
                    if !due || matches!(subscription.pending, Pending::Nothing) {
                        None
                    } else {
                        subscription.pending = Pending::Nothing;
                        subscription.last_sent_ms = Some(now_ms);
                        let small = downscale(frame, subscription.factor);
                        Some(subscription.encoder.encode(&small.as_frame(), None)?)
                            .filter(|b| !b.ops.is_empty())
                    }
                }
            };
            if let Some(batch) = batch.filter(|batch| !batch.ops.is_empty()) {
                self.outbox.push_back(Message::Batch(batch));
            }
        }
        Ok(())
    }

    /// Ends the session, keeping interactive encoders so the device can resume.
    pub fn close(&mut self, reason: &str, store: &mut ResumeStore) {
        if let State::Open { grants, .. } = self.state {
            for (key, subscription) in std::mem::take(&mut self.subscriptions) {
                self.keep_for_resume(grants.device, key, subscription, store);
            }
            self.outbox.push_back(Message::Goodbye {
                reason: reason.to_owned(),
            });
        }
        self.state = State::Closed;
    }

    fn subscription_for(
        &mut self,
        info: &SurfaceInfo,
        generation: Generation,
        profile: Profile,
        cache_bytes: usize,
    ) -> Subscription {
        let (factor, size, cache, id) = match profile {
            Profile::Interactive => (1, info.size, cache_bytes, info.id),
            Profile::Thumbnail => {
                let factor = preview_factor(info.size, self.config.thumbnail_longest_side);
                let size = Size::new(
                    info.size.width().div_ceil(factor),
                    info.size.height().div_ceil(factor),
                )
                .expect("a preview is never larger than its surface");
                (
                    factor,
                    size,
                    THUMBNAIL_CACHE_BYTES,
                    info.id | THUMBNAIL_SURFACE_BIT,
                )
            }
        };
        let encoder = match profile {
            Profile::Interactive => self.resumable.remove(&info.id),
            Profile::Thumbnail => None,
        }
        .filter(|encoder| encoder.size() == size)
        .unwrap_or_else(|| {
            Encoder::new(
                SurfaceId(id),
                generation,
                size,
                EncoderConfig {
                    cache_bytes: cache,
                    ..EncoderConfig::default()
                },
            )
        });
        let acked = encoder.next_sequence() - 1;
        Subscription {
            profile,
            encoder,
            factor,
            acked,
            pending: Pending::Everything,
            last_sent_ms: None,
            viewport: None,
            masked: None,
        }
    }

    fn keep_for_resume(
        &mut self,
        device: u64,
        key: u32,
        subscription: Subscription,
        store: &mut ResumeStore,
    ) {
        if subscription.profile == Profile::Interactive {
            store.encoders.insert((device, key), subscription.encoder);
        }
    }

    fn fail(
        &mut self,
        error: SessionError,
        reason: &str,
        store: &mut ResumeStore,
    ) -> Result<Vec<HostEvent>, SessionError> {
        match self.state {
            State::Open { .. } => self.close(reason, store),
            _ => {
                self.outbox.push_back(Message::Goodbye {
                    reason: reason.to_owned(),
                });
                self.state = State::Closed;
            }
        }
        Err(error)
    }
}

const fn key_for(surface: u32, profile: Profile) -> u32 {
    match profile {
        Profile::Interactive => surface,
        Profile::Thumbnail => surface | THUMBNAIL_SURFACE_BIT,
    }
}
