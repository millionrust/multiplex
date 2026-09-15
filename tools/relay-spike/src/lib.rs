use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::time::Instant;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const RELAY_VERSION: u16 = 1;
pub const ENVELOPE_HEADER_BYTES: usize = 52;
pub const MAX_CIPHERTEXT_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_QUEUE_BYTES: usize = 2 * MAX_CIPHERTEXT_BYTES;
pub const DEFAULT_MAX_QUEUE_FRAMES: usize = 32;
pub const DEFAULT_MAX_TOTAL_QUEUE_BYTES: usize = 64 * MAX_CIPHERTEXT_BYTES;
pub const DEFAULT_MAX_ROUTES: usize = 1_024;
pub const DEFAULT_MAX_PENDING_CHALLENGES: usize = 4_096;
pub const CHALLENGE_LIFETIME_TICKS: u64 = 30;
pub const REPORT_SCHEMA_VERSION: u32 = 1;
pub const FIXTURE_SEED: u64 = 0x2201_2026;

const ENVELOPE_MAGIC: [u8; 4] = *b"TRR1";
const PROOF_DOMAIN: &[u8] = b"termirust-relay-admission-v1\0";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RelayRouteId(pub [u8; 32]);

impl fmt::Debug for RelayRouteId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayRouteId([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum EndpointRole {
    Host = 1,
    Controller = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Direction {
    HostToController = 1,
    ControllerToHost = 2,
}

impl Direction {
    fn for_sender(role: EndpointRole) -> Self {
        match role {
            EndpointRole::Host => Self::HostToController,
            EndpointRole::Controller => Self::ControllerToHost,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RelayEnvelopeV1 {
    route_id: RelayRouteId,
    direction: Direction,
    sequence: u64,
    ciphertext: Vec<u8>,
}

impl fmt::Debug for RelayEnvelopeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayEnvelopeV1")
            .field("route_id", &"[REDACTED]")
            .field("direction", &self.direction)
            .field("sequence", &self.sequence)
            .field("ciphertext_bytes", &self.ciphertext.len())
            .finish()
    }
}

impl RelayEnvelopeV1 {
    pub fn new(
        route_id: RelayRouteId,
        direction: Direction,
        sequence: u64,
        ciphertext: Vec<u8>,
    ) -> Result<Self, RelayError> {
        if ciphertext.is_empty() || ciphertext.len() > MAX_CIPHERTEXT_BYTES {
            return Err(RelayError::FrameLimit);
        }
        Ok(Self {
            route_id,
            direction,
            sequence,
            ciphertext,
        })
    }

    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    pub fn direction(&self) -> Direction {
        self.direction
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(ENVELOPE_HEADER_BYTES + self.ciphertext.len());
        bytes.extend_from_slice(&ENVELOPE_MAGIC);
        bytes.extend_from_slice(&RELAY_VERSION.to_be_bytes());
        bytes.push(self.direction as u8);
        bytes.push(0);
        bytes.extend_from_slice(&self.route_id.0);
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        bytes.extend_from_slice(&(self.ciphertext.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&self.ciphertext);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RelayError> {
        if bytes.len() < ENVELOPE_HEADER_BYTES || bytes[..4] != ENVELOPE_MAGIC {
            return Err(RelayError::InvalidEnvelope);
        }
        let version = u16::from_be_bytes(read_array(bytes, 4)?);
        if version != RELAY_VERSION {
            return Err(RelayError::VersionMismatch);
        }
        if bytes[7] != 0 {
            return Err(RelayError::InvalidEnvelope);
        }
        let direction = match bytes[6] {
            1 => Direction::HostToController,
            2 => Direction::ControllerToHost,
            _ => return Err(RelayError::InvalidEnvelope),
        };
        let route_id = RelayRouteId(read_array(bytes, 8)?);
        let sequence = u64::from_be_bytes(read_array(bytes, 40)?);
        let ciphertext_len = u32::from_be_bytes(read_array(bytes, 48)?) as usize;
        if ciphertext_len == 0
            || ciphertext_len > MAX_CIPHERTEXT_BYTES
            || bytes.len() != ENVELOPE_HEADER_BYTES + ciphertext_len
        {
            return Err(RelayError::FrameLimit);
        }
        Self::new(
            route_id,
            direction,
            sequence,
            bytes[ENVELOPE_HEADER_BYTES..].to_vec(),
        )
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct AdmissionCredential {
    secret: [u8; 32],
}

impl fmt::Debug for AdmissionCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdmissionCredential([REDACTED])")
    }
}

impl AdmissionCredential {
    pub fn generate() -> Self {
        let mut secret = [0_u8; 32];
        OsRng.fill_bytes(&mut secret);
        Self { secret }
    }

    pub fn from_fixture_bytes(bytes: [u8; 32]) -> Self {
        Self { secret: bytes }
    }

    pub fn public_key(&self) -> [u8; 32] {
        SigningKey::from_bytes(&self.secret)
            .verifying_key()
            .to_bytes()
    }

    pub fn prove(&self, challenge: AdmissionChallenge) -> AdmissionProof {
        let signing = SigningKey::from_bytes(&self.secret);
        let signature = signing.sign(&challenge.signing_bytes()).to_bytes();
        AdmissionProof {
            challenge,
            signature,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AdmissionChallenge {
    route_id: RelayRouteId,
    role: EndpointRole,
    revocation_epoch: u64,
    serial: u64,
    expires_at_tick: u64,
    nonce: [u8; 32],
}

impl fmt::Debug for AdmissionChallenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdmissionChallenge")
            .field("role", &self.role)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("serial", &self.serial)
            .field("expires_at_tick", &self.expires_at_tick)
            .field("route_id", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .finish()
    }
}

impl AdmissionChallenge {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PROOF_DOMAIN.len() + 32 + 1 + 8 + 8 + 8 + 32);
        bytes.extend_from_slice(PROOF_DOMAIN);
        bytes.extend_from_slice(&self.route_id.0);
        bytes.push(self.role as u8);
        bytes.extend_from_slice(&self.revocation_epoch.to_be_bytes());
        bytes.extend_from_slice(&self.serial.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at_tick.to_be_bytes());
        bytes.extend_from_slice(&self.nonce);
        bytes
    }
}

pub struct AdmissionProof {
    challenge: AdmissionChallenge,
    signature: [u8; 64],
}

impl fmt::Debug for AdmissionProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdmissionProof([REDACTED])")
    }
}

#[derive(Clone)]
pub struct RouteRegistration {
    pub route_id: RelayRouteId,
    pub host_public_key: [u8; 32],
    pub controller_public_key: [u8; 32],
    pub revocation_epoch: u64,
    pub max_queue_bytes: usize,
    pub max_queue_frames: usize,
    pub revoked: bool,
}

impl fmt::Debug for RouteRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RouteRegistration")
            .field("route_id", &"[REDACTED]")
            .field("revocation_epoch", &self.revocation_epoch)
            .field("max_queue_bytes", &self.max_queue_bytes)
            .field("max_queue_frames", &self.max_queue_frames)
            .field("revoked", &self.revoked)
            .finish()
    }
}

impl RouteRegistration {
    pub fn new(
        route_id: RelayRouteId,
        host: &AdmissionCredential,
        controller: &AdmissionCredential,
    ) -> Self {
        Self {
            route_id,
            host_public_key: host.public_key(),
            controller_public_key: controller.public_key(),
            revocation_epoch: 0,
            max_queue_bytes: DEFAULT_MAX_QUEUE_BYTES,
            max_queue_frames: DEFAULT_MAX_QUEUE_FRAMES,
            revoked: false,
        }
    }

    fn validate(&self) -> Result<(), RelayError> {
        if self.max_queue_bytes < MAX_CIPHERTEXT_BYTES
            || self.max_queue_bytes > 8 * MAX_CIPHERTEXT_BYTES
            || self.max_queue_frames == 0
            || self.max_queue_frames > 128
        {
            return Err(RelayError::InvalidRegistration);
        }
        VerifyingKey::from_bytes(&self.host_public_key)
            .map_err(|_| RelayError::InvalidRegistration)?;
        VerifyingKey::from_bytes(&self.controller_public_key)
            .map_err(|_| RelayError::InvalidRegistration)?;
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct EndpointHandle {
    route_id: RelayRouteId,
    role: EndpointRole,
    endpoint_id: u64,
    epoch: u64,
}

impl fmt::Debug for EndpointHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EndpointHandle")
            .field("role", &self.role)
            .field("endpoint_id", &self.endpoint_id)
            .field("epoch", &self.epoch)
            .field("route_id", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteState {
    Registered,
    HostWaiting,
    ControllerWaiting,
    PairedForwarding,
    Revoked,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RelayStats {
    pub accepted_connections: u64,
    pub rejected_connections: u64,
    pub forwarded_frames: u64,
    pub ingress_bytes: u64,
    pub egress_bytes: u64,
    pub queue_drops: u64,
    pub active_endpoints: usize,
    pub stored_ciphertext_bytes: usize,
    pub persistent_ciphertext_bytes: usize,
    pub per_route_log_bytes: usize,
}

struct RouteRuntime {
    registration: RouteRegistration,
    host_endpoint: Option<u64>,
    controller_endpoint: Option<u64>,
    host_queue: VecDeque<RelayEnvelopeV1>,
    controller_queue: VecDeque<RelayEnvelopeV1>,
    host_queue_bytes: usize,
    controller_queue_bytes: usize,
    next_host_sequence: u64,
    next_controller_sequence: u64,
    revoked: bool,
}

impl RouteRuntime {
    fn new(registration: RouteRegistration) -> Self {
        let revoked = registration.revoked;
        Self {
            registration,
            host_endpoint: None,
            controller_endpoint: None,
            host_queue: VecDeque::new(),
            controller_queue: VecDeque::new(),
            host_queue_bytes: 0,
            controller_queue_bytes: 0,
            next_host_sequence: 0,
            next_controller_sequence: 0,
            revoked,
        }
    }

    fn state(&self) -> RouteState {
        if self.revoked {
            RouteState::Revoked
        } else {
            match (self.host_endpoint, self.controller_endpoint) {
                (None, None) => RouteState::Registered,
                (Some(_), None) => RouteState::HostWaiting,
                (None, Some(_)) => RouteState::ControllerWaiting,
                (Some(_), Some(_)) => RouteState::PairedForwarding,
            }
        }
    }

    fn clear_queues(&mut self) {
        self.host_queue.clear();
        self.controller_queue.clear();
        self.host_queue_bytes = 0;
        self.controller_queue_bytes = 0;
    }
}

pub struct RelayHarness {
    routes: BTreeMap<RelayRouteId, RouteRuntime>,
    challenges: BTreeMap<u64, AdmissionChallenge>,
    next_serial: u64,
    next_endpoint_id: u64,
    stats: RelayStats,
}

impl fmt::Debug for RelayHarness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayHarness")
            .field("routes", &self.routes.len())
            .field("pending_challenges", &self.challenges.len())
            .field("stats", &self.stats)
            .finish()
    }
}

impl Default for RelayHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayHarness {
    pub fn new() -> Self {
        Self {
            routes: BTreeMap::new(),
            challenges: BTreeMap::new(),
            next_serial: 1,
            next_endpoint_id: 1,
            stats: RelayStats::default(),
        }
    }

    pub fn restart_from(registrations: Vec<RouteRegistration>) -> Result<Self, RelayError> {
        let mut relay = Self::new();
        for registration in registrations {
            relay.register(registration)?;
        }
        Ok(relay)
    }

    pub fn register(&mut self, registration: RouteRegistration) -> Result<(), RelayError> {
        registration.validate()?;
        if self.routes.len() >= DEFAULT_MAX_ROUTES {
            return Err(RelayError::RouteLimit);
        }
        if self.routes.contains_key(&registration.route_id) {
            return Err(RelayError::DuplicateRoute);
        }
        self.routes
            .insert(registration.route_id, RouteRuntime::new(registration));
        Ok(())
    }

    pub fn registrations(&self) -> Vec<RouteRegistration> {
        self.routes
            .values()
            .map(|route| route.registration.clone())
            .collect()
    }

    pub fn issue_challenge(
        &mut self,
        route_id: RelayRouteId,
        role: EndpointRole,
        now_tick: u64,
    ) -> Result<AdmissionChallenge, RelayError> {
        if self.challenges.len() >= DEFAULT_MAX_PENDING_CHALLENGES {
            return Err(RelayError::ChallengeLimit);
        }
        let route = self.routes.get(&route_id).ok_or(RelayError::UnknownRoute)?;
        if route.revoked {
            return Err(RelayError::Revoked);
        }
        let serial = self.next_serial;
        self.next_serial = self
            .next_serial
            .checked_add(1)
            .ok_or(RelayError::Exhausted)?;
        let mut hasher = Sha256::new();
        hasher.update(b"termirust-relay-challenge-v1\0");
        hasher.update(route_id.0);
        hasher.update([role as u8]);
        hasher.update(serial.to_be_bytes());
        hasher.update(now_tick.to_be_bytes());
        let nonce: [u8; 32] = hasher.finalize().into();
        let challenge = AdmissionChallenge {
            route_id,
            role,
            revocation_epoch: route.registration.revocation_epoch,
            serial,
            expires_at_tick: now_tick.saturating_add(CHALLENGE_LIFETIME_TICKS),
            nonce,
        };
        self.challenges.insert(serial, challenge.clone());
        Ok(challenge)
    }

    pub fn connect(
        &mut self,
        proof: AdmissionProof,
        now_tick: u64,
    ) -> Result<EndpointHandle, RelayError> {
        let challenge = self
            .challenges
            .remove(&proof.challenge.serial)
            .ok_or_else(|| self.reject(RelayError::ReplayedProof))?;
        if challenge.signing_bytes() != proof.challenge.signing_bytes() {
            return Err(self.reject(RelayError::InvalidProof));
        }
        if now_tick > challenge.expires_at_tick {
            return Err(self.reject(RelayError::ExpiredProof));
        }
        let Some(route) = self.routes.get_mut(&challenge.route_id) else {
            return Err(self.reject(RelayError::UnknownRoute));
        };
        if route.revoked || route.registration.revocation_epoch != challenge.revocation_epoch {
            self.stats.rejected_connections += 1;
            return Err(RelayError::Revoked);
        }
        let public_key = match challenge.role {
            EndpointRole::Host => route.registration.host_public_key,
            EndpointRole::Controller => route.registration.controller_public_key,
        };
        let verifying =
            VerifyingKey::from_bytes(&public_key).map_err(|_| RelayError::InvalidRegistration)?;
        verifying
            .verify(
                &challenge.signing_bytes(),
                &Signature::from_bytes(&proof.signature),
            )
            .map_err(|_| RelayError::InvalidProof)
            .inspect_err(|_| self.stats.rejected_connections += 1)?;
        let slot = match challenge.role {
            EndpointRole::Host => &mut route.host_endpoint,
            EndpointRole::Controller => &mut route.controller_endpoint,
        };
        if slot.is_some() {
            self.stats.rejected_connections += 1;
            return Err(RelayError::DuplicateEndpoint);
        }
        let endpoint_id = self.next_endpoint_id;
        self.next_endpoint_id = self
            .next_endpoint_id
            .checked_add(1)
            .ok_or(RelayError::Exhausted)?;
        *slot = Some(endpoint_id);
        if route.host_endpoint.is_some() && route.controller_endpoint.is_some() {
            route.next_host_sequence = 0;
            route.next_controller_sequence = 0;
            route.clear_queues();
        }
        self.stats.accepted_connections += 1;
        self.stats.active_endpoints += 1;
        Ok(EndpointHandle {
            route_id: challenge.route_id,
            role: challenge.role,
            endpoint_id,
            epoch: challenge.revocation_epoch,
        })
    }

    pub fn route_state(&self, route_id: RelayRouteId) -> Result<RouteState, RelayError> {
        self.routes
            .get(&route_id)
            .map(RouteRuntime::state)
            .ok_or(RelayError::UnknownRoute)
    }

    pub fn send(
        &mut self,
        handle: &EndpointHandle,
        envelope: RelayEnvelopeV1,
    ) -> Result<(), RelayError> {
        let route = self
            .routes
            .get_mut(&handle.route_id)
            .ok_or(RelayError::UnknownRoute)?;
        validate_handle(route, handle)?;
        if route.state() != RouteState::PairedForwarding {
            return Err(RelayError::PeerOffline);
        }
        if envelope.route_id != handle.route_id
            || envelope.direction != Direction::for_sender(handle.role)
        {
            return Err(RelayError::RouteMismatch);
        }
        let (expected, queue, queue_bytes) = match handle.role {
            EndpointRole::Host => (
                &mut route.next_host_sequence,
                &mut route.controller_queue,
                &mut route.controller_queue_bytes,
            ),
            EndpointRole::Controller => (
                &mut route.next_controller_sequence,
                &mut route.host_queue,
                &mut route.host_queue_bytes,
            ),
        };
        if envelope.sequence != *expected {
            return Err(if envelope.sequence < *expected {
                RelayError::ReplayedFrame
            } else {
                RelayError::SequenceGap
            });
        }
        if queue.len() >= route.registration.max_queue_frames
            || queue_bytes
                .checked_add(envelope.ciphertext.len())
                .is_none_or(|bytes| bytes > route.registration.max_queue_bytes)
            || self
                .stats
                .stored_ciphertext_bytes
                .checked_add(envelope.ciphertext.len())
                .is_none_or(|bytes| bytes > DEFAULT_MAX_TOTAL_QUEUE_BYTES)
        {
            self.stats.queue_drops += 1;
            return Err(RelayError::Backpressure);
        }
        let bytes = envelope.ciphertext.len();
        *expected = expected.checked_add(1).ok_or(RelayError::Exhausted)?;
        *queue_bytes += bytes;
        queue.push_back(envelope);
        self.stats.forwarded_frames += 1;
        self.stats.ingress_bytes += bytes as u64;
        self.stats.egress_bytes += bytes as u64;
        self.stats.stored_ciphertext_bytes += bytes;
        Ok(())
    }

    pub fn receive(
        &mut self,
        handle: &EndpointHandle,
    ) -> Result<Option<RelayEnvelopeV1>, RelayError> {
        let route = self
            .routes
            .get_mut(&handle.route_id)
            .ok_or(RelayError::UnknownRoute)?;
        validate_handle(route, handle)?;
        let (queue, queue_bytes) = match handle.role {
            EndpointRole::Host => (&mut route.host_queue, &mut route.host_queue_bytes),
            EndpointRole::Controller => (
                &mut route.controller_queue,
                &mut route.controller_queue_bytes,
            ),
        };
        let envelope = queue.pop_front();
        if let Some(envelope) = envelope.as_ref() {
            *queue_bytes -= envelope.ciphertext.len();
            self.stats.stored_ciphertext_bytes -= envelope.ciphertext.len();
        }
        Ok(envelope)
    }

    pub fn disconnect(&mut self, handle: &EndpointHandle) -> Result<(), RelayError> {
        let route = self
            .routes
            .get_mut(&handle.route_id)
            .ok_or(RelayError::UnknownRoute)?;
        validate_handle(route, handle)?;
        match handle.role {
            EndpointRole::Host => route.host_endpoint = None,
            EndpointRole::Controller => route.controller_endpoint = None,
        }
        self.stats.active_endpoints = self.stats.active_endpoints.saturating_sub(1);
        self.stats.stored_ciphertext_bytes = self
            .stats
            .stored_ciphertext_bytes
            .saturating_sub(route.host_queue_bytes + route.controller_queue_bytes);
        route.clear_queues();
        Ok(())
    }

    pub fn revoke(&mut self, route_id: RelayRouteId) -> Result<(), RelayError> {
        let route = self
            .routes
            .get_mut(&route_id)
            .ok_or(RelayError::UnknownRoute)?;
        self.stats.active_endpoints = self
            .stats
            .active_endpoints
            .saturating_sub(usize::from(route.host_endpoint.is_some()))
            .saturating_sub(usize::from(route.controller_endpoint.is_some()));
        self.stats.stored_ciphertext_bytes = self
            .stats
            .stored_ciphertext_bytes
            .saturating_sub(route.host_queue_bytes + route.controller_queue_bytes);
        route.clear_queues();
        route.host_endpoint = None;
        route.controller_endpoint = None;
        route.revoked = true;
        route.registration.revoked = true;
        route.registration.revocation_epoch = route
            .registration
            .revocation_epoch
            .checked_add(1)
            .ok_or(RelayError::Exhausted)?;
        self.challenges
            .retain(|_, challenge| challenge.route_id != route_id);
        Ok(())
    }

    pub fn stats(&self) -> RelayStats {
        self.stats.clone()
    }

    fn reject(&mut self, error: RelayError) -> RelayError {
        self.stats.rejected_connections += 1;
        error
    }
}

fn validate_handle(route: &RouteRuntime, handle: &EndpointHandle) -> Result<(), RelayError> {
    if route.revoked || route.registration.revocation_epoch != handle.epoch {
        return Err(RelayError::Revoked);
    }
    let expected = match handle.role {
        EndpointRole::Host => route.host_endpoint,
        EndpointRole::Controller => route.controller_endpoint,
    };
    if expected != Some(handle.endpoint_id) {
        return Err(RelayError::StaleEndpoint);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayError {
    InvalidEnvelope,
    VersionMismatch,
    FrameLimit,
    InvalidRegistration,
    RouteLimit,
    DuplicateRoute,
    UnknownRoute,
    ChallengeLimit,
    InvalidProof,
    ReplayedProof,
    ExpiredProof,
    Revoked,
    DuplicateEndpoint,
    RouteMismatch,
    ReplayedFrame,
    SequenceGap,
    PeerOffline,
    Backpressure,
    StaleEndpoint,
    Exhausted,
}

impl fmt::Display for RelayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEnvelope => "invalid_envelope",
            Self::VersionMismatch => "version_mismatch",
            Self::FrameLimit => "frame_limit",
            Self::InvalidRegistration => "invalid_registration",
            Self::RouteLimit => "route_limit",
            Self::DuplicateRoute => "duplicate_route",
            Self::UnknownRoute => "unknown_route",
            Self::ChallengeLimit => "challenge_limit",
            Self::InvalidProof => "invalid_proof",
            Self::ReplayedProof => "replayed_proof",
            Self::ExpiredProof => "expired_proof",
            Self::Revoked => "revoked",
            Self::DuplicateEndpoint => "duplicate_endpoint",
            Self::RouteMismatch => "route_mismatch",
            Self::ReplayedFrame => "replayed_frame",
            Self::SequenceGap => "sequence_gap",
            Self::PeerOffline => "peer_offline",
            Self::Backpressure => "backpressure",
            Self::StaleEndpoint => "stale_endpoint",
            Self::Exhausted => "exhausted",
        })
    }
}

impl std::error::Error for RelayError {}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], RelayError> {
    bytes
        .get(offset..offset + N)
        .ok_or(RelayError::InvalidEnvelope)?
        .try_into()
        .map_err(|_| RelayError::InvalidEnvelope)
}

pub fn fixture_route(index: usize) -> RelayRouteId {
    let mut hasher = Sha256::new();
    hasher.update(b"termirust-relay-route-fixture-v1\0");
    hasher.update(FIXTURE_SEED.to_be_bytes());
    hasher.update((index as u64).to_be_bytes());
    RelayRouteId(hasher.finalize().into())
}

pub fn fixture_credential(index: usize, role: EndpointRole) -> AdmissionCredential {
    let mut hasher = Sha256::new();
    hasher.update(b"termirust-relay-credential-fixture-v1\0");
    hasher.update(FIXTURE_SEED.to_be_bytes());
    hasher.update((index as u64).to_be_bytes());
    hasher.update([role as u8]);
    AdmissionCredential::from_fixture_bytes(hasher.finalize().into())
}

pub fn connect_fixture_pair(
    relay: &mut RelayHarness,
    index: usize,
    now_tick: u64,
) -> Result<(EndpointHandle, EndpointHandle), RelayError> {
    let route_id = fixture_route(index);
    let host = fixture_credential(index, EndpointRole::Host);
    let controller = fixture_credential(index, EndpointRole::Controller);
    relay.register(RouteRegistration::new(route_id, &host, &controller))?;
    let host_challenge = relay.issue_challenge(route_id, EndpointRole::Host, now_tick)?;
    let controller_challenge =
        relay.issue_challenge(route_id, EndpointRole::Controller, now_tick)?;
    let host_handle = relay.connect(host.prove(host_challenge), now_tick)?;
    let controller_handle = relay.connect(controller.prove(controller_challenge), now_tick)?;
    Ok((host_handle, controller_handle))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DutyCycle {
    Idle,
    Interactive,
    Burst,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawRun {
    pub connect_micros: u64,
    pub forward_micros: u64,
    pub forwarded_frames: u64,
    pub ciphertext_bytes: u64,
    pub throughput_bytes_per_second: u64,
    pub cpu_percent: f64,
    pub max_rss_bytes: u64,
    pub queue_drops: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioReport {
    pub pairs: usize,
    pub duty_cycle: DutyCycle,
    pub runs: Vec<RawRun>,
    pub connect_p50_micros: u64,
    pub connect_p95_micros: u64,
    pub connect_p99_micros: u64,
    pub forward_p50_micros: u64,
    pub forward_p95_micros: u64,
    pub forward_p99_micros: u64,
    pub throughput_p50_bytes_per_second: u64,
    pub max_rss_bytes: u64,
    pub max_queue_drops: u64,
    pub logical_sockets: usize,
    pub persistent_storage_bytes: usize,
    pub per_route_log_bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineReport {
    pub os: String,
    pub arch: String,
    pub hardware: String,
    pub toolchain: String,
    pub build_profile: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpikeReport {
    pub schema_version: u32,
    pub generated_at: String,
    pub local_only: bool,
    pub fixture_seed: u64,
    pub runs_per_scenario: usize,
    pub machine: MachineReport,
    pub scenarios: Vec<ScenarioReport>,
}

pub fn benchmark(
    pair_counts: &[usize],
    runs: usize,
    generated_at: String,
    machine: MachineReport,
) -> Result<SpikeReport, RelayError> {
    if !(10..=100).contains(&runs) || pair_counts.is_empty() {
        return Err(RelayError::InvalidRegistration);
    }
    let mut scenarios = Vec::new();
    for &pairs in pair_counts {
        if pairs == 0 || pairs > DEFAULT_MAX_ROUTES {
            return Err(RelayError::RouteLimit);
        }
        for duty_cycle in [DutyCycle::Idle, DutyCycle::Interactive, DutyCycle::Burst] {
            let mut raw = Vec::with_capacity(runs);
            for _ in 0..runs {
                raw.push(benchmark_run(pairs, duty_cycle)?);
            }
            scenarios.push(summarize_scenario(pairs, duty_cycle, raw));
        }
    }
    Ok(SpikeReport {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at,
        local_only: true,
        fixture_seed: FIXTURE_SEED,
        runs_per_scenario: runs,
        machine,
        scenarios,
    })
}

fn benchmark_run(pairs: usize, duty_cycle: DutyCycle) -> Result<RawRun, RelayError> {
    let cpu_start = cpu_micros();
    let wall_start = Instant::now();
    let connect_start = Instant::now();
    let mut relay = RelayHarness::new();
    let mut handles = Vec::with_capacity(pairs);
    for index in 0..pairs {
        handles.push(connect_fixture_pair(&mut relay, index, 100)?);
    }
    let connect_micros = elapsed_micros(connect_start);
    let forward_start = Instant::now();
    if !matches!(duty_cycle, DutyCycle::Idle) {
        for (index, (host, controller)) in handles.iter().enumerate() {
            relay.send(
                host,
                RelayEnvelopeV1::new(
                    host.route_id,
                    Direction::HostToController,
                    0,
                    vec![0xA5; 1024],
                )?,
            )?;
            let _ = relay.receive(controller)?;
            if matches!(duty_cycle, DutyCycle::Burst) || index % 20 == 0 {
                relay.send(
                    controller,
                    RelayEnvelopeV1::new(
                        controller.route_id,
                        Direction::ControllerToHost,
                        0,
                        vec![0x5A; 64 * 1024],
                    )?,
                )?;
                let _ = relay.receive(host)?;
            }
            if matches!(duty_cycle, DutyCycle::Burst) && index % 100 == 0 {
                relay.send(
                    host,
                    RelayEnvelopeV1::new(
                        host.route_id,
                        Direction::HostToController,
                        1,
                        vec![0x3C; MAX_CIPHERTEXT_BYTES],
                    )?,
                )?;
                let _ = relay.receive(controller)?;
            }
        }
    }
    let forward_micros = elapsed_micros(forward_start);
    let elapsed_micros = elapsed_micros(wall_start).max(1);
    let cpu_elapsed = cpu_micros().saturating_sub(cpu_start);
    let stats = relay.stats();
    let throughput_bytes_per_second = stats
        .egress_bytes
        .saturating_mul(1_000_000)
        .checked_div(forward_micros.max(1))
        .unwrap_or(0);
    Ok(RawRun {
        connect_micros,
        forward_micros,
        forwarded_frames: stats.forwarded_frames,
        ciphertext_bytes: stats.egress_bytes,
        throughput_bytes_per_second,
        cpu_percent: (cpu_elapsed as f64 / elapsed_micros as f64) * 100.0,
        max_rss_bytes: max_rss_bytes(),
        queue_drops: stats.queue_drops,
    })
}

fn summarize_scenario(pairs: usize, duty_cycle: DutyCycle, runs: Vec<RawRun>) -> ScenarioReport {
    let connect: Vec<u64> = runs.iter().map(|run| run.connect_micros).collect();
    let forward: Vec<u64> = runs.iter().map(|run| run.forward_micros).collect();
    let throughput: Vec<u64> = runs
        .iter()
        .map(|run| run.throughput_bytes_per_second)
        .collect();
    ScenarioReport {
        pairs,
        duty_cycle,
        connect_p50_micros: percentile(&connect, 50),
        connect_p95_micros: percentile(&connect, 95),
        connect_p99_micros: percentile(&connect, 99),
        forward_p50_micros: percentile(&forward, 50),
        forward_p95_micros: percentile(&forward, 95),
        forward_p99_micros: percentile(&forward, 99),
        throughput_p50_bytes_per_second: percentile(&throughput, 50),
        max_rss_bytes: runs.iter().map(|run| run.max_rss_bytes).max().unwrap_or(0),
        max_queue_drops: runs.iter().map(|run| run.queue_drops).max().unwrap_or(0),
        logical_sockets: pairs * 2,
        persistent_storage_bytes: 0,
        per_route_log_bytes: 0,
        runs,
    }
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    let mut values = values.to_vec();
    values.sort_unstable();
    let index = ((values.len().saturating_sub(1)) * percentile).div_ceil(100);
    values.get(index).copied().unwrap_or(0)
}

fn elapsed_micros(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[cfg(unix)]
fn cpu_micros() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the provided rusage on success.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    // SAFETY: the successful call above initialized usage.
    let usage = unsafe { usage.assume_init() };
    timeval_micros(usage.ru_utime).saturating_add(timeval_micros(usage.ru_stime))
}

#[cfg(unix)]
fn timeval_micros(value: libc::timeval) -> u64 {
    (value.tv_sec as u64)
        .saturating_mul(1_000_000)
        .saturating_add(value.tv_usec as u64)
}

#[cfg(not(unix))]
fn cpu_micros() -> u64 {
    0
}

#[cfg(unix)]
fn max_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the provided rusage on success.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    // SAFETY: the successful call above initialized usage.
    let usage = unsafe { usage.assume_init() };
    #[cfg(target_os = "macos")]
    {
        usage.ru_maxrss as u64
    }
    #[cfg(not(target_os = "macos"))]
    {
        (usage.ru_maxrss as u64).saturating_mul(1024)
    }
}

#[cfg(not(unix))]
fn max_rss_bytes() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip_is_exact_and_bounded() {
        let envelope = RelayEnvelopeV1::new(
            fixture_route(1),
            Direction::HostToController,
            9,
            vec![7; 128],
        )
        .unwrap();
        assert_eq!(
            RelayEnvelopeV1::decode(&envelope.encode()).unwrap(),
            envelope
        );
        assert_eq!(
            RelayEnvelopeV1::new(
                fixture_route(1),
                Direction::HostToController,
                0,
                vec![0; MAX_CIPHERTEXT_BYTES + 1],
            ),
            Err(RelayError::FrameLimit)
        );
    }

    #[test]
    fn debug_views_redact_route_credentials_and_ciphertext() {
        let credential = fixture_credential(0, EndpointRole::Host);
        let route = fixture_route(0);
        let envelope = RelayEnvelopeV1::new(
            route,
            Direction::HostToController,
            0,
            b"never-log-this".to_vec(),
        )
        .unwrap();
        let debug = format!("{route:?} {credential:?} {envelope:?}");
        assert!(!debug.contains("never-log-this"));
        assert!(!debug.contains(&hex_for_test(&route.0)));
        assert!(debug.contains("[REDACTED]"));
    }

    fn hex_for_test(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
