use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use termirust_controller_security::{
    CONTROLLER_V1, CapabilitySet, DeviceStaticPublicKey, PairingNonce, PairingOfferCore,
    StaticPrivateKey,
};
use termirust_domain::{
    AddressFamily, AuthenticatedPeer, ControllerCapabilities, ControllerCapability,
    ControllerDeviceId, ControllerListenPolicy, ControllerNetworkRevision, DevicePublicKey,
    DiscoveryPolicy, PairingOfferId, PairingOfferState, RouteCandidate,
};
use termirust_store::{
    ControllerDeviceRepository, ControllerNetworkRepository, ProjectRepository, SessionRepository,
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize as _;

use crate::{
    AuthoritySnapshot, ControllerAuthorityProvider, ControllerPairingAuthority,
    FirewallObserver as _, HostBackendFactory, HostPairingDecision, ListenerControlCommand,
    ListenerError, ListenerErrorCode, ListenerProcessEvent, ListenerRuntime, ListenerServices,
    PairingAuthoritySnapshot, ProcessPairingDecision, SourceBucketKey, SystemBinder,
    SystemFirewallObserver, SystemGeneratedPortSource, SystemInterfaceProvider,
    bind_selected_route,
};

const LAUNCH_FORMAT_VERSION: u16 = 1;
const MAX_LAUNCH_DESCRIPTOR_BYTES: u64 = 32 * 1024;
const PAIRING_OFFER_LIFETIME_SECONDS: u64 = 5 * 60;

#[derive(Clone)]
struct ListenerEventSink {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl ListenerEventSink {
    fn new(writer: impl Write + Send + 'static) -> Self {
        Self {
            writer: Arc::new(Mutex::new(Box::new(writer))),
        }
    }

    fn send(&self, event: &ListenerProcessEvent) -> Result<(), ListenerError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        event.write(&mut *writer)
    }
}

#[derive(Clone, Default)]
struct PairingDecisionBroker {
    pending: Arc<Mutex<HashMap<PairingOfferId, tokio::sync::oneshot::Sender<HostPairingDecision>>>>,
}

impl PairingDecisionBroker {
    fn register(
        &self,
        offer_id: PairingOfferId,
    ) -> Result<tokio::sync::oneshot::Receiver<HostPairingDecision>, ListenerError> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        match pending.entry(offer_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(sender);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
            }
        }
        Ok(receiver)
    }

    fn resolve(
        &self,
        offer_id: PairingOfferId,
        decision: HostPairingDecision,
    ) -> Result<(), ListenerError> {
        let sender = self
            .pending
            .lock()
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?
            .remove(&offer_id)
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        sender
            .send(decision)
            .map_err(|_| ListenerError::new(ListenerErrorCode::Cancelled))
    }

    fn remove(&self, offer_id: PairingOfferId) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&offer_id);
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerLaunchDescriptor {
    pub format_version: u16,
    pub controller_root: PathBuf,
    pub project_root: PathBuf,
    pub session_data_root: PathBuf,
    pub runtime_parent: PathBuf,
    pub network_revision: ControllerNetworkRevision,
    pub policy: ControllerListenPolicy,
    host_private: [u8; 32],
}

