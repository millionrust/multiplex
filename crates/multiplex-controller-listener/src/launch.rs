use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use multiplex_controller_security::{
    CONTROLLER_V2, CapabilitySet, DeviceStaticPublicKey, PairingNonce, PairingOfferCore,
    StaticPrivateKey,
};
use multiplex_domain::{
    AuthenticatedPeer, ControllerCapabilities, ControllerCapability, ControllerDeviceId,
    ControllerListenPolicy, ControllerNetworkRevision, DevicePublicKey, ListeningAddress,
    PairingOfferId, PairingOfferState,
};
use multiplex_store::{
    ControllerDeviceRepository, ControllerNetworkRepository, ProjectRepository, SessionRepository,
};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize as _;

use crate::runtime::serve_authenticated_stdio_stream_after_purpose;
use crate::{
    AuthoritySnapshot, ControllerAuthorityProvider, ControllerConnectionPurpose,
    ControllerPairingAuthority, FirewallObserver as _, HostBackendFactory, HostPairingDecision,
    ListenerControlCommand, ListenerError, ListenerErrorCode, ListenerOwnership,
    ListenerProcessEvent, ListenerRuntime, ListenerServices, PairingAuthoritySnapshot,
    ProcessPairingDecision, SourceBucketKey, SshControllerPairingOffer, SshHostPairingPrompt,
    SystemBinder, SystemFirewallObserver, SystemGeneratedPortSource, SystemHandshakeEntropy,
    SystemInterfaceProvider, TmuxSessionSource, bind_private_addresses, pair_controller,
    request_ssh_host_pairing_decision,
};

