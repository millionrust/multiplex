use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{ObjectFit, RenderImage, StyledImage as _, img};
use gpui_component::Disableable as _;
use image::{Frame, Rgba, RgbaImage};
use qrcode::{Color, QrCode};
use smallvec::SmallVec;
use termirust_controller_listener::{
    GeneratedPortSource as _, ListenerLaunchDescriptor, ListenerProcessEvent,
    ProcessPairingDecision, SystemGeneratedPortSource,
};
use termirust_controller_security::StaticPrivateKey;
use termirust_domain::{
    ControllerCapability, ControllerDeviceId, ControllerListenPolicy, ControllerNetworkRevision,
    ControllerPort, DiscoveryPolicy, HostIdentityPublic, HostIdentityState, ListenerState,
    ListeningAddress, PairedDeviceRecord, PairedDeviceStatus, PairingOfferId,
};
use termirust_store::{
    ControllerDeviceRepository, ControllerDeviceStoreError, ControllerNetworkRepository,
};

use crate::controller::host_identity::{
    HostIdentityService, OldSecretDeletion, OsIdentityEntropy, OsSecretStore,
};
use crate::controller::lan::ControllerListenerProcess;
use crate::controller::ssh_pairing::SshPairingBroker;

use super::controller_coordinator::{
    ControllerListenerEventProjection, ControllerPairingFailureKind,
};

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteDevicesFailure {
    Corrupt,
    Newer,
    Permission,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
enum PairingUiState {
    Idle,
    Generating,
    Waiting,
    SasReady,
    SasMismatch,
    Expired,
    RateLimited,
    StorageFailure,
    Uncertain,
    Paired,
    Revoked,
    CodeShown,
    CodeWrong,
    CodeExhausted,
}

pub(super) struct RemoteDevicesState {
    repository: Option<ControllerDeviceRepository>,
    identity_state: HostIdentityState,
    identity: Option<HostIdentityPublic>,
    devices: Vec<PairedDeviceRecord>,
    failure: Option<RemoteDevicesFailure>,
    route_available: bool,
    network_repository: Option<ControllerNetworkRepository>,
    network_revision: ControllerNetworkRevision,
    network_policy: ControllerListenPolicy,
    listening_addresses: Vec<ListeningAddress>,
    listener_state: ListenerState,
    listener_process: Option<ControllerListenerProcess>,
    desktop_pane_bridge: Option<termirust_controller_listener::DesktopPaneBridgeEndpoint>,
    tmux_sessions: bool,
    listener_last_polled: Instant,
    host_private: Option<StaticPrivateKey>,
    pairing_state: PairingUiState,
    pairing_offer_id: Option<PairingOfferId>,
    pairing_offer_text: Option<String>,
    pairing_offer_qr: Option<Arc<RenderImage>>,
    pairing_sas: Option<String>,
    /// The six-digit code while pairing mode is open. Shown on this desktop only.
    pairing_code: Option<String>,
    pairing_code_expires_at: Option<u64>,
    pairing_code_attempts_left: u8,
    /// This computer's Tailscale name, looked up in the background when pairing starts.
    tailscale_name: Arc<std::sync::Mutex<Option<String>>>,
    ssh_pairing_broker: Option<SshPairingBroker>,
    ssh_pairing_active: bool,
    ssh_pairing_expires_at: Option<u64>,
    ssh_pairing_device_count: usize,
    editing_device_id: Option<ControllerDeviceId>,
}

impl RemoteDevicesState {
    #[cfg(not(test))]
    pub(super) fn open_default(
        controller_coordinator: &ControllerCoordinator,
        desktop_pane_bridge: Option<termirust_controller_listener::DesktopPaneBridgeEndpoint>,
        tmux_sessions: bool,
    ) -> Self {
        let root = match crate::storage::controller_store_dir() {
            Ok(root) => root,
            Err(_) => return Self::failed(RemoteDevicesFailure::Unavailable),
        };
        let repository =
            match ControllerDeviceRepository::open(root.clone()).map_err(classify_store_failure) {
                Ok(repository) => repository,
                Err(failure) => return Self::failed(failure),
            };
        let identity_service =
            HostIdentityService::new(repository.clone(), OsSecretStore, OsIdentityEntropy);
        let identity = match identity_service.load_or_create() {
            Ok(identity) => identity,
            Err(_) => return Self::failed(RemoteDevicesFailure::Permission),
        };
        let network_repository = match ControllerNetworkRepository::open(&root) {
            Ok(repository) => repository,
            Err(_) => return Self::failed(RemoteDevicesFailure::Unavailable),
        };
        let network = match network_repository.load() {
            Ok(snapshot) => snapshot,
            Err(_) => return Self::failed(RemoteDevicesFailure::Unavailable),
        };
        let host_private = identity.static_private_key();
        let ssh_pairing_broker =
            SshPairingBroker::bind(durable_runtime_parent(&root).join("controller-pairing.sock"))
                .ok();
        match repository.load() {
            Ok(snapshot) => {
                let devices = deduplicated_devices(snapshot.authority.devices);
                let device_count = devices.len();
                let mut state = Self {
                    repository: Some(repository),
                    identity_state: identity.state,
                    identity: identity.public,
                    devices,
                    failure: None,
                    route_available: false,
                    network_repository: Some(network_repository),
                    network_revision: network.revision,
                    network_policy: network.policy,
                    listening_addresses: Vec::new(),
                    listener_state: ListenerState::Disabled,
                    listener_process: None,
                    desktop_pane_bridge,
                    tmux_sessions,
                    listener_last_polled: Instant::now(),
                    host_private,
                    pairing_state: PairingUiState::Idle,
                    pairing_offer_id: None,
                    pairing_offer_text: None,
                    pairing_offer_qr: None,
                    pairing_sas: None,
                    pairing_code: None,
                    pairing_code_expires_at: None,
                    pairing_code_attempts_left: 0,
                    tailscale_name: Arc::default(),
                    ssh_pairing_broker,
                    ssh_pairing_active: false,
                    ssh_pairing_expires_at: None,
                    ssh_pairing_device_count: device_count,
                    editing_device_id: None,
                };
                if state.network_policy.enabled {
                    let _ = state.start_listener_process(controller_coordinator);
                }
                state
            }
            Err(error) => Self::failed(classify_store_failure(error)),
        }
    }

    #[cfg(test)]
    pub(super) fn open_default(
        _controller_coordinator: &ControllerCoordinator,
        _desktop_pane_bridge: Option<termirust_controller_listener::DesktopPaneBridgeEndpoint>,
        tmux_sessions: bool,
    ) -> Self {
        Self {
            repository: None,
            identity_state: HostIdentityState::Ready,
            identity: Some(HostIdentityPublic::new(
                termirust_domain::HostIdentityGeneration::INITIAL,
                termirust_domain::HostPublicKey([4; 32]),
            )),
            devices: Vec::new(),
            failure: None,
            route_available: false,
            network_repository: None,
            network_revision: ControllerNetworkRevision::ZERO,
            network_policy: ControllerListenPolicy::default(),
            listening_addresses: Vec::new(),
            listener_state: ListenerState::Disabled,
            listener_process: None,
            desktop_pane_bridge: None,
            tmux_sessions,
            listener_last_polled: Instant::now(),
            host_private: Some(StaticPrivateKey::from_fixture_bytes([3; 32])),
            pairing_state: PairingUiState::Idle,
            pairing_offer_id: None,
            pairing_offer_text: None,
            pairing_offer_qr: None,
            pairing_sas: None,
            pairing_code: None,
            pairing_code_expires_at: None,
            pairing_code_attempts_left: 0,
            tailscale_name: Arc::default(),
            ssh_pairing_broker: None,
            ssh_pairing_active: false,
            ssh_pairing_expires_at: None,
            ssh_pairing_device_count: 0,
            editing_device_id: None,
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    fn failed(failure: RemoteDevicesFailure) -> Self {
        Self {
            repository: None,
            identity_state: HostIdentityState::Lost,
            identity: None,
            devices: Vec::new(),
            failure: Some(failure),
            route_available: false,
            network_repository: None,
            network_revision: ControllerNetworkRevision::ZERO,
            network_policy: ControllerListenPolicy::default(),
            listening_addresses: Vec::new(),
            listener_state: ListenerState::Failed(termirust_domain::ListenerFailureCode::Internal),
            listener_process: None,
            desktop_pane_bridge: None,
            tmux_sessions: false,
            listener_last_polled: Instant::now(),
            host_private: None,
            pairing_state: PairingUiState::StorageFailure,
            pairing_offer_id: None,
            pairing_offer_text: None,
            pairing_offer_qr: None,
            pairing_sas: None,
            pairing_code: None,
            pairing_code_expires_at: None,
            pairing_code_attempts_left: 0,
            tailscale_name: Arc::default(),
            ssh_pairing_broker: None,
            ssh_pairing_active: false,
            ssh_pairing_expires_at: None,
            ssh_pairing_device_count: 0,
            editing_device_id: None,
        }
    }

    fn refresh(&mut self) -> Result<(), ()> {
        let repository = self.repository.as_ref().ok_or(())?;
        let snapshot = repository.load().map_err(|error| {
            self.failure = Some(classify_store_failure(error));
        })?;
        self.identity_state = snapshot.authority.state;
        self.identity = snapshot.authority.identity;
        self.devices = deduplicated_devices(snapshot.authority.devices);
        self.failure = None;
        Ok(())
    }

    fn reset_identity(
        &mut self,
        controller_coordinator: &ControllerCoordinator,
    ) -> Result<OldSecretDeletion, ()> {
        self.stop_listener(controller_coordinator)?;
        let repository = self.repository.clone().ok_or(())?;
        let outcome = HostIdentityService::new(repository, OsSecretStore, OsIdentityEntropy)
            .reset()
            .map_err(|_| ())?;
        self.host_private = outcome.identity.static_private_key();
        self.identity_state = outcome.identity.state;
        self.identity = outcome.identity.public;
        self.refresh()?;
        Ok(outcome.deletion)
    }

    /// Turns remote access on for every private address. An empty port keeps the port used
    /// before, so paired phones find the computer where they left it.
    fn enable_listener(
        &mut self,
        fixed_port: Option<u16>,
        controller_coordinator: &ControllerCoordinator,
    ) -> Result<(), ()> {
        let port = match (fixed_port, self.network_policy.port) {
            (Some(port), _) => ControllerPort::user_fixed(port).map_err(|_| ())?,
            (None, Some(previous @ ControllerPort::Generated(_))) => previous,
            (None, _) => {
                let mut generator = SystemGeneratedPortSource;
                ControllerPort::generated(generator.next_port().map_err(|_| ())?).map_err(|_| ())?
            }
        };
        let policy = ControllerListenPolicy {
            enabled: true,
            port: Some(port),
            discovery: DiscoveryPolicy::Off,
        };
        let repository = self.network_repository.clone().ok_or(())?;
        let saved = repository
            .save(self.network_revision, policy)
            .map_err(|_| ())?;
        self.network_revision = saved.revision;
        self.network_policy = saved.policy;
        if self.start_listener_process(controller_coordinator).is_err() {
            let _ = self.disable_saved_policy();
            return Err(());
        }
        Ok(())
    }

    fn start_listener_process(
        &mut self,
        controller_coordinator: &ControllerCoordinator,
    ) -> Result<(), ()> {
        self.listener_state = ListenerState::Binding;
        let host_private = self.host_private.as_ref().ok_or(())?;
        let app_root = crate::storage::app_dir().map_err(|_| ())?;
        // A background service serving the route hands it over before this listener binds.
        #[cfg(not(test))]
        crate::controller::background_service::request_yield(&durable_runtime_parent(&app_root));
        let descriptor = ListenerLaunchDescriptor::new(
            crate::storage::controller_store_dir().map_err(|_| ())?,
            crate::storage::project_store_dir().map_err(|_| ())?,
            app_root.join("durable-sessions"),
            durable_runtime_parent(&app_root),
            self.network_revision,
            self.network_policy.clone(),
            host_private,
        )
        .and_then(|descriptor| {
            descriptor
                .with_desktop_pane_bridge(self.desktop_pane_bridge.clone())
                .map(|descriptor| descriptor.with_tmux_sessions(self.tmux_sessions))
        })
        .map_err(|_| ())?;
        match controller_coordinator.start_listener(&descriptor) {
            Ok(process) => {
                self.listening_addresses = process.ready_addresses.clone();
                self.listener_process = Some(process);
                if let Some(repository) = &self.network_repository
                    && let Ok(snapshot) = repository.load()
                {
                    self.network_revision = snapshot.revision;
                    self.network_policy = snapshot.policy;
                }
                self.listener_state = ListenerState::Ready {
                    authenticated_connections: 0,
                };
                self.route_available = true;
                Ok(())
            }
            Err(_) => {
                self.listener_process = None;
                self.listener_state =
                    ListenerState::Failed(termirust_domain::ListenerFailureCode::Internal);
                self.route_available = false;
                Err(())
            }
        }
    }

    fn stop_listener(&mut self, controller_coordinator: &ControllerCoordinator) -> Result<(), ()> {
        self.listener_state = ListenerState::ShuttingDown;
        if let Some(mut process) = self.listener_process.take() {
            controller_coordinator.stop_listener(&mut process);
        }
        self.disable_saved_policy()?;
        self.listener_state = ListenerState::Disabled;
        self.listening_addresses.clear();
        self.route_available = false;
        self.clear_pairing(PairingUiState::Idle, controller_coordinator);
        Ok(())
    }

    /// Changes whether paired devices see tmux sessions. A running listener restarts so the
    /// change applies to new connections; open connections close with it.
    #[cfg(test)]
    pub(super) fn tmux_sessions(&self) -> bool {
        self.tmux_sessions
    }

    pub(super) fn set_tmux_sessions(
        &mut self,
        enabled: bool,
        controller_coordinator: &ControllerCoordinator,
    ) -> Result<(), ()> {
        if self.tmux_sessions == enabled {
            return Ok(());
        }
        self.tmux_sessions = enabled;
        let Some(mut process) = self.listener_process.take() else {
            return Ok(());
        };
        controller_coordinator.stop_listener(&mut process);
        self.clear_pairing(PairingUiState::Idle, controller_coordinator);
        self.start_listener_process(controller_coordinator)
    }

    /// Opens pairing mode: the listener makes a six-digit code a phone can pair with.
    fn begin_code_pairing(
        &mut self,
        controller_coordinator: &ControllerCoordinator,
    ) -> Result<(), ()> {
        self.stop_code_pairing(controller_coordinator);
        let process = self.listener_process.as_mut().ok_or(())?;
        controller_coordinator
            .begin_code_pairing(process)
            .map_err(|_| ())?;
        self.clear_pairing(PairingUiState::Generating, controller_coordinator);
        if self.tailscale_address().is_some() {
            let name = Arc::clone(&self.tailscale_name);
            std::thread::spawn(move || {
                let found = crate::controller::tailscale::magic_dns_name();
                if let Ok(mut name) = name.lock() {
                    *name = found;
                }
            });
        }
        Ok(())
    }

    /// Closes pairing mode, discarding its code, if it is open.
    fn stop_code_pairing(&mut self, controller_coordinator: &ControllerCoordinator) {
        if self.pairing_code.is_none() {
            return;
        }
        if let (Some(offer_id), Some(process)) =
            (self.pairing_offer_id, self.listener_process.as_mut())
        {
            let _ = controller_coordinator.cancel_code_pairing(process, offer_id);
        }
        self.clear_pairing(PairingUiState::Idle, controller_coordinator);
    }

    /// The Tailscale address a phone should type, as `name:port` or `100.x.y.z:port`, when
    /// the listener is reachable over Tailscale.
    fn tailscale_address(&self) -> Option<String> {
        let tailnet = self.listening_addresses.iter().find(|listening| {
            matches!(listening.address.ip(), std::net::IpAddr::V4(ip)
                if ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
        })?;
        let name = self
            .tailscale_name
            .lock()
            .ok()
            .and_then(|name| name.clone());
        Some(match name {
            Some(name) => format!("{name}:{}", tailnet.address.port()),
            None => tailnet.address.to_string(),
        })
    }

    fn begin_pairing(&mut self, controller_coordinator: &ControllerCoordinator) -> Result<(), ()> {
        self.stop_code_pairing(controller_coordinator);
        let process = self.listener_process.as_mut().ok_or(())?;
        controller_coordinator
            .begin_pairing(process)
            .map_err(|_| ())?;
        self.clear_pairing(PairingUiState::Generating, controller_coordinator);
        Ok(())
    }

    fn decide_pairing(
        &mut self,
        decision: ProcessPairingDecision,
        controller_coordinator: &ControllerCoordinator,
    ) -> Result<(), ()> {
        let offer_id = self.pairing_offer_id.ok_or(())?;
        if self.ssh_pairing_active {
            controller_coordinator
                .decide_ssh_pairing(
                    self.ssh_pairing_broker.as_mut().ok_or(())?,
                    offer_id,
                    decision,
                )
                .map_err(|_| ())?;
        } else {
            controller_coordinator
                .decide_listener_pairing(
                    self.listener_process.as_mut().ok_or(())?,
                    offer_id,
                    decision,
                )
                .map_err(|_| ())?;
        }
        if decision == ProcessPairingDecision::Reject {
            self.clear_pairing(PairingUiState::SasMismatch, controller_coordinator);
        } else {
            self.pairing_sas = None;
            self.pairing_state = PairingUiState::Waiting;
        }
        Ok(())
    }

    fn refresh_listener_process(
        &mut self,
        controller_coordinator: &ControllerCoordinator,
    ) -> Result<bool, ()> {
        if self.listener_last_polled.elapsed()
            < Duration::from_millis(
                termirust_ui_contract::DesignTokens::new(termirust_ui_contract::ThemeKind::System)
                    .motion_remote_device_poll(false)
                    .0 as u64,
            )
        {
            return Ok(false);
        }
        self.listener_last_polled = Instant::now();
        let mut changed = false;
        if let Some(prompt) = self
            .ssh_pairing_broker
            .as_mut()
            .and_then(|broker| controller_coordinator.poll_ssh_pairing(broker))
        {
            if self.pairing_offer_id.is_some() {
                if let Some(broker) = self.ssh_pairing_broker.as_mut() {
                    let _ = controller_coordinator.decide_ssh_pairing(
                        broker,
                        prompt.offer_id,
                        ProcessPairingDecision::Reject,
                    );
                }
            } else {
                self.pairing_offer_id = Some(prompt.offer_id);
                self.pairing_offer_text = None;
                self.pairing_sas = Some(prompt.sas);
                self.pairing_state = PairingUiState::SasReady;
                self.ssh_pairing_active = true;
                self.ssh_pairing_expires_at = Some(prompt.expires_at_unix_seconds);
                self.ssh_pairing_device_count = self.devices.len();
                changed = true;
            }
        }
        if self.ssh_pairing_active && self.pairing_state == PairingUiState::Waiting {
            self.refresh()?;
            if self.devices.len() > self.ssh_pairing_device_count {
                self.clear_pairing(PairingUiState::Paired, controller_coordinator);
                changed = true;
            } else if self
                .ssh_pairing_expires_at
                .is_some_and(|expires_at| unix_seconds() > expires_at)
            {
                self.clear_pairing(PairingUiState::Expired, controller_coordinator);
                changed = true;
            }
        }
        if self
            .pairing_code_expires_at
            .is_some_and(|expires_at| unix_seconds() > expires_at)
        {
            self.stop_code_pairing(controller_coordinator);
            self.pairing_state = PairingUiState::Expired;
            changed = true;
        }
        let events = match self.listener_process.as_mut() {
            Some(process) => controller_coordinator
                .drain_listener_events(process)
                .map_err(|_| ())?,
            None => return Ok(changed),
        };
        for event in events {
            self.apply_listener_event(event, controller_coordinator)?;
            changed = true;
        }
        let exited = self
            .listener_process
            .as_mut()
            .is_some_and(|process| !controller_coordinator.listener_is_running(process));
        if !exited {
            return Ok(changed);
        }
        self.listener_process.take();
        self.listener_state =
            ListenerState::Failed(termirust_domain::ListenerFailureCode::Internal);
        self.listening_addresses.clear();
        self.route_available = false;
        Err(())
    }

    fn apply_listener_event(
        &mut self,
        event: ListenerProcessEvent,
        controller_coordinator: &ControllerCoordinator,
    ) -> Result<(), ()> {
        let projection = controller_coordinator
            .project_listener_event(self.pairing_offer_id, event)
            .map_err(|_| ())?;
        match projection {
            ControllerListenerEventProjection::Offer {
                offer_id,
                offer_text,
            } => {
                self.pairing_offer_qr = pairing_offer_qr(&offer_text);
                self.pairing_offer_id = Some(offer_id);
                self.pairing_offer_text = Some(offer_text);
                self.pairing_sas = None;
                self.pairing_state = PairingUiState::Waiting;
            }
            ControllerListenerEventProjection::SasReady { sas } => {
                self.pairing_sas = Some(sas);
                self.pairing_state = PairingUiState::SasReady;
            }
            ControllerListenerEventProjection::Complete => {
                self.clear_pairing(PairingUiState::Paired, controller_coordinator);
                self.refresh()?;
            }
            ControllerListenerEventProjection::Failed { failure } => {
                let state = match failure {
                    ControllerPairingFailureKind::RateLimited => PairingUiState::RateLimited,
                    ControllerPairingFailureKind::CodeAttemptsExhausted => {
                        PairingUiState::CodeExhausted
                    }
                    ControllerPairingFailureKind::Expired => PairingUiState::Expired,
                    ControllerPairingFailureKind::Uncertain => PairingUiState::Uncertain,
                    ControllerPairingFailureKind::Storage => PairingUiState::StorageFailure,
                };
                self.clear_pairing(state, controller_coordinator);
            }
            ControllerListenerEventProjection::Code {
                offer_id,
                code,
                expires_at_unix_seconds,
                attempts_left,
            } => {
                self.pairing_offer_id = Some(offer_id);
                self.pairing_offer_text = None;
                self.pairing_offer_qr = None;
                self.pairing_sas = None;
                self.pairing_code = Some(code);
                self.pairing_code_expires_at = Some(expires_at_unix_seconds);
                self.pairing_code_attempts_left = attempts_left;
                self.pairing_state = PairingUiState::CodeShown;
            }
            ControllerListenerEventProjection::CodeAttemptFailed { attempts_left } => {
                self.pairing_code_attempts_left = attempts_left;
                self.pairing_state = PairingUiState::CodeWrong;
            }
            ControllerListenerEventProjection::Addresses { addresses } => {
                self.listening_addresses = addresses
                    .into_iter()
                    .filter(|address| address.validate().is_ok())
                    .collect();
            }
        }
        Ok(())
    }

    fn clear_pairing(
        &mut self,
        state: PairingUiState,
        controller_coordinator: &ControllerCoordinator,
    ) {
        if self.ssh_pairing_active
            && let (Some(offer_id), Some(broker)) =
                (self.pairing_offer_id, self.ssh_pairing_broker.as_mut())
            && broker.pending_offer_id() == Some(offer_id)
        {
            let _ = controller_coordinator.decide_ssh_pairing(
                broker,
                offer_id,
                ProcessPairingDecision::Reject,
            );
        }
        self.pairing_state = state;
        self.pairing_offer_id = None;
        self.pairing_offer_text = None;
        self.pairing_offer_qr = None;
        self.pairing_sas = None;
        self.pairing_code = None;
        self.pairing_code_expires_at = None;
        self.pairing_code_attempts_left = 0;
        self.ssh_pairing_active = false;
        self.ssh_pairing_expires_at = None;
        self.ssh_pairing_device_count = self.devices.len();
    }

    fn disable_saved_policy(&mut self) -> Result<(), ()> {
        if !self.network_policy.enabled {
            return Ok(());
        }
        let mut policy = self.network_policy.clone();
        policy.enabled = false;
        let saved = self
            .network_repository
            .clone()
            .ok_or(())?
            .save(self.network_revision, policy)
            .map_err(|_| ())?;
        self.network_revision = saved.revision;
        self.network_policy = saved.policy;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(super) fn durable_runtime_parent(_: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/private/tmp/termirust-{}", unsafe {
        libc::geteuid()
    }))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn durable_runtime_parent(_: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/tmp/termirust-{}", unsafe { libc::geteuid() }))
}

#[cfg(not(unix))]
pub(super) fn durable_runtime_parent(app_root: &std::path::Path) -> std::path::PathBuf {
    app_root.join("session-host-runtime")
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl TermiRustApp {
    fn render_remote_devices_content(&self, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .gap_3()
            .child(self.render_remote_route_section(cx))
            .child(self.settings_divider())
            .child(self.render_remote_terminals_section(cx))
            .child(self.settings_divider())
            .child(self.render_remote_identity_section(cx))
            .child(self.settings_divider())
            .child(self.render_trusted_remote_devices(cx))
            .child(self.settings_divider())
            .child(self.render_remote_identity_reset_section(cx))
            .into_any_element()
    }

    pub(super) fn render_remote_devices_settings_card(&self, cx: &Context<Self>) -> Div {
        self.settings_section_card(
            localization::remote_devices_title(),
            localization::remote_devices_description(),
            self.render_remote_devices_content(cx),
        )
    }

    pub(super) fn render_devices_view(&self, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id("devices-view")
            .debug_selector(|| "devices-view".to_string())
            .flex_1()
            .min_h_0()
            .bg(theme::library_bg())
            .child(
                v_flex()
                    .flex_none()
                    .gap(px(theme::SPACE_2))
                    .px(px(theme::SPACE_6))
                    .py(px(theme::SPACE_5))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_size(px(theme::TYPE_HEADING_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(localization::remote_devices_title()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::text_muted())
                            .child(localization::remote_devices_description()),
                    ),
            )
            .child(
                v_flex()
                    .id("devices-scroll")
                    .debug_selector(|| "devices-scroll".to_string())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(theme::SPACE_6))
                    .child(self.render_remote_devices_content(cx)),
            )
            .into_any_element()
    }

    fn render_remote_route_section(&self, cx: &Context<Self>) -> AnyElement {
        let recording_friendly = self.activity_center.policy().recording_friendly;
        let ready = matches!(
            self.remote_devices.listener_state,
            ListenerState::Ready { .. }
        );
        let starting = matches!(
            self.remote_devices.listener_state,
            ListenerState::Binding | ListenerState::ShuttingDown
        );
        let pairing_offer_qr = self.remote_devices.pairing_offer_qr.clone();
        let pairing_busy = matches!(
            self.remote_devices.pairing_state,
            PairingUiState::Generating | PairingUiState::Waiting | PairingUiState::SasReady
        );
        let pairing_code = self.remote_devices.pairing_code.clone();
        let code_card = pairing_code.map(|code| {
            let minutes_left = self
                .remote_devices
                .pairing_code_expires_at
                .map(|expires_at| expires_at.saturating_sub(unix_seconds()).div_ceil(60))
                .unwrap_or_default();
            let tailscale = self.remote_devices.tailscale_address();
            v_flex()
                .gap_2()
                .p_3()
                .rounded(px(theme::CONTROL_RADIUS))
                .border_1()
                .border_color(theme::with_alpha(theme::accent(), 0.45))
                .bg(theme::accent_soft())
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::remote_devices_pairing_code_help()),
                )
                .child(
                    div()
                        .debug_selector(|| "remote-devices-pairing-code".to_string())
                        .text_size(px(theme::TYPE_TITLE_SIZE))
                        .font_family(theme::current_design_tokens().font_mono_family().0)
                        .font_semibold()
                        .text_color(theme::text_main())
                        .child(grouped_pairing_code(&code)),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(format!(
                            "{} | {}",
                            localization::remote_devices_pairing_code_expiry(minutes_left),
                            localization::remote_devices_pairing_code_attempts(
                                self.remote_devices.pairing_code_attempts_left
                            )
                        )),
                )
                .when_some(tailscale, |this, address| {
                    this.child(
                        div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(localization::remote_devices_pairing_tailscale_hint(
                                &if recording_friendly {
                                    localization::remote_devices_private_address_hidden()
                                } else {
                                    address
                                },
                            )),
                    )
                })
                .child(
                    h_flex().child(
                        Button::new("remote-devices-stop-code-pairing")
                            .debug_selector(|| "remote-devices-stop-code-pairing".to_string())
                            .small()
                            .label(localization::remote_devices_pairing_code_stop_action())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop_code_pairing(cx);
                            })),
                    ),
                )
        });
        let content = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .font_medium()
                                    .text_color(theme::text_main())
                                    .child(localization::remote_devices_route_label()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(listener_state_label(
                                        self.remote_devices.listener_state,
                                    )),
                            ),
                    )
                    .child(if ready {
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("remote-devices-pair-phone")
                                    .debug_selector(|| "remote-devices-pair-phone".to_string())
                                    .small()
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label(localization::remote_devices_pair_phone_action())
                                    .disabled(pairing_busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.begin_code_pairing(cx);
                                    })),
                            )
                            .child(
                                Button::new("remote-devices-stop-listener")
                                    .debug_selector(|| "remote-devices-stop-listener".to_string())
                                    .small()
                                    .danger()
                                    .label(localization::remote_devices_listener_stop_action())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.stop_remote_listener(cx);
                                    })),
                            )
                    } else {
                        h_flex().child(
                            Button::new("remote-listener-confirm")
                                .debug_selector(|| "remote-listener-confirm".to_string())
                                .small()
                                .icon(IconName::Globe)
                                .label(localization::remote_devices_listener_enable_action())
                                .disabled(starting)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.enable_remote_listener(cx);
                                })),
                        )
                    }),
            )
            .when(ready, |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_MICRO_SIZE))
                        .font_medium()
                        .text_color(theme::text_muted())
                        .child(localization::remote_devices_listening_on()),
                )
                .when(self.remote_devices.listening_addresses.is_empty(), |this| {
                    this.child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded(px(theme::CONTROL_RADIUS))
                            .bg(theme::with_alpha(theme::warning(), 0.08))
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(localization::remote_devices_listener_no_interface()),
                    )
                })
                .children(
                    self.remote_devices
                        .listening_addresses
                        .iter()
                        .enumerate()
                        .map(|(index, address)| {
                            h_flex()
                                .id(("remote-listener-address", index))
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .py_1()
                                .border_t_1()
                                .border_color(theme::soft_border())
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .font_medium()
                                        .text_color(theme::text_main())
                                        .child(format!(
                                            "{} | {}",
                                            address.label,
                                            interface_kind_label(address.kind)
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .font_family(
                                            theme::current_design_tokens().font_mono_family().0,
                                        )
                                        .text_color(theme::text_muted())
                                        .child(private_socket_display(
                                            address.address,
                                            recording_friendly,
                                        )),
                                )
                        }),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_MICRO_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::remote_devices_listener_guidance()),
                )
                .when_some(code_card, |this, card| this.child(card))
                .when(
                    self.remote_devices.pairing_offer_text.is_none()
                        && self.remote_devices.pairing_sas.is_none(),
                    |this| {
                        this.child(
                            h_flex().child(
                                Button::new("remote-devices-add-controller")
                                    .debug_selector(|| "remote-devices-add-controller".to_string())
                                    .xsmall()
                                    .ghost()
                                    .label(localization::remote_devices_pairing_other_ways_action())
                                    .disabled(pairing_busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.begin_controller_pairing(cx);
                                    })),
                            ),
                        )
                    },
                )
            })
            .when(!ready, |this| {
                this.child(
                    h_flex()
                        .items_center()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(theme::TYPE_MICRO_SIZE))
                                .text_color(theme::text_muted())
                                .child(localization::remote_devices_listener_port_help()),
                        )
                        .child(
                            div()
                                .w(px(theme::CONNECT_PORT_WIDTH))
                                .child(Input::new(&self.settings_inputs.remote_listener_port)),
                        ),
                )
            })
            .when_some(
                self.remote_devices.pairing_offer_text.clone(),
                |this, offer_text| {
                    let copy_value = offer_text.clone();
                    this.child(
                        v_flex()
                            .gap_3()
                            .p_3()
                            .rounded(px(theme::CONTROL_RADIUS))
                            .border_1()
                            .border_color(theme::soft_border())
                            .bg(theme::library_card())
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::remote_devices_pairing_offer_help()),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .flex_wrap()
                                    .gap_3()
                                    .when_some(pairing_offer_qr, |this, qr| {
                                        this.child(
                                            div()
                                                .p_2()
                                                .rounded(px(theme::CONTROL_RADIUS))
                                                // QR codes need a white quiet zone in every theme to scan.
                                                .bg(gpui::white())
                                                .child(
                                                    img(qr)
                                                        .w(px(theme::PAIRING_QR_SIZE))
                                                        .h(px(theme::PAIRING_QR_SIZE))
                                                        .object_fit(ObjectFit::Contain),
                                                ),
                                        )
                                    })
                                    .child(
                                        Button::new("remote-devices-copy-pairing-offer")
                                            .small()
                                            .icon(IconName::Copy)
                                            .label(localization::remote_devices_pairing_offer_copy_action())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy_value.clone(),
                                                ));
                                                this.status_message = localization::remote_devices_pairing_offer_copied();
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                },
            )
            .when_some(self.remote_devices.pairing_sas.clone(), |this, sas| {
                this.child(
                    v_flex()
                        .gap_2()
                        .p_3()
                        .rounded(px(theme::CONTROL_RADIUS))
                        .border_1()
                        .border_color(theme::with_alpha(theme::warning(), 0.55))
                        .bg(theme::with_alpha(theme::warning(), 0.08))
                        .child(
                            div()
                                .text_size(px(theme::TYPE_MICRO_SIZE))
                                .text_color(theme::text_muted())
                                .child(localization::remote_devices_pairing_sas_ready()),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TYPE_METRIC_SIZE))
                                .font_family(theme::current_design_tokens().font_mono_family().0)
                                .font_medium()
                                .text_color(theme::text_main())
                                .child(sas),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("remote-devices-reject-pairing")
                                        .small()
                                        .danger()
                                        .label(localization::remote_devices_pairing_reject_action())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.decide_controller_pairing(
                                                ProcessPairingDecision::Reject,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    Button::new("remote-devices-confirm-pairing")
                                        .small()
                                        .label(localization::remote_devices_pairing_match_action())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.decide_controller_pairing(
                                                ProcessPairingDecision::Confirm,
                                                cx,
                                            );
                                        })),
                                ),
                        ),
                )
            });

        content
            .child(
                div()
                    .text_size(px(theme::TYPE_MICRO_SIZE))
                    .text_color(theme::text_muted())
                    .child(if ready {
                        pairing_ui_status(self.remote_devices.pairing_state)
                    } else {
                        localization::remote_devices_route_required()
                    }),
            )
            .into_any_element()
    }

    fn render_remote_identity_section(&self, cx: &Context<Self>) -> AnyElement {
        let identity_status = remote_identity_status(
            self.remote_devices.identity_state,
            self.remote_devices.failure,
        );
        let fingerprint = self
            .remote_devices
            .identity
            .as_ref()
            .map(|identity| identity.fingerprint.canonical());
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child(localization::remote_devices_identity_label()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .text_color(theme::text_muted())
                            .child(identity_status),
                    ),
            )
            .when_some(fingerprint, |this, fingerprint| {
                let copy_value = fingerprint.clone();
                this.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .font_family(theme::current_design_tokens().font_mono_family().0)
                                .text_color(theme::text_main())
                                .child(fingerprint),
                        )
                        .child(
                            Button::new("remote-devices-copy-fingerprint")
                                .small()
                                .icon(IconName::Copy)
                                .label(localization::remote_devices_copy_action())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_value.clone(),
                                    ));
                                    this.status_message =
                                        localization::remote_devices_fingerprint_copied();
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::remote_devices_fingerprint_explanation()),
            )
            .into_any_element()
    }

    fn render_trusted_remote_devices(&self, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(localization::remote_devices_trusted_title()),
            )
            .when(self.remote_devices.devices.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::remote_devices_empty()),
                )
            })
            .children(
                self.remote_devices
                    .devices
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, device)| self.render_remote_device_row(index, device, cx)),
            )
            .into_any_element()
    }

    fn render_remote_identity_reset_section(&self, cx: &Context<Self>) -> AnyElement {
        let reset_matches = self
            .settings_inputs
            .remote_identity_reset
            .read(cx)
            .value()
            .as_ref()
            == "RESET";
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_medium()
                    .text_color(theme::danger())
                    .child(localization::remote_devices_reset_title()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::remote_devices_reset_description()),
            )
            .child(Input::new(&self.settings_inputs.remote_identity_reset))
            .child(
                Button::new("remote-devices-reset")
                    .debug_selector(|| "remote-devices-reset".to_string())
                    .small()
                    .danger()
                    .icon(IconName::Delete)
                    .label(localization::remote_devices_reset_action())
                    .disabled(!reset_matches)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reset_remote_identity(cx);
                    })),
            )
            .into_any_element()
    }

    fn render_remote_device_row(
        &self,
        index: usize,
        device: PairedDeviceRecord,
        cx: &Context<Self>,
    ) -> AnyElement {
        let device_id = device.device_id;
        let revoked = device.status == PairedDeviceStatus::Revoked;
        let editing = self.remote_devices.editing_device_id == Some(device_id);
        let input_allowed = device
            .capabilities
            .contains(ControllerCapability::SendInput);
        let status = remote_device_status(device.status);
        let last_seen = device
            .last_seen_at
            .map(|value| {
                localization::remote_devices_last_seen(format_relative_time(value).as_str())
            })
            .unwrap_or_else(localization::remote_devices_never_seen);
        v_flex()
            .id(("remote-device-row", index))
            .gap_2()
            .py_3()
            .border_t_1()
            .border_color(theme::soft_border())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .font_medium()
                                    .text_color(theme::text_main())
                                    .child(device.display_name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::remote_devices_device_detail(
                                        device.fingerprint_suffix(),
                                        status,
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(last_seen),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new(("remote-device-capabilities", index))
                                    .small()
                                    .label(if input_allowed {
                                        localization::remote_devices_restrict_input_action()
                                    } else {
                                        localization::remote_devices_allow_input_action()
                                    })
                                    .disabled(revoked)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_remote_device_input(device_id, cx);
                                    })),
                            )
                            .child(
                                Button::new(("remote-device-revoke", index))
                                    .small()
                                    .danger()
                                    .label(localization::remote_devices_revoke_action())
                                    .disabled(revoked)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.revoke_remote_device(device_id, cx);
                                    })),
                            ),
                    ),
            )
            .when(editing, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(
                            div()
                                .min_w(px(theme::HOST_SIDEBAR_WIDTH))
                                .flex_1()
                                .child(Input::new(&self.settings_inputs.remote_device_name)),
                        )
                        .child(
                            Button::new(("remote-device-name-save", index))
                                .small()
                                .label(localization::remote_devices_name_save_action())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_remote_device_name(device_id, cx);
                                })),
                        )
                        .child(
                            Button::new(("remote-device-name-cancel", index))
                                .small()
                                .label(localization::common_cancel())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remote_devices.editing_device_id = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(!editing, |this| {
                this.child(
                    Button::new(("remote-device-name-edit", index))
                        .small()
                        .label(localization::remote_devices_name_edit_action())
                        .disabled(revoked)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_remote_device_rename(
                                device_id,
                                device.display_name.clone(),
                                window,
                                cx,
                            );
                        })),
                )
            })
            .into_any_element()
    }

    fn start_remote_device_rename(
        &mut self,
        device_id: ControllerDeviceId,
        display_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remote_devices.editing_device_id = Some(device_id);
        self.settings_inputs
            .remote_device_name
            .update(cx, |state, cx| state.set_value(display_name, window, cx));
        cx.notify();
    }

    fn enable_remote_listener(&mut self, cx: &mut Context<Self>) {
        let port_text = self
            .settings_inputs
            .remote_listener_port
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let fixed_port = match parse_listener_port(&port_text) {
            Ok(port) => port,
            Err(()) => {
                self.error_message = localization::remote_devices_listener_port_invalid();
                cx.notify();
                return;
            }
        };
        match self
            .remote_devices
            .enable_listener(fixed_port, &self.controller_coordinator)
        {
            Ok(()) => {
                self.status_message = localization::remote_devices_listener_ready_notice();
                self.error_message.clear();
            }
            Err(()) => {
                self.error_message = localization::remote_devices_listener_start_failed();
            }
        }
        cx.notify();
    }

    fn stop_remote_listener(&mut self, cx: &mut Context<Self>) {
        match self
            .remote_devices
            .stop_listener(&self.controller_coordinator)
        {
            Ok(()) => {
                self.status_message = localization::remote_devices_listener_stopped_notice();
                self.error_message.clear();
            }
            Err(()) => {
                self.error_message = localization::remote_devices_listener_stop_failed();
            }
        }
        cx.notify();
    }

    fn begin_code_pairing(&mut self, cx: &mut Context<Self>) {
        if self
            .remote_devices
            .begin_code_pairing(&self.controller_coordinator)
            .is_err()
        {
            self.error_message = localization::remote_devices_operation_failed();
        } else {
            self.error_message.clear();
        }
        cx.notify();
    }

    fn stop_code_pairing(&mut self, cx: &mut Context<Self>) {
        self.remote_devices
            .stop_code_pairing(&self.controller_coordinator);
        cx.notify();
    }

    fn begin_controller_pairing(&mut self, cx: &mut Context<Self>) {
        if self
            .remote_devices
            .begin_pairing(&self.controller_coordinator)
            .is_err()
        {
            self.error_message = localization::remote_devices_operation_failed();
        } else {
            self.error_message.clear();
        }
        cx.notify();
    }

    fn decide_controller_pairing(
        &mut self,
        decision: ProcessPairingDecision,
        cx: &mut Context<Self>,
    ) {
        if self
            .remote_devices
            .decide_pairing(decision, &self.controller_coordinator)
            .is_err()
        {
            self.error_message = localization::remote_devices_operation_failed();
        } else {
            self.error_message.clear();
        }
        cx.notify();
    }

    pub(super) fn refresh_remote_listener_process(&mut self, cx: &mut Context<Self>) {
        match self
            .remote_devices
            .refresh_listener_process(&self.controller_coordinator)
        {
            Ok(true) => cx.notify(),
            Ok(false) => {}
            Err(()) => {
                self.error_message = localization::remote_devices_listener_start_failed();
                cx.notify();
            }
        }
    }

    fn save_remote_device_name(&mut self, device_id: ControllerDeviceId, cx: &mut Context<Self>) {
        let display_name = self
            .settings_inputs
            .remote_device_name
            .read(cx)
            .value()
            .to_string();
        let result = self
            .remote_devices
            .repository
            .clone()
            .ok_or(())
            .and_then(|repository| {
                self.controller_coordinator
                    .rename_device(repository, device_id, display_name)
                    .map_err(|_| ())
            })
            .and_then(|()| self.remote_devices.refresh());
        if result.is_ok() {
            self.remote_devices.editing_device_id = None;
            self.status_message = localization::remote_devices_name_saved();
        } else {
            self.error_message = localization::remote_devices_operation_failed();
        }
        cx.notify();
    }

    fn revoke_remote_device(&mut self, device_id: ControllerDeviceId, cx: &mut Context<Self>) {
        let result = self
            .remote_devices
            .repository
            .clone()
            .ok_or(())
            .and_then(|repository| {
                self.controller_coordinator
                    .revoke_device(repository, device_id)
                    .map_err(|_| ())
            })
            .and_then(|()| {
                self.remote_devices.pairing_state = PairingUiState::Revoked;
                self.remote_devices.refresh()
            });
        if result.is_ok() {
            self.status_message = localization::remote_devices_revoked_notice();
        } else {
            self.error_message = localization::remote_devices_operation_failed();
        }
        cx.notify();
    }

    fn toggle_remote_device_input(
        &mut self,
        device_id: ControllerDeviceId,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .remote_devices
            .devices
            .iter()
            .find(|device| device.device_id == device_id)
            .map(|device| device.capabilities)
            .ok_or(())
            .and_then(|capabilities| {
                self.remote_devices
                    .repository
                    .clone()
                    .ok_or(())
                    .map(|repository| (repository, capabilities))
            })
            .and_then(|(repository, capabilities)| {
                self.controller_coordinator
                    .toggle_input(repository, device_id, capabilities)
                    .map_err(|_| ())
            })
            .and_then(|()| self.remote_devices.refresh());
        if result.is_ok() {
            self.status_message = localization::remote_devices_capabilities_saved();
        } else {
            self.error_message = localization::remote_devices_operation_failed();
        }
        cx.notify();
    }

    fn reset_remote_identity(&mut self, cx: &mut Context<Self>) {
        let confirmed = self
            .settings_inputs
            .remote_identity_reset
            .read(cx)
            .value()
            .as_ref()
            == "RESET";
        if !confirmed {
            self.error_message = localization::remote_devices_reset_confirmation_required();
            cx.notify();
            return;
        }
        match self
            .remote_devices
            .reset_identity(&self.controller_coordinator)
        {
            Ok(OldSecretDeletion::Failed(_)) => {
                self.status_message = localization::remote_devices_reset_old_key_warning();
            }
            Ok(_) => self.status_message = localization::remote_devices_reset_complete(),
            Err(()) => self.error_message = localization::remote_devices_operation_failed(),
        }
        cx.notify();
    }
}

