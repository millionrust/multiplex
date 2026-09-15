use std::time::{Duration, Instant};

use async_trait::async_trait;
use termirust_controller_security::{
    CODE_PAIRING_SHARE_BYTES, CodeKeyExchange, ConfirmedPairing, ControllerCapability,
    ControllerFrameKind, DeviceStaticPublicKey, MAX_PAIRING_OFFER_LIFETIME_SECONDS, PairingCode,
    PairingMachine, PairingNonce, PairingOfferCore, PairingRole, PairingState, RevocationEpoch,
    SasCode, StaticPrivateKey,
};
use termirust_domain::{
    AuthenticatedPeer, ControllerDeviceId, HostIdentityGeneration, PairingOfferId,
    PairingOfferState,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use crate::{
    CodePairingHello, HandshakeEntropy, ListenerError, ListenerErrorCode, PairingConnectRequest,
    PairingDeviceRegistration, PairingHostAck, SshControllerPairingOffer, read_bounded_frame,
    write_bounded_frame,
};

const MAX_PAIRING_HANDSHAKE_BYTES: usize = 1_024;
const MAX_PAIRING_SECURE_FRAME_BYTES: usize = 64 * 1024;
// Pairing includes a deliberate human SAS comparison. Bound the connection by the
// signed offer lifetime instead of the shorter machine-handshake deadline.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(MAX_PAIRING_OFFER_LIFETIME_SECONDS);
// Code pairing starts after the code is typed, so it only needs the machine handshake budget.
const CODE_PAIRING_TIMEOUT: Duration = Duration::from_secs(60);

pub struct PairingAuthoritySnapshot {
    pub offer: PairingOfferCore,
    pub host_private: StaticPrivateKey,
    pub identity_generation: HostIdentityGeneration,
    pub revocation_epoch: u64,
    pub session_generation: u64,
}

impl std::fmt::Debug for PairingAuthoritySnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PairingAuthoritySnapshot")
            .field("offer", &"[REDACTED]")
            .field("host_private", &"[REDACTED]")
            .field("identity_generation", &self.identity_generation)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("session_generation", &self.session_generation)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostPairingDecision {
    Confirm,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerClientPairingResult {
    pub device_id: ControllerDeviceId,
    pub host_public_key: termirust_controller_security::HostStaticPublicKey,
    pub identity_generation: u64,
    pub revocation_epoch: u64,
    pub session_generation: u64,
    pub capability_bits: u16,
}

#[async_trait]
pub trait ControllerPairingAuthority: Send + Sync {
    fn snapshot(&self, offer_id: PairingOfferId)
    -> Result<PairingAuthoritySnapshot, ListenerError>;

    fn set_offer_state(
        &self,
        offer_id: PairingOfferId,
        state: PairingOfferState,
    ) -> Result<(), ListenerError>;

    async fn await_host_decision(
        &self,
        offer_id: PairingOfferId,
        sas: &SasCode,
        cancel: &CancellationToken,
    ) -> Result<HostPairingDecision, ListenerError>;

    fn persist(
        &self,
        offer_id: PairingOfferId,
        device_id: ControllerDeviceId,
        device_key: DeviceStaticPublicKey,
        display_name: String,
        now_unix_seconds: u64,
    ) -> Result<AuthenticatedPeer, ListenerError>;

    fn acknowledge(
        &self,
        offer_id: PairingOfferId,
        device_key: DeviceStaticPublicKey,
    ) -> Result<(), ListenerError>;

    /// Takes one attempt at the code offer the desktop is showing. The attempt is spent
    /// before the key exchange starts, so a wrong code, a dropped connection, and a
    /// successful pairing all use it.
    fn begin_code_attempt(&self) -> Result<CodePairingAttempt, ListenerError> {
        Err(ListenerError::new(ListenerErrorCode::Unauthorized))
    }

    /// Reports how a code attempt ended. `paired` is true only once the device was saved
    /// and acknowledged.
    fn finish_code_attempt(&self, _offer_id: PairingOfferId, _paired: bool) {}
}

/// Most attempts one pairing code allows before the Host discards it.
pub const MAX_CODE_PAIRING_ATTEMPTS: u8 = 3;

/// One spent attempt at a code offer.
pub struct CodePairingAttempt {
    pub offer_id: PairingOfferId,
    pub code: PairingCode,
    pub snapshot: PairingAuthoritySnapshot,
}

impl std::fmt::Debug for CodePairingAttempt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodePairingAttempt")
            .field("offer_id", &self.offer_id)
            .field("code", &self.code)
            .field("snapshot", &self.snapshot)
            .finish()
    }
}

