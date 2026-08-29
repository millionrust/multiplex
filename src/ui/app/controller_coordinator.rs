use std::sync::Arc;

use termirust_domain::{ControllerCapabilities, ControllerCapability, ControllerDeviceId};
use termirust_store::ControllerDeviceRepository;

use crate::controller::devices::{ControllerDeviceService, NoControllerChannels};

#[derive(Clone, Debug, Eq, PartialEq)]
enum ControllerDeviceMutation {
    Rename {
        device_id: ControllerDeviceId,
        display_name: String,
    },
    SetCapabilities {
        device_id: ControllerDeviceId,
        capabilities: ControllerCapabilities,
    },
    Revoke {
        device_id: ControllerDeviceId,
    },
}

struct ControllerDeviceMutationRequest {
    repository: ControllerDeviceRepository,
    mutation: ControllerDeviceMutation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControllerDeviceMutationError {
    Service,
}

trait ControllerDeviceMutator: Send + Sync {
    fn mutate(
        &self,
        request: ControllerDeviceMutationRequest,
    ) -> Result<(), ControllerDeviceMutationError>;
}

struct SystemControllerDeviceMutator;

impl ControllerDeviceMutator for SystemControllerDeviceMutator {
    fn mutate(
        &self,
        request: ControllerDeviceMutationRequest,
    ) -> Result<(), ControllerDeviceMutationError> {
        let service =
            ControllerDeviceService::new(request.repository, Arc::new(NoControllerChannels));
        let result = match request.mutation {
            ControllerDeviceMutation::Rename {
                device_id,
                display_name,
            } => service.rename(device_id, display_name),
            ControllerDeviceMutation::SetCapabilities {
                device_id,
                capabilities,
            } => service.set_capabilities(device_id, capabilities),
            ControllerDeviceMutation::Revoke { device_id } => service.revoke(device_id),
        };
        result.map_err(|_| ControllerDeviceMutationError::Service)
    }
}

#[derive(Clone)]
pub(super) struct ControllerCoordinator {
    device_mutator: Arc<dyn ControllerDeviceMutator>,
}

impl Default for ControllerCoordinator {
    fn default() -> Self {
        Self {
            device_mutator: Arc::new(SystemControllerDeviceMutator),
        }
    }
}

impl ControllerCoordinator {
    pub fn rename_device(
        &self,
        repository: ControllerDeviceRepository,
        device_id: ControllerDeviceId,
        display_name: String,
    ) -> Result<(), ControllerDeviceMutationError> {
        self.device_mutator.mutate(ControllerDeviceMutationRequest {
            repository,
            mutation: ControllerDeviceMutation::Rename {
                device_id,
                display_name,
            },
        })
    }

    pub fn toggle_input(
        &self,
        repository: ControllerDeviceRepository,
        device_id: ControllerDeviceId,
        current: ControllerCapabilities,
    ) -> Result<(), ControllerDeviceMutationError> {
        self.device_mutator.mutate(ControllerDeviceMutationRequest {
            repository,
            mutation: ControllerDeviceMutation::SetCapabilities {
                device_id,
                capabilities: toggled_input_capabilities(current),
            },
        })
    }