fn classify_store_failure(error: ControllerDeviceStoreError) -> RemoteDevicesFailure {
    match error {
        ControllerDeviceStoreError::Corrupt
        | ControllerDeviceStoreError::TooLarge
        | ControllerDeviceStoreError::UnsafeEntry
        | ControllerDeviceStoreError::Domain(_) => RemoteDevicesFailure::Corrupt,
        ControllerDeviceStoreError::Newer { .. } => RemoteDevicesFailure::Newer,
        ControllerDeviceStoreError::Io {
            kind: std::io::ErrorKind::PermissionDenied,
            ..
        } => RemoteDevicesFailure::Permission,
        ControllerDeviceStoreError::Io { .. }
        | ControllerDeviceStoreError::StaleRevision { .. }
        | ControllerDeviceStoreError::RevisionOverflow => RemoteDevicesFailure::Unavailable,
    }
}

fn remote_identity_status(
    state: HostIdentityState,
    failure: Option<RemoteDevicesFailure>,
) -> String {
    if let Some(failure) = failure {
        return match failure {
            RemoteDevicesFailure::Corrupt => localization::remote_devices_store_corrupt(),
            RemoteDevicesFailure::Newer => localization::remote_devices_store_newer(),
            RemoteDevicesFailure::Permission => localization::remote_devices_permission_denied(),
            RemoteDevicesFailure::Unavailable => localization::remote_devices_unavailable(),
        };
    }
    match state {
        HostIdentityState::Ready => localization::remote_devices_identity_ready(),
        HostIdentityState::Locked => localization::remote_devices_identity_locked(),
        HostIdentityState::PermissionDenied => localization::remote_devices_permission_denied(),
        HostIdentityState::Lost => localization::remote_devices_identity_lost(),
        HostIdentityState::ResetRequired => localization::remote_devices_reset_required(),
    }
}