const LAUNCH_FORMAT_VERSION: u16 = 1;
const MAX_LAUNCH_DESCRIPTOR_BYTES: u64 = 32 * 1024;
const PAIRING_OFFER_LIFETIME_SECONDS: u64 = 5 * 60;
/// How often the listener checks who is watching its screens, to tell the app.
const SCREEN_WATCHER_POLL: std::time::Duration = std::time::Duration::from_millis(500);

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
    #[serde(default)]
    pub desktop_pane_bridge: Option<crate::DesktopPaneBridgeEndpoint>,
    /// Lists and attaches tmux sessions the app did not create. Opt-in.
    #[serde(default)]
    pub tmux_sessions: bool,
    /// Serves this computer's screens to paired devices that may watch them. Opt-in.
    #[serde(default)]
    pub screen_sharing: bool,
    /// What a previous run was given for reopening a screen grant without asking again. Only
    /// Wayland has anything to put here; everywhere else capture needs no per-screen permission.
    #[serde(default)]
    pub screen_restore_token: Option<String>,
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
            desktop_pane_bridge: None,
            tmux_sessions: false,
            screen_sharing: false,
            screen_restore_token: None,
            network_revision,
            policy,
            host_private: host_private.copy_for_process_handoff(),
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn with_desktop_pane_bridge(
        mut self,
        endpoint: Option<crate::DesktopPaneBridgeEndpoint>,
    ) -> Result<Self, ListenerError> {
        self.desktop_pane_bridge = endpoint;
        self.validate()?;
        Ok(self)
    }

    pub fn with_tmux_sessions(mut self, enabled: bool) -> Self {
        self.tmux_sessions = enabled;
        self
    }

    pub fn with_screen_sharing(mut self, enabled: bool) -> Self {
        self.screen_sharing = enabled;
        self
    }

    /// Hands back the grant token a previous run reported, so the person is not asked again.
    pub fn with_screen_restore_token(mut self, token: Option<String>) -> Self {
        self.screen_restore_token = token;
        self
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
        if let Some(endpoint) = &self.desktop_pane_bridge {
            endpoint.validate()?;
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
            .field("tmux_sessions", &self.tmux_sessions)
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
    code_offer: Mutex<Option<ActiveCodeOffer>>,
}

/// The pairing mode the desktop is showing. It lives only in this process, so the code is
/// never written to disk, and at most one attempt runs at a time.
struct ActiveCodeOffer {
    offer_id: PairingOfferId,
    code: multiplex_controller_security::PairingCode,
    expires_at: u64,
    attempts_left: u8,
    attempt_running: bool,
}

impl RepositoryAuthority {
    /// Opens pairing mode with a new code for the addresses the listener accepts on, replacing
    /// any code already shown.
    fn create_code_offer(
        &self,
        routes: &[ListeningAddress],
    ) -> Result<ListenerProcessEvent, ListenerError> {
        let ListenerProcessEvent::PairingOffer {
            offer_id,
            expires_at_unix_seconds,
            ..
        } = self.create_offer(routes)?
        else {
            return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
        };
        let code = multiplex_controller_security::PairingCode::generate(&mut rand::rngs::OsRng);
        let event = ListenerProcessEvent::pairing_code(
            offer_id,
            code.as_str().to_owned(),
            expires_at_unix_seconds,
            crate::MAX_CODE_PAIRING_ATTEMPTS,
        );
        let replaced = self
            .code_offer
            .lock()
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?
            .replace(ActiveCodeOffer {
                offer_id,
                code,
                expires_at: expires_at_unix_seconds,
                attempts_left: crate::MAX_CODE_PAIRING_ATTEMPTS,
                attempt_running: false,
            });
        if let Some(replaced) = replaced {
            let _ = self.set_offer_state(replaced.offer_id, PairingOfferState::Rejected);
        }
        Ok(event)
    }

    fn cancel_code_offer(&self, offer_id: PairingOfferId) -> Result<(), ListenerError> {
        let mut active = self
            .code_offer
            .lock()
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        if active.as_ref().map(|offer| offer.offer_id) == Some(offer_id) {
            active.take();
            drop(active);
            self.set_offer_state(offer_id, PairingOfferState::Rejected)?;
        }
        Ok(())
    }

    fn create_offer(
        &self,
        routes: &[ListeningAddress],
    ) -> Result<ListenerProcessEvent, ListenerError> {
        if routes.is_empty() {
            return Err(ListenerError::new(ListenerErrorCode::NoEligibleInterface));
        }
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
                record = Some(
                    authority.create_offer(
                        offer_id,
                        nonce,
                        now,
                        expires_at,
                        capabilities,
                        routes
                            .iter()
                            .take(crate::MAX_PAIRING_ROUTES)
                            .map(|route| route.address.to_string())
                            .collect(),
                    )?,
                );
                Ok(())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let record =
            record.ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let core = PairingOfferCore {
            version: CONTROLLER_V2,
            expires_at_unix_seconds: record.expires_at,
            nonce: PairingNonce(record.nonce),
            host_static_public_key: multiplex_controller_security::HostStaticPublicKey(
                record.identity.public_key.0,
            ),
            capabilities: CapabilitySet::from_bits(record.capabilities.bits())
                .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?,
        };
        let offer = crate::ControllerPairingOffer::new(
            offer_id,
            routes,
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
        reconcile_authenticated_pairing(&self.repository, peer)
    }
}

struct RepositoryStdioAuthority {
    repository: ControllerDeviceRepository,
    host_private: StaticPrivateKey,
    pairing_broker_path: PathBuf,
}

impl RepositoryStdioAuthority {
    fn create_offer(&self) -> Result<SshControllerPairingOffer, ListenerError> {
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
                    vec!["ssh-controller-v1".into()],
                )?);
                Ok(())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let record =
            record.ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let offer = PairingOfferCore {
            version: CONTROLLER_V2,
            expires_at_unix_seconds: record.expires_at,
            nonce: PairingNonce(record.nonce),
            host_static_public_key: multiplex_controller_security::HostStaticPublicKey(
                record.identity.public_key.0,
            ),
            capabilities: CapabilitySet::from_bits(record.capabilities.bits())
                .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?,
        };
        SshControllerPairingOffer::new(
            offer_id,
            &offer,
            record.identity.generation.get(),
            saved.authority.revocation_epoch,
            saved.authority.session_generation,
        )
    }
}

impl ControllerAuthorityProvider for RepositoryStdioAuthority {
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
        reconcile_authenticated_pairing(&self.repository, peer)
    }
}

