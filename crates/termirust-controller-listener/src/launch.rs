use std::fmt;
use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use termirust_controller_security::{
    CONTROLLER_V1, CapabilitySet, DeviceStaticPublicKey, PairingNonce, PairingOfferCore,
    StaticPrivateKey,
};
use termirust_domain::{
    AuthenticatedPeer, ControllerCapabilities, ControllerDeviceId, ControllerListenPolicy,
    ControllerNetworkRevision, DevicePublicKey, PairingOfferId, PairingOfferState,
};
use termirust_store::{ControllerDeviceRepository, ControllerNetworkRepository, SessionRepository};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize as _;

use crate::{
    AuthoritySnapshot, ControllerAuthorityProvider, ControllerPairingAuthority, HostBackendFactory,
    HostPairingDecision, ListenerError, ListenerErrorCode, ListenerRuntime, ListenerServices,
    PairingAuthoritySnapshot, SourceBucketKey, SystemBinder, SystemGeneratedPortSource,
    SystemInterfaceProvider, bind_selected_route,
};

const LAUNCH_FORMAT_VERSION: u16 = 1;
const MAX_LAUNCH_DESCRIPTOR_BYTES: u64 = 32 * 1024;

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
        _: PairingOfferId,
        _: &termirust_controller_security::SasCode,
        _: &CancellationToken,
    ) -> Result<HostPairingDecision, ListenerError> {
        Ok(HostPairingDecision::Reject)
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
        self.repository
            .update(snapshot.revision, |authority| {
                authority
                    .acknowledge_pairing(offer_id, DevicePublicKey(device_key.0))
                    .map(|_| ())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        Ok(())
    }
}

pub fn run_listener_worker(
    reader: impl BufRead,
    mut readiness: impl Write,
) -> Result<(), ListenerError> {
    let mut descriptor = ListenerLaunchDescriptor::read(reader)?;
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
    let repository_authority = Arc::new(RepositoryAuthority {
        repository: devices,
        host_private: StaticPrivateKey::from_bytes(descriptor.host_private),
    });
    let authority: Arc<dyn ControllerAuthorityProvider> = repository_authority.clone();
    let pairing: Arc<dyn ControllerPairingAuthority> = repository_authority;
    let backends = Arc::new(HostBackendFactory::new(
        sessions,
        descriptor.runtime_parent.clone(),
    ));
    let mut source_key = [0; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut source_key)
        .map_err(|_| ListenerError::new(ListenerErrorCode::RandomUnavailable))?;
    let runtime = ListenerRuntime::new(SourceBucketKey::from_random(source_key))?;
    source_key.zeroize();
    writeln!(
        readiness,
        "{{\"schema_version\":1,\"lifecycle\":\"ready\",\"code\":\"controller_listener_ready\",\"port\":{}}}",
        bound.route.address.port()
    )
    .and_then(|()| readiness.flush())
    .map_err(ListenerError::from)?;

    let cancel = CancellationToken::new();
    let cancel_on_eof = cancel.clone();
    std::thread::spawn(move || {
        let mut byte = [0; 1];
        let _ = std::io::Read::read(&mut std::io::stdin(), &mut byte);
        cancel_on_eof.cancel();
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

fn safe_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::{AddressFamily, ControllerPort, DiscoveryPolicy, NetworkInterfaceId};

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
}