fn listener_state_label(state: ListenerState) -> String {
    match state {
        ListenerState::Disabled => localization::remote_devices_route_off(),
        ListenerState::Binding => localization::remote_devices_listener_binding(),
        ListenerState::Ready { .. } => localization::remote_devices_listener_ready(),
        ListenerState::InterfaceGone => localization::remote_devices_listener_interface_gone(),
        ListenerState::PortConflict => localization::remote_devices_listener_port_conflict(),
        ListenerState::FirewallBlocked => localization::remote_devices_listener_firewall_blocked(),
        ListenerState::Failed(_) => localization::remote_devices_listener_failed(),
        ListenerState::ShuttingDown => localization::remote_devices_listener_stopping(),
    }
}

fn interface_kind_label(kind: termirust_domain::NetworkInterfaceKind) -> &'static str {
    match kind {
        termirust_domain::NetworkInterfaceKind::Lan => "LAN",
        termirust_domain::NetworkInterfaceKind::Vpn => "VPN",
    }
}

fn parse_listener_port(value: &str) -> Result<Option<u16>, ()> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port >= termirust_domain::USER_FIXED_PORT_MIN)
        .map(Some)
        .ok_or(())
}

fn private_socket_display(address: std::net::SocketAddr, recording_friendly: bool) -> String {
    if recording_friendly {
        localization::remote_devices_private_address_hidden()
    } else {
        format!("\u{2068}{address}\u{2069}")
    }
}

