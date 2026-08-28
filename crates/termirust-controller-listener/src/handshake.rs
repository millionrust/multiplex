use std::time::{Duration, Instant};

use rand::RngCore as _;
use termirust_controller_security::{
    AuthenticatedConnection, CONTROLLER_V1, CapabilitySet, ConnectionChallenge,
    ConnectionInitiator, ConnectionPrelude, ConnectionResponder, DeviceStaticPublicKey,
    HostStaticPublicKey, RevocationEpoch, StaticPrivateKey, host_public_key_from_private,
};
use termirust_domain::{
    AuthenticatedPeer, ControllerCapabilities, ControllerDeviceAuthority, DevicePublicKey,
    HostIdentityState, HostPublicKey, PairedDeviceStatus,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::{
    ControllerConnectionPurpose, ListenerError, ListenerErrorCode, read_bounded_frame,
    write_bounded_frame,
};

const MAX_HANDSHAKE_MESSAGE_BYTES: usize = 1_024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

pub trait HandshakeEntropy {
    fn nonce(&mut self) -> Result<[u8; 32], ListenerError>;
    fn ephemeral_private(&mut self) -> Result<StaticPrivateKey, ListenerError>;
}

#[derive(Debug, Default)]
pub struct SystemHandshakeEntropy;

impl HandshakeEntropy for SystemHandshakeEntropy {
    fn nonce(&mut self) -> Result<[u8; 32], ListenerError> {
        random_bytes()
    }

    fn ephemeral_private(&mut self) -> Result<StaticPrivateKey, ListenerError> {
        random_bytes().map(StaticPrivateKey::from_bytes)
    }
}

pub struct AuthenticatedControllerConnection {
    pub peer: AuthenticatedPeer,
    pub connection: AuthenticatedConnection,
}

pub async fn initiate_controller<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    identity_generation: u64,
    revocation_epoch: u64,
    host_key: HostStaticPublicKey,
    device_private: StaticPrivateKey,
    requested_capabilities: CapabilitySet,
    entropy: &mut impl HandshakeEntropy,
) -> Result<AuthenticatedConnection, ListenerError> {
    tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        initiate_controller_inner(
            stream,
            identity_generation,
            revocation_epoch,
            host_key,
            device_private,
            requested_capabilities,
            entropy,
        ),
    )
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))?
}

async fn initiate_controller_inner<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    identity_generation: u64,
    revocation_epoch: u64,
    host_key: HostStaticPublicKey,
    device_private: StaticPrivateKey,
    requested_capabilities: CapabilitySet,
    entropy: &mut impl HandshakeEntropy,
) -> Result<AuthenticatedConnection, ListenerError> {
    let started = Instant::now();
    ControllerConnectionPurpose::Authenticate
        .write_to(stream)
        .await?;
    let prelude = ConnectionPrelude {
        version: CONTROLLER_V1,
        identity_generation,
        revocation_epoch: RevocationEpoch(revocation_epoch),
        client_nonce: entropy.nonce()?,
    };
    stream
        .write_all(
            &prelude
                .encode()
                .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?,
        )
        .await
        .map_err(ListenerError::from)?;
    stream.flush().await.map_err(ListenerError::from)?;
    let mut challenge_bytes = [0; ConnectionChallenge::ENCODED_BYTES];
    stream
        .read_exact(&mut challenge_bytes)
        .await
        .map_err(ListenerError::from)?;
    let challenge = ConnectionChallenge::decode(&challenge_bytes)
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let mut initiator = ConnectionInitiator::new(
        prelude,
        &challenge,
        host_key,
        device_private,
        entropy.ephemeral_private()?,
        requested_capabilities,
        elapsed_millis(started),
    )
    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let hello = initiator
        .write_hello(elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, hello.as_bytes(), MAX_HANDSHAKE_MESSAGE_BYTES).await?;
    let accept = read_bounded_frame(stream, MAX_HANDSHAKE_MESSAGE_BYTES).await?;
    initiator
        .read_accept(&accept, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))
}

impl std::fmt::Debug for AuthenticatedControllerConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticatedControllerConnection")
            .field("peer", &self.peer)
            .field("connection", &"[REDACTED]")
            .finish()
    }
}

pub async fn authenticate_controller<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authority: &ControllerDeviceAuthority,
    host_private: StaticPrivateKey,
    entropy: &mut impl HandshakeEntropy,
) -> Result<AuthenticatedControllerConnection, ListenerError> {
    tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        authenticate_controller_inner(stream, authority, host_private, entropy),
    )
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))?
}

