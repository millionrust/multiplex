use crate::{RelayServerError, RelayServerLimits};
use rand_core::{OsRng, RngCore};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::net::IpAddr;
use termirust_relay_protocol::{
    ADMISSION_LIFETIME_SECONDS, FAILED_ADMISSION_WINDOW_SECONDS, MAX_FAILED_ADMISSIONS_PER_SOURCE,
    MAX_UNAUTHENTICATED_HANDSHAKES, RelayAdmissionChallenge, RelayAdmissionProof,
    RelayConnectionId, RelayConnectionSequence, RelayDiagnosticCode, RelayDirection,
    RelayEndpointRole, RelayEnvelopeV1, RelayRouteId, RelayRouteRegistration, RelayRouteState,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayCoreSnapshot {
    pub registered_routes: usize,
    pub forwarding_pairs: usize,
    pub active_endpoints: usize,
    pub queued_messages: usize,
    pub queued_encoded_bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayDiagnosticSnapshot {
    pub accepted_connections: u64,
    pub rejected_connections: u64,
    pub forwarded_messages: u64,
    pub dropped_messages: u64,
    pub ingress_encoded_bytes: u64,
    pub egress_encoded_bytes: u64,
    pub persistent_ciphertext_bytes: u64,
    pub per_route_log_bytes: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ConnectedEndpoint {
    pub route_id: RelayRouteId,
    pub role: RelayEndpointRole,
    pub connection_id: RelayConnectionId,
    pub epoch: u64,
}

impl fmt::Debug for ConnectedEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectedEndpoint")
            .field("route_id", &"[REDACTED]")
            .field("role", &self.role)
            .field("connection_id", &self.connection_id)
            .field("epoch", &self.epoch)
            .finish()
    }
}

struct EndpointRuntime {
    connection_id: RelayConnectionId,
    epoch: u64,
    tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
    queued_messages: usize,
    queued_encoded_bytes: usize,
}

struct RouteRuntime {
    registration: RelayRouteRegistration,
    host: Option<EndpointRuntime>,
    controller: Option<EndpointRuntime>,
    host_next_sequence: RelayConnectionSequence,
    controller_next_sequence: RelayConnectionSequence,
    rate: TokenBucket,
    closed: bool,
    close_code: Option<RelayDiagnosticCode>,
}

impl RouteRuntime {
    fn new(registration: RelayRouteRegistration, now_millis: u64) -> Self {
        let rate = TokenBucket::new(
            registration.quota.rate_bytes_per_second,
            registration.quota.rate_burst_bytes,
            now_millis,
        );
        Self {
            closed: false,
            registration,
            host: None,
            controller: None,
            host_next_sequence: RelayConnectionSequence(0),
            controller_next_sequence: RelayConnectionSequence(0),
            rate,
            close_code: None,
        }
    }

    fn endpoint(&self, role: RelayEndpointRole) -> Option<&EndpointRuntime> {
        match role {
            RelayEndpointRole::Host => self.host.as_ref(),
            RelayEndpointRole::Controller => self.controller.as_ref(),
        }
    }

    fn endpoint_mut(&mut self, role: RelayEndpointRole) -> Option<&mut EndpointRuntime> {
        match role {
            RelayEndpointRole::Host => self.host.as_mut(),
            RelayEndpointRole::Controller => self.controller.as_mut(),
        }
    }

    fn endpoint_slot_mut(&mut self, role: RelayEndpointRole) -> &mut Option<EndpointRuntime> {
        match role {
            RelayEndpointRole::Host => &mut self.host,
            RelayEndpointRole::Controller => &mut self.controller,
        }
    }

    fn peer_role(role: RelayEndpointRole) -> RelayEndpointRole {
        match role {
            RelayEndpointRole::Host => RelayEndpointRole::Controller,
            RelayEndpointRole::Controller => RelayEndpointRole::Host,
        }
    }

    fn route_state(&self) -> RelayRouteState {
        if self.registration.revoked {
            RelayRouteState::Revoked
        } else if self.closed && self.host.is_none() && self.controller.is_none() {
            RelayRouteState::Closed
        } else {
            match (self.host.is_some(), self.controller.is_some()) {
                (false, false) => RelayRouteState::Registered,
                (true, false) => RelayRouteState::HostWaiting,
                (false, true) => RelayRouteState::ControllerWaiting,
                (true, true) => RelayRouteState::Forwarding,
            }
        }
    }

    fn cancel_and_clear(&mut self) -> (usize, usize) {
        let mut messages = 0;
        let mut bytes = 0;
        for endpoint in [&self.host, &self.controller].into_iter().flatten() {
            messages += endpoint.queued_messages;
            bytes += endpoint.queued_encoded_bytes;
            endpoint.cancel.cancel();
        }
        self.host = None;
        self.controller = None;
        self.closed = true;
        (messages, bytes)
    }
}

struct PendingChallenge {
    challenge: RelayAdmissionChallenge,
    source: IpAddr,
}

#[derive(Clone, Copy)]
struct FailedAdmissionBucket {
    window_started_seconds: u64,
    failures: u32,
}

struct TokenBucket {
    tokens: u64,
    rate_per_second: u64,
    burst: u64,
    last_millis: u64,
}

impl TokenBucket {
    fn new(rate_per_second: u64, burst: u64, now_millis: u64) -> Self {
        Self {
            tokens: burst,
            rate_per_second,
            burst,
            last_millis: now_millis,
        }
    }

    fn charge(&mut self, bytes: usize, now_millis: u64) -> bool {
        let elapsed = now_millis.saturating_sub(self.last_millis);
        let refill = elapsed.saturating_mul(self.rate_per_second) / 1_000;
        self.tokens = self.tokens.saturating_add(refill).min(self.burst);
        self.last_millis = now_millis;
        let Ok(bytes) = u64::try_from(bytes) else {
            return false;
        };
        if bytes > self.tokens {
            return false;
        }
        self.tokens -= bytes;
        true
    }
}

pub(crate) struct RelayCore {
    routes: BTreeMap<RelayRouteId, RouteRuntime>,
    challenges: BTreeMap<u64, PendingChallenge>,
    failed_admissions: HashMap<IpAddr, FailedAdmissionBucket>,
    next_serial: u64,
    next_connection_id: u64,
    limits: RelayServerLimits,
    diagnostics: RelayDiagnosticSnapshot,
}

impl fmt::Debug for RelayCore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayCore")
            .field("routes", &self.routes.len())
            .field("pending_challenges", &self.challenges.len())
            .field("diagnostics", &self.diagnostics)
            .finish()
    }
}