fn remote_device_status(status: PairedDeviceStatus) -> String {
    match status {
        PairedDeviceStatus::Online => localization::remote_devices_status_online(),
        PairedDeviceStatus::Offline => localization::remote_devices_status_offline(),
        PairedDeviceStatus::Revoked => localization::remote_devices_status_revoked(),
    }
}

fn pairing_offer_qr(offer_text: &str) -> Option<Arc<RenderImage>> {
    const QUIET_ZONE_MODULES: usize = 4;
    const TARGET_PIXELS: usize = 512;

    let code = QrCode::new(offer_text.as_bytes()).ok()?;
    let modules = code.width();
    let total_modules = modules.checked_add(QUIET_ZONE_MODULES * 2)?;
    let module_pixels = (TARGET_PIXELS / total_modules).max(1);
    let image_size = u32::try_from(total_modules.checked_mul(module_pixels)?).ok()?;
    let mut image = RgbaImage::from_pixel(image_size, image_size, Rgba([255, 255, 255, 255]));

    for y in 0..modules {
        for x in 0..modules {
            if code[(x, y)] != Color::Dark {
                continue;
            }
            let left = (x + QUIET_ZONE_MODULES) * module_pixels;
            let top = (y + QUIET_ZONE_MODULES) * module_pixels;
            for pixel_y in top..top + module_pixels {
                for pixel_x in left..left + module_pixels {
                    image.put_pixel(
                        u32::try_from(pixel_x).ok()?,
                        u32::try_from(pixel_y).ok()?,
                        Rgba([0, 0, 0, 255]),
                    );
                }
            }
        }
    }

    Some(Arc::new(RenderImage::new(SmallVec::from_vec(vec![
        Frame::new(image),
    ]))))
}

