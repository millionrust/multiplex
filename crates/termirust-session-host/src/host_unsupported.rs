//! The Session Host needs Unix sockets, advisory file locks, and process groups. Other platforms
//! get the same public API, and starting a host fails the way the client's connect does there.

use std::path::{Path, PathBuf};

use termirust_domain::{HostLifecycle, OutputSequence};
use tokio_util::sync::CancellationToken;

use crate::descriptor::LaunchDescriptor;
use crate::{HostError, HostErrorCode};

pub const MAX_LIVE_HOSTS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionHostStats {
    pub lifecycle: HostLifecycle,
    pub latest_sequence: OutputSequence,
    pub active_connections: usize,
    pub recording_paused: bool,
}

/// Never constructed on this platform.
#[derive(Debug)]
pub struct SessionHostHandle {
    runtime_root: PathBuf,
}

impl SessionHostHandle {
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub async fn stats(&self) -> SessionHostStats {
        SessionHostStats {
            lifecycle: HostLifecycle::Exited,
            latest_sequence: OutputSequence::ZERO,
            active_connections: 0,
            recording_paused: false,
        }
    }

    pub async fn wait(self) -> Result<(), HostError> {
        Err(HostError::new(HostErrorCode::JoinFailed))
    }

    pub async fn shutdown(self) -> Result<(), HostError> {
        Err(HostError::new(HostErrorCode::JoinFailed))
    }
}

pub async fn start(descriptor: LaunchDescriptor) -> Result<SessionHostHandle, HostError> {
    start_with_cancel(descriptor, &CancellationToken::new()).await
}

pub async fn start_with_cancel(
    descriptor: LaunchDescriptor,
    launch_cancel: &CancellationToken,
) -> Result<SessionHostHandle, HostError> {
    if launch_cancel.is_cancelled() {
        return Err(HostError::new(HostErrorCode::Cancelled));
    }
    descriptor.validate()?;
    Err(HostError::new(HostErrorCode::PermissionDenied))
}