#[async_trait::async_trait]
impl ControllerPairingAuthority for RepositoryStdioAuthority {
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
                version: CONTROLLER_V2,
                expires_at_unix_seconds: offer.expires_at,
                nonce: PairingNonce(offer.nonce),
                host_static_public_key: multiplex_controller_security::HostStaticPublicKey(
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
                    .ok_or(multiplex_domain::ControllerDeviceError::OfferNotFound)?;
                offer.state = state;
                Ok(())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        Ok(())
    }

    async fn await_host_decision(
        &self,
        offer_id: PairingOfferId,
        sas: &multiplex_controller_security::SasCode,
        cancel: &CancellationToken,
    ) -> Result<HostPairingDecision, ListenerError> {
        let expires_at = self
            .repository
            .load()
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?
            .authority
            .offers
            .iter()
            .find(|offer| offer.offer_id == offer_id)
            .map(|offer| offer.expires_at)
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let prompt = SshHostPairingPrompt::new(offer_id, sas.as_str().to_owned(), expires_at)?;
        tokio::select! {
            _ = cancel.cancelled() => Err(ListenerError::new(ListenerErrorCode::Cancelled)),
            decision = request_ssh_host_pairing_decision(&self.pairing_broker_path, &prompt) => decision,
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

fn reconcile_authenticated_pairing(
    repository: &ControllerDeviceRepository,
    peer: &AuthenticatedPeer,
) -> Result<(), ListenerError> {
    let snapshot = repository
        .load()
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let Some(device) =
        snapshot.authority.devices.iter().find(|device| {
            device.device_id == peer.device_id && device.public_key == peer.public_key
        })
    else {
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
    repository
        .update(snapshot.revision, |authority| {
            authority
                .acknowledge_pairing(offer_id, peer.public_key)
                .map(|_| ())
        })
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    Ok(())
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
                version: CONTROLLER_V2,
                expires_at_unix_seconds: offer.expires_at,
                nonce: PairingNonce(offer.nonce),
                host_static_public_key: multiplex_controller_security::HostStaticPublicKey(
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
                    .ok_or(multiplex_domain::ControllerDeviceError::OfferNotFound)?;
                offer.state = state;
                Ok(())
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        Ok(())
    }

    async fn await_host_decision(
        &self,
        offer_id: PairingOfferId,
        sas: &multiplex_controller_security::SasCode,
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

    fn begin_code_attempt(&self) -> Result<crate::CodePairingAttempt, ListenerError> {
        let (offer_id, code) = {
            let mut active = self
                .code_offer
                .lock()
                .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
            let offer = active
                .as_mut()
                .ok_or_else(|| ListenerError::new(ListenerErrorCode::Unauthorized))?;
            if offer.expires_at < unix_seconds() {
                let offer_id = offer.offer_id;
                active.take();
                drop(active);
                let _ = self.set_offer_state(offer_id, PairingOfferState::Expired);
                return Err(ListenerError::new(ListenerErrorCode::Unauthorized));
            }
            if offer.attempt_running || offer.attempts_left == 0 {
                return Err(ListenerError::new(ListenerErrorCode::RateLimited));
            }
            offer.attempts_left -= 1;
            offer.attempt_running = true;
            (offer.offer_id, offer.code.clone())
        };
        match ControllerPairingAuthority::snapshot(self, offer_id) {
            Ok(snapshot) => Ok(crate::CodePairingAttempt {
                offer_id,
                code,
                snapshot,
            }),
            Err(error) => {
                self.finish_code_attempt(offer_id, false);
                Err(error)
            }
        }
    }

    fn finish_code_attempt(&self, offer_id: PairingOfferId, paired: bool) {
        let Ok(mut active) = self.code_offer.lock() else {
            return;
        };
        let Some(offer) = active.as_mut().filter(|offer| offer.offer_id == offer_id) else {
            return;
        };
        offer.attempt_running = false;
        if paired {
            active.take();
            return;
        }
        let attempts_left = offer.attempts_left;
        if attempts_left == 0 {
            active.take();
        }
        drop(active);
        if attempts_left == 0 {
            let _ = self.set_offer_state(offer_id, PairingOfferState::Rejected);
            let _ = self.events.send(&ListenerProcessEvent::pairing_failed(
                Some(offer_id),
                "code_attempts_exhausted",
            ));
        } else {
            let _ = self.set_offer_state(offer_id, PairingOfferState::Offered);
            let _ = self
                .events
                .send(&ListenerProcessEvent::pairing_code_attempt_failed(
                    offer_id,
                    attempts_left,
                ));
        }
    }
}

struct SplitControllerIo<R, W> {
    reader: R,
    writer: W,
}

impl<R: AsyncRead + Unpin, W: Unpin> AsyncRead for SplitControllerIo<R, W> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().reader).poll_read(context, buffer)
    }
}

impl<R: Unpin, W: AsyncWrite + Unpin> AsyncWrite for SplitControllerIo<R, W> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().writer).poll_write(context, bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_shutdown(context)
    }
}

