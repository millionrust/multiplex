use std::fmt;
use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use termirust_controller_security::StaticPrivateKey;
use termirust_domain::{ControllerListenPolicy, ControllerNetworkRevision};
use termirust_store::{ControllerDeviceRepository, ControllerNetworkRepository, SessionRepository};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize as _;

use crate::{
    AuthoritySnapshot, ControllerAuthorityProvider, HostBackendFactory, ListenerError,
    ListenerErrorCode, ListenerRuntime, SourceBucketKey, SystemBinder, SystemGeneratedPortSource,
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
    let authority: Arc<dyn ControllerAuthorityProvider> = Arc::new(RepositoryAuthority {
        repository: devices,
        host_private: StaticPrivateKey::from_bytes(descriptor.host_private),
    });
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
        runtime
            .run(
                listener,
                descriptor.policy.clone(),
                interfaces,
                authority,
                backends,
                cancel,
            )
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