impl ListenerLaunchDescriptor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        controller_root: PathBuf,
        project_root: PathBuf,
        session_data_root: PathBuf,
        runtime_parent: PathBuf,
        network_revision: ControllerNetworkRevision,
        policy: ControllerListenPolicy,
        host_private: &StaticPrivateKey,
    ) -> Result<Self, ListenerError> {
        let descriptor = Self {
            format_version: LAUNCH_FORMAT_VERSION,
            controller_root,
            project_root,
            session_data_root,
            runtime_parent,
            network_revision,
            policy,
            host_private: host_private.copy_for_process_handoff(),
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn read(reader: impl BufRead) -> Result<Self, ListenerError> {
        let mut bytes = Vec::new();
        reader
            .take(MAX_LAUNCH_DESCRIPTOR_BYTES + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(ListenerError::from)?;
        if bytes.is_empty()
            || bytes.len() as u64 > MAX_LAUNCH_DESCRIPTOR_BYTES
            || bytes.last() != Some(&b'\n')
        {
            bytes.zeroize();
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        let descriptor = serde_json::from_slice::<Self>(&bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame));
        bytes.zeroize();
        let descriptor = descriptor?;
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn write(&self, mut writer: impl Write) -> Result<(), ListenerError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        if bytes.len() as u64 >= MAX_LAUNCH_DESCRIPTOR_BYTES {
            bytes.zeroize();
            return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
        }
        bytes.push(b'\n');
        let result = writer
            .write_all(&bytes)
            .and_then(|()| writer.flush())
            .map_err(ListenerError::from);
        bytes.zeroize();
        result
    }

    fn validate(&self) -> Result<(), ListenerError> {
        if self.format_version != LAUNCH_FORMAT_VERSION || self.host_private == [0; 32] {
            return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
        }
        self.policy.validate()?;
        if !self.policy.enabled
            || [
                &self.controller_root,
                &self.project_root,
                &self.session_data_root,
                &self.runtime_parent,
            ]
            .into_iter()
            .any(|path| !safe_absolute_path(path))
        {
            return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
        }
        Ok(())
    }
}

impl fmt::Debug for ListenerLaunchDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ListenerLaunchDescriptor")
            .field("format_version", &self.format_version)
            .field("paths", &"[REDACTED]")
            .field("network_revision", &self.network_revision)
            .field("policy", &"[REDACTED]")
            .field("host_private", &"[REDACTED]")
            .finish()
    }
}

impl Drop for ListenerLaunchDescriptor {
    fn drop(&mut self) {
        self.host_private.zeroize();
    }
}

struct RepositoryAuthority {
    repository: ControllerDeviceRepository,
    host_private: StaticPrivateKey,
    events: ListenerEventSink,
    decisions: PairingDecisionBroker,
}

impl RepositoryAuthority {
    fn create_offer(&self, route: &RouteCandidate) -> Result<ListenerProcessEvent, ListenerError> {
        let mut nonce = [0; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce)
            .map_err(|_| ListenerError::new(ListenerErrorCode::RandomUnavailable))?;
        let offer_id = PairingOfferId::new();
        let now = unix_seconds();
        let expires_at = now.saturating_add(PAIRING_OFFER_LIFETIME_SECONDS);
        let capabilities = ControllerCapabilities::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput);
        let snapshot = self
            .repository
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let mut record = None;
        let saved = self
            .repository
            .update(snapshot.revision, |authority| {
                record = Some(authority.create_offer(
                    offer_id,
                    nonce,
                    now,
                    expires_at,
                    capabilities,
                    vec![format!("{}:{}", route.address, route.port.value())],
                )?);
                Ok(())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let record =
            record.ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let core = PairingOfferCore {
            version: CONTROLLER_V1,
            expires_at_unix_seconds: record.expires_at,
            nonce: PairingNonce(record.nonce),
            host_static_public_key: termirust_controller_security::HostStaticPublicKey(
                record.identity.public_key.0,
            ),
            capabilities: CapabilitySet::from_bits(record.capabilities.bits())
                .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?,
        };
        let offer = crate::ControllerPairingOffer::new(
            offer_id,
            route,
            &core,
            record.identity.generation.get(),
            saved.authority.revocation_epoch,
            saved.authority.session_generation,
        )?;
        Ok(ListenerProcessEvent::pairing_offer(
            offer_id,
            offer.encode_text()?,
            expires_at,
        ))
    }
}

impl ControllerAuthorityProvider for RepositoryAuthority {
    fn snapshot(&self) -> Result<AuthoritySnapshot, ListenerError> {
        let authority = self
            .repository
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?
            .authority;
        Ok(AuthoritySnapshot {
            authority,
            host_private: self.host_private.clone(),
        })
    }

