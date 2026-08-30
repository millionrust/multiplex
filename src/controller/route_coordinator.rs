use termirust_client::SshRouteState;
use termirust_domain::{
    ControllerPlatform, RemoteRouteCapability, RemoteRouteEvent, RemoteRouteKind,
    RemoteRouteMutationDisposition, RemoteRoutePhase, RemoteRoutePolicy, RemoteRouteState,
    RemoteRouteTransition, RemoteRouteTransitionError, RemoteRouteTrustLayer,
};
use termirust_relay_client::RelayClientState;

const ROUTE_KINDS: [RemoteRouteKind; 4] = [
    RemoteRouteKind::LocalIpc,
    RemoteRouteKind::PrivateNetwork,
    RemoteRouteKind::Ssh,
    RemoteRouteKind::SelfHostedRelay,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopRouteAvailability {
    pub local_ipc: bool,
    pub private_network: bool,
    pub ssh: bool,
    pub self_hosted_relay: bool,
}

impl DesktopRouteAvailability {
    pub const fn available(self, route: RemoteRouteKind) -> bool {
        match route {
            RemoteRouteKind::LocalIpc => self.local_ipc,
            RemoteRouteKind::PrivateNetwork => self.private_network,
            RemoteRouteKind::Ssh => self.ssh,
            RemoteRouteKind::SelfHostedRelay => self.self_hosted_relay,
        }
    }

    fn set(&mut self, route: RemoteRouteKind, available: bool) {
        match route {
            RemoteRouteKind::LocalIpc => self.local_ipc = available,
            RemoteRouteKind::PrivateNetwork => self.private_network = available,
            RemoteRouteKind::Ssh => self.ssh = available,
            RemoteRouteKind::SelfHostedRelay => self.self_hosted_relay = available,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopRouteRecovery {
    None,
    Enable,
    Configure,
    Retry,
    Reauthorize,
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopRouteProjection {
    pub route: RemoteRouteKind,
    pub selected: bool,
    pub available: bool,
    pub phase: RemoteRoutePhase,
    pub terminal_allowed: bool,
    pub trust_layers: Vec<RemoteRouteTrustLayer>,
    pub capabilities: Vec<RemoteRouteCapability>,
    pub recovery: DesktopRouteRecovery,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DesktopRoutePlan {
    pub start_transport: Option<RemoteRouteKind>,
    pub disconnect_transport: Option<RemoteRouteKind>,
    pub clear_pending_input: bool,
    pub release_writer: bool,
    pub retry_idempotent_reads: bool,
    pub mutation_disposition: Option<RemoteRouteMutationDisposition>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopRouteCoordinatorError {
    NoSelectedRoute,
    RouteNotSelected,
    WriterRequiresOnlineRoute,
    Transition(RemoteRouteTransitionError),
}

impl From<RemoteRouteTransitionError> for DesktopRouteCoordinatorError {
    fn from(error: RemoteRouteTransitionError) -> Self {
        Self::Transition(error)
    }
}

pub struct DesktopRouteCoordinator {
    availability: DesktopRouteAvailability,
    routes: [RemoteRouteState; 4],
    selected: Option<RemoteRouteKind>,
}

impl DesktopRouteCoordinator {
    pub fn new(availability: DesktopRouteAvailability) -> Self {
        Self {
            availability,
            routes: ROUTE_KINDS.map(|route| RemoteRouteState {
                route,
                phase: if availability.available(route) {
                    RemoteRoutePhase::Idle
                } else {
                    RemoteRoutePhase::Unavailable
                },
                writer_held: false,
            }),
            selected: None,
        }
    }

    pub const fn selected(&self) -> Option<RemoteRouteKind> {
        self.selected
    }

    pub fn projections(&self) -> [DesktopRouteProjection; 4] {
        ROUTE_KINDS.map(|route| {
            let state = self.state(route);
            let policy = RemoteRoutePolicy::for_kind(route);
            DesktopRouteProjection {
                route,
                selected: self.selected == Some(route),
                available: self.availability.available(route),
                phase: state.phase,
                terminal_allowed: self.selected == Some(route)
                    && state.phase == RemoteRoutePhase::Online,
                trust_layers: policy.trust_layers,
                capabilities: policy.capabilities,
                recovery: recovery_for_phase(state.phase),
            }
        })
    }

    pub fn select(
        &mut self,
        target: RemoteRouteKind,
        explicitly_confirmed: bool,
    ) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        if let Some(selected) = self.selected {
            let source = self.state(selected);
            let target_state = self.state(target);
            let decision = source.switch_to(
                target,
                ControllerPlatform::Desktop,
                self.availability.available(target),
                explicitly_confirmed,
            )?;
            self.replace_state(RemoteRouteState {
                phase: inactive_phase(source.phase, self.availability.available(selected)),
                writer_held: false,
                ..source
            });
            self.replace_state(RemoteRouteState {
                phase: selectable_phase(target_state.phase),
                writer_held: false,
                ..target_state
            });
            self.selected = Some(target);
            return Ok(DesktopRoutePlan {
                disconnect_transport: decision.disconnect_source.then_some(selected),
                clear_pending_input: decision.clear_pending_input,
                release_writer: decision.release_writer,
                ..DesktopRoutePlan::default()
            });
        }

        if !explicitly_confirmed {
            return Err(RemoteRouteTransitionError::ExplicitConfirmationRequired.into());
        }
        if !RemoteRoutePolicy::for_kind(target).supports(ControllerPlatform::Desktop) {
            return Err(RemoteRouteTransitionError::UnsupportedPlatform.into());
        }
        if !self.availability.available(target) {
            return Err(RemoteRouteTransitionError::TargetUnavailable.into());
        }
        self.selected = Some(target);
        Ok(DesktopRoutePlan::default())
    }

    pub fn connect_selected(&mut self) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        let route = self
            .selected
            .ok_or(DesktopRouteCoordinatorError::NoSelectedRoute)?;
        let transition = self.apply(route, RemoteRouteEvent::Connect)?;
        Ok(DesktopRoutePlan {
            start_transport: Some(route),
            ..plan_for_transition(route, transition)
        })
    }

    pub fn transport_ready(
        &mut self,
        route: RemoteRouteKind,
    ) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        self.apply_selected(route, RemoteRouteEvent::TransportReady)
    }

    pub fn authenticated(
        &mut self,
        route: RemoteRouteKind,
    ) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        self.apply_selected(route, RemoteRouteEvent::Authenticated)
    }

    pub fn failed(
        &mut self,
        route: RemoteRouteKind,
        retryable: bool,
        mutation_in_flight: bool,
    ) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        self.apply_selected(
            route,
            RemoteRouteEvent::Failure {
                retryable,
                mutation_in_flight,
            },
        )
    }

    pub fn retry_selected(&mut self) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        let route = self
            .selected
            .ok_or(DesktopRouteCoordinatorError::NoSelectedRoute)?;
        let transition = self.apply(route, RemoteRouteEvent::Retry)?;
        Ok(DesktopRoutePlan {
            start_transport: Some(route),
            ..plan_for_transition(route, transition)
        })
    }

    pub fn cancel_selected(&mut self) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        let route = self
            .selected
            .ok_or(DesktopRouteCoordinatorError::NoSelectedRoute)?;
        let transition = self.apply(route, RemoteRouteEvent::Cancel)?;
        Ok(plan_for_transition(route, transition))
    }

    pub fn revoke_selected(&mut self) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        let route = self
            .selected
            .ok_or(DesktopRouteCoordinatorError::NoSelectedRoute)?;
        let transition = self.apply(route, RemoteRouteEvent::Revoke)?;
        Ok(plan_for_transition(route, transition))
    }

    pub fn disable_selected(&mut self) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        let route = self
            .selected
            .ok_or(DesktopRouteCoordinatorError::NoSelectedRoute)?;
        let transition = self.apply(route, RemoteRouteEvent::Disable)?;
        Ok(plan_for_transition(route, transition))
    }

    pub fn enable_selected(&mut self) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        let route = self
            .selected
            .ok_or(DesktopRouteCoordinatorError::NoSelectedRoute)?;
        let available = self.availability.available(route);
        let transition = self.apply(route, RemoteRouteEvent::Enable { available })?;
        Ok(plan_for_transition(route, transition))
    }

    pub fn authorization_restored(
        &mut self,
        route: RemoteRouteKind,
    ) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        if self.selected != Some(route) {
            return Err(DesktopRouteCoordinatorError::RouteNotSelected);
        }
        let available = self.availability.available(route);
        let transition =
            self.apply(route, RemoteRouteEvent::AuthorizationRestored { available })?;
        Ok(plan_for_transition(route, transition))
    }

    pub fn set_writer_held(&mut self, held: bool) -> Result<(), DesktopRouteCoordinatorError> {
        let route = self
            .selected
            .ok_or(DesktopRouteCoordinatorError::NoSelectedRoute)?;
        let mut state = self.state(route);
        if held && state.phase != RemoteRoutePhase::Online {
            return Err(DesktopRouteCoordinatorError::WriterRequiresOnlineRoute);
        }
        state.writer_held = held;
        self.replace_state(state);
        Ok(())
    }

    pub fn set_available(
        &mut self,
        route: RemoteRouteKind,
        available: bool,
    ) -> Result<Option<DesktopRoutePlan>, DesktopRouteCoordinatorError> {
        if self.availability.available(route) == available {
            return Ok(None);
        }
        self.availability.set(route, available);
        let state = self.state(route);
        if !available {
            if matches!(
                state.phase,
                RemoteRoutePhase::Disabled | RemoteRoutePhase::Revoked
            ) {
                return Ok(None);
            }
            if self.selected == Some(route) {
                let transition = self.apply(route, RemoteRouteEvent::AvailabilityLost)?;
                return Ok(Some(plan_for_transition(route, transition)));
            }
            self.replace_state(RemoteRouteState {
                phase: RemoteRoutePhase::Unavailable,
                writer_held: false,
                ..state
            });
        } else if state.phase == RemoteRoutePhase::Unavailable {
            let transition = state.transition(RemoteRouteEvent::Enable { available: true })?;
            self.replace_state(transition.state);
        }
        Ok(None)
    }

    fn apply_selected(
        &mut self,
        route: RemoteRouteKind,
        event: RemoteRouteEvent,
    ) -> Result<DesktopRoutePlan, DesktopRouteCoordinatorError> {
        if self.selected != Some(route) {
            return Err(DesktopRouteCoordinatorError::RouteNotSelected);
        }
        let transition = self.apply(route, event)?;
        Ok(plan_for_transition(route, transition))
    }

    fn apply(
        &mut self,
        route: RemoteRouteKind,
        event: RemoteRouteEvent,
    ) -> Result<RemoteRouteTransition, DesktopRouteCoordinatorError> {
        let transition = self.state(route).transition(event)?;
        self.replace_state(transition.state);
        Ok(transition)
    }

    fn state(&self, route: RemoteRouteKind) -> RemoteRouteState {
        self.routes[route_index(route)]
    }

    fn replace_state(&mut self, state: RemoteRouteState) {
        self.routes[route_index(state.route)] = state;
    }
}