fn deduplicated_devices(devices: Vec<PairedDeviceRecord>) -> Vec<PairedDeviceRecord> {
    let mut unique: Vec<PairedDeviceRecord> = Vec::with_capacity(devices.len());
    for device in devices {
        if let Some(existing) = unique
            .iter_mut()
            .find(|existing| existing.device_id == device.device_id)
        {
            if device.created_at >= existing.created_at {
                *existing = device;
            }
        } else {
            unique.push(device);
        }
    }
    unique
}

/// Shows a six-digit code as two groups of three, which is easier to read and type.
fn grouped_pairing_code(code: &str) -> String {
    if code.len() == 6 && code.is_ascii() {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_owned()
    }
}

fn pairing_ui_status(state: PairingUiState) -> String {
    match state {
        PairingUiState::Idle => localization::remote_devices_pairing_idle(),
        PairingUiState::Generating => localization::remote_devices_pairing_generating(),
        PairingUiState::Waiting => localization::remote_devices_pairing_waiting(),
        PairingUiState::SasReady => localization::remote_devices_pairing_sas_ready(),
        PairingUiState::SasMismatch => localization::remote_devices_pairing_sas_mismatch(),
        PairingUiState::Expired => localization::remote_devices_pairing_expired(),
        PairingUiState::RateLimited => localization::remote_devices_pairing_rate_limited(),
        PairingUiState::StorageFailure => localization::remote_devices_pairing_storage_failure(),
        PairingUiState::Uncertain => localization::remote_devices_pairing_uncertain(),
        PairingUiState::Paired => localization::remote_devices_pairing_paired(),
        PairingUiState::Revoked => localization::remote_devices_pairing_revoked(),
        PairingUiState::CodeShown => localization::remote_devices_pairing_code_help(),
        PairingUiState::CodeWrong => localization::remote_devices_pairing_code_wrong(),
        PairingUiState::CodeExhausted => localization::remote_devices_pairing_code_exhausted(),
    }
}

