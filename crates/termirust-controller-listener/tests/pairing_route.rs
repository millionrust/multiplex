use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use termirust_controller_listener::{
    ControllerPairingAuthority, HandshakeEntropy, HostPairingDecision, ListenerError,
    PairingAuthoritySnapshot, PairingConnectRequest, SshControllerPairingOffer, pair_controller,
    pair_controller_client, read_bounded_frame, write_bounded_frame,
};
use termirust_controller_security::{
    CONTROLLER_V1, CapabilitySet, ControllerCapability, DeviceStaticPublicKey, PairingMachine,
    PairingNonce, PairingOfferCore, SasCode, StaticPrivateKey, device_public_key_from_private,
    host_public_key_from_private,
};
use termirust_domain::{
    AuthenticatedPeer, ControllerCapabilities, ControllerDeviceId, DevicePublicKey,
    HostIdentityGeneration, PairingOfferId, PairingOfferState,
};
use tokio_util::sync::CancellationToken;

struct Entropy(Option<StaticPrivateKey>);

impl HandshakeEntropy for Entropy {
    fn nonce(&mut self) -> Result<[u8; 32], ListenerError> {
        unreachable!("pairing does not request a reconnect nonce")
    }

    fn ephemeral_private(&mut self) -> Result<StaticPrivateKey, ListenerError> {
        Ok(self.0.take().expect("one pairing ephemeral key"))
    }
}

struct Authority {
    offer_id: PairingOfferId,
    offer: PairingOfferCore,
    host_private: StaticPrivateKey,
    decision: HostPairingDecision,
    decision_delay: Duration,
    states: Mutex<Vec<PairingOfferState>>,
    persisted: Mutex<Option<AuthenticatedPeer>>,
    acknowledged: Mutex<bool>,
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
        offer_id: PairingOfferId,
        sas: &SasCode,
        _: &CancellationToken,
    ) -> Result<HostPairingDecision, ListenerError> {
        assert_eq!(offer_id, self.offer_id);
        assert_eq!(sas.as_str().len(), 9);
        tokio::time::sleep(self.decision_delay).await;
        Ok(self.decision)
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
        assert_eq!(display_name, "Test iPhone");
        let peer = AuthenticatedPeer {
            device_id,
            public_key: DevicePublicKey(device_key.0),
            identity_generation: HostIdentityGeneration::INITIAL,
            revocation_epoch: 3,
            capabilities: ControllerCapabilities::default()
                .with(termirust_domain::ControllerCapability::ObserveSessions),
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
        *self.acknowledged.lock().unwrap() = true;
        Ok(())
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test(start_paused = true)]
async fn synthetic_controller_allows_human_confirmation_after_thirty_seconds() {
    let now = unix_seconds();
    let offer_id = PairingOfferId::new();
    let host_private = StaticPrivateKey::from_fixture_bytes([1; 32]);
    let device_private = StaticPrivateKey::from_fixture_bytes([2; 32]);
    let offer = PairingOfferCore {
        version: CONTROLLER_V1,
        expires_at_unix_seconds: now + 60,
        nonce: PairingNonce([3; 32]),
        host_static_public_key: host_public_key_from_private(&host_private),
        capabilities: CapabilitySet::default().with(ControllerCapability::ObserveSessions),
    };
    let authority = Authority {
        offer_id,
        offer: offer.clone(),
        host_private,
        decision: HostPairingDecision::Confirm,
        decision_delay: Duration::from_secs(31),
        states: Mutex::new(Vec::new()),
        persisted: Mutex::new(None),
        acknowledged: Mutex::new(false),
    };
    let (mut device_stream, mut host_stream) = tokio::io::duplex(16 * 1024);
    let cancel = CancellationToken::new();
    let mut host_entropy = Entropy(Some(StaticPrivateKey::from_fixture_bytes([4; 32])));
    let host = pair_controller(&mut host_stream, &authority, &mut host_entropy, cancel);
    let device = async {
        let envelope = SshControllerPairingOffer::new(offer_id, &offer, 1, 3, 5).unwrap();
        pair_controller_client(
            &mut device_stream,
            envelope,
            device_private.clone(),
            StaticPrivateKey::from_fixture_bytes([5; 32]),
            ControllerDeviceId::new(),
            "Test iPhone".into(),
            |_| Ok(true),
            |_| Ok(()),
        )
        .await
        .unwrap()
    };

    let (peer, ack) = tokio::join!(host, device);
    let peer = peer.unwrap();
    assert_eq!(peer.device_id, ack.device_id);
    assert_eq!(ack.identity_generation, 1);
    assert_eq!(ack.revocation_epoch, 3);
    assert_eq!(
        peer.public_key,
        DevicePublicKey(device_public_key_from_private(&device_private).0)
    );
    assert_eq!(
        authority.states.lock().unwrap().as_slice(),
        &[
            PairingOfferState::Handshaking,
            PairingOfferState::SasReady,
            PairingOfferState::HostConfirmed,
        ]
    );
    assert_eq!(authority.persisted.lock().unwrap().as_ref(), Some(&peer));
    assert!(*authority.acknowledged.lock().unwrap());
}

#[tokio::test]
async fn rejected_sas_never_persists_or_acknowledges_a_device() {
    let now = unix_seconds();
    let offer_id = PairingOfferId::new();
    let host_private = StaticPrivateKey::from_fixture_bytes([11; 32]);
    let offer = PairingOfferCore {
        version: CONTROLLER_V1,
        expires_at_unix_seconds: now + 60,
        nonce: PairingNonce([12; 32]),
        host_static_public_key: host_public_key_from_private(&host_private),
        capabilities: CapabilitySet::default().with(ControllerCapability::ObserveSessions),
    };
    let authority = Authority {
        offer_id,
        offer: offer.clone(),
        host_private,
        decision: HostPairingDecision::Reject,
        decision_delay: Duration::ZERO,
        states: Mutex::new(Vec::new()),
        persisted: Mutex::new(None),
        acknowledged: Mutex::new(false),
    };
    let (mut device_stream, mut host_stream) = tokio::io::duplex(8 * 1024);
    let mut host_entropy = Entropy(Some(StaticPrivateKey::from_fixture_bytes([13; 32])));
    let cancel = CancellationToken::new();
    let host = pair_controller(&mut host_stream, &authority, &mut host_entropy, cancel);
    let device = async {
        PairingConnectRequest::new(offer_id)
            .write_to(&mut device_stream)
            .await
            .unwrap();
        let mut machine = PairingMachine::new_device_initiator(
            offer,
            StaticPrivateKey::from_fixture_bytes([14; 32]),
            StaticPrivateKey::from_fixture_bytes([15; 32]),
            0,
            now,
        )
        .unwrap();
        let hello = machine.write_next(0).unwrap();
        write_bounded_frame(&mut device_stream, hello.as_bytes(), 1_024)
            .await
            .unwrap();
        let proof = read_bounded_frame(&mut device_stream, 1_024).await.unwrap();
        machine.read_next(&proof, 0).unwrap();
        let proof = machine.write_next(0).unwrap();
        write_bounded_frame(&mut device_stream, proof.as_bytes(), 1_024)
            .await
            .unwrap();
    };
    let (result, ()) = tokio::join!(host, device);
    assert!(result.is_err());
    assert!(authority.persisted.lock().unwrap().is_none());
    assert!(!*authority.acknowledged.lock().unwrap());
    assert_eq!(
        authority.states.lock().unwrap().last(),
        Some(&PairingOfferState::Rejected)
    );
}