/// The Host side of code pairing: CPace keyed by the displayed code, then the Noise XX
/// pairing bound to its result. There is no SAS; the code already authenticated both keys.
pub async fn pair_controller_with_code<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authority: &dyn ControllerPairingAuthority,
    entropy: &mut impl HandshakeEntropy,
    cancel: CancellationToken,
) -> Result<AuthenticatedPeer, ListenerError> {
    let hello = tokio::time::timeout(CODE_PAIRING_TIMEOUT, CodePairingHello::read_from(stream))
        .await
        .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))??;
    let attempt = authority.begin_code_attempt()?;
    let offer_id = attempt.offer_id;
    let result = tokio::time::timeout(
        CODE_PAIRING_TIMEOUT,
        pair_controller_with_code_inner(stream, authority, entropy, cancel, hello, attempt),
    )
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))
    .and_then(|result| result);
    authority.finish_code_attempt(offer_id, result.is_ok());
    result
}

async fn pair_controller_with_code_inner<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authority: &dyn ControllerPairingAuthority,
    entropy: &mut impl HandshakeEntropy,
    cancel: CancellationToken,
    hello: CodePairingHello,
    attempt: CodePairingAttempt,
) -> Result<AuthenticatedPeer, ListenerError> {
    let started = Instant::now();
    let CodePairingAttempt {
        offer_id,
        code,
        snapshot,
    } = attempt;
    SshControllerPairingOffer::new(
        offer_id,
        &snapshot.offer,
        snapshot.identity_generation.get(),
        snapshot.revocation_epoch,
        snapshot.session_generation,
    )?
    .write_to(stream)
    .await?;
    let device_share = read_bounded_frame(stream, CODE_PAIRING_SHARE_BYTES).await?;
    authority.set_offer_state(offer_id, PairingOfferState::Handshaking)?;
    let exchange = CodeKeyExchange::new(
        PairingRole::HostResponder,
        &code,
        &snapshot.offer,
        &PairingNonce(hello.nonce()?),
        entropy.scalar_entropy()?,
    )
    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, &exchange.share(), CODE_PAIRING_SHARE_BYTES).await?;
    let binding = exchange
        .finish(&device_share)
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    drop(code);

    let mut machine = PairingMachine::new_host_responder_with_code(
        snapshot.offer.clone(),
        &binding,
        snapshot.host_private.clone(),
        entropy.ephemeral_private()?,
        elapsed_millis(started),
        unix_seconds(),
    )
    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    drop(binding);
    run_host_handshake(stream, &mut machine, started).await?;
    authority.set_offer_state(offer_id, PairingOfferState::SasReady)?;
    let confirmed = machine
        .confirm_code_authenticated(RevocationEpoch(snapshot.revocation_epoch))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    authority.set_offer_state(offer_id, PairingOfferState::HostConfirmed)?;
    register_paired_device(stream, authority, offer_id, &snapshot, confirmed, &cancel).await
}

