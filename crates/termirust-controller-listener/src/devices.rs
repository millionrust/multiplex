use std::fmt;
use std::sync::Arc;

use termirust_domain::{
    ControllerAuthorizationDecision, ControllerAuthorizationRequest, ControllerCapabilities,
    ControllerDeviceError, ControllerDeviceId, DeviceStoreRevision, PairedDeviceStatus,
};
use termirust_store::{
    ControllerDeviceRepository, ControllerDeviceSnapshot, ControllerDeviceStoreError,
};

pub trait ControllerChannelCloser: Send + Sync {
    fn close_device_channels(&self, device_id: ControllerDeviceId);
    fn close_all_channels(&self);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoControllerChannels;

impl ControllerChannelCloser for NoControllerChannels {
    fn close_device_channels(&self, _device_id: ControllerDeviceId) {}

    fn close_all_channels(&self) {}
}

#[derive(Debug)]
pub enum ControllerDeviceServiceError {
    Store(ControllerDeviceStoreError),
    Domain(ControllerDeviceError),
}

impl fmt::Display for ControllerDeviceServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Domain(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ControllerDeviceServiceError {}

impl From<ControllerDeviceStoreError> for ControllerDeviceServiceError {
    fn from(error: ControllerDeviceStoreError) -> Self {
        Self::Store(error)
    }
}

pub struct ControllerDeviceService {
    repository: ControllerDeviceRepository,
    channels: Arc<dyn ControllerChannelCloser>,
}

impl ControllerDeviceService {
    pub fn new(
        repository: ControllerDeviceRepository,
        channels: Arc<dyn ControllerChannelCloser>,
    ) -> Self {
        Self {
            repository,
            channels,
        }
    }

    pub fn rename(
        &self,
        device_id: ControllerDeviceId,
        display_name: String,
    ) -> Result<(), ControllerDeviceServiceError> {
        let snapshot = self.repository.load()?;
        self.repository.update(snapshot.revision, |authority| {
            let mut found = false;
            for device in authority
                .devices
                .iter_mut()
                .filter(|device| device.device_id == device_id)
            {
                device.display_name = display_name.clone();
                device.validate()?;
                found = true;
            }
            found
                .then_some(())
                .ok_or(ControllerDeviceError::DeviceNotFound)
        })?;
        Ok(())
    }

    pub fn set_capabilities(
        &self,
        device_id: ControllerDeviceId,
        capabilities: ControllerCapabilities,
    ) -> Result<(), ControllerDeviceServiceError> {
        ControllerCapabilities::from_bits(capabilities.bits())
            .map_err(ControllerDeviceServiceError::Domain)?;
        let snapshot = self.repository.load()?;
        self.repository.update(snapshot.revision, |authority| {
            let mut found = false;
            for device in authority
                .devices
                .iter_mut()
                .filter(|device| device.device_id == device_id)
            {
                if device.status != PairedDeviceStatus::Revoked {
                    device.capabilities = capabilities;
                    device.validate()?;
                    found = true;
                }
            }
            found
                .then_some(())
                .ok_or(ControllerDeviceError::DeviceNotFound)
        })?;
        Ok(())
    }

    pub fn revoke(
        &self,
        device_id: ControllerDeviceId,
    ) -> Result<(), ControllerDeviceServiceError> {
        let snapshot = self.repository.load()?;
        self.revoke_at_revision(device_id, snapshot.revision)?;
        Ok(())
    }

    pub fn revoke_at_revision(
        &self,
        device_id: ControllerDeviceId,
        expected_revision: DeviceStoreRevision,
    ) -> Result<ControllerDeviceSnapshot, ControllerDeviceServiceError> {
        let snapshot = self.repository.update(expected_revision, |authority| {
            authority.revoke_device(device_id).map(|_| ())
        })?;
        self.channels.close_device_channels(device_id);
        Ok(snapshot)
    }

    pub fn authorize(
        &self,
        request: ControllerAuthorizationRequest,
    ) -> Result<ControllerAuthorizationDecision, ControllerDeviceServiceError> {
        Ok(self.repository.load()?.authority.authorize(request))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use termirust_domain::{
        ControllerCapability, ControllerDeviceAuthority, ControllerProtocolRange, DevicePublicKey,
        HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
        HostPublicKey, PairedDeviceRecord, PairingOfferId,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingChannels {
        closed: Mutex<Vec<ControllerDeviceId>>,
    }

    impl ControllerChannelCloser for RecordingChannels {
        fn close_device_channels(&self, device_id: ControllerDeviceId) {
            self.closed.lock().unwrap().push(device_id);
        }

        fn close_all_channels(&self) {}
    }

    fn authority(device_id: ControllerDeviceId) -> ControllerDeviceAuthority {
        ControllerDeviceAuthority {
            identity: Some(HostIdentityPublic::new(
                HostIdentityGeneration::INITIAL,
                HostPublicKey([1; 32]),
            )),
            secret_ref: Some(HostIdentitySecretRef::new("identity:test").unwrap()),
            state: HostIdentityState::Ready,
            devices: vec![PairedDeviceRecord {
                device_id,
                public_key: DevicePublicKey([2; 32]),
                display_name: "Phone".into(),
                capabilities: ControllerCapabilities::default()
                    .with(ControllerCapability::ObserveSessions),
                protocol_range: ControllerProtocolRange::V1,
                created_at: 1,
                last_seen_at: None,
                revocation_epoch: 0,
                identity_generation: HostIdentityGeneration::INITIAL,
                status: PairedDeviceStatus::Offline,
                source_offer_id: PairingOfferId::new(),
            }],
            ..ControllerDeviceAuthority::default()
        }
    }

    #[test]
    fn controller_devices_revoke_commits_before_closing_channels() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let device_id = ControllerDeviceId::new();
        repository
            .save(DeviceStoreRevision::ZERO, authority(device_id))
            .unwrap();
        let channels = Arc::new(RecordingChannels::default());
        let service = ControllerDeviceService::new(repository.clone(), channels.clone());
        let before = repository.load().unwrap();
        let result = service
            .revoke_at_revision(device_id, before.revision)
            .unwrap();
        assert_eq!(
            result.authority.devices[0].status,
            PairedDeviceStatus::Revoked
        );
        assert_eq!(*channels.closed.lock().unwrap(), vec![device_id]);
    }

    #[test]
    fn stale_device_revocation_does_not_close_channels() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let device_id = ControllerDeviceId::new();
        repository
            .save(DeviceStoreRevision::ZERO, authority(device_id))
            .unwrap();
        let channels = Arc::new(RecordingChannels::default());
        let service = ControllerDeviceService::new(repository, channels.clone());
        assert!(matches!(
            service.revoke_at_revision(device_id, DeviceStoreRevision::ZERO),
            Err(ControllerDeviceServiceError::Store(
                ControllerDeviceStoreError::StaleRevision { .. }
            ))
        ));
        assert!(channels.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn capability_updates_reconcile_legacy_duplicate_device_ids() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let device_id = ControllerDeviceId::new();
        let mut saved_authority = authority(device_id);
        let mut repaired = saved_authority.devices[0].clone();
        repaired.public_key = DevicePublicKey([3; 32]);
        repaired.created_at = 2;
        repaired.source_offer_id = PairingOfferId::new();
        saved_authority.devices.push(repaired);
        repository
            .save(DeviceStoreRevision::ZERO, saved_authority)
            .unwrap();
        let service =
            ControllerDeviceService::new(repository.clone(), Arc::new(NoControllerChannels));
        let interactive = ControllerCapabilities::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput)
            .with(ControllerCapability::Resize);

        service.set_capabilities(device_id, interactive).unwrap();

        let snapshot = repository.load().unwrap();
        let matching: Vec<_> = snapshot
            .authority
            .devices
            .iter()
            .filter(|device| device.device_id == device_id)
            .collect();
        assert_eq!(matching.len(), 2);
        assert!(
            matching
                .iter()
                .all(|device| device.capabilities == interactive)
        );
    }
}