#[cfg(test)]
mod network_tests {
    use termirust_controller_listener::ListenerProcessEvent;
    use termirust_domain::{
        HostIdentityState, ListenerFailureCode, ListenerState, PairedDeviceStatus, PairingOfferId,
    };

    use crate::ui::localization;

    use super::{
        ControllerCoordinator, PairingUiState, RemoteDevicesState, listener_state_label,
        pairing_ui_status, parse_listener_port, private_socket_display, remote_device_status,
        remote_identity_status,
    };

    #[test]
    fn remote_devices_add_controller_is_disabled_without_route() {
        let state =
            RemoteDevicesState::open_default(&ControllerCoordinator::default(), None, false);
        assert!(!state.route_available);
        assert!(state.devices.is_empty());
        assert_eq!(
            localization::remote_devices_route_required(),
            "Turn on remote access to pair a phone."
        );
    }

    #[test]
    fn listener_port_input_defaults_to_generated_and_rejects_unsafe_values() {
        assert_eq!(parse_listener_port(""), Ok(None));
        assert_eq!(parse_listener_port(" 49152 "), Ok(Some(49_152)));
        assert_eq!(parse_listener_port("1024"), Ok(Some(1_024)));
        assert_eq!(parse_listener_port("65535"), Ok(Some(65_535)));
        assert_eq!(parse_listener_port("0"), Err(()));
        assert_eq!(parse_listener_port("1023"), Err(()));
        assert_eq!(parse_listener_port("65536"), Err(()));
        assert_eq!(parse_listener_port("not-a-port"), Err(()));
    }

