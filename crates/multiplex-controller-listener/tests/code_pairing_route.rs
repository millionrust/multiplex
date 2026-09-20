use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use multiplex_controller_listener::{
    CodePairingAttempt, ControllerPairingAuthority, HandshakeEntropy, HostPairingDecision,
    ListenerError, ListenerErrorCode, MAX_CODE_PAIRING_ATTEMPTS, PairingAuthoritySnapshot,
    pair_controller_with_code, pair_controller_with_code_client,
};
use multiplex_controller_security::{
    CONTROLLER_V2, CapabilitySet, ControllerCapability, DeviceStaticPublicKey, PairingCode,
    PairingNonce, PairingOfferCore, SasCode, StaticPrivateKey, device_public_key_from_private,
    host_public_key_from_private,
};
use multiplex_domain::{
    AuthenticatedPeer, ControllerCapabilities, ControllerDeviceId, DevicePublicKey,
    HostIdentityGeneration, PairingOfferId, PairingOfferState,
};
use tokio_util::sync::CancellationToken;

struct Entropy {
    seed: u8,
}

impl HandshakeEntropy for Entropy {
    fn nonce(&mut self) -> Result<[u8; 32], ListenerError> {
        self.seed = self.seed.wrapping_add(1);
        Ok([self.seed; 32])
    }

    fn ephemeral_private(&mut self) -> Result<StaticPrivateKey, ListenerError> {
        Ok(StaticPrivateKey::from_fixture_bytes(self.nonce()?))
    }
}

/// The desktop's pairing mode: one code, a few attempts, one attempt at a time.
struct Authority {
    offer_id: PairingOfferId,
    offer: PairingOfferCore,
    host_private: StaticPrivateKey,
    code: PairingCode,
    attempts_left: Mutex<u8>,
    outcomes: Mutex<Vec<bool>>,
    states: Mutex<Vec<PairingOfferState>>,
    persisted: Mutex<Option<AuthenticatedPeer>>,
}

#[async_trait]
impl ControllerPairingAuthority for Authority {
    fn snapshot(
        &self,
        offer_id: PairingOfferId,
    ) -> Result<PairingAuthoritySnapshot, ListenerError> {
        assert_eq!(offer_id, self.offer_id);
        Ok(PairingAuthoritySnapshot {
            offer: self.offer.clone(),
            host_private: self.host_private.clone(),
            identity_generation: HostIdentityGeneration::INITIAL,
            revocation_epoch: 3,
            session_generation: 5,
        })
    }

    fn set_offer_state(
        &self,
        offer_id: PairingOfferId,
        state: PairingOfferState,
    ) -> Result<(), ListenerError> {
        assert_eq!(offer_id, self.offer_id);
        self.states.lock().unwrap().push(state);
        Ok(())
    }

    async fn await_host_decision(
        &self,
        _: PairingOfferId,
        _: &SasCode,
        _: &CancellationToken,
    ) -> Result<HostPairingDecision, ListenerError> {
        panic!("code pairing never asks the desktop to compare a SAS")
    }

    fn persist(
        &self,
        offer_id: PairingOfferId,
        device_id: ControllerDeviceId,
        device_key: DeviceStaticPublicKey,
        display_name: String,
        _: u64,
    ) -> Result<AuthenticatedPeer, ListenerError> {
        assert_eq!(offer_id, self.offer_id);
        assert_eq!(display_name, "Test Pixel");
        let peer = AuthenticatedPeer {
            device_id,
            public_key: DevicePublicKey(device_key.0),
            identity_generation: HostIdentityGeneration::INITIAL,
            revocation_epoch: 3,
            capabilities: ControllerCapabilities::default()
                .with(multiplex_domain::ControllerCapability::ObserveSessions),
        };
        *self.persisted.lock().unwrap() = Some(peer.clone());
        Ok(peer)
    }

    fn acknowledge(
        &self,
        offer_id: PairingOfferId,
        _: DeviceStaticPublicKey,
    ) -> Result<(), ListenerError> {
        assert_eq!(offer_id, self.offer_id);
        Ok(())
    }