fn route_index(route: RemoteRouteKind) -> usize {
    match route {
        RemoteRouteKind::LocalIpc => 0,
        RemoteRouteKind::PrivateNetwork => 1,
        RemoteRouteKind::Ssh => 2,
        RemoteRouteKind::SelfHostedRelay => 3,
    }
}

const fn inactive_phase(phase: RemoteRoutePhase, available: bool) -> RemoteRoutePhase {
    match phase {
        RemoteRoutePhase::Revoked => RemoteRoutePhase::Revoked,
        RemoteRoutePhase::Disabled => RemoteRoutePhase::Disabled,
        RemoteRoutePhase::Unavailable => RemoteRoutePhase::Unavailable,
        _ if available => RemoteRoutePhase::Idle,
        _ => RemoteRoutePhase::Unavailable,
    }
}

const fn selectable_phase(phase: RemoteRoutePhase) -> RemoteRoutePhase {
    match phase {
        RemoteRoutePhase::Revoked => RemoteRoutePhase::Revoked,
        RemoteRoutePhase::Disabled => RemoteRoutePhase::Disabled,
        RemoteRoutePhase::Unavailable => RemoteRoutePhase::Unavailable,
        _ => RemoteRoutePhase::Idle,
    }
}

fn plan_for_transition(
    route: RemoteRouteKind,
    transition: RemoteRouteTransition,
) -> DesktopRoutePlan {
    DesktopRoutePlan {
        disconnect_transport: transition.disconnect_transport.then_some(route),
        clear_pending_input: transition.clear_pending_input,
        release_writer: transition.release_writer,
        retry_idempotent_reads: transition.retry_idempotent_reads,
        mutation_disposition: (transition.mutation_disposition
            != RemoteRouteMutationDisposition::None)
            .then_some(transition.mutation_disposition),
        ..DesktopRoutePlan::default()
    }
}