impl RelayCore {
    pub fn new(
        registrations: Vec<RelayRouteRegistration>,
        limits: RelayServerLimits,
        now_millis: u64,
    ) -> Result<Self, RelayServerError> {
        limits.validate()?;
        if registrations.len() > limits.registered_routes {
            return Err(RelayServerError::new(RelayDiagnosticCode::InvalidConfig));
        }
        let mut routes = BTreeMap::new();
        for registration in registrations {
            registration.validate()?;
            if routes
                .insert(
                    registration.route_id,
                    RouteRuntime::new(registration, now_millis),
                )
                .is_some()
            {
                return Err(RelayServerError::new(RelayDiagnosticCode::StateCorrupt));
            }
        }
        Ok(Self {
            routes,
            challenges: BTreeMap::new(),
            failed_admissions: HashMap::new(),
            next_serial: 1,
            next_connection_id: 1,
            limits,
            diagnostics: RelayDiagnosticSnapshot::default(),
        })
    }

    pub fn registrations(&self) -> Vec<RelayRouteRegistration> {
        self.routes
            .values()
            .map(|route| route.registration.clone())
            .collect()
    }

    pub fn registrations_with_added_route(
        &self,
        registration: RelayRouteRegistration,
    ) -> Result<Vec<RelayRouteRegistration>, RelayServerError> {
        registration.validate()?;
        if self.routes.len() >= self.limits.registered_routes
            || self.routes.contains_key(&registration.route_id)
        {
            return Err(RelayServerError::new(RelayDiagnosticCode::InvalidConfig));
        }
        let mut registrations = self.registrations();
        registrations.push(registration);
        Ok(registrations)
    }

    pub fn insert_registration(
        &mut self,
        registration: RelayRouteRegistration,
        now_millis: u64,
    ) -> Result<(), RelayServerError> {
        registration.validate()?;
        if self.routes.len() >= self.limits.registered_routes
            || self.routes.contains_key(&registration.route_id)
        {
            return Err(RelayServerError::new(RelayDiagnosticCode::InvalidConfig));
        }
        self.routes.insert(
            registration.route_id,
            RouteRuntime::new(registration, now_millis),
        );
        Ok(())
    }