    fn begin_code_attempt(&self) -> Result<CodePairingAttempt, ListenerError> {
        let mut attempts = self.attempts_left.lock().unwrap();
        if *attempts == 0 {
            return Err(ListenerError::new(ListenerErrorCode::RateLimited));
        }
        *attempts -= 1;
        Ok(CodePairingAttempt {
            offer_id: self.offer_id,
            code: self.code.clone(),
            snapshot: self.snapshot(self.offer_id)?,
        })
    }

    fn finish_code_attempt(&self, offer_id: PairingOfferId, paired: bool) {
        assert_eq!(offer_id, self.offer_id);
        self.outcomes.lock().unwrap().push(paired);
    }
}

fn authority(code: &str) -> Authority {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let host_private = StaticPrivateKey::from_fixture_bytes([41; 32]);
    Authority {
        offer_id: PairingOfferId::new(),
        offer: PairingOfferCore {
            version: CONTROLLER_V2,
            expires_at_unix_seconds: now + 300,
            nonce: PairingNonce([42; 32]),
            host_static_public_key: host_public_key_from_private(&host_private),
            capabilities: CapabilitySet::default().with(ControllerCapability::ObserveSessions),
        },
        host_private,
        code: PairingCode::parse(code).unwrap(),
        attempts_left: Mutex::new(MAX_CODE_PAIRING_ATTEMPTS),
        outcomes: Mutex::new(Vec::new()),
        states: Mutex::new(Vec::new()),
        persisted: Mutex::new(None),
    }
}

async fn attempt(
    authority: &Authority,
    typed_code: &str,
    device_private: &StaticPrivateKey,
) -> (
    Result<AuthenticatedPeer, ListenerError>,
    Result<multiplex_controller_listener::ControllerClientPairingResult, ListenerError>,
) {
    let (mut device_stream, mut host_stream) = tokio::io::duplex(16 * 1024);
    // Each side closes its end when it finishes, as a dropped connection would.
    let host = async {
        let result = pair_controller_with_code(
            &mut host_stream,
            authority,
            &mut Entropy { seed: 100 },
            CancellationToken::new(),
        )
        .await;
        drop(host_stream);
        result
    };
    let device = async {
        let result = pair_controller_with_code_client(
            &mut device_stream,
            &PairingCode::parse(typed_code).unwrap(),
            device_private.clone(),
            StaticPrivateKey::from_fixture_bytes([44; 32]),
            &mut Entropy { seed: 7 },
            ControllerDeviceId::new(),
            "Test Pixel".into(),
            |_| Ok(()),
        )
        .await;
        drop(device_stream);
        result
    };
    tokio::join!(host, device)
}

#[tokio::test]
async fn the_right_code_pairs_without_a_sas_and_learns_the_host_key() {
    let authority = authority("305917");
    let device_private = StaticPrivateKey::from_fixture_bytes([43; 32]);
    let (peer, result) = attempt(&authority, "305917", &device_private).await;
    let (peer, result) = (peer.unwrap(), result.unwrap());

    assert_eq!(
        result.host_public_key,
        authority.offer.host_static_public_key
    );
    assert_eq!(result.device_id, peer.device_id);
    assert_eq!(
        peer.public_key,
        DevicePublicKey(device_public_key_from_private(&device_private).0)
    );
    assert_eq!(*authority.outcomes.lock().unwrap(), [true]);
    assert_eq!(
        authority.states.lock().unwrap().as_slice(),
        [
            PairingOfferState::Handshaking,
            PairingOfferState::SasReady,
            PairingOfferState::HostConfirmed
        ]
    );
    assert_eq!(authority.persisted.lock().unwrap().as_ref(), Some(&peer));
}

#[tokio::test]
async fn wrong_codes_spend_attempts_and_the_code_closes_after_three() {
    let authority = authority("305917");
    let device_private = StaticPrivateKey::from_fixture_bytes([43; 32]);
    for wrong in ["305918", "000000", "999999"] {
        let (host, device) = attempt(&authority, wrong, &device_private).await;
        assert!(host.is_err());
        assert!(device.is_err());
    }
    assert_eq!(*authority.outcomes.lock().unwrap(), [false, false, false]);
    assert!(authority.persisted.lock().unwrap().is_none());

    // With no attempts left even the right code is refused.
    let (host, device) = attempt(&authority, "305917", &device_private).await;
    assert_eq!(host.unwrap_err().code, ListenerErrorCode::RateLimited);
    assert!(device.is_err());
    assert!(authority.persisted.lock().unwrap().is_none());
}