fn recovery_for_phase(phase: RemoteRoutePhase) -> DesktopRouteRecovery {
    match phase {
        RemoteRoutePhase::Disabled => DesktopRouteRecovery::Enable,
        RemoteRoutePhase::Unavailable => DesktopRouteRecovery::Configure,
        RemoteRoutePhase::Degraded => DesktopRouteRecovery::Retry,
        RemoteRoutePhase::Revoked => DesktopRouteRecovery::Reauthorize,
        RemoteRoutePhase::Connecting
        | RemoteRoutePhase::Authenticating
        | RemoteRoutePhase::Reconnecting => DesktopRouteRecovery::Cancel,
        RemoteRoutePhase::Idle | RemoteRoutePhase::Online => DesktopRouteRecovery::None,
    }
}

pub const fn phase_for_ssh_state(state: SshRouteState) -> RemoteRoutePhase {
    match state {
        SshRouteState::Disconnected => RemoteRoutePhase::Idle,
        SshRouteState::Connecting => RemoteRoutePhase::Connecting,
        SshRouteState::Authenticating | SshRouteState::Pairing => RemoteRoutePhase::Authenticating,
        SshRouteState::Ready => RemoteRoutePhase::Online,
        SshRouteState::Reconnecting => RemoteRoutePhase::Reconnecting,
        SshRouteState::HostKeyChanged | SshRouteState::Failed => RemoteRoutePhase::Degraded,
    }
}

