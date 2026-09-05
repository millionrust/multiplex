use std::fmt;

use termirust_controller_security::{
    CONTROLLER_V1, CapabilitySet, ControllerSecurityError, PairingMachine, PairingNonce,
    PairingOfferCore, PairingState, RevocationEpoch, SasCode, StaticPrivateKey,
};
use termirust_domain::{
    ControllerCapabilities, ControllerDeviceError, ControllerDeviceId, DevicePublicKey,
    HostIdentityState, PairingOfferId, PairingOfferRecord, PairingOfferState,
};
use termirust_store::{ControllerDeviceRepository, ControllerDeviceStoreError};

use super::host_identity::LoadedHostIdentity;

#[derive(Debug)]
pub enum PairingServiceError {
    Store(ControllerDeviceStoreError),
    Domain(ControllerDeviceError),
    Security(ControllerSecurityError),
    IdentityUnavailable,
    State,
}

impl fmt::Display for PairingServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Domain(error) => error.fmt(formatter),
            Self::Security(error) => error.fmt(formatter),
            Self::IdentityUnavailable => formatter.write_str("Host identity is unavailable"),
            Self::State => formatter.write_str("pairing state is invalid"),
        }
    }
}

impl std::error::Error for PairingServiceError {}

impl From<ControllerDeviceStoreError> for PairingServiceError {
    fn from(error: ControllerDeviceStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ControllerDeviceError> for PairingServiceError {
    fn from(error: ControllerDeviceError) -> Self {
        Self::Domain(error)
    }
}

impl From<ControllerSecurityError> for PairingServiceError {
    fn from(error: ControllerSecurityError) -> Self {
        Self::Security(error)
    }
}

pub trait PairingAckSink {
    fn send_ack(&mut self, device_id: ControllerDeviceId) -> Result<(), ()>;
}

pub struct HostPairingSession {
    offer_id: PairingOfferId,
    machine: Option<PairingMachine>,
}

impl fmt::Debug for HostPairingSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostPairingSession")
            .field("offer_id", &self.offer_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingCompletion {
    Acknowledged(ControllerDeviceId),
    Uncertain(ControllerDeviceId),
}

#[derive(Clone)]
pub struct PairingCoordinator {
    repository: ControllerDeviceRepository,
}

impl PairingCoordinator {
    pub fn new(repository: ControllerDeviceRepository) -> Self {
        Self { repository }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_offer(
        &self,
        offer_id: PairingOfferId,
        nonce: [u8; 32],
        now_unix_seconds: u64,
        expires_at_unix_seconds: u64,
        capabilities: ControllerCapabilities,
        route_candidates: Vec<String>,
    ) -> Result<PairingOfferRecord, PairingServiceError> {
        let snapshot = self.repository.load()?;
        let mut created = None;
        self.repository.update(snapshot.revision, |authority| {
            created = Some(authority.create_offer(
                offer_id,
                nonce,
                now_unix_seconds,
                expires_at_unix_seconds,
                capabilities,
                route_candidates,
            )?);
            Ok(())
        })?;
        created.ok_or(PairingServiceError::State)
    }

    pub fn begin(
        &self,
        offer_id: PairingOfferId,
        identity: &LoadedHostIdentity,
        ephemeral_private: StaticPrivateKey,
        now_millis: u64,
        now_unix_seconds: u64,
    ) -> Result<HostPairingSession, PairingServiceError> {
        if identity.state != HostIdentityState::Ready {
            return Err(PairingServiceError::IdentityUnavailable);
        }
        let host_private = identity
            .static_private_key()
            .ok_or(PairingServiceError::IdentityUnavailable)?;
        let snapshot = self.repository.load()?;
        let offer = snapshot
            .authority
            .offers
            .iter()
            .find(|offer| offer.offer_id == offer_id)
            .cloned()
            .ok_or(ControllerDeviceError::OfferNotFound)?;
        let core = security_offer(&offer)?;
        self.set_offer_state(offer_id, PairingOfferState::Handshaking)?;
        Ok(HostPairingSession {
            offer_id,
            machine: Some(PairingMachine::new_host_responder(
                core,
                host_private,
                ephemeral_private,
                now_millis,
                now_unix_seconds,
            )?),
        })
    }

    pub fn read_device_bytes(
        &self,
        session: &mut HostPairingSession,
        bytes: &[u8],
        now_millis: u64,
    ) -> Result<(), PairingServiceError> {
        let machine = session.machine.as_mut().ok_or(PairingServiceError::State)?;
        machine.read_next(bytes, now_millis)?;
        if machine.state() == PairingState::SasReady {
            self.set_offer_state(session.offer_id, PairingOfferState::SasReady)?;
        }
        Ok(())
    }

    pub fn write_host_bytes(
        &self,
        session: &mut HostPairingSession,
        now_millis: u64,
    ) -> Result<Vec<u8>, PairingServiceError> {
        let machine = session.machine.as_mut().ok_or(PairingServiceError::State)?;
        Ok(machine.write_next(now_millis)?.as_bytes().to_vec())
    }

    pub fn sas<'a>(
        &self,
        session: &'a HostPairingSession,
    ) -> Result<&'a SasCode, PairingServiceError> {
        session
            .machine
            .as_ref()
            .and_then(PairingMachine::sas)
            .ok_or(PairingServiceError::State)
    }

    pub fn reject(&self, mut session: HostPairingSession) -> Result<(), PairingServiceError> {
        if let Some(machine) = session.machine.as_mut() {
            let _ = machine.cancel();
        }
        self.set_offer_state(session.offer_id, PairingOfferState::Rejected)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn confirm_and_persist(
        &self,
        mut session: HostPairingSession,
        compared_sas: &SasCode,
        device_id: ControllerDeviceId,
        display_name: String,
        now_unix_seconds: u64,
        ack: &mut dyn PairingAckSink,
    ) -> Result<PairingCompletion, PairingServiceError> {
        self.set_offer_state(session.offer_id, PairingOfferState::HostConfirmed)?;
        let snapshot = self.repository.load()?;
        let epoch = snapshot.authority.revocation_epoch;
        let confirmed = match session
            .machine
            .take()
            .ok_or(PairingServiceError::State)?
            .confirm(compared_sas, RevocationEpoch(epoch))
        {
            Ok(confirmed) => confirmed,
            Err(error) => {
                self.set_offer_state(session.offer_id, PairingOfferState::Rejected)?;
                return Err(error.into());
            }
        };
        let device_key = DevicePublicKey(confirmed.device_key.0);
        let mut persisted_id = None;
        let snapshot = self.repository.load()?;
        self.repository.update(snapshot.revision, |authority| {
            persisted_id = Some(
                authority
                    .persist_pairing(
                        session.offer_id,
                        device_id,
                        device_key,
                        display_name,
                        now_unix_seconds,
                    )?
                    .device_id,
            );
            Ok(())
        })?;
        let persisted_id = persisted_id.ok_or(PairingServiceError::State)?;
        if ack.send_ack(persisted_id).is_err() {
            self.set_offer_state(session.offer_id, PairingOfferState::Uncertain)?;
            return Ok(PairingCompletion::Uncertain(persisted_id));
        }
        let snapshot = self.repository.load()?;
        self.repository.update(snapshot.revision, |authority| {
            authority
                .acknowledge_pairing(session.offer_id, device_key)
                .map(|_| ())
        })?;
        Ok(PairingCompletion::Acknowledged(persisted_id))
    }

    pub fn reconcile(
        &self,
        offer_id: PairingOfferId,
        device_key: DevicePublicKey,
    ) -> Result<ControllerDeviceId, PairingServiceError> {
        let snapshot = self.repository.load()?;
        let mut device_id = None;
        self.repository.update(snapshot.revision, |authority| {
            device_id = Some(authority.acknowledge_pairing(offer_id, device_key)?);
            Ok(())
        })?;
        device_id.ok_or(PairingServiceError::State)
    }

    fn set_offer_state(
        &self,
        offer_id: PairingOfferId,
        state: PairingOfferState,
    ) -> Result<(), PairingServiceError> {
        let snapshot = self.repository.load()?;
        self.repository.update(snapshot.revision, |authority| {
            let offer = authority
                .offers
                .iter_mut()
                .find(|offer| offer.offer_id == offer_id)
                .ok_or(ControllerDeviceError::OfferNotFound)?;
            offer.state = state;
            Ok(())
        })?;
        Ok(())
    }
}

fn security_offer(offer: &PairingOfferRecord) -> Result<PairingOfferCore, PairingServiceError> {
    Ok(PairingOfferCore {
        version: CONTROLLER_V1,
        expires_at_unix_seconds: offer.expires_at,
        nonce: PairingNonce(offer.nonce),
        host_static_public_key: termirust_controller_security::HostStaticPublicKey(
            offer.identity.public_key.0,
        ),
        capabilities: CapabilitySet::from_bits(offer.capabilities.bits())?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use termirust_controller_security::{
        ControllerCapability as SecurityCapability, PairingMachine,
    };
    use termirust_domain::{ControllerCapability, DeviceStoreRevision, HostIdentitySecretRef};

    use super::super::host_identity::{
        HostIdentityError, HostIdentitySecret, HostIdentityService, IdentityEntropy, SecretStore,
        SecretStoreError,
    };
    use super::*;

    #[derive(Clone, Default)]
    struct Secrets(Arc<Mutex<HashMap<String, HostIdentitySecret>>>);

    impl SecretStore for Secrets {
        fn put(
            &self,
            reference: &HostIdentitySecretRef,
            secret: &HostIdentitySecret,
        ) -> Result<(), SecretStoreError> {
            self.0
                .lock()
                .unwrap()
                .insert(reference.expose_reference().into(), secret.clone());
            Ok(())
        }

        fn get(
            &self,
            reference: &HostIdentitySecretRef,
        ) -> Result<HostIdentitySecret, SecretStoreError> {
            self.0
                .lock()
                .unwrap()
                .get(reference.expose_reference())
                .cloned()
                .ok_or(SecretStoreError::Missing)
        }

        fn delete(&self, reference: &HostIdentitySecretRef) -> Result<bool, SecretStoreError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .remove(reference.expose_reference())
                .is_some())
        }
    }

    #[derive(Clone, Copy)]
    struct Entropy;

    impl IdentityEntropy for Entropy {
        fn secret(&self) -> Result<HostIdentitySecret, HostIdentityError> {
            Ok(HostIdentitySecret::from_bytes([7; 32]))
        }

        fn reference(&self) -> Result<HostIdentitySecretRef, HostIdentityError> {
            Ok(HostIdentitySecretRef::new("identity:pairing").unwrap())
        }
    }

    struct Ack(bool);

    impl PairingAckSink for Ack {
        fn send_ack(&mut self, _device_id: ControllerDeviceId) -> Result<(), ()> {
            if self.0 { Ok(()) } else { Err(()) }
        }
    }

    fn setup(
        path: &std::path::Path,
    ) -> (
        PairingCoordinator,
        LoadedHostIdentity,
        PairingOfferId,
        PairingOfferCore,
    ) {
        let repository = ControllerDeviceRepository::open(path).unwrap();
        let identity = HostIdentityService::new(repository.clone(), Secrets::default(), Entropy)
            .load_or_create()
            .unwrap();
        let coordinator = PairingCoordinator::new(repository.clone());
        let offer_id = PairingOfferId::new();
        let capabilities =
            ControllerCapabilities::default().with(ControllerCapability::ObserveSessions);
        let record = coordinator
            .create_offer(
                offer_id,
                [9; 32],
                100,
                160,
                capabilities,
                vec!["synthetic:memory".into()],
            )
            .unwrap();
        (
            coordinator,
            identity,
            offer_id,
            security_offer(&record).unwrap(),
        )
    }

    fn exchange(
        coordinator: &PairingCoordinator,
        identity: &LoadedHostIdentity,
        offer_id: PairingOfferId,
        offer: PairingOfferCore,
    ) -> (HostPairingSession, PairingMachine, StaticPrivateKey) {
        let device_static = StaticPrivateKey::from_bytes([11; 32]);
        let mut device = PairingMachine::new_device_initiator(
            offer,
            device_static.clone(),
            StaticPrivateKey::from_bytes([12; 32]),
            1_000,
            100,
        )
        .unwrap();
        let mut host = coordinator
            .begin(
                offer_id,
                identity,
                StaticPrivateKey::from_bytes([13; 32]),
                1_000,
                100,
            )
            .unwrap();
        let first = device.write_next(1_001).unwrap();
        coordinator
            .read_device_bytes(&mut host, first.as_bytes(), 1_002)
            .unwrap();
        let second = coordinator.write_host_bytes(&mut host, 1_003).unwrap();
        device.read_next(&second, 1_004).unwrap();
        let third = device.write_next(1_005).unwrap();
        coordinator
            .read_device_bytes(&mut host, third.as_bytes(), 1_006)
            .unwrap();
        (host, device, device_static)
    }

    #[test]
    fn controller_pairing_synthetic_transport_persists_before_ack_and_reconciles_lost_ack() {
        let fixture = tempfile::tempdir().unwrap();
        let (coordinator, identity, offer_id, offer) = setup(fixture.path());
        let (host, device, device_static) = exchange(&coordinator, &identity, offer_id, offer);
        assert_eq!(
            coordinator.sas(&host).unwrap().as_str(),
            device.sas().unwrap().as_str()
        );
        let sas = coordinator.sas(&host).unwrap().clone();
        let device_id = ControllerDeviceId::new();
        let completion = coordinator
            .confirm_and_persist(
                host,
                &sas,
                device_id,
                "Test phone".into(),
                101,
                &mut Ack(false),
            )
            .unwrap();
        assert_eq!(completion, PairingCompletion::Uncertain(device_id));
        let device_key =
            termirust_controller_security::device_public_key_from_private(&device_static);
        assert_eq!(
            coordinator
                .reconcile(offer_id, DevicePublicKey(device_key.0))
                .unwrap(),
            device_id
        );
        let authority = ControllerDeviceRepository::open(fixture.path())
            .unwrap()
            .load()
            .unwrap()
            .authority;
        assert_eq!(authority.devices.len(), 1);
        assert_eq!(authority.offers[0].state, PairingOfferState::Acknowledged);
    }

    #[test]
    fn controller_pairing_reject_consumes_offer() {
        let fixture = tempfile::tempdir().unwrap();
        let (coordinator, identity, offer_id, offer) = setup(fixture.path());
        let (host, _, _) = exchange(&coordinator, &identity, offer_id, offer);
        coordinator.reject(host).unwrap();
        assert_eq!(
            ControllerDeviceRepository::open(fixture.path())
                .unwrap()
                .load()
                .unwrap()
                .authority
                .offers[0]
                .state,
            PairingOfferState::Rejected
        );
    }

    #[test]
    fn controller_pairing_has_no_network_route_and_uses_closed_capabilities() {
        let fixture = tempfile::tempdir().unwrap();
        let (_, _, _, offer) = setup(fixture.path());
        assert_eq!(
            offer.capabilities,
            CapabilitySet::default().with(SecurityCapability::ObserveSessions)
        );
        assert_eq!(
            ControllerDeviceRepository::open(fixture.path())
                .unwrap()
                .load()
                .unwrap()
                .revision,
            DeviceStoreRevision::new(2)
        );
    }
}