    pub fn issue_challenge(
        &mut self,
        route_id: RelayRouteId,
        role: RelayEndpointRole,
        source: IpAddr,
        now_seconds: u64,
    ) -> Result<RelayAdmissionChallenge, RelayServerError> {
        self.prune_expired(now_seconds);
        if self.source_is_limited(source, now_seconds) {
            return Err(self.reject(RelayDiagnosticCode::AdmissionRateLimited));
        }
        if self.challenges.len() >= MAX_UNAUTHENTICATED_HANDSHAKES {
            return Err(self.reject(RelayDiagnosticCode::HandshakeLimit));
        }
        let Some(route) = self.routes.get(&route_id) else {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::UnknownRoute));
        };
        if route.registration.revoked {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::Revoked));
        }
        if route.endpoint(role).is_some() {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::DuplicateRole));
        }

        let serial = self.next_serial;
        self.next_serial = self
            .next_serial
            .checked_add(1)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::Internal))?;
        let mut nonce = [0_u8; 32];
        OsRng.fill_bytes(&mut nonce);
        let challenge = RelayAdmissionChallenge {
            route_id,
            role,
            verifier: route.registration.verifier_for(role),
            revocation_epoch: route.registration.revocation_epoch,
            serial,
            expires_at_unix_seconds: now_seconds.saturating_add(ADMISSION_LIFETIME_SECONDS),
            nonce,
        };
        self.challenges.insert(
            serial,
            PendingChallenge {
                challenge: challenge.clone(),
                source,
            },
        );
        Ok(challenge)
    }

    pub fn admit(
        &mut self,
        proof: RelayAdmissionProof,
        source: IpAddr,
        now_seconds: u64,
        tx: mpsc::Sender<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<ConnectedEndpoint, RelayServerError> {
        let Some(pending) = self.challenges.remove(&proof.serial) else {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::ReplayedProof));
        };
        if pending.source != source {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::InvalidProof));
        }
        if now_seconds > pending.challenge.expires_at_unix_seconds {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::ExpiredProof));
        }
        if proof.verify(&pending.challenge).is_err() {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::InvalidProof));
        }
        let forwarding_pairs = self.forwarding_pairs();
        let route = self
            .routes
            .get_mut(&pending.challenge.route_id)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::UnknownRoute))?;
        if route.registration.revoked
            || route.registration.revocation_epoch != pending.challenge.revocation_epoch
        {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::Revoked));
        }
        if route.endpoint(pending.challenge.role).is_some() {
            self.record_failed(source, now_seconds);
            return Err(self.reject(RelayDiagnosticCode::DuplicateRole));
        }
        let creates_pair = route
            .endpoint(RouteRuntime::peer_role(pending.challenge.role))
            .is_some();
        if creates_pair && forwarding_pairs >= self.limits.forwarding_pairs {
            return Err(self.reject(RelayDiagnosticCode::PairLimit));
        }

        let connection_id = RelayConnectionId(self.next_connection_id);
        self.next_connection_id = self
            .next_connection_id
            .checked_add(1)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::Internal))?;
        let epoch = route.registration.revocation_epoch.0;
        *route.endpoint_slot_mut(pending.challenge.role) = Some(EndpointRuntime {
            connection_id,
            epoch,
            tx,
            cancel,
            queued_messages: 0,
            queued_encoded_bytes: 0,
        });
        route.closed = false;
        route.close_code = None;
        if creates_pair {
            route.host_next_sequence = RelayConnectionSequence(0);
            route.controller_next_sequence = RelayConnectionSequence(0);
        }
        self.diagnostics.accepted_connections += 1;
        Ok(ConnectedEndpoint {
            route_id: pending.challenge.route_id,
            role: pending.challenge.role,
            connection_id,
            epoch,
        })
    }

    pub fn forward(
        &mut self,
        sender: &ConnectedEndpoint,
        envelope: RelayEnvelopeV1,
        encoded: Vec<u8>,
        now_millis: u64,
    ) -> Result<(), RelayServerError> {
        let Some(route) = self.routes.get_mut(&sender.route_id) else {
            return Err(RelayServerError::new(RelayDiagnosticCode::UnknownRoute));
        };
        validate_endpoint(route, sender)?;
        if route.route_state() != RelayRouteState::Forwarding {
            return Err(RelayServerError::new(RelayDiagnosticCode::PeerOffline));
        }
        if envelope.route_id() != sender.route_id {
            return self.close_with(sender.route_id, RelayDiagnosticCode::RouteMismatch);
        }
        if envelope.direction() != RelayDirection::for_sender(sender.role) {
            return self.close_with(sender.route_id, RelayDiagnosticCode::DirectionMismatch);
        }
        let expected = match sender.role {
            RelayEndpointRole::Host => route.host_next_sequence,
            RelayEndpointRole::Controller => route.controller_next_sequence,
        };
        if envelope.sequence().0 < expected.0 {
            return self.close_with(sender.route_id, RelayDiagnosticCode::SequenceReplay);
        }
        if envelope.sequence() != expected {
            return self.close_with(sender.route_id, RelayDiagnosticCode::SequenceGap);
        }
        let peer_role = RouteRuntime::peer_role(sender.role);
        let quota = route.registration.quota;
        let Some(peer) = route.endpoint(peer_role) else {
            return Err(RelayServerError::new(RelayDiagnosticCode::PeerOffline));
        };
        if peer.queued_messages >= quota.queue_messages
            || queue_would_exceed(
                peer.queued_encoded_bytes,
                encoded.len(),
                quota.queue_encoded_bytes,
            )
        {
            self.diagnostics.dropped_messages += 1;
            return self.close_with(sender.route_id, RelayDiagnosticCode::QueueLimit);
        }
        let peer_tx = peer.tx.clone();
        if !route.rate.charge(encoded.len(), now_millis) {
            self.diagnostics.dropped_messages += 1;
            return self.close_with(sender.route_id, RelayDiagnosticCode::RateLimit);
        }
        if peer_tx.try_send(encoded).is_err() {
            self.diagnostics.dropped_messages += 1;
            return self.close_with(sender.route_id, RelayDiagnosticCode::QueueLimit);
        }
        let encoded_len = envelope.encoded_len();
        let peer = route
            .endpoint_mut(peer_role)
            .expect("peer was checked above");
        peer.queued_messages += 1;
        peer.queued_encoded_bytes += encoded_len;
        match sender.role {
            RelayEndpointRole::Host => route.host_next_sequence.0 += 1,
            RelayEndpointRole::Controller => route.controller_next_sequence.0 += 1,
        }
        self.diagnostics.forwarded_messages += 1;
        self.diagnostics.ingress_encoded_bytes += encoded_len as u64;
        self.diagnostics.egress_encoded_bytes += encoded_len as u64;
        Ok(())
    }

    pub fn delivered(&mut self, endpoint: &ConnectedEndpoint, encoded_bytes: usize) {
        let Some(route) = self.routes.get_mut(&endpoint.route_id) else {
            return;
        };
        let Some(runtime) = route.endpoint_mut(endpoint.role) else {
            return;
        };
        if runtime.connection_id == endpoint.connection_id && runtime.epoch == endpoint.epoch {
            runtime.queued_messages = runtime.queued_messages.saturating_sub(1);
            runtime.queued_encoded_bytes =
                runtime.queued_encoded_bytes.saturating_sub(encoded_bytes);
        }
    }

    pub fn disconnect(&mut self, endpoint: &ConnectedEndpoint) {
        let Some(route) = self.routes.get_mut(&endpoint.route_id) else {
            return;
        };
        if route
            .endpoint(endpoint.role)
            .is_some_and(|runtime| runtime.connection_id == endpoint.connection_id)
        {
            route.close_code = Some(RelayDiagnosticCode::PeerDisconnected);
            route.cancel_and_clear();
        }
    }

    pub fn revoked_registrations(
        &self,
        route_id: RelayRouteId,
    ) -> Result<Vec<RelayRouteRegistration>, RelayServerError> {
        let mut registrations = self.registrations();
        let registration = registrations
            .iter_mut()
            .find(|registration| registration.route_id == route_id)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::UnknownRoute))?;
        registration.revocation_epoch.0 = registration
            .revocation_epoch
            .0
            .checked_add(1)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::Internal))?;
        registration.revoked = true;
        Ok(registrations)
    }

    pub fn apply_revocation(
        &mut self,
        route_id: RelayRouteId,
        epoch: u64,
    ) -> Result<(), RelayServerError> {
        let route = self
            .routes
            .get_mut(&route_id)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::UnknownRoute))?;
        if epoch <= route.registration.revocation_epoch.0 {
            return Err(RelayServerError::new(RelayDiagnosticCode::Internal));
        }
        route.registration.revocation_epoch.0 = epoch;
        route.registration.revoked = true;
        route.close_code = Some(RelayDiagnosticCode::RevokedLive);
        route.cancel_and_clear();
        self.challenges
            .retain(|_, pending| pending.challenge.route_id != route_id);
        Ok(())
    }

    pub fn route_state(&self, route_id: RelayRouteId) -> Result<RelayRouteState, RelayServerError> {
        self.routes
            .get(&route_id)
            .map(RouteRuntime::route_state)
            .ok_or_else(|| RelayServerError::new(RelayDiagnosticCode::UnknownRoute))
    }

    pub fn close_code(&self, route_id: RelayRouteId) -> Option<RelayDiagnosticCode> {
        self.routes
            .get(&route_id)
            .and_then(|route| route.close_code)
    }

    pub fn snapshot(&self) -> RelayCoreSnapshot {
        let mut snapshot = RelayCoreSnapshot {
            registered_routes: self.routes.len(),
            forwarding_pairs: self.forwarding_pairs(),
            ..RelayCoreSnapshot::default()
        };
        for route in self.routes.values() {
            for endpoint in [&route.host, &route.controller].into_iter().flatten() {
                snapshot.active_endpoints += 1;
                snapshot.queued_messages += endpoint.queued_messages;
                snapshot.queued_encoded_bytes += endpoint.queued_encoded_bytes;
            }
        }
        snapshot
    }

    pub fn diagnostics(&self) -> RelayDiagnosticSnapshot {
        self.diagnostics.clone()
    }

    fn forwarding_pairs(&self) -> usize {
        self.routes
            .values()
            .filter(|route| route.route_state() == RelayRouteState::Forwarding)
            .count()
    }

    fn close_with<T>(
        &mut self,
        route_id: RelayRouteId,
        code: RelayDiagnosticCode,
    ) -> Result<T, RelayServerError> {
        if let Some(route) = self.routes.get_mut(&route_id) {
            route.close_code = Some(code);
            route.cancel_and_clear();
        }
        Err(RelayServerError::new(code))
    }

    fn reject(&mut self, code: RelayDiagnosticCode) -> RelayServerError {
        self.diagnostics.rejected_connections += 1;
        RelayServerError::new(code)
    }

    fn prune_expired(&mut self, now_seconds: u64) {
        self.challenges
            .retain(|_, pending| pending.challenge.expires_at_unix_seconds >= now_seconds);
        self.failed_admissions.retain(|_, bucket| {
            now_seconds.saturating_sub(bucket.window_started_seconds)
                <= FAILED_ADMISSION_WINDOW_SECONDS
        });
    }

    fn source_is_limited(&self, source: IpAddr, now_seconds: u64) -> bool {
        self.failed_admissions.get(&source).is_some_and(|bucket| {
            now_seconds.saturating_sub(bucket.window_started_seconds)
                <= FAILED_ADMISSION_WINDOW_SECONDS
                && bucket.failures >= MAX_FAILED_ADMISSIONS_PER_SOURCE
        })
    }

    fn record_failed(&mut self, source: IpAddr, now_seconds: u64) {
        let bucket = self
            .failed_admissions
            .entry(source)
            .or_insert(FailedAdmissionBucket {
                window_started_seconds: now_seconds,
                failures: 0,
            });
        if now_seconds.saturating_sub(bucket.window_started_seconds)
            > FAILED_ADMISSION_WINDOW_SECONDS
        {
            *bucket = FailedAdmissionBucket {
                window_started_seconds: now_seconds,
                failures: 0,
            };
        }
        bucket.failures = bucket.failures.saturating_add(1);
    }
}