pub const fn phase_for_relay_state(state: RelayClientState) -> RemoteRoutePhase {
    match state {
        RelayClientState::Disabled => RemoteRoutePhase::Idle,
        RelayClientState::Connecting
        | RelayClientState::TlsAuthenticating
        | RelayClientState::Admitting
        | RelayClientState::WaitingPeer => RemoteRoutePhase::Connecting,
        RelayClientState::AuthenticatingController => RemoteRoutePhase::Authenticating,
        RelayClientState::Ready => RemoteRoutePhase::Online,
        RelayClientState::Reconnecting => RemoteRoutePhase::Reconnecting,
        RelayClientState::Revoked => RemoteRoutePhase::Revoked,
        RelayClientState::CredentialLost => RemoteRoutePhase::Unavailable,
        RelayClientState::Failed => RemoteRoutePhase::Degraded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn availability() -> DesktopRouteAvailability {
        DesktopRouteAvailability {
            local_ipc: true,
            private_network: true,
            ssh: true,
            self_hosted_relay: false,
        }
    }

    fn all_available() -> DesktopRouteAvailability {
        DesktopRouteAvailability {
            local_ipc: true,
            private_network: true,
            ssh: true,
            self_hosted_relay: true,
        }
    }

    fn connect_online(coordinator: &mut DesktopRouteCoordinator, route: RemoteRouteKind) {
        coordinator.select(route, true).unwrap();
        assert_eq!(
            coordinator.connect_selected().unwrap().start_transport,
            Some(route)
        );
        coordinator.transport_ready(route).unwrap();
        coordinator.authenticated(route).unwrap();
        let projection = coordinator
            .projections()
            .into_iter()
            .find(|projection| projection.route == route)
            .unwrap();
        assert!(projection.selected);
        assert_eq!(projection.phase, RemoteRoutePhase::Online);
        assert!(projection.terminal_allowed);
    }

    #[test]
    fn coordinator_requires_explicit_available_selection() {
        let mut coordinator = DesktopRouteCoordinator::new(availability());
        assert_eq!(
            coordinator.select(RemoteRouteKind::PrivateNetwork, false),
            Err(DesktopRouteCoordinatorError::Transition(
                RemoteRouteTransitionError::ExplicitConfirmationRequired
            ))
        );
        assert_eq!(
            coordinator.select(RemoteRouteKind::SelfHostedRelay, true),
            Err(DesktopRouteCoordinatorError::Transition(
                RemoteRouteTransitionError::TargetUnavailable
            ))
        );
        assert_eq!(
            coordinator.select(RemoteRouteKind::PrivateNetwork, true),
            Ok(DesktopRoutePlan::default())
        );
        assert_eq!(
            coordinator.selected(),
            Some(RemoteRouteKind::PrivateNetwork)
        );
    }

    #[test]
    fn coordinator_connects_authenticates_and_never_switches_on_failure() {
        let mut coordinator = DesktopRouteCoordinator::new(availability());
        coordinator
            .select(RemoteRouteKind::PrivateNetwork, true)
            .unwrap();
        assert_eq!(
            coordinator.connect_selected().unwrap().start_transport,
            Some(RemoteRouteKind::PrivateNetwork)
        );
        coordinator
            .transport_ready(RemoteRouteKind::PrivateNetwork)
            .unwrap();
        coordinator
            .authenticated(RemoteRouteKind::PrivateNetwork)
            .unwrap();
        coordinator.set_writer_held(true).unwrap();

        let plan = coordinator
            .failed(RemoteRouteKind::PrivateNetwork, true, true)
            .unwrap();
        assert_eq!(
            coordinator.selected(),
            Some(RemoteRouteKind::PrivateNetwork)
        );
        assert_eq!(
            plan.disconnect_transport,
            Some(RemoteRouteKind::PrivateNetwork)
        );
        assert!(plan.clear_pending_input);
        assert!(plan.release_writer);
        assert!(plan.retry_idempotent_reads);
        assert_eq!(
            plan.mutation_disposition,
            Some(RemoteRouteMutationDisposition::QueryByCommandId)
        );
    }

    #[test]
    fn confirmed_switch_cleans_source_without_starting_target() {
        let mut coordinator = DesktopRouteCoordinator::new(availability());
        coordinator
            .select(RemoteRouteKind::PrivateNetwork, true)
            .unwrap();
        coordinator.connect_selected().unwrap();
        coordinator
            .transport_ready(RemoteRouteKind::PrivateNetwork)
            .unwrap();
        coordinator
            .authenticated(RemoteRouteKind::PrivateNetwork)
            .unwrap();
        coordinator.set_writer_held(true).unwrap();

        let plan = coordinator.select(RemoteRouteKind::Ssh, true).unwrap();
        assert_eq!(
            plan.disconnect_transport,
            Some(RemoteRouteKind::PrivateNetwork)
        );
        assert_eq!(plan.start_transport, None);
        assert!(plan.clear_pending_input);
        assert!(plan.release_writer);
        assert_eq!(coordinator.selected(), Some(RemoteRouteKind::Ssh));
    }

    #[test]
    fn configuration_loss_is_visible_and_cleans_selected_route() {
        let mut coordinator = DesktopRouteCoordinator::new(availability());
        coordinator.select(RemoteRouteKind::Ssh, true).unwrap();
        coordinator.connect_selected().unwrap();
        let plan = coordinator
            .set_available(RemoteRouteKind::Ssh, false)
            .unwrap()
            .unwrap();
        assert_eq!(plan.disconnect_transport, Some(RemoteRouteKind::Ssh));
        assert!(plan.clear_pending_input);
        let selected = coordinator
            .projections()
            .into_iter()
            .find(|route| route.selected)
            .unwrap();
        assert_eq!(selected.phase, RemoteRoutePhase::Unavailable);
        assert_eq!(selected.recovery, DesktopRouteRecovery::Configure);
    }

    #[test]
    fn transport_state_projections_are_total_and_truthful() {
        assert_eq!(
            phase_for_ssh_state(SshRouteState::Ready),
            RemoteRoutePhase::Online
        );
        assert_eq!(
            phase_for_ssh_state(SshRouteState::HostKeyChanged),
            RemoteRoutePhase::Degraded
        );
        assert_eq!(
            phase_for_relay_state(RelayClientState::AuthenticatingController),
            RemoteRoutePhase::Authenticating
        );
        assert_eq!(
            phase_for_relay_state(RelayClientState::CredentialLost),
            RemoteRoutePhase::Unavailable
        );
    }

    #[test]
    fn writer_authority_requires_online_selected_route() {
        let mut coordinator = DesktopRouteCoordinator::new(availability());
        coordinator.select(RemoteRouteKind::LocalIpc, true).unwrap();
        assert_eq!(
            coordinator.set_writer_held(true),
            Err(DesktopRouteCoordinatorError::WriterRequiresOnlineRoute)
        );
        assert_eq!(
            coordinator.transport_ready(RemoteRouteKind::Ssh),
            Err(DesktopRouteCoordinatorError::RouteNotSelected)
        );
    }

    #[test]
    fn revoked_route_requires_fresh_authorization_after_switching_away_and_back() {
        let mut coordinator = DesktopRouteCoordinator::new(availability());
        coordinator.select(RemoteRouteKind::Ssh, true).unwrap();
        coordinator.connect_selected().unwrap();
        coordinator.transport_ready(RemoteRouteKind::Ssh).unwrap();
        coordinator.authenticated(RemoteRouteKind::Ssh).unwrap();
        coordinator.revoke_selected().unwrap();

        coordinator
            .select(RemoteRouteKind::PrivateNetwork, true)
            .unwrap();
        coordinator.select(RemoteRouteKind::Ssh, true).unwrap();
        assert_eq!(
            coordinator.connect_selected(),
            Err(DesktopRouteCoordinatorError::Transition(
                RemoteRouteTransitionError::InvalidTransition
            ))
        );
        let ssh = coordinator
            .projections()
            .into_iter()
            .find(|projection| projection.route == RemoteRouteKind::Ssh)
            .unwrap();
        assert_eq!(ssh.phase, RemoteRoutePhase::Revoked);
        assert_eq!(ssh.recovery, DesktopRouteRecovery::Reauthorize);

        coordinator
            .authorization_restored(RemoteRouteKind::Ssh)
            .unwrap();
        assert_eq!(
            coordinator.connect_selected().unwrap().start_transport,
            Some(RemoteRouteKind::Ssh)
        );
    }

    #[test]
    fn repeated_availability_signal_does_not_duplicate_cleanup() {
        let mut coordinator = DesktopRouteCoordinator::new(availability());
        coordinator.select(RemoteRouteKind::Ssh, true).unwrap();
        coordinator.connect_selected().unwrap();
        assert!(
            coordinator
                .set_available(RemoteRouteKind::Ssh, false)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            coordinator
                .set_available(RemoteRouteKind::Ssh, false)
                .unwrap(),
            None
        );
        assert_eq!(coordinator.selected(), Some(RemoteRouteKind::Ssh));
    }

    #[test]
    fn configuration_changes_never_erase_revocation_or_explicit_disable() {
        let mut coordinator = DesktopRouteCoordinator::new(all_available());
        connect_online(&mut coordinator, RemoteRouteKind::Ssh);
        coordinator.revoke_selected().unwrap();
        assert_eq!(
            coordinator
                .set_available(RemoteRouteKind::Ssh, false)
                .unwrap(),
            None
        );
        assert_eq!(
            coordinator
                .set_available(RemoteRouteKind::Ssh, true)
                .unwrap(),
            None
        );
        assert_eq!(
            coordinator.connect_selected(),
            Err(DesktopRouteCoordinatorError::Transition(
                RemoteRouteTransitionError::InvalidTransition
            ))
        );

        coordinator
            .authorization_restored(RemoteRouteKind::Ssh)
            .unwrap();
        coordinator.disable_selected().unwrap();
        coordinator
            .set_available(RemoteRouteKind::Ssh, false)
            .unwrap();
        coordinator
            .set_available(RemoteRouteKind::Ssh, true)
            .unwrap();
        let ssh = coordinator
            .projections()
            .into_iter()
            .find(|projection| projection.route == RemoteRouteKind::Ssh)
            .unwrap();
        assert_eq!(ssh.phase, RemoteRoutePhase::Disabled);
        assert_eq!(ssh.recovery, DesktopRouteRecovery::Enable);
        coordinator.enable_selected().unwrap();
        assert_eq!(
            coordinator.connect_selected().unwrap().start_transport,
            Some(RemoteRouteKind::Ssh)
        );
    }

    #[test]
    fn every_desktop_route_passes_normal_degraded_and_reconnect_lifecycle() {
        for route in ROUTE_KINDS {
            let mut coordinator = DesktopRouteCoordinator::new(all_available());
            connect_online(&mut coordinator, route);
            coordinator.set_writer_held(true).unwrap();

            let reconnect = coordinator.failed(route, true, true).unwrap();
            assert_eq!(coordinator.selected(), Some(route));
            assert_eq!(reconnect.disconnect_transport, Some(route));
            assert!(reconnect.release_writer);
            assert!(reconnect.retry_idempotent_reads);
            assert_eq!(
                reconnect.mutation_disposition,
                Some(RemoteRouteMutationDisposition::QueryByCommandId)
            );
            coordinator.transport_ready(route).unwrap();
            coordinator.authenticated(route).unwrap();

            let degraded = coordinator.failed(route, false, false).unwrap();
            assert_eq!(degraded.disconnect_transport, Some(route));
            assert!(!degraded.retry_idempotent_reads);
            assert_eq!(coordinator.selected(), Some(route));
            let projection = coordinator
                .projections()
                .into_iter()
                .find(|projection| projection.route == route)
                .unwrap();
            assert_eq!(projection.phase, RemoteRoutePhase::Degraded);
            assert_eq!(projection.recovery, DesktopRouteRecovery::Retry);
            assert_eq!(
                coordinator.retry_selected().unwrap().start_transport,
                Some(route)
            );
        }
    }

    #[test]
    fn every_desktop_route_passes_cancel_and_revoke_cleanup() {
        for route in ROUTE_KINDS {
            let mut coordinator = DesktopRouteCoordinator::new(all_available());
            coordinator.select(route, true).unwrap();
            coordinator.connect_selected().unwrap();
            let cancel = coordinator.cancel_selected().unwrap();
            assert_eq!(cancel.disconnect_transport, Some(route));
            assert!(cancel.clear_pending_input);

            coordinator.connect_selected().unwrap();
            coordinator.transport_ready(route).unwrap();
            coordinator.authenticated(route).unwrap();
            coordinator.set_writer_held(true).unwrap();
            let revoke = coordinator.revoke_selected().unwrap();
            assert_eq!(revoke.disconnect_transport, Some(route));
            assert!(revoke.clear_pending_input);
            assert!(revoke.release_writer);
            let projection = coordinator
                .projections()
                .into_iter()
                .find(|projection| projection.route == route)
                .unwrap();
            assert_eq!(projection.phase, RemoteRoutePhase::Revoked);
            assert_eq!(projection.recovery, DesktopRouteRecovery::Reauthorize);
        }
    }

    #[test]
    fn every_desktop_route_switch_is_explicit_and_never_starts_the_target() {
        for source in ROUTE_KINDS {
            for target in ROUTE_KINDS {
                if source == target {
                    continue;
                }
                let mut coordinator = DesktopRouteCoordinator::new(all_available());
                connect_online(&mut coordinator, source);
                assert_eq!(
                    coordinator.select(target, false),
                    Err(DesktopRouteCoordinatorError::Transition(
                        RemoteRouteTransitionError::ExplicitConfirmationRequired
                    ))
                );
                assert_eq!(coordinator.selected(), Some(source));

                let switch = coordinator.select(target, true).unwrap();
                assert_eq!(switch.disconnect_transport, Some(source));
                assert_eq!(switch.start_transport, None);
                assert!(switch.clear_pending_input);
                assert_eq!(coordinator.selected(), Some(target));
            }
        }
    }
}