/// Live sessions a repository bridge offers beyond durable sessions. The SSH and relay routes
/// run the bridge in their own process, so they receive these explicitly instead of from a
/// launch descriptor.
#[derive(Clone, Default)]
pub struct RepositoryBridgeSources {
    /// The running desktop app's live panes, usually found with
    /// [`crate::DesktopPaneBridgeEndpoint::discover`].
    pub desktop_pane_bridge: Option<crate::DesktopPaneBridgeEndpoint>,
    /// tmux sessions, when the user turned sharing on.
    pub tmux_sessions: Option<TmuxSessionSource>,
    /// Screens, when the user turned screen sharing on and this build can capture them.
    pub screens: Option<Arc<dyn crate::ScreenSessionFactory>>,
}

impl std::fmt::Debug for RepositoryBridgeSources {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepositoryBridgeSources")
            .field("desktop_pane_bridge", &self.desktop_pane_bridge)
            .field("tmux_sessions", &self.tmux_sessions.is_some())
            .field("screens", &self.screens.is_some())
            .finish()
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve_repository_stdio_bridge<R, W>(
    reader: R,
    writer: W,
    controller_root: PathBuf,
    project_root: PathBuf,
    session_data_root: PathBuf,
    runtime_parent: PathBuf,
    pairing_broker_path: PathBuf,
    host_private: StaticPrivateKey,
    sources: RepositoryBridgeSources,
    cancel: CancellationToken,
) -> Result<(), ListenerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let devices = ControllerDeviceRepository::open(controller_root)
        .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
    let sessions = SessionRepository::open(project_root.clone(), session_data_root)
        .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
    let projects = ProjectRepository::open(project_root)
        .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
    let authority = Arc::new(RepositoryStdioAuthority {
        repository: devices,
        host_private,
        pairing_broker_path,
    });
    let backends: Arc<dyn crate::ControllerBackendFactory> = Arc::new(
        HostBackendFactory::new(sessions, projects, runtime_parent)
            .with_desktop_pane_bridge(sources.desktop_pane_bridge)
            .with_tmux_sessions(sources.tmux_sessions)
            .with_screens(sources.screens),
    );
    let mut stream = SplitControllerIo { reader, writer };
    let purpose = tokio::time::timeout(
        std::time::Duration::from_secs(
            multiplex_domain::ConnectionBudget::default().handshake_timeout_seconds,
        ),
        ControllerConnectionPurpose::read_from(&mut stream),
    )
    .await
    .map_err(|_| ListenerError::new(ListenerErrorCode::HandshakeTimeout))??;
    match purpose {
        ControllerConnectionPurpose::Authenticate => {
            let provider: Arc<dyn ControllerAuthorityProvider> = authority;
            serve_authenticated_stdio_stream_after_purpose(&mut stream, provider, backends, cancel)
                .await
        }
        ControllerConnectionPurpose::Pair => {
            let offer = authority.create_offer()?;
            offer.write_to(&mut stream).await?;
            pair_controller(
                &mut stream,
                authority.as_ref(),
                &mut SystemHandshakeEntropy,
                cancel,
            )
            .await
            .map(|_| ())
        }
        // SSH already authenticates the user; code pairing is only for the network listener.
        ControllerConnectionPurpose::PairCode => {
            Err(ListenerError::new(ListenerErrorCode::Unauthorized))
        }
    }
}