    fn reconcile_authenticated_pairing(
        &self,
        peer: &AuthenticatedPeer,
    ) -> Result<(), ListenerError> {
        let snapshot = self
            .repository
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let Some(device) = snapshot.authority.devices.iter().find(|device| {
            device.device_id == peer.device_id && device.public_key == peer.public_key
        }) else {
            return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
        };
        let offer_id = device.source_offer_id;
        let should_reconcile = snapshot.authority.offers.iter().any(|offer| {
            offer.offer_id == offer_id
                && matches!(
                    offer.state,
                    PairingOfferState::Persisted | PairingOfferState::Uncertain
                )
        });
        if !should_reconcile {
            return Ok(());
        }
        self.repository
            .update(snapshot.revision, |authority| {
                authority
                    .acknowledge_pairing(offer_id, peer.public_key)
                    .map(|_| ())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ControllerPairingAuthority for RepositoryAuthority {
    fn snapshot(
        &self,
        offer_id: PairingOfferId,
    ) -> Result<PairingAuthoritySnapshot, ListenerError> {
        let authority = self
            .repository
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?
            .authority;
        let offer = authority
            .offers
            .iter()
            .find(|offer| offer.offer_id == offer_id && offer.is_pending())
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let identity = authority
            .identity
            .as_ref()
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        Ok(PairingAuthoritySnapshot {
            offer: PairingOfferCore {
                version: CONTROLLER_V1,
                expires_at_unix_seconds: offer.expires_at,
                nonce: PairingNonce(offer.nonce),
                host_static_public_key: termirust_controller_security::HostStaticPublicKey(
                    identity.public_key.0,
                ),
                capabilities: CapabilitySet::from_bits(offer.capabilities.bits())
                    .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?,
            },
            host_private: self.host_private.clone(),
            identity_generation: identity.generation,
            revocation_epoch: authority.revocation_epoch,
            session_generation: authority.session_generation,
        })
    }

    fn set_offer_state(
        &self,
        offer_id: PairingOfferId,
        state: PairingOfferState,
    ) -> Result<(), ListenerError> {
        let snapshot = self
            .repository
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        self.repository
            .update(snapshot.revision, |authority| {
                let offer = authority
                    .offers
                    .iter_mut()
                    .find(|offer| offer.offer_id == offer_id)
                    .ok_or(termirust_domain::ControllerDeviceError::OfferNotFound)?;
                offer.state = state;
                Ok(())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        Ok(())
    }

    async fn await_host_decision(
        &self,
        offer_id: PairingOfferId,
        sas: &termirust_controller_security::SasCode,
        cancel: &CancellationToken,
    ) -> Result<HostPairingDecision, ListenerError> {
        let receiver = self.decisions.register(offer_id)?;
        if let Err(error) = self.events.send(&ListenerProcessEvent::pairing_sas_ready(
            offer_id,
            sas.as_str().to_owned(),
        )) {
            self.decisions.remove(offer_id);
            return Err(error);
        }
        tokio::select! {
            _ = cancel.cancelled() => {
                self.decisions.remove(offer_id);
                Err(ListenerError::new(ListenerErrorCode::Cancelled))
            }
            decision = receiver => decision
                .map_err(|_| ListenerError::new(ListenerErrorCode::Cancelled)),
        }
    }

    fn persist(
        &self,
        offer_id: PairingOfferId,
        device_id: ControllerDeviceId,
        device_key: DeviceStaticPublicKey,
        display_name: String,
        now_unix_seconds: u64,
    ) -> Result<AuthenticatedPeer, ListenerError> {
        let snapshot = self
            .repository
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let mut record = None;
        let saved = self
            .repository
            .update(snapshot.revision, |authority| {
                record = Some(authority.persist_pairing(
                    offer_id,
                    device_id,
                    DevicePublicKey(device_key.0),
                    display_name,
                    now_unix_seconds,
                )?);
                Ok(())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let record =
            record.ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        Ok(AuthenticatedPeer {
            device_id: record.device_id,
            public_key: record.public_key,
            identity_generation: record.identity_generation,
            revocation_epoch: saved.authority.revocation_epoch,
            capabilities: ControllerCapabilities::from_bits(record.capabilities.bits())
                .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?,
        })
    }

    fn acknowledge(
        &self,
        offer_id: PairingOfferId,
        device_key: DeviceStaticPublicKey,
    ) -> Result<(), ListenerError> {
        let snapshot = self
            .repository
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let saved = self
            .repository
            .update(snapshot.revision, |authority| {
                authority
                    .acknowledge_pairing(offer_id, DevicePublicKey(device_key.0))
                    .map(|_| ())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let device_id = saved
            .authority
            .devices
            .iter()
            .find(|device| device.source_offer_id == offer_id)
            .map(|device| device.device_id)
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        self.events
            .send(&ListenerProcessEvent::pairing_complete(offer_id, device_id))?;
        Ok(())
    }
}

pub fn run_listener_worker<R, W>(mut reader: R, readiness: W) -> Result<(), ListenerError>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    let mut descriptor = ListenerLaunchDescriptor::read(&mut reader)?;
    let devices = ControllerDeviceRepository::open(&descriptor.controller_root)
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let network = ControllerNetworkRepository::open(&descriptor.controller_root)
        .map_err(|_| ListenerError::new(ListenerErrorCode::InvalidPolicy))?;
    let network_snapshot = network
        .load()
        .map_err(|_| ListenerError::new(ListenerErrorCode::InvalidPolicy))?;
    if network_snapshot.revision != descriptor.network_revision
        || network_snapshot.policy != descriptor.policy
    {
        return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
    }

    let interfaces = Arc::new(SystemInterfaceProvider);
    let bound = bind_selected_route(
        &descriptor.policy,
        interfaces.as_ref(),
        &SystemBinder,
        &mut SystemGeneratedPortSource,
    )?;
    if descriptor.policy.port != Some(bound.route.port) {
        descriptor.policy.port = Some(bound.route.port);
        let saved = network
            .save(descriptor.network_revision, descriptor.policy.clone())
            .map_err(|_| ListenerError::new(ListenerErrorCode::InvalidPolicy))?;
        descriptor.network_revision = saved.revision;
    }

    let sessions = SessionRepository::open(
        descriptor.project_root.clone(),
        descriptor.session_data_root.clone(),
    )
    .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
    let projects = ProjectRepository::open(descriptor.project_root.clone())
        .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
    let events = ListenerEventSink::new(readiness);
    let decisions = PairingDecisionBroker::default();
    let repository_authority = Arc::new(RepositoryAuthority {
        repository: devices,
        host_private: StaticPrivateKey::from_bytes(descriptor.host_private),
        events: events.clone(),
        decisions: decisions.clone(),
    });
    let authority: Arc<dyn ControllerAuthorityProvider> = repository_authority.clone();
    let pairing: Arc<dyn ControllerPairingAuthority> = repository_authority.clone();
    let backends = Arc::new(HostBackendFactory::new(
        sessions,
        projects,
        descriptor.runtime_parent.clone(),
    ));
    let mut source_key = [0; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut source_key)
        .map_err(|_| ListenerError::new(ListenerErrorCode::RandomUnavailable))?;
    let runtime = ListenerRuntime::new(SourceBucketKey::from_random(source_key))?;
    source_key.zeroize();
    let route = descriptor
        .policy
        .route()?
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::Disabled))?;
    let firewall = SystemFirewallObserver.observe(&route)?;
    events.send(&ListenerProcessEvent::ready_with_firewall(
        bound.route.address.port(),
        firewall,
    ))?;

    let cancel = CancellationToken::new();
    let control_cancel = cancel.clone();
    let control_events = events.clone();
    let control_authority = repository_authority;
    let control_route = RouteCandidate {
        interface_id: bound.route.interface_id.clone(),
        address_family: if bound.route.address.is_ipv4() {
            AddressFamily::Ipv4
        } else {
            AddressFamily::Ipv6
        },
        address: bound.route.address.ip(),
        port: bound.route.port,
        discovery: DiscoveryPolicy::Off,
    };
    std::thread::spawn(move || {
        loop {
            let command = match ListenerControlCommand::read(&mut reader) {
                Ok(Some(command)) => command,
                Ok(None) => break,
                Err(_) => {
                    let _ = control_events.send(&ListenerProcessEvent::pairing_failed(
                        None,
                        "invalid_control_command",
                    ));
                    break;
                }
            };
            let (offer_id, result) = match command {
                ListenerControlCommand::BeginPairing { .. } => (
                    None,
                    control_authority
                        .create_offer(&control_route)
                        .and_then(|event| control_events.send(&event)),
                ),
                ListenerControlCommand::DecidePairing {
                    offer_id, decision, ..
                } => (
                    Some(offer_id),
                    decisions.resolve(
                        offer_id,
                        match decision {
                            ProcessPairingDecision::Confirm => HostPairingDecision::Confirm,
                            ProcessPairingDecision::Reject => HostPairingDecision::Reject,
                        },
                    ),
                ),
            };
            if let Err(error) = result {
                let _ = control_events.send(&ListenerProcessEvent::pairing_failed(
                    offer_id,
                    error.code.stable_code(),
                ));
            }
        }
        control_cancel.cancel();
    });
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
    tokio_runtime.block_on(async move {
        let listener =
            tokio::net::TcpListener::from_std(bound.listener).map_err(ListenerError::from)?;
        let services = ListenerServices::new(interfaces, authority, pairing, backends);
        runtime
            .run(listener, descriptor.policy.clone(), services, cancel)
            .await
    })?;
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn safe_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use crate::{GeneratedPortSource as _, InterfaceProvider as _};
    use termirust_controller_security::host_public_key_from_private;
    use termirust_domain::{
        AddressFamily, ControllerPort, DiscoveryPolicy, HostIdentityGeneration, HostIdentityPublic,
        HostIdentitySecretRef, HostIdentityState, HostPublicKey, NetworkInterfaceId,
    };

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("shared buffer poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn launch_descriptor_round_trips_bounded_and_redacts_paths_and_secret() {
        let descriptor = ListenerLaunchDescriptor::new(
            PathBuf::from("/private/controller"),
            PathBuf::from("/private/projects"),
            PathBuf::from("/private/sessions"),
            PathBuf::from("/private/runtime"),
            ControllerNetworkRevision::ZERO,
            ControllerListenPolicy {
                enabled: true,
                interface_id: Some(NetworkInterfaceId::new("4:en0").unwrap()),
                address_family: Some(AddressFamily::Ipv4),
                selected_address: Some("192.168.1.9".parse().unwrap()),
                port: Some(ControllerPort::Generated(55_555)),
                discovery: DiscoveryPolicy::Off,
            },
            &StaticPrivateKey::from_fixture_bytes([7; 32]),
        )
        .unwrap();
        let mut bytes = Vec::new();
        descriptor.write(&mut bytes).unwrap();
        let decoded = ListenerLaunchDescriptor::read(bytes.as_slice()).unwrap();
        assert_eq!(decoded.policy, descriptor.policy);
        let debug = format!("{descriptor:?}");
        assert!(!debug.contains("private/controller"));
        assert!(!debug.contains("7, 7"));
    }

    #[test]
    fn launch_descriptor_rejects_relative_parent_and_oversize_input() {
        let mut oversized = vec![b'x'; MAX_LAUNCH_DESCRIPTOR_BYTES as usize];
        oversized.push(b'\n');
        assert!(ListenerLaunchDescriptor::read(oversized.as_slice()).is_err());
        assert!(!safe_absolute_path(Path::new("relative")));
        assert!(!safe_absolute_path(Path::new("/safe/../unsafe")));
    }

    #[test]
    fn authenticated_reconnect_reconciles_uncertain_pairing_without_duplicate_device() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let host_private = StaticPrivateKey::from_fixture_bytes([31; 32]);
        let host_public = host_public_key_from_private(&host_private);
        let offer_id = PairingOfferId::new();
        let device_id = ControllerDeviceId::new();
        let device_key = DevicePublicKey([32; 32]);
        let snapshot = repository.load().unwrap();
        let mut peer = None;
        repository
            .update(snapshot.revision, |authority| {
                authority.identity = Some(HostIdentityPublic::new(
                    HostIdentityGeneration::INITIAL,
                    HostPublicKey(host_public.0),
                ));
                authority.secret_ref =
                    Some(HostIdentitySecretRef::new("identity:reconcile-test").unwrap());
                authority.state = HostIdentityState::Ready;
                authority.create_offer(
                    offer_id,
                    [33; 32],
                    100,
                    200,
                    ControllerCapabilities::default().with(ControllerCapability::ObserveSessions),
                    vec!["192.168.1.9:55555".into()],
                )?;
                authority
                    .offers
                    .iter_mut()
                    .find(|offer| offer.offer_id == offer_id)
                    .unwrap()
                    .state = PairingOfferState::SasReady;
                let device = authority.persist_pairing(
                    offer_id,
                    device_id,
                    device_key,
                    "Test iPhone".into(),
                    150,
                )?;
                authority
                    .offers
                    .iter_mut()
                    .find(|offer| offer.offer_id == offer_id)
                    .unwrap()
                    .state = PairingOfferState::Uncertain;
                peer = Some(AuthenticatedPeer {
                    device_id: device.device_id,
                    public_key: device.public_key,
                    identity_generation: device.identity_generation,
                    revocation_epoch: device.revocation_epoch,
                    capabilities: device.capabilities,
                });
                Ok(())
            })
            .unwrap();
        let authority = RepositoryAuthority {
            repository: repository.clone(),
            host_private,
            events: ListenerEventSink::new(Vec::<u8>::new()),
            decisions: PairingDecisionBroker::default(),
        };

        authority
            .reconcile_authenticated_pairing(&peer.unwrap())
            .unwrap();

        let saved = repository.load().unwrap();
        assert_eq!(saved.authority.devices.len(), 1);
        assert_eq!(
            saved
                .authority
                .offers
                .iter()
                .find(|offer| offer.offer_id == offer_id)
                .unwrap()
                .state,
            PairingOfferState::Acknowledged
        );
    }

    #[test]
    fn owned_worker_emits_ready_and_offer_then_stops_on_control_eof() {
        let Some(interface) = SystemInterfaceProvider
            .eligible_interfaces()
            .unwrap()
            .into_iter()
            .next()
        else {
            return;
        };
        let fixture = tempfile::tempdir().unwrap();
        let controller_root = fixture.path().join("controller");
        let project_root = fixture.path().join("projects");
        let session_root = fixture.path().join("sessions");
        let runtime_root = fixture.path().join("runtime");
        let private = StaticPrivateKey::from_fixture_bytes([17; 32]);
        let public = host_public_key_from_private(&private);
        let devices = ControllerDeviceRepository::open(&controller_root).unwrap();
        let snapshot = devices.load().unwrap();
        devices
            .update(snapshot.revision, |authority| {
                authority.identity = Some(HostIdentityPublic::new(
                    HostIdentityGeneration::INITIAL,
                    HostPublicKey(public.0),
                ));
                authority.secret_ref =
                    Some(HostIdentitySecretRef::new("identity:test-worker").unwrap());
                authority.state = HostIdentityState::Ready;
                Ok(())
            })
            .unwrap();
        let port = SystemGeneratedPortSource.next_port().unwrap();
        let policy = ControllerListenPolicy {
            enabled: true,
            interface_id: Some(interface.id),
            address_family: Some(interface.address_family),
            selected_address: Some(interface.address),
            port: Some(ControllerPort::Generated(port)),
            discovery: DiscoveryPolicy::Off,
        };
        let network = ControllerNetworkRepository::open(&controller_root).unwrap();
        let network_snapshot = network.load().unwrap();
        let saved = network
            .save(network_snapshot.revision, policy.clone())
            .unwrap();
        let descriptor = ListenerLaunchDescriptor::new(
            controller_root,
            project_root,
            session_root,
            runtime_root,
            saved.revision,
            policy,
            &private,
        )
        .unwrap();
        let mut control = Vec::new();
        descriptor.write(&mut control).unwrap();
        ListenerControlCommand::begin_pairing()
            .write(&mut control)
            .unwrap();
        let output = SharedBuffer::default();
        run_listener_worker(Cursor::new(control), output.clone()).unwrap();

        let bytes = output.0.lock().unwrap().clone();
        let mut reader = Cursor::new(bytes);
        assert!(matches!(
            ListenerProcessEvent::read(&mut reader).unwrap(),
            Some(ListenerProcessEvent::Ready { port, .. }) if port >= 49_152
        ));
        let offer_text = match ListenerProcessEvent::read(&mut reader).unwrap() {
            Some(ListenerProcessEvent::PairingOffer { offer_text, .. }) => offer_text,
            event => panic!("expected redacted pairing offer event, got {event:?}"),
        };
        let offer = crate::ControllerPairingOffer::decode_text(&offer_text).unwrap();
        assert_eq!(offer.address, interface.address);
        assert_eq!(offer.address_family, interface.address_family);
    }

    #[tokio::test]
    async fn decision_broker_accepts_one_explicit_decision_per_offer() {
        let broker = PairingDecisionBroker::default();
        let offer_id = PairingOfferId::new();
        let receiver = broker.register(offer_id).unwrap();
        assert_eq!(
            broker.register(offer_id).unwrap_err().code,
            ListenerErrorCode::AuthenticationFailed
        );
        broker
            .resolve(offer_id, HostPairingDecision::Confirm)
            .unwrap();
        assert_eq!(receiver.await.unwrap(), HostPairingDecision::Confirm);
        assert_eq!(
            broker
                .resolve(offer_id, HostPairingDecision::Reject)
                .unwrap_err()
                .code,
            ListenerErrorCode::AuthenticationFailed
        );
    }
}