async fn authenticate_controller_inner<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    authority: &ControllerDeviceAuthority,
    host_private: StaticPrivateKey,
    entropy: &mut impl HandshakeEntropy,
) -> Result<AuthenticatedControllerConnection, ListenerError> {
    authority
        .validate()
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if authority.state != HostIdentityState::Ready {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    let identity = authority
        .identity
        .as_ref()
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if HostPublicKey(host_public_key_from_private(&host_private).0) != identity.public_key {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }

    let started = Instant::now();
    let mut prelude_bytes = [0; ConnectionPrelude::ENCODED_BYTES];
    stream
        .read_exact(&mut prelude_bytes)
        .await
        .map_err(ListenerError::from)?;
    let prelude = ConnectionPrelude::decode(&prelude_bytes)
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    if prelude.identity_generation != identity.generation.get()
        || prelude.revocation_epoch.0 != authority.revocation_epoch
    {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }

    let challenge = ConnectionChallenge {
        server_nonce: entropy.nonce()?,
    };
    stream
        .write_all(
            &challenge
                .encode()
                .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?,
        )
        .await
        .map_err(ListenerError::from)?;
    stream.flush().await.map_err(ListenerError::from)?;

    let mut responder = ConnectionResponder::new(
        prelude,
        &challenge,
        host_private,
        entropy.ephemeral_private()?,
        elapsed_millis(started),
    )
    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let hello = read_bounded_frame(stream, MAX_HANDSHAKE_MESSAGE_BYTES).await?;
    let claim = responder
        .read_hello(&hello, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let device = find_authorized_device(authority, claim.device_key)?;
    if claim.identity_generation != device.identity_generation.get()
        || claim.revocation_epoch.0 != device.revocation_epoch
    {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    let granted_bits = claim.requested_capabilities.bits() & device.capabilities.bits();
    if granted_bits == 0 {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    let granted = CapabilitySet::from_bits(granted_bits)
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let (accept, connection) = responder
        .write_accept(granted, elapsed_millis(started))
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    write_bounded_frame(stream, accept.as_bytes(), MAX_HANDSHAKE_MESSAGE_BYTES).await?;

    Ok(AuthenticatedControllerConnection {
        peer: AuthenticatedPeer {
            device_id: device.device_id,
            public_key: device.public_key,
            identity_generation: device.identity_generation,
            revocation_epoch: device.revocation_epoch,
            capabilities: ControllerCapabilities::from_bits(granted.bits())
                .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?,
        },
        connection,
    })
}

fn find_authorized_device(
    authority: &ControllerDeviceAuthority,
    key: DeviceStaticPublicKey,
) -> Result<&termirust_domain::PairedDeviceRecord, ListenerError> {
    authority
        .devices
        .iter()
        .find(|device| {
            device.public_key == DevicePublicKey(key.0)
                && device.status != PairedDeviceStatus::Revoked
                && device.revocation_epoch == authority.revocation_epoch
        })
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn random_bytes() -> Result<[u8; 32], ListenerError> {
    let mut bytes = [0; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| ListenerError::new(ListenerErrorCode::RandomUnavailable))?;
    if bytes == [0; 32] {
        return Err(ListenerError::new(ListenerErrorCode::RandomUnavailable));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_controller_security::ControllerCapability as SecurityCapability;
    use termirust_domain::{
        ControllerCapabilities, ControllerDeviceId, ControllerProtocolRange,
        HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, PairingAttemptLedger,
        PairingOfferId,
    };

    struct Entropy(Vec<[u8; 32]>);

    impl HandshakeEntropy for Entropy {
        fn nonce(&mut self) -> Result<[u8; 32], ListenerError> {
            Ok(self.0.remove(0))
        }

        fn ephemeral_private(&mut self) -> Result<StaticPrivateKey, ListenerError> {
            Ok(StaticPrivateKey::from_fixture_bytes(self.0.remove(0)))
        }
    }

    fn authority(
        host_private: &StaticPrivateKey,
        device_private: &StaticPrivateKey,
    ) -> ControllerDeviceAuthority {
        let device_key =
            termirust_controller_security::device_public_key_from_private(device_private);
        let capabilities = ControllerCapabilities::default()
            .with(termirust_domain::ControllerCapability::ObserveSessions)
            .with(termirust_domain::ControllerCapability::AttachOutput);
        ControllerDeviceAuthority {
            identity: Some(HostIdentityPublic::new(
                HostIdentityGeneration::INITIAL,
                HostPublicKey(host_public_key_from_private(host_private).0),
            )),
            secret_ref: Some(HostIdentitySecretRef::new("identity:test").unwrap()),
            state: HostIdentityState::Ready,
            revocation_epoch: 7,
            session_generation: 9,
            devices: vec![termirust_domain::PairedDeviceRecord {
                device_id: ControllerDeviceId::new(),
                public_key: DevicePublicKey(device_key.0),
                display_name: "Phone".to_owned(),
                capabilities,
                protocol_range: ControllerProtocolRange::V1,
                created_at: 1,
                last_seen_at: None,
                revocation_epoch: 7,
                identity_generation: HostIdentityGeneration::INITIAL,
                status: PairedDeviceStatus::Online,
                source_offer_id: PairingOfferId::new(),
            }],
            offers: Vec::new(),
            attempts: PairingAttemptLedger::default(),
        }
    }

    #[tokio::test]
    async fn authenticated_controller_handshake_grants_only_persisted_capabilities() {
        let host_private = StaticPrivateKey::from_fixture_bytes([1; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([2; 32]);
        let authority = authority(&host_private, &device_private);
        let (mut client, mut server) = tokio::io::duplex(4_096);
        let server_authority = authority.clone();
        let server_host_private = host_private.clone();
        let server_task = tokio::spawn(async move {
            authenticate_controller(
                &mut server,
                &server_authority,
                server_host_private,
                &mut Entropy(vec![[4; 32], [5; 32]]),
            )
            .await
            .unwrap()
        });

        let prelude = ConnectionPrelude {
            version: CONTROLLER_V1,
            identity_generation: 1,
            revocation_epoch: RevocationEpoch(7),
            client_nonce: [3; 32],
        };
        client.write_all(&prelude.encode().unwrap()).await.unwrap();
        let mut challenge_bytes = [0; ConnectionChallenge::ENCODED_BYTES];
        client.read_exact(&mut challenge_bytes).await.unwrap();
        let challenge = ConnectionChallenge::decode(&challenge_bytes).unwrap();
        let requested = CapabilitySet::default()
            .with(SecurityCapability::ObserveSessions)
            .with(SecurityCapability::AttachOutput)
            .with(SecurityCapability::SendInput);
        let mut initiator = ConnectionInitiator::new(
            prelude,
            &challenge,
            HostStaticPublicKey(host_public_key_from_private(&host_private).0),
            device_private,
            StaticPrivateKey::from_fixture_bytes([6; 32]),
            requested,
            0,
        )
        .unwrap();
        let hello = initiator.write_hello(0).unwrap();
        write_bounded_frame(&mut client, hello.as_bytes(), MAX_HANDSHAKE_MESSAGE_BYTES)
            .await
            .unwrap();
        let accept = read_bounded_frame(&mut client, MAX_HANDSHAKE_MESSAGE_BYTES)
            .await
            .unwrap();
        let client_connection = initiator.read_accept(&accept, 0).unwrap();
        assert!(
            client_connection
                .capabilities
                .contains(SecurityCapability::ObserveSessions)
        );
        assert!(
            !client_connection
                .capabilities
                .contains(SecurityCapability::SendInput)
        );

        let authenticated = server_task.await.unwrap();
        assert_eq!(
            authenticated.peer.public_key,
            authority.devices[0].public_key
        );
        assert_eq!(authenticated.peer.capabilities.bits(), 0b11);
    }

    #[tokio::test]
    async fn reusable_client_handshake_matches_the_authoritative_responder() {
        let host_private = StaticPrivateKey::from_fixture_bytes([11; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([12; 32]);
        let authority = authority(&host_private, &device_private);
        let (mut client, mut server) = tokio::io::duplex(4_096);
        let server_authority = authority.clone();
        let server_host_private = host_private.clone();
        let server_task = tokio::spawn(async move {
            ControllerConnectionPurpose::read_from(&mut server)
                .await
                .unwrap();
            authenticate_controller(
                &mut server,
                &server_authority,
                server_host_private,
                &mut Entropy(vec![[14; 32], [15; 32]]),
            )
            .await
            .unwrap()
        });
        let requested = CapabilitySet::default()
            .with(SecurityCapability::ObserveSessions)
            .with(SecurityCapability::AttachOutput)
            .with(SecurityCapability::SendInput);
        let authenticated = initiate_controller(
            &mut client,
            1,
            7,
            HostStaticPublicKey(host_public_key_from_private(&host_private).0),
            device_private,
            requested,
            &mut Entropy(vec![[13; 32], [16; 32]]),
        )
        .await
        .unwrap();
        assert!(
            authenticated
                .capabilities
                .contains(SecurityCapability::ObserveSessions)
        );
        assert!(
            !authenticated
                .capabilities
                .contains(SecurityCapability::SendInput)
        );
        let server = server_task.await.unwrap();
        assert_eq!(server.connection.capabilities, authenticated.capabilities);
    }

    #[tokio::test]
    async fn handshake_rejects_host_secret_mismatch_before_reading_peer_bytes() {
        let host_private = StaticPrivateKey::from_fixture_bytes([1; 32]);
        let device_private = StaticPrivateKey::from_fixture_bytes([2; 32]);
        let authority = authority(&host_private, &device_private);
        let mut stream = tokio::io::duplex(64).1;
        assert_eq!(
            authenticate_controller(
                &mut stream,
                &authority,
                StaticPrivateKey::from_fixture_bytes([9; 32]),
                &mut Entropy(vec![[4; 32], [5; 32]]),
            )
            .await
            .unwrap_err()
            .code,
            ListenerErrorCode::AuthenticationFailed
        );
    }
}