    #[test]
    fn route_display_is_bidi_isolated_or_recording_safe() {
        let address = "192.168.1.20:55000".parse().unwrap();
        let visible = private_socket_display(address, false);
        assert!(visible.starts_with('\u{2068}'));
        assert!(visible.ends_with('\u{2069}'));
        assert!(visible.contains("192.168.1.20"));
        let hidden = private_socket_display(address, true);
        assert!(!hidden.contains("192.168.1.20"));
    }

    #[test]
    fn every_listener_state_has_an_explicit_label() {
        for state in [
            ListenerState::Disabled,
            ListenerState::Binding,
            ListenerState::Ready {
                authenticated_connections: 0,
            },
            ListenerState::InterfaceGone,
            ListenerState::PortConflict,
            ListenerState::FirewallBlocked,
            ListenerState::Failed(ListenerFailureCode::Internal),
            ListenerState::ShuttingDown,
        ] {
            assert!(!listener_state_label(state).is_empty());
        }
    }

    #[test]
    fn remote_devices_reports_every_pairing_and_identity_recovery_state() {
        for state in [
            PairingUiState::Idle,
            PairingUiState::Generating,
            PairingUiState::Waiting,
            PairingUiState::SasReady,
            PairingUiState::SasMismatch,
            PairingUiState::Expired,
            PairingUiState::RateLimited,
            PairingUiState::StorageFailure,
            PairingUiState::Uncertain,
            PairingUiState::Paired,
            PairingUiState::Revoked,
        ] {
            assert!(!pairing_ui_status(state).is_empty());
        }
        for state in [
            HostIdentityState::Ready,
            HostIdentityState::Locked,
            HostIdentityState::PermissionDenied,
            HostIdentityState::Lost,
            HostIdentityState::ResetRequired,
        ] {
            assert!(!remote_identity_status(state, None).is_empty());
        }
    }

