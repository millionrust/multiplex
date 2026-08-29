use std::sync::Arc;

use termirust_controller_listener::{
    ListenerLaunchDescriptor, ProcessPairingDecision, SshHostPairingDecisionValue,
};
use termirust_domain::{
    ControllerCapabilities, ControllerCapability, ControllerDeviceId, PairingOfferId,
};
use termirust_store::ControllerDeviceRepository;

use crate::controller::devices::{ControllerDeviceService, NoControllerChannels};
use crate::controller::lan::{ControllerListenerProcess, ListenerProcessError};
use crate::controller::ssh_pairing::SshPairingBroker;

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

#[derive(Debug)]
pub(super) enum ControllerPairingCommandError {
    Listener(ListenerProcessError),
    Ssh(std::io::Error),
}

impl std::fmt::Display for ControllerPairingCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listener(error) => {
                write!(formatter, "listener pairing command failed: {error:?}")
            }
            Self::Ssh(error) => write!(formatter, "SSH pairing command failed: {error}"),
        }
    }
}

trait ControllerDeviceMutator: Send + Sync {
    fn mutate(
        &self,
        request: ControllerDeviceMutationRequest,
    ) -> Result<(), ControllerDeviceMutationError>;
}

trait ControllerListenerSpawner: Send + Sync {
    fn start(
        &self,
        descriptor: &ListenerLaunchDescriptor,
    ) -> Result<ControllerListenerProcess, ListenerProcessError>;
}

struct SystemControllerDeviceMutator;
struct SystemControllerListenerSpawner;

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

impl ControllerListenerSpawner for SystemControllerListenerSpawner {
    fn start(
        &self,
        descriptor: &ListenerLaunchDescriptor,
    ) -> Result<ControllerListenerProcess, ListenerProcessError> {
        ControllerListenerProcess::start(descriptor)
    }
}

#[derive(Clone)]
pub(super) struct ControllerCoordinator {
    device_mutator: Arc<dyn ControllerDeviceMutator>,
    listener_spawner: Arc<dyn ControllerListenerSpawner>,
}