fn queue_would_exceed(current: usize, additional: usize, maximum: usize) -> bool {
    current
        .checked_add(additional)
        .is_none_or(|total| total > maximum)
}

fn validate_endpoint(
    route: &RouteRuntime,
    endpoint: &ConnectedEndpoint,
) -> Result<(), RelayServerError> {
    if route.registration.revoked || route.registration.revocation_epoch.0 != endpoint.epoch {
        return Err(RelayServerError::new(RelayDiagnosticCode::RevokedLive));
    }
    if !route.endpoint(endpoint.role).is_some_and(|runtime| {
        runtime.connection_id == endpoint.connection_id && runtime.epoch == endpoint.epoch
    }) {
        return Err(RelayServerError::new(RelayDiagnosticCode::PeerDisconnected));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_relay_protocol::{
        RelayAdmissionCredential, RelayConnectionSequence, RelayDirection, RelayEnvelopeV1,
        RelayRouteRegistration,
    };

    fn fixture_registration(
        byte: u8,
    ) -> (
        RelayRouteRegistration,
        RelayAdmissionCredential,
        RelayAdmissionCredential,
    ) {
        let host = RelayAdmissionCredential::from_fixture_bytes([byte; 32]);
        let controller = RelayAdmissionCredential::from_fixture_bytes([byte.wrapping_add(1); 32]);
        (
            RelayRouteRegistration::new(RelayRouteId([byte; 32]), &host, &controller),
            host,
            controller,
        )
    }

    fn admit_pair(
        core: &mut RelayCore,
        route: RelayRouteId,
        host: &RelayAdmissionCredential,
        controller: &RelayAdmissionCredential,
    ) -> (
        ConnectedEndpoint,
        ConnectedEndpoint,
        mpsc::Receiver<Vec<u8>>,
    ) {
        let source = IpAddr::from([127, 0, 0, 1]);
        let (host_tx, _host_rx) = mpsc::channel(64);
        let (controller_tx, controller_rx) = mpsc::channel(64);
        let host_challenge = core
            .issue_challenge(route, RelayEndpointRole::Host, source, 1)
            .unwrap();
        let host_endpoint = core
            .admit(
                host.prove(&host_challenge),
                source,
                1,
                host_tx,
                CancellationToken::new(),
            )
            .unwrap();
        let controller_challenge = core
            .issue_challenge(route, RelayEndpointRole::Controller, source, 1)
            .unwrap();
        let controller_endpoint = core
            .admit(
                controller.prove(&controller_challenge),
                source,
                1,
                controller_tx,
                CancellationToken::new(),
            )
            .unwrap();
        (host_endpoint, controller_endpoint, controller_rx)
    }

    #[tokio::test]
    async fn exact_roles_forward_bounded_opaque_messages() {
        let (registration, host, controller) = fixture_registration(7);
        let route = registration.route_id;
        let mut core = RelayCore::new(vec![registration], RelayServerLimits::default(), 0).unwrap();
        let (host_endpoint, _, mut controller_rx) =
            admit_pair(&mut core, route, &host, &controller);
        let envelope = RelayEnvelopeV1::new(
            route,
            RelayDirection::HostToController,
            RelayConnectionSequence(0),
            b"opaque-ciphertext".to_vec(),
        )
        .unwrap();
        core.forward(&host_endpoint, envelope.clone(), envelope.encode(), 1)
            .unwrap();
        assert_eq!(controller_rx.recv().await.unwrap(), envelope.encode());
        assert_eq!(core.diagnostics.forwarded_messages, 1);
        assert_eq!(core.diagnostics.persistent_ciphertext_bytes, 0);
    }

    #[tokio::test]
    async fn replay_and_queue_limit_close_both_endpoints() {
        let (mut registration, host, controller) = fixture_registration(9);
        registration.quota.queue_messages = 1;
        let route = registration.route_id;
        let mut core = RelayCore::new(vec![registration], RelayServerLimits::default(), 0).unwrap();
        let (host_endpoint, _, _controller_rx) = admit_pair(&mut core, route, &host, &controller);
        let first = RelayEnvelopeV1::new(
            route,
            RelayDirection::HostToController,
            RelayConnectionSequence(0),
            vec![1],
        )
        .unwrap();
        core.forward(&host_endpoint, first.clone(), first.encode(), 1)
            .unwrap();
        let second = RelayEnvelopeV1::new(
            route,
            RelayDirection::HostToController,
            RelayConnectionSequence(1),
            vec![2],
        )
        .unwrap();
        assert_eq!(
            core.forward(&host_endpoint, second.clone(), second.encode(), 1)
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::QueueLimit
        );
        assert_eq!(core.route_state(route).unwrap(), RelayRouteState::Closed);
        assert_eq!(core.snapshot().queued_encoded_bytes, 0);
    }

    #[tokio::test]
    async fn queue_message_and_byte_boundaries_are_exact() {
        let (mut registration, host, controller) = fixture_registration(13);
        registration.quota.queue_messages = 64;
        let route = registration.route_id;
        let mut core = RelayCore::new(vec![registration], RelayServerLimits::default(), 0).unwrap();
        let (host_endpoint, _, _controller_rx) = admit_pair(&mut core, route, &host, &controller);
        for sequence in 0..64 {
            let envelope = RelayEnvelopeV1::new(
                route,
                RelayDirection::HostToController,
                RelayConnectionSequence(sequence),
                vec![1],
            )
            .unwrap();
            core.forward(&host_endpoint, envelope.clone(), envelope.encode(), 1)
                .unwrap();
            if sequence == 62 {
                assert_eq!(core.snapshot().queued_messages, 63);
            }
        }
        assert_eq!(core.snapshot().queued_messages, 64);
        let sixty_fifth = RelayEnvelopeV1::new(
            route,
            RelayDirection::HostToController,
            RelayConnectionSequence(64),
            vec![1],
        )
        .unwrap();
        assert_eq!(
            core.forward(&host_endpoint, sixty_fifth.clone(), sixty_fifth.encode(), 1)
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::QueueLimit
        );

        let (registration, host, controller) = fixture_registration(14);
        let route = registration.route_id;
        let mut core = RelayCore::new(vec![registration], RelayServerLimits::default(), 0).unwrap();
        let (host_endpoint, _, _controller_rx) = admit_pair(&mut core, route, &host, &controller);
        let payloads = [1_048_576_usize, 1_048_576, 1_048_576, 1_048_368];
        for (sequence, payload) in payloads.into_iter().enumerate() {
            let envelope = RelayEnvelopeV1::new(
                route,
                RelayDirection::HostToController,
                RelayConnectionSequence(sequence as u64),
                vec![2; payload],
            )
            .unwrap();
            core.forward(&host_endpoint, envelope.clone(), envelope.encode(), 1)
                .unwrap();
        }
        assert_eq!(core.snapshot().queued_encoded_bytes, 4_194_304);
        let over = RelayEnvelopeV1::new(
            route,
            RelayDirection::HostToController,
            RelayConnectionSequence(4),
            vec![3],
        )
        .unwrap();
        assert_eq!(
            core.forward(&host_endpoint, over.clone(), over.encode(), 1)
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::QueueLimit
        );
        assert_eq!(core.snapshot().queued_encoded_bytes, 0);
    }

    #[test]
    fn queue_byte_limit_checks_minus_equal_plus_one() {
        const MAXIMUM: usize = 4_194_304;

        assert!(!queue_would_exceed(0, MAXIMUM - 1, MAXIMUM));
        assert!(!queue_would_exceed(0, MAXIMUM, MAXIMUM));
        assert!(queue_would_exceed(0, MAXIMUM + 1, MAXIMUM));
        assert!(queue_would_exceed(usize::MAX, 1, MAXIMUM));
    }

    #[tokio::test]
    async fn route_pair_handshake_failure_and_rate_caps_are_exact() {
        let registrations: Vec<_> = (0..999)
            .map(|index| indexed_registration(index).0)
            .collect();
        let mut core = RelayCore::new(registrations, RelayServerLimits::default(), 0).unwrap();
        assert_eq!(core.snapshot().registered_routes, 999);
        core.insert_registration(indexed_registration(999).0, 0)
            .unwrap();
        assert_eq!(core.snapshot().registered_routes, 1_000);
        assert_eq!(
            core.insert_registration(indexed_registration(1_000).0, 0)
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::InvalidConfig
        );

        let source = IpAddr::from([127, 0, 0, 1]);
        for index in 0..4 {
            core.issue_challenge(
                indexed_registration(index).0.route_id,
                RelayEndpointRole::Host,
                source,
                1,
            )
            .unwrap();
        }
        assert_eq!(
            core.issue_challenge(
                indexed_registration(4).0.route_id,
                RelayEndpointRole::Host,
                source,
                1,
            )
            .unwrap_err()
            .code(),
            RelayDiagnosticCode::HandshakeLimit
        );

        let mut pair_core = RelayCore::new(
            (0..101)
                .map(|index| indexed_registration(index).0)
                .collect(),
            RelayServerLimits::default(),
            0,
        )
        .unwrap();
        for index in 0..99 {
            let (registration, host, controller) = indexed_registration(index);
            let _ = admit_pair(&mut pair_core, registration.route_id, &host, &controller);
        }
        assert_eq!(pair_core.snapshot().forwarding_pairs, 99);
        let (registration, host, controller) = indexed_registration(99);
        let _ = admit_pair(&mut pair_core, registration.route_id, &host, &controller);
        assert_eq!(pair_core.snapshot().forwarding_pairs, 100);
        let (registration, host, controller) = indexed_registration(100);
        let route = registration.route_id;
        let host_challenge = pair_core
            .issue_challenge(route, RelayEndpointRole::Host, source, 1)
            .unwrap();
        let (host_tx, _host_rx) = mpsc::channel(64);
        pair_core
            .admit(
                host.prove(&host_challenge),
                source,
                1,
                host_tx,
                CancellationToken::new(),
            )
            .unwrap();
        let controller_challenge = pair_core
            .issue_challenge(route, RelayEndpointRole::Controller, source, 1)
            .unwrap();
        let (controller_tx, _controller_rx) = mpsc::channel(64);
        assert_eq!(
            pair_core
                .admit(
                    controller.prove(&controller_challenge),
                    source,
                    1,
                    controller_tx,
                    CancellationToken::new(),
                )
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::PairLimit
        );

        let mut failed_core = RelayCore::new(Vec::new(), RelayServerLimits::default(), 0).unwrap();
        for _ in 0..5 {
            assert_eq!(
                failed_core
                    .issue_challenge(RelayRouteId([0; 32]), RelayEndpointRole::Host, source, 1)
                    .unwrap_err()
                    .code(),
                RelayDiagnosticCode::UnknownRoute
            );
        }
        assert_eq!(
            failed_core
                .issue_challenge(RelayRouteId([0; 32]), RelayEndpointRole::Host, source, 1)
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::AdmissionRateLimited
        );

        let (registration, host, controller) = fixture_registration(15);
        let route = registration.route_id;
        let mut rate_core =
            RelayCore::new(vec![registration], RelayServerLimits::default(), 0).unwrap();
        let (host_endpoint, controller_endpoint, mut controller_rx) =
            admit_pair(&mut rate_core, route, &host, &controller);
        let payloads = [
            1_048_576_usize,
            1_048_576,
            1_048_576,
            1_048_576,
            1_048_576,
            1_048_576,
            1_048_576,
            1_048_160,
        ];
        for (sequence, payload) in payloads.into_iter().enumerate() {
            let envelope = RelayEnvelopeV1::new(
                route,
                RelayDirection::HostToController,
                RelayConnectionSequence(sequence as u64),
                vec![4; payload],
            )
            .unwrap();
            let encoded_len = envelope.encoded_len();
            rate_core
                .forward(&host_endpoint, envelope.clone(), envelope.encode(), 0)
                .unwrap();
            let _ = controller_rx.try_recv().unwrap();
            rate_core.delivered(&controller_endpoint, encoded_len);
        }
        let over = RelayEnvelopeV1::new(
            route,
            RelayDirection::HostToController,
            RelayConnectionSequence(8),
            vec![5],
        )
        .unwrap();
        assert_eq!(
            rate_core
                .forward(&host_endpoint, over.clone(), over.encode(), 0)
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::RateLimit
        );
    }

    #[tokio::test]
    async fn replay_expiry_and_role_swap_fail_closed() {
        let (registration, host, _) = fixture_registration(16);
        let route = registration.route_id;
        let source = IpAddr::from([127, 0, 0, 1]);
        let mut core = RelayCore::new(vec![registration], RelayServerLimits::default(), 0).unwrap();
        let challenge = core
            .issue_challenge(route, RelayEndpointRole::Host, source, 1)
            .unwrap();
        let proof = host.prove(&challenge);
        let replay = RelayAdmissionProof::decode(&proof.encode()).unwrap();
        let (tx, _rx) = mpsc::channel(64);
        core.admit(proof, source, 1, tx, CancellationToken::new())
            .unwrap();
        let (tx, _rx) = mpsc::channel(64);
        assert_eq!(
            core.admit(replay, source, 1, tx, CancellationToken::new())
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::ReplayedProof
        );

        let (registration, host, _) = fixture_registration(17);
        let route = registration.route_id;
        let mut core = RelayCore::new(vec![registration], RelayServerLimits::default(), 0).unwrap();
        let challenge = core
            .issue_challenge(route, RelayEndpointRole::Host, source, 1)
            .unwrap();
        let (tx, _rx) = mpsc::channel(64);
        assert_eq!(
            core.admit(
                host.prove(&challenge),
                source,
                challenge.expires_at_unix_seconds + 1,
                tx,
                CancellationToken::new()
            )
            .unwrap_err()
            .code(),
            RelayDiagnosticCode::ExpiredProof
        );

        let (registration, host, _) = fixture_registration(18);
        let route = registration.route_id;
        let mut core = RelayCore::new(vec![registration], RelayServerLimits::default(), 0).unwrap();
        let challenge = core
            .issue_challenge(route, RelayEndpointRole::Host, source, 1)
            .unwrap();
        let mut proof = host.prove(&challenge);
        proof.role = RelayEndpointRole::Controller;
        let (tx, _rx) = mpsc::channel(64);
        assert_eq!(
            core.admit(proof, source, 1, tx, CancellationToken::new())
                .unwrap_err()
                .code(),
            RelayDiagnosticCode::InvalidProof
        );
    }

    fn indexed_registration(
        index: usize,
    ) -> (
        RelayRouteRegistration,
        RelayAdmissionCredential,
        RelayAdmissionCredential,
    ) {
        let mut route = [0xA5; 32];
        route[..8].copy_from_slice(&(index as u64).to_be_bytes());
        let mut host_secret = [0x5A; 32];
        host_secret[..8].copy_from_slice(&(index as u64).to_be_bytes());
        let mut controller_secret = [0x3C; 32];
        controller_secret[..8].copy_from_slice(&(index as u64).to_be_bytes());
        let host = RelayAdmissionCredential::from_fixture_bytes(host_secret);
        let controller = RelayAdmissionCredential::from_fixture_bytes(controller_secret);
        (
            RelayRouteRegistration::new(RelayRouteId(route), &host, &controller),
            host,
            controller,
        )
    }
}