/// The phone side of code pairing, for tests and command-line clients. The caller sends the
/// `PairCode` connection purpose first, as for the other connection kinds.
#[allow(clippy::too_many_arguments)]
pub async fn pair_controller_with_code_client<S, G>(
    stream: &mut S,
    code: &PairingCode,
    device_static_private: StaticPrivateKey,
    device_ephemeral_private: StaticPrivateKey,
    entropy: &mut impl HandshakeEntropy,
    device_id: ControllerDeviceId,
    display_name: String,
    prepare_registration: G,
) -> Result<ControllerClientPairingResult, ListenerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    G: FnOnce(&ControllerClientPairingResult) -> Result<(), ListenerError>,
{
    let device_nonce = entropy.nonce()?;
    let scalar_entropy = entropy.scalar_entropy()?;
    tokio::time::timeout(CODE_PAIRING_TIMEOUT, async {
        CodePairingHello::new(device_nonce).write_to(stream).await?;
        let envelope = SshControllerPairingOffer::read_from(stream).await?;
        let offer = envelope.offer()?;
        let exchange = CodeKeyExchange::new(
            PairingRole::DeviceInitiator,
            code,
            &offer,
            &PairingNonce(device_nonce),
            scalar_entropy,
        )
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        write_bounded_frame(stream, &exchange.share(), CODE_PAIRING_SHARE_BYTES).await?;
        let host_share = read_bounded_frame(stream, CODE_PAIRING_SHARE_BYTES).await?;
        let binding = exchange
            .finish(&host_share)
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let started = Instant::now();
        let mut machine = PairingMachine::new_device_initiator_with_code(
            offer.clone(),
            &binding,
            device_static_private,
            device_ephemeral_private,
            elapsed_millis(started),
            unix_seconds(),
        )
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        drop(binding);
        run_device_handshake(stream, &mut machine, started).await?;
        let confirmed = machine
            .confirm_code_authenticated(RevocationEpoch(envelope.revocation_epoch))
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        send_registration(
            stream,
            &envelope,
            &offer,
            confirmed,
            device_id,
            display_name,
            prepare_registration,
        )
        .await
    })
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))?
}

async fn run_host_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    machine: &mut PairingMachine,
    started: Instant,
) -> Result<(), ListenerError> {
    let device_hello = read_bounded_frame(stream, MAX_PAIRING_HANDSHAKE_BYTES).await?;
    machine
        .read_next(&device_hello, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let host_proof = machine
        .write_next(elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, host_proof.as_bytes(), MAX_PAIRING_HANDSHAKE_BYTES).await?;
    let device_proof = read_bounded_frame(stream, MAX_PAIRING_HANDSHAKE_BYTES).await?;
    machine
        .read_next(&device_proof, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if machine.state() != PairingState::SasReady {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    Ok(())
}

async fn run_device_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    machine: &mut PairingMachine,
    started: Instant,
) -> Result<(), ListenerError> {
    let device_hello = machine
        .write_next(elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, device_hello.as_bytes(), MAX_PAIRING_HANDSHAKE_BYTES).await?;
    let host_proof = read_bounded_frame(stream, MAX_PAIRING_HANDSHAKE_BYTES).await?;
    machine
        .read_next(&host_proof, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let device_proof = machine
        .write_next(elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, device_proof.as_bytes(), MAX_PAIRING_HANDSHAKE_BYTES).await?;
    if machine.state() != PairingState::SasReady {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    Ok(())
}

pub async fn pair_controller<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authority: &dyn ControllerPairingAuthority,
    entropy: &mut impl HandshakeEntropy,
    cancel: CancellationToken,
) -> Result<AuthenticatedPeer, ListenerError> {
    tokio::time::timeout(
        PAIRING_TIMEOUT,
        pair_controller_inner(stream, authority, entropy, cancel),
    )
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))?
}

#[allow(clippy::too_many_arguments)]
pub async fn pair_controller_client<S, F, G>(
    stream: &mut S,
    envelope: SshControllerPairingOffer,
    device_static_private: StaticPrivateKey,
    device_ephemeral_private: StaticPrivateKey,
    device_id: ControllerDeviceId,
    display_name: String,
    confirm_sas: F,
    prepare_registration: G,
) -> Result<ControllerClientPairingResult, ListenerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce(&SasCode) -> Result<bool, ListenerError>,
    G: FnOnce(&ControllerClientPairingResult) -> Result<(), ListenerError>,
{
    tokio::time::timeout(
        PAIRING_TIMEOUT,
        pair_controller_client_inner(
            stream,
            envelope,
            device_static_private,
            device_ephemeral_private,
            device_id,
            display_name,
            confirm_sas,
            prepare_registration,
        ),
    )
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))?
}