/// How long a starting listener waits for another to hand over the route.
pub const LISTENER_OWNERSHIP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn run_listener_worker<R, W>(reader: R, readiness: W) -> Result<(), ListenerError>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    run_listener_worker_with_screens(reader, readiness, None)
}

/// The same worker, able to serve Remote Screens when the descriptor asks for it and the caller
/// supplied a factory. Capture and input injection happen in this process, which therefore needs
/// the operating system's screen-recording and accessibility permissions.
pub fn run_listener_worker_with_screens<R, W>(
    mut reader: R,
    readiness: W,
    screens: Option<Arc<dyn crate::ScreenSessionFactory>>,
) -> Result<(), ListenerError>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    let mut descriptor = ListenerLaunchDescriptor::read(&mut reader)?;
    // Held until this worker returns, so the background service and the desktop app never
    // serve the route together.
    let _ownership =
        ListenerOwnership::acquire_within(&descriptor.controller_root, LISTENER_OWNERSHIP_WAIT)?;
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
    let bound = bind_private_addresses(
        &descriptor.policy,
        interfaces.as_ref(),
        &SystemBinder,
        &mut SystemGeneratedPortSource,
    )?;
    if descriptor.policy.port != Some(bound.port) {
        descriptor.policy.port = Some(bound.port);
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
        code_offer: Mutex::new(None),
    });
    let authority: Arc<dyn ControllerAuthorityProvider> = repository_authority.clone();
    let pairing: Arc<dyn ControllerPairingAuthority> = repository_authority.clone();
    let backends = Arc::new(
        HostBackendFactory::new(sessions, projects, descriptor.runtime_parent.clone())
            .with_desktop_pane_bridge(descriptor.desktop_pane_bridge.clone())
            .with_tmux_sessions(
                descriptor
                    .tmux_sessions
                    .then(|| TmuxSessionSource::system(descriptor.runtime_parent.clone()))
                    .transpose()?,
            )
            .with_screens(screens.clone().filter(|_| descriptor.screen_sharing)),
    );
    // Whatever this host must settle before it can offer a screen at all, settled now rather than
    // when a device arrives. On Wayland that is the portal dialog, and a person who has just
    // turned sharing on is expecting to be asked; a person whose phone connects an hour later is
    // not, and by then the session already needs to know what it is sharing.
    if descriptor.screen_sharing
        && let Some(screens) = screens.as_ref()
    {
        screens.prepare(descriptor.screen_restore_token.as_deref());
    }
    let mut source_key = [0; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut source_key)
        .map_err(|_| ListenerError::new(ListenerErrorCode::RandomUnavailable))?;
    let runtime = ListenerRuntime::new(SourceBucketKey::from_random(source_key))?;
    source_key.zeroize();
    let initial_addresses = bound.addresses();
    let firewall = SystemFirewallObserver.observe(&initial_addresses)?;
    events.send(&ListenerProcessEvent::ready_with_firewall(
        bound.port.value(),
        initial_addresses.clone(),
        firewall,
    ))?;
    // A failure to announce never stops the listener; phones can still type the address.
    let announcement = (descriptor.policy.discovery == multiplex_domain::DiscoveryPolicy::Bonjour)
        .then(|| {
            let host = multiplex_controller_security::host_public_key_from_private(
                &StaticPrivateKey::from_bytes(descriptor.host_private),
            );
            crate::BonjourAnnouncement::start(multiplex_domain::HostPublicKey(host.0)).ok()
        })
        .flatten()
        .map(|mut announcement| {
            let _ = announcement.update(&initial_addresses);
            Mutex::new(announcement)
        });
    // Pairing offers list the addresses the runtime is accepting on at the moment they are made.
    let listening = Arc::new(Mutex::new(initial_addresses));

    let cancel = CancellationToken::new();
    // The computer being watched has to be able to say so, so the app hears about every change.
    if let Some(screens) = screens.filter(|_| descriptor.screen_sharing) {
        let watcher_events = events.clone();
        let watcher_cancel = cancel.clone();
        std::thread::spawn(move || {
            let mut reported = Vec::new();
            let mut reported_token: Option<String> = None;
            while !watcher_cancel.is_cancelled() {
                let watching = screens.watchers();
                if watching != reported {
                    if watcher_events
                        .send(&ListenerProcessEvent::screen_watchers(watching.clone()))
                        .is_err()
                    {
                        return;
                    }
                    reported = watching;
                }
                // The grant token appears only once the person has answered the portal, which is
                // some time after this thread starts, so it is polled rather than read at startup.
                let token = screens.restore_token();
                if let Some(token) = token.filter(|token| Some(token) != reported_token.as_ref()) {
                    if watcher_events
                        .send(&ListenerProcessEvent::screen_restore_token(token.clone()))
                        .is_err()
                    {
                        return;
                    }
                    reported_token = Some(token);
                }
                std::thread::sleep(SCREEN_WATCHER_POLL);
            }
        });
    }
    let control_cancel = cancel.clone();
    let control_events = events.clone();
    let control_authority = repository_authority;
    let control_listening = Arc::clone(&listening);
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
                    control_listening
                        .lock()
                        .map(|addresses| addresses.clone())
                        .map_err(|_| ListenerError::new(ListenerErrorCode::Io))
                        .and_then(|addresses| control_authority.create_offer(&addresses))
                        .and_then(|event| control_events.send(&event)),
                ),
                ListenerControlCommand::BeginCodePairing { .. } => (
                    None,
                    control_listening
                        .lock()
                        .map(|addresses| addresses.clone())
                        .map_err(|_| ListenerError::new(ListenerErrorCode::Io))
                        .and_then(|addresses| control_authority.create_code_offer(&addresses))
                        .and_then(|event| control_events.send(&event)),
                ),
                ListenerControlCommand::CancelCodePairing { offer_id, .. } => (
                    Some(offer_id),
                    control_authority.cancel_code_offer(offer_id),
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
    let address_events = events.clone();
    let observer: crate::ListeningAddressObserver = Arc::new(move |addresses| {
        if let Ok(mut current) = listening.lock()
            && current.as_slice() != addresses
        {
            *current = addresses.to_vec();
            if let Some(Ok(mut announcement)) = announcement.as_ref().map(Mutex::lock) {
                let _ = announcement.update(addresses);
            }
            let _ = address_events.send(&ListenerProcessEvent::listening_addresses(
                addresses.to_vec(),
            ));
        }
    });
    tokio_runtime.block_on(async move {
        let services = ListenerServices::new(interfaces, authority, pairing, backends)
            .with_address_observer(observer);
        runtime.run(bound, services, cancel).await
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
    use multiplex_controller_security::host_public_key_from_private;
    use multiplex_domain::{
        ControllerPort, DiscoveryPolicy, HostIdentityGeneration, HostIdentityPublic,
        HostIdentitySecretRef, HostIdentityState, HostPublicKey,
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

    /// A directory a descriptor would accept. A path is absolute on Windows only with a drive
    /// behind it, and a descriptor keeps only absolute ones.
    fn private_directory(name: &str) -> PathBuf {
        #[cfg(unix)]
        return PathBuf::from(format!("/private/{name}"));
        #[cfg(windows)]
        return PathBuf::from(format!(r"C:\private\{name}"));
    }

    #[test]
    fn launch_descriptor_round_trips_bounded_and_redacts_paths_and_secret() {
        let controller = private_directory("controller");
        let descriptor = ListenerLaunchDescriptor::new(
            controller.clone(),
            private_directory("projects"),
            private_directory("sessions"),
            private_directory("runtime"),
            ControllerNetworkRevision::ZERO,
            ControllerListenPolicy {
                enabled: true,
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
        assert!(
            !decoded.tmux_sessions,
            "tmux discovery is off unless asked for"
        );
        let mut bytes = Vec::new();
        descriptor
            .clone()
            .with_tmux_sessions(true)
            .write(&mut bytes)
            .unwrap();
        assert!(
            ListenerLaunchDescriptor::read(bytes.as_slice())
                .unwrap()
                .tmux_sessions
        );
        let debug = format!("{descriptor:?}");
        // The path as this platform writes it, so the check cannot pass by spelling.
        assert!(!debug.contains(&controller.display().to_string()));
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
    fn pairing_mode_spends_one_attempt_per_try_and_closes_after_the_last() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let host_private = StaticPrivateKey::from_fixture_bytes([51; 32]);
        let host_public = host_public_key_from_private(&host_private);
        let snapshot = repository.load().unwrap();
        repository
            .update(snapshot.revision, |authority| {
                authority.identity = Some(HostIdentityPublic::new(
                    HostIdentityGeneration::INITIAL,
                    HostPublicKey(host_public.0),
                ));
                authority.secret_ref =
                    Some(HostIdentitySecretRef::new("identity:code-test").unwrap());
                authority.state = HostIdentityState::Ready;
                Ok(())
            })
            .unwrap();
        let output = SharedBuffer::default();
        let authority = RepositoryAuthority {
            repository,
            host_private,
            events: ListenerEventSink::new(output.clone()),
            decisions: PairingDecisionBroker::default(),
            code_offer: Mutex::new(None),
        };
        let routes = [ListeningAddress {
            interface_id: multiplex_domain::NetworkInterfaceId::new("4:en0").unwrap(),
            label: "en0".into(),
            kind: multiplex_domain::NetworkInterfaceKind::Lan,
            address: "192.168.1.9:55555".parse().unwrap(),
        }];

        let ListenerProcessEvent::PairingCode {
            offer_id,
            code,
            attempts_left,
            ..
        } = authority.create_code_offer(&routes).unwrap()
        else {
            panic!("pairing mode reports its code");
        };
        assert_eq!(attempts_left, crate::MAX_CODE_PAIRING_ATTEMPTS);
        assert!(code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit()));

        let first = authority.begin_code_attempt().unwrap();
        assert_eq!(first.code.as_str(), code);
        assert_eq!(
            authority.begin_code_attempt().unwrap_err().code,
            ListenerErrorCode::RateLimited,
            "only one attempt runs at a time"
        );
        authority.finish_code_attempt(offer_id, false);
        for _ in 0..2 {
            authority.begin_code_attempt().unwrap();
            authority.finish_code_attempt(offer_id, false);
        }
        assert_eq!(
            authority.begin_code_attempt().unwrap_err().code,
            ListenerErrorCode::Unauthorized
        );

        let bytes = output.0.lock().unwrap().clone();
        let mut reader = Cursor::new(bytes);
        let mut events = Vec::new();
        while let Some(event) = ListenerProcessEvent::read(&mut reader).unwrap() {
            events.push(event);
        }
        assert!(matches!(
            events.as_slice(),
            [
                ListenerProcessEvent::PairingCodeAttemptFailed { attempts_left: 2, .. },
                ListenerProcessEvent::PairingCodeAttemptFailed { attempts_left: 1, .. },
                ListenerProcessEvent::PairingFailed { code, .. },
            ] if code == "code_attempts_exhausted"
        ));
        assert!(
            !format!("{events:?}").contains(&code),
            "events never log the code"
        );
        let state = authority
            .repository
            .load()
            .unwrap()
            .authority
            .offers
            .iter()
            .find(|offer| offer.offer_id == offer_id)
            .unwrap()
            .state;
        assert_eq!(state, PairingOfferState::Rejected);
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
            code_offer: Mutex::new(None),
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

    fn events(output: &SharedBuffer) -> Vec<ListenerProcessEvent> {
        let bytes = output.0.lock().unwrap().clone();
        let mut reader = Cursor::new(bytes);
        let mut events = Vec::new();
        while let Ok(Some(event)) = ListenerProcessEvent::read(&mut reader) {
            events.push(event);
        }
        events
    }

    fn wait_for<T>(
        output: &SharedBuffer,
        mut find: impl FnMut(&[ListenerProcessEvent]) -> Option<T>,
    ) -> T {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some(found) = find(&events(output)) {
                return found;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "listener event did not arrive"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn worker_pairs_a_phone_over_tcp_with_the_code_it_shows() {
        if SystemInterfaceProvider
            .eligible_interfaces()
            .unwrap()
            .is_empty()
        {
            return;
        }
        let fixture = tempfile::tempdir().unwrap();
        let controller_root = fixture.path().join("controller");
        let private = StaticPrivateKey::from_fixture_bytes([61; 32]);
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
                    Some(HostIdentitySecretRef::new("identity:code-worker").unwrap());
                authority.state = HostIdentityState::Ready;
                Ok(())
            })
            .unwrap();
        let policy = ControllerListenPolicy {
            enabled: true,
            port: Some(ControllerPort::Generated(
                SystemGeneratedPortSource.next_port().unwrap(),
            )),
            discovery: DiscoveryPolicy::Off,
        };
        let network = ControllerNetworkRepository::open(&controller_root).unwrap();
        let saved = network
            .save(network.load().unwrap().revision, policy.clone())
            .unwrap();
        let descriptor = ListenerLaunchDescriptor::new(
            controller_root.clone(),
            fixture.path().join("projects"),
            fixture.path().join("sessions"),
            fixture.path().join("runtime"),
            saved.revision,
            policy,
            &private,
        )
        .unwrap();

        let (control_reader, mut control) = std::io::pipe().unwrap();
        let output = SharedBuffer::default();
        let worker_output = output.clone();
        let worker = std::thread::spawn(move || {
            run_listener_worker(std::io::BufReader::new(control_reader), worker_output)
        });
        descriptor.write(&mut control).unwrap();
        let address = wait_for(&output, |events| {
            events.iter().find_map(|event| match event {
                ListenerProcessEvent::Ready { addresses, .. } => addresses
                    .iter()
                    .find(|address| address.address.is_ipv4())
                    .or(addresses.first())
                    .map(|address| address.address),
                _ => None,
            })
        });
        ListenerControlCommand::begin_code_pairing()
            .write(&mut control)
            .unwrap();
        let (offer_id, code) = wait_for(&output, |events| {
            events.iter().find_map(|event| match event {
                ListenerProcessEvent::PairingCode { offer_id, code, .. } => {
                    Some((*offer_id, code.clone()))
                }
                _ => None,
            })
        });

        let paired = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
                ControllerConnectionPurpose::PairCode
                    .write_to(&mut stream)
                    .await
                    .unwrap();
                crate::pair_controller_with_code_client(
                    &mut stream,
                    &multiplex_controller_security::PairingCode::parse(&code).unwrap(),
                    StaticPrivateKey::from_fixture_bytes([62; 32]),
                    StaticPrivateKey::from_fixture_bytes([63; 32]),
                    &mut SystemHandshakeEntropy,
                    ControllerDeviceId::new(),
                    "Test phone".into(),
                    |_| Ok(()),
                )
                .await
            })
            .unwrap_or_else(|error| {
                panic!(
                    "pairing failed: {error:?}; events: {:?}",
                    events(&output)
                        .iter()
                        .map(|event| serde_json::to_string(event).unwrap())
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(paired.host_public_key.0, public.0);

        wait_for(&output, |events| {
            events.iter().find_map(|event| match event {
                ListenerProcessEvent::PairingComplete {
                    offer_id: completed,
                    device_id,
                    ..
                } if *completed == offer_id && *device_id == paired.device_id => Some(()),
                _ => None,
            })
        });
        let authority = ControllerDeviceRepository::open(&controller_root)
            .unwrap()
            .load()
            .unwrap()
            .authority;
        assert!(
            authority
                .devices
                .iter()
                .any(|device| device.device_id == paired.device_id)
        );
        drop(control);
        worker.join().unwrap().unwrap();
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
        assert!(
            offer
                .routes
                .iter()
                .any(|route| route.address == interface.address)
        );
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