    #[test]
    fn pairing_events_require_one_matching_offer_and_fail_closed() {
        let coordinator = ControllerCoordinator::default();
        let mut state = RemoteDevicesState::open_default(&coordinator, None, false);
        let offer_id = PairingOfferId::new();
        state
            .apply_listener_event(
                ListenerProcessEvent::pairing_offer(offer_id, "bounded-offer".to_owned(), 300),
                &coordinator,
            )
            .unwrap();
        assert_eq!(state.pairing_state, PairingUiState::Waiting);
        assert_eq!(state.pairing_offer_text.as_deref(), Some("bounded-offer"));
        assert!(state.pairing_offer_qr.is_some());

        assert!(
            state
                .apply_listener_event(
                    ListenerProcessEvent::pairing_sas_ready(
                        PairingOfferId::new(),
                        "WRONG-000".to_owned(),
                    ),
                    &coordinator,
                )
                .is_err()
        );
        assert!(state.pairing_sas.is_none());

        state
            .apply_listener_event(
                ListenerProcessEvent::pairing_sas_ready(offer_id, "ABCD-1234".to_owned()),
                &coordinator,
            )
            .unwrap();
        assert_eq!(state.pairing_state, PairingUiState::SasReady);
        assert_eq!(state.pairing_sas.as_deref(), Some("ABCD-1234"));

        state
            .apply_listener_event(
                ListenerProcessEvent::pairing_failed(Some(offer_id), "rate_limited"),
                &coordinator,
            )
            .unwrap();
        assert_eq!(state.pairing_state, PairingUiState::RateLimited);
        assert!(state.pairing_offer_id.is_none());
        assert!(state.pairing_offer_text.is_none());
        assert!(state.pairing_offer_qr.is_none());
        assert!(state.pairing_sas.is_none());
    }

    #[test]
    fn remote_devices_safe_reset_and_status_labels_are_explicit() {
        assert_eq!(
            localization::remote_devices_reset_placeholder(),
            "Type RESET to confirm"
        );
        assert_eq!(remote_device_status(PairedDeviceStatus::Revoked), "Revoked");
        assert!(localization::remote_devices_reset_description().contains("all paired devices"));
    }
}