#[allow(clippy::too_many_arguments)]
async fn pair_controller_client_inner<S, F, G>(
    stream: &mut S,
    envelope: SshControllerPairingOffer,
    device_static_private: StaticPrivateKey,
    device_ephemeral_private: StaticPrivateKey,
    device_id: ControllerDeviceId,
    display_name: String,
    confirm_sas: F,
    prepare_registration: G,
) -> Result<ControllerClientPairingResult, ListenerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce(&SasCode) -> Result<bool, ListenerError>,
    G: FnOnce(&ControllerClientPairingResult) -> Result<(), ListenerError>,
{
    let started = Instant::now();
    let now = unix_seconds();
    let offer = envelope.offer()?;
    let offer_for_result = offer.clone();
    PairingConnectRequest::new(envelope.offer_id)
        .write_to(stream)
        .await?;
    let mut machine = PairingMachine::new_device_initiator(
        offer,
        device_static_private,
        device_ephemeral_private,
        elapsed_millis(started),
        now,
    )
    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let device_hello = machine
        .write_next(elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, device_hello.as_bytes(), MAX_PAIRING_HANDSHAKE_BYTES).await?;
    let host_proof = read_bounded_frame(stream, MAX_PAIRING_HANDSHAKE_BYTES).await?;
    machine
        .read_next(&host_proof, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let device_proof = machine
        .write_next(elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, device_proof.as_bytes(), MAX_PAIRING_HANDSHAKE_BYTES).await?;
    if machine.state() != PairingState::SasReady {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    let sas = machine
        .sas()
        .cloned()
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if !confirm_sas(&sas)? {
        let _ = machine.reject();
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    let confirmed = machine
        .confirm(&sas, RevocationEpoch(envelope.revocation_epoch))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    send_registration(
        stream,
        &envelope,
        &offer_for_result,
        confirmed,
        device_id,
        display_name,
        prepare_registration,
    )
    .await
}

/// Registers the phone over a confirmed pairing and checks the Host's acknowledgement.
async fn send_registration<S, G>(
    stream: &mut S,
    envelope: &SshControllerPairingOffer,
    offer: &PairingOfferCore,
    mut confirmed: ConfirmedPairing,
    device_id: ControllerDeviceId,
    display_name: String,
    prepare_registration: G,
) -> Result<ControllerClientPairingResult, ListenerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    G: FnOnce(&ControllerClientPairingResult) -> Result<(), ListenerError>,
{
    let result = ControllerClientPairingResult {
        device_id,
        host_public_key: offer.host_static_public_key,
        identity_generation: envelope.identity_generation,
        revocation_epoch: envelope.revocation_epoch,
        session_generation: envelope.session_generation,
        capability_bits: offer.capabilities.bits(),
    };
    prepare_registration(&result)?;
    let registration = PairingDeviceRegistration::new(device_id, display_name).encode()?;
    let registration = confirmed
        .transport
        .seal(
            ControllerFrameKind::Control,
            ControllerCapability::ObserveSessions,
            RevocationEpoch(envelope.revocation_epoch),
            &registration,
        )
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(
        stream,
        registration.as_bytes(),
        MAX_PAIRING_SECURE_FRAME_BYTES,
    )
    .await?;
    let ack = read_bounded_frame(stream, MAX_PAIRING_SECURE_FRAME_BYTES).await?;
    let ack = confirmed
        .transport
        .open(&ack)
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if ack.kind != ControllerFrameKind::Control
        || ack.capability != ControllerCapability::ObserveSessions
        || ack.revocation_epoch.0 != envelope.revocation_epoch
    {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    let ack = PairingHostAck::decode(&ack.payload)?;
    if ack.device_id != device_id
        || ack.identity_generation != envelope.identity_generation
        || ack.revocation_epoch != envelope.revocation_epoch
        || ack.session_generation != envelope.session_generation
        || ack.capability_bits != result.capability_bits
        || confirmed.host_key != result.host_public_key
    {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    Ok(result)
}

async fn pair_controller_inner<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authority: &dyn ControllerPairingAuthority,
    entropy: &mut impl HandshakeEntropy,
    cancel: CancellationToken,
) -> Result<AuthenticatedPeer, ListenerError> {
    let started = Instant::now();
    let request = PairingConnectRequest::read_from(stream).await?;
    let initial = authority.snapshot(request.offer_id)?;
    authority.set_offer_state(request.offer_id, PairingOfferState::Handshaking)?;
    let mut machine = PairingMachine::new_host_responder(
        initial.offer.clone(),
        initial.host_private.clone(),
        entropy.ephemeral_private()?,
        elapsed_millis(started),
        unix_seconds(),
    )
    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;

    let device_hello = read_bounded_frame(stream, MAX_PAIRING_HANDSHAKE_BYTES).await?;
    machine
        .read_next(&device_hello, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let host_proof = machine
        .write_next(elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, host_proof.as_bytes(), MAX_PAIRING_HANDSHAKE_BYTES).await?;
    let device_proof = read_bounded_frame(stream, MAX_PAIRING_HANDSHAKE_BYTES).await?;
    machine
        .read_next(&device_proof, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if machine.state() != PairingState::SasReady {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    authority.set_offer_state(request.offer_id, PairingOfferState::SasReady)?;
    let sas = machine
        .sas()
        .cloned()
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    match authority
        .await_host_decision(request.offer_id, &sas, &cancel)
        .await?
    {
        HostPairingDecision::Confirm => {
            authority.set_offer_state(request.offer_id, PairingOfferState::HostConfirmed)?;
        }
        HostPairingDecision::Reject => {
            let _ = machine.reject();
            authority.set_offer_state(request.offer_id, PairingOfferState::Rejected)?;
            return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
        }
    }

    let confirmed = machine
        .confirm(&sas, RevocationEpoch(initial.revocation_epoch))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    register_paired_device(
        stream,
        authority,
        request.offer_id,
        &initial,
        confirmed,
        &cancel,
    )
    .await
}

/// Receives the phone's registration over a confirmed pairing, saves the device, and
/// acknowledges it.
async fn register_paired_device<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authority: &dyn ControllerPairingAuthority,
    offer_id: PairingOfferId,
    initial: &PairingAuthoritySnapshot,
    confirmed: ConfirmedPairing,
    cancel: &CancellationToken,
) -> Result<AuthenticatedPeer, ListenerError> {
    let mut transport = confirmed.transport;
    let registration_frame = tokio::select! {
        _ = cancel.cancelled() => return Err(ListenerError::new(ListenerErrorCode::Cancelled)),
        frame = read_bounded_frame(stream, MAX_PAIRING_SECURE_FRAME_BYTES) => frame?,
    };
    let registration = transport
        .open(&registration_frame)
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if registration.kind != ControllerFrameKind::Control
        || registration.capability != ControllerCapability::ObserveSessions
        || registration.revocation_epoch.0 != initial.revocation_epoch
    {
        return Err(ListenerError::new(ListenerErrorCode::Unauthorized));
    }
    let registration = PairingDeviceRegistration::decode(&registration.payload)?;
    let peer = authority.persist(
        offer_id,
        registration.device_id,
        confirmed.device_key,
        registration.display_name,
        unix_seconds(),
    )?;
    let ack = PairingHostAck {
        schema_version: 1,
        device_id: peer.device_id,
        identity_generation: peer.identity_generation.get(),
        revocation_epoch: peer.revocation_epoch,
        session_generation: initial.session_generation,
        capability_bits: peer.capabilities.bits(),
    };
    let ack = transport
        .seal(
            ControllerFrameKind::Control,
            ControllerCapability::ObserveSessions,
            RevocationEpoch(peer.revocation_epoch),
            &ack.encode()?,
        )
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if let Err(error) =
        write_bounded_frame(stream, ack.as_bytes(), MAX_PAIRING_SECURE_FRAME_BYTES).await
    {
        let _ = authority.set_offer_state(offer_id, PairingOfferState::Uncertain);
        return Err(error);
    }
    authority.acknowledge(offer_id, confirmed.device_key)?;
    Ok(peer)
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