    pub fn revoke_device(
        &self,
        repository: ControllerDeviceRepository,
        device_id: ControllerDeviceId,
    ) -> Result<(), ControllerDeviceMutationError> {
        self.device_mutator.mutate(ControllerDeviceMutationRequest {
            repository,
            mutation: ControllerDeviceMutation::Revoke { device_id },
        })
    }
}

fn toggled_input_capabilities(current: ControllerCapabilities) -> ControllerCapabilities {
    if current.contains(ControllerCapability::SendInput) {
        ControllerCapabilities::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
    } else {
        current
            .with(ControllerCapability::SendInput)
            .with(ControllerCapability::Resize)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use termirust_domain::{
        ControllerCapabilities, ControllerCapability, ControllerDeviceAuthority,
        ControllerDeviceId, ControllerProtocolRange, DevicePublicKey, DeviceStoreRevision,
        HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
        HostPublicKey, PairedDeviceRecord, PairedDeviceStatus, PairingOfferId,
    };
    use termirust_store::ControllerDeviceRepository;

    use super::{
        ControllerCoordinator, ControllerDeviceMutation, ControllerDeviceMutationError,
        ControllerDeviceMutationRequest, ControllerDeviceMutator, toggled_input_capabilities,
    };

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct MutationCall {
        repository_path: PathBuf,
        mutation: ControllerDeviceMutation,
    }

    struct RecordingMutator {
        calls: Arc<Mutex<Vec<MutationCall>>>,
        fail: bool,
    }

    impl ControllerDeviceMutator for RecordingMutator {
        fn mutate(
            &self,
            request: ControllerDeviceMutationRequest,
        ) -> Result<(), ControllerDeviceMutationError> {
            self.calls.lock().unwrap().push(MutationCall {
                repository_path: request.repository.metadata_path(),
                mutation: request.mutation,
            });
            if self.fail {
                Err(ControllerDeviceMutationError::Service)
            } else {
                Ok(())
            }
        }
    }

    fn coordinator(calls: Arc<Mutex<Vec<MutationCall>>>, fail: bool) -> ControllerCoordinator {
        ControllerCoordinator {
            device_mutator: Arc::new(RecordingMutator { calls, fail }),
        }
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
                display_name: "Phone".to_owned(),
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
    fn device_mutations_forward_exact_repository_ids_values_and_failures() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let repository_path = repository.metadata_path();
        let device_id = ControllerDeviceId::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recording_coordinator = coordinator(calls.clone(), false);
        let current = ControllerCapabilities::default().with(ControllerCapability::ObserveSessions);

        recording_coordinator
            .rename_device(repository.clone(), device_id, "Exact phone".to_owned())
            .unwrap();
        recording_coordinator
            .toggle_input(repository.clone(), device_id, current)
            .unwrap();
        recording_coordinator
            .revoke_device(repository.clone(), device_id)
            .unwrap();

        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [
                MutationCall {
                    repository_path: repository_path.clone(),
                    mutation: ControllerDeviceMutation::Rename {
                        device_id,
                        display_name: "Exact phone".to_owned(),
                    },
                },
                MutationCall {
                    repository_path: repository_path.clone(),
                    mutation: ControllerDeviceMutation::SetCapabilities {
                        device_id,
                        capabilities: current
                            .with(ControllerCapability::SendInput)
                            .with(ControllerCapability::Resize),
                    },
                },
                MutationCall {
                    repository_path,
                    mutation: ControllerDeviceMutation::Revoke { device_id },
                },
            ]
        );

        let failure = coordinator(Arc::new(Mutex::new(Vec::new())), true);
        assert_eq!(
            failure.revoke_device(repository, device_id),
            Err(ControllerDeviceMutationError::Service)
        );
    }

    #[test]
    fn input_toggle_preserves_the_existing_enable_and_disable_policy() {
        let observe_only =
            ControllerCapabilities::default().with(ControllerCapability::ObserveSessions);
        assert_eq!(
            toggled_input_capabilities(observe_only),
            observe_only
                .with(ControllerCapability::SendInput)
                .with(ControllerCapability::Resize)
        );

        let enabled = observe_only
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput)
            .with(ControllerCapability::Resize)
            .with(ControllerCapability::RespondToApproval);
        assert_eq!(
            toggled_input_capabilities(enabled),
            ControllerCapabilities::default()
                .with(ControllerCapability::ObserveSessions)
                .with(ControllerCapability::AttachOutput)
        );
    }

    #[test]
    fn system_coordinator_applies_rename_capability_and_revoke_mutations() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let device_id = ControllerDeviceId::new();
        repository
            .save(DeviceStoreRevision::ZERO, authority(device_id))
            .unwrap();
        let coordinator = ControllerCoordinator::default();

        coordinator
            .rename_device(repository.clone(), device_id, "Renamed phone".to_owned())
            .unwrap();
        let renamed = repository.load().unwrap().authority.devices.remove(0);
        assert_eq!(renamed.display_name, "Renamed phone");

        coordinator
            .toggle_input(repository.clone(), device_id, renamed.capabilities)
            .unwrap();
        let enabled = repository.load().unwrap().authority.devices.remove(0);
        assert!(
            enabled
                .capabilities
                .contains(ControllerCapability::SendInput)
        );
        assert!(enabled.capabilities.contains(ControllerCapability::Resize));

        coordinator
            .revoke_device(repository.clone(), device_id)
            .unwrap();
        assert_eq!(
            repository.load().unwrap().authority.devices[0].status,
            PairedDeviceStatus::Revoked
        );
    }

    #[test]
    fn coordinator_module_has_no_ui_framework_dependency() {
        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("controller_coordinator.rs").contains(&forbidden_crate));
    }

    #[test]
    fn controller_coordinator_is_the_only_ui_device_service_boundary() {
        let app_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ui/app");
        for entry in std::fs::read_dir(app_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs")
                || path.file_name().and_then(|name| name.to_str())
                    == Some("controller_coordinator.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(
                !source.contains("ControllerDeviceService::new"),
                "{} bypasses ControllerCoordinator device mutations",
                path.display()
            );
        }
    }
}