impl Default for ControllerCoordinator {
    fn default() -> Self {
        Self {
            device_mutator: Arc::new(SystemControllerDeviceMutator),
            listener_spawner: Arc::new(SystemControllerListenerSpawner),
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

    pub fn start_listener(
        &self,
        descriptor: &ListenerLaunchDescriptor,
    ) -> Result<ControllerListenerProcess, ListenerProcessError> {
        self.listener_spawner.start(descriptor)
    }

    pub fn stop_listener(&self, process: &mut ControllerListenerProcess) {
        process.stop();
    }

    pub fn begin_pairing(
        &self,
        process: &mut ControllerListenerProcess,
    ) -> Result<(), ControllerPairingCommandError> {
        process
            .begin_pairing()
            .map_err(ControllerPairingCommandError::Listener)
    }

    pub fn decide_listener_pairing(
        &self,
        process: &mut ControllerListenerProcess,
        offer_id: PairingOfferId,
        decision: ProcessPairingDecision,
    ) -> Result<(), ControllerPairingCommandError> {
        process
            .decide_pairing(offer_id, decision)
            .map_err(ControllerPairingCommandError::Listener)
    }

    pub fn decide_ssh_pairing(
        &self,
        broker: &mut SshPairingBroker,
        offer_id: PairingOfferId,
        decision: ProcessPairingDecision,
    ) -> Result<(), ControllerPairingCommandError> {
        broker
            .decide(offer_id, ssh_pairing_decision(decision))
            .map_err(ControllerPairingCommandError::Ssh)
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

fn ssh_pairing_decision(decision: ProcessPairingDecision) -> SshHostPairingDecisionValue {
    match decision {
        ProcessPairingDecision::Confirm => SshHostPairingDecisionValue::Confirm,
        ProcessPairingDecision::Reject => SshHostPairingDecisionValue::Reject,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::io::BufReader;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    #[cfg(unix)]
    use std::thread;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    use termirust_controller_listener::{
        ListenerLaunchDescriptor, ProcessPairingDecision, SshHostPairingDecision,
        SshHostPairingDecisionValue, SshHostPairingPrompt,
    };
    use termirust_controller_security::StaticPrivateKey;
    use termirust_domain::{
        AddressFamily, ControllerCapabilities, ControllerCapability, ControllerDeviceAuthority,
        ControllerDeviceId, ControllerListenPolicy, ControllerNetworkRevision, ControllerPort,
        ControllerProtocolRange, DevicePublicKey, DeviceStoreRevision, DiscoveryPolicy,
        HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
        HostPublicKey, NetworkInterfaceId, PairedDeviceRecord, PairedDeviceStatus, PairingOfferId,
    };
    use termirust_store::ControllerDeviceRepository;

    use super::{
        ControllerCoordinator, ControllerDeviceMutation, ControllerDeviceMutationError,
        ControllerDeviceMutationRequest, ControllerDeviceMutator, ControllerListenerSpawner,
        SystemControllerListenerSpawner, ssh_pairing_decision, toggled_input_capabilities,
    };
    use crate::controller::lan::{ControllerListenerProcess, ListenerProcessError};
    #[cfg(unix)]
    use crate::controller::ssh_pairing::SshPairingBroker;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct MutationCall {
        repository_path: PathBuf,
        mutation: ControllerDeviceMutation,
    }

    struct RecordingMutator {
        calls: Arc<Mutex<Vec<MutationCall>>>,
        fail: bool,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ListenerStartCall {
        controller_root: PathBuf,
        project_root: PathBuf,
        session_data_root: PathBuf,
        runtime_parent: PathBuf,
        network_revision: ControllerNetworkRevision,
        policy: ControllerListenPolicy,
    }

    struct RecordingListenerSpawner {
        calls: Arc<Mutex<Vec<ListenerStartCall>>>,
        error: ListenerProcessError,
    }

    impl ControllerListenerSpawner for RecordingListenerSpawner {
        fn start(
            &self,
            descriptor: &ListenerLaunchDescriptor,
        ) -> Result<ControllerListenerProcess, ListenerProcessError> {
            self.calls.lock().unwrap().push(ListenerStartCall {
                controller_root: descriptor.controller_root.clone(),
                project_root: descriptor.project_root.clone(),
                session_data_root: descriptor.session_data_root.clone(),
                runtime_parent: descriptor.runtime_parent.clone(),
                network_revision: descriptor.network_revision,
                policy: descriptor.policy.clone(),
            });
            Err(self.error)
        }
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
            listener_spawner: Arc::new(SystemControllerListenerSpawner),
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
    fn listener_start_forwards_the_exact_descriptor_and_typed_error() {
        let fixture = tempfile::tempdir().unwrap();
        let policy = ControllerListenPolicy {
            enabled: true,
            interface_id: Some(NetworkInterfaceId::new("en-test").unwrap()),
            address_family: Some(AddressFamily::Ipv4),
            selected_address: Some("192.168.1.20".parse().unwrap()),
            port: Some(ControllerPort::user_fixed(49_152).unwrap()),
            discovery: DiscoveryPolicy::Off,
        };
        let descriptor = ListenerLaunchDescriptor::new(
            fixture.path().join("controller"),
            fixture.path().join("projects"),
            fixture.path().join("sessions"),
            fixture.path().join("runtime"),
            ControllerNetworkRevision::ZERO,
            policy.clone(),
            &StaticPrivateKey::from_fixture_bytes([7; 32]),
        )
        .unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let coordinator = ControllerCoordinator {
            device_mutator: Arc::new(RecordingMutator {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail: false,
            }),
            listener_spawner: Arc::new(RecordingListenerSpawner {
                calls: calls.clone(),
                error: ListenerProcessError::ReadinessTimeout,
            }),
        };

        assert!(matches!(
            coordinator.start_listener(&descriptor),
            Err(ListenerProcessError::ReadinessTimeout)
        ));
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [ListenerStartCall {
                controller_root: fixture.path().join("controller"),
                project_root: fixture.path().join("projects"),
                session_data_root: fixture.path().join("sessions"),
                runtime_parent: fixture.path().join("runtime"),
                network_revision: ControllerNetworkRevision::ZERO,
                policy,
            }]
        );
    }

    #[test]
    fn ssh_pairing_decision_mapping_is_exact() {
        assert_eq!(
            ssh_pairing_decision(ProcessPairingDecision::Confirm),
            SshHostPairingDecisionValue::Confirm
        );
        assert_eq!(
            ssh_pairing_decision(ProcessPairingDecision::Reject),
            SshHostPairingDecisionValue::Reject
        );
    }

    #[cfg(unix)]
    #[test]
    fn coordinator_dispatches_one_matching_ssh_pairing_decision() {
        let fixture = tempfile::tempdir().unwrap();
        let socket_path = fixture.path().join("pairing.sock");
        let mut broker = SshPairingBroker::bind(socket_path.clone()).unwrap();
        let offer_id = PairingOfferId::new();
        let client = thread::spawn(move || {
            let mut stream = UnixStream::connect(socket_path).unwrap();
            SshHostPairingPrompt::new(offer_id, "ABCD-1234".to_owned(), 500)
                .unwrap()
                .write(&mut stream)
                .unwrap();
            SshHostPairingDecision::read(&mut BufReader::new(stream)).unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if broker.poll().is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "pairing prompt was not delivered"
            );
            thread::sleep(Duration::from_millis(10));
        }

        ControllerCoordinator::default()
            .decide_ssh_pairing(&mut broker, offer_id, ProcessPairingDecision::Confirm)
            .unwrap();

        let decision = client.join().unwrap();
        assert_eq!(decision.offer_id, offer_id);
        assert_eq!(decision.decision, SshHostPairingDecisionValue::Confirm);
    }

    #[test]
    fn coordinator_module_has_no_ui_framework_dependency() {
        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("controller_coordinator.rs").contains(&forbidden_crate));
    }

    #[test]
    fn controller_coordinator_is_the_only_ui_controller_runtime_boundary() {
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
            assert!(
                !source.contains("ControllerListenerProcess::start"),
                "{} bypasses ControllerCoordinator listener start",
                path.display()
            );
            assert!(
                !source.contains("process.stop()"),
                "{} bypasses ControllerCoordinator listener stop",
                path.display()
            );
            assert!(
                !source.contains("process.begin_pairing()"),
                "{} bypasses ControllerCoordinator pairing begin",
                path.display()
            );
            assert!(
                !source.contains(".decide_pairing(offer_id, decision)"),
                "{} bypasses ControllerCoordinator listener pairing decision",
                path.display()
            );
            assert!(
                !source.contains("broker.decide("),
                "{} bypasses ControllerCoordinator SSH pairing decision",
                path.display()
            );
        }
    }
}
