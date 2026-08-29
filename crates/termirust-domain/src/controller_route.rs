use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRouteKind {
    LocalIpc,
    PrivateNetwork,
    Ssh,
    SelfHostedRelay,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlatform {
    Desktop,
    AppleMobile,
    Android,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRouteTrustLayer {
    SameUserOsBoundary,
    PrivateAddress,
    SshHostKey,
    SystemTls,
    SpkiPin,
    RelayAdmission,
    ControllerAuthentication,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRouteConfigRequirement {
    PrivateEndpoint,
    SshEndpoint,
    SshCredential,
    PairedDevice,
    RelayEndpoint,
    RelaySpkiPin,
    RelayCredential,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRouteCapability {
    ListSessions,
    AttachOutput,
    SendInput,
    Resize,
    RespondToApproval,
    Detach,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoteRoutePolicy {
    pub kind: RemoteRouteKind,
    pub platforms: Vec<ControllerPlatform>,
    pub trust_layers: Vec<RemoteRouteTrustLayer>,
    pub configuration: Vec<RemoteRouteConfigRequirement>,
    pub capabilities: Vec<RemoteRouteCapability>,
    pub allows_automatic_switch: bool,
    pub allows_offline_mutations: bool,
}

impl RemoteRoutePolicy {
    pub fn for_kind(kind: RemoteRouteKind) -> Self {
        let all_platforms = vec![
            ControllerPlatform::Desktop,
            ControllerPlatform::AppleMobile,
            ControllerPlatform::Android,
        ];
        let capabilities = vec![
            RemoteRouteCapability::ListSessions,
            RemoteRouteCapability::AttachOutput,
            RemoteRouteCapability::SendInput,
            RemoteRouteCapability::Resize,
            RemoteRouteCapability::RespondToApproval,
            RemoteRouteCapability::Detach,
        ];
        let (platforms, trust_layers, configuration) = match kind {
            RemoteRouteKind::LocalIpc => (
                vec![ControllerPlatform::Desktop],
                vec![RemoteRouteTrustLayer::SameUserOsBoundary],
                vec![],
            ),
            RemoteRouteKind::PrivateNetwork => (
                all_platforms.clone(),
                vec![
                    RemoteRouteTrustLayer::PrivateAddress,
                    RemoteRouteTrustLayer::ControllerAuthentication,
                ],
                vec![
                    RemoteRouteConfigRequirement::PrivateEndpoint,
                    RemoteRouteConfigRequirement::PairedDevice,
                ],
            ),
            RemoteRouteKind::Ssh => (
                all_platforms.clone(),
                vec![
                    RemoteRouteTrustLayer::SshHostKey,
                    RemoteRouteTrustLayer::ControllerAuthentication,
                ],
                vec![
                    RemoteRouteConfigRequirement::SshEndpoint,
                    RemoteRouteConfigRequirement::SshCredential,
                    RemoteRouteConfigRequirement::PairedDevice,
                ],
            ),
            RemoteRouteKind::SelfHostedRelay => (
                all_platforms,
                vec![
                    RemoteRouteTrustLayer::SystemTls,
                    RemoteRouteTrustLayer::SpkiPin,
                    RemoteRouteTrustLayer::RelayAdmission,
                    RemoteRouteTrustLayer::ControllerAuthentication,
                ],
                vec![
                    RemoteRouteConfigRequirement::RelayEndpoint,
                    RemoteRouteConfigRequirement::RelaySpkiPin,
                    RemoteRouteConfigRequirement::RelayCredential,
                    RemoteRouteConfigRequirement::PairedDevice,
                ],
            ),
        };
        Self {
            kind,
            platforms,
            trust_layers,
            configuration,
            capabilities,
            allows_automatic_switch: false,
            allows_offline_mutations: false,
        }
    }

    pub fn supports(&self, platform: ControllerPlatform) -> bool {
        self.platforms.contains(&platform)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRoutePhase {
    Disabled,
    Unavailable,
    Idle,
    Connecting,
    Authenticating,
    Online,
    Reconnecting,
    Degraded,
    Revoked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoteRouteState {
    pub route: RemoteRouteKind,
    pub phase: RemoteRoutePhase,
    pub writer_held: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteRouteEvent {
    Enable {
        available: bool,
    },
    Connect,
    TransportReady,
    Authenticated,
    Failure {
        retryable: bool,
        mutation_in_flight: bool,
    },
    Retry,
    Cancel,
    Revoke,
    Disable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRouteMutationDisposition {
    None,
    MaySend,
    DoNotReplay,
    QueryByCommandId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRouteMutationCompletion {
    NotSent,
    Acknowledged,
    Unknown,
    Rejected,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoteRouteTransition {
    pub state: RemoteRouteState,
    pub terminal_allowed: bool,
    pub disconnect_transport: bool,
    pub clear_pending_input: bool,
    pub release_writer: bool,
    pub retry_idempotent_reads: bool,
    pub mutation_disposition: RemoteRouteMutationDisposition,
    pub requires_explicit_action: bool,
}

impl RemoteRouteState {
    pub fn transition(
        self,
        event: RemoteRouteEvent,
    ) -> Result<RemoteRouteTransition, RemoteRouteTransitionError> {
        use RemoteRouteEvent as Event;
        use RemoteRouteMutationDisposition as Mutation;
        use RemoteRoutePhase as Phase;

        let neutral = |state: RemoteRouteState| RemoteRouteTransition {
            terminal_allowed: state.phase == Phase::Online,
            state,
            disconnect_transport: false,
            clear_pending_input: false,
            release_writer: false,
            retry_idempotent_reads: false,
            mutation_disposition: Mutation::None,
            requires_explicit_action: false,
        };

        let result = match event {
            Event::Enable { available }
                if matches!(self.phase, Phase::Disabled | Phase::Unavailable) =>
            {
                neutral(RemoteRouteState {
                    phase: if available {
                        Phase::Idle
                    } else {
                        Phase::Unavailable
                    },
                    writer_held: false,
                    ..self
                })
            }
            Event::Connect if matches!(self.phase, Phase::Idle | Phase::Degraded) => {
                neutral(RemoteRouteState {
                    phase: Phase::Connecting,
                    writer_held: false,
                    ..self
                })
            }
            Event::TransportReady
                if matches!(self.phase, Phase::Connecting | Phase::Reconnecting) =>
            {
                neutral(RemoteRouteState {
                    phase: Phase::Authenticating,
                    writer_held: false,
                    ..self
                })
            }
            Event::Authenticated if self.phase == Phase::Authenticating => {
                neutral(RemoteRouteState {
                    phase: Phase::Online,
                    ..self
                })
            }
            Event::Failure {
                retryable,
                mutation_in_flight,
            } if matches!(
                self.phase,
                Phase::Connecting | Phase::Authenticating | Phase::Online | Phase::Reconnecting
            ) =>
            {
                RemoteRouteTransition {
                    state: RemoteRouteState {
                        phase: if retryable {
                            Phase::Reconnecting
                        } else {
                            Phase::Degraded
                        },
                        writer_held: false,
                        ..self
                    },
                    terminal_allowed: false,
                    disconnect_transport: true,
                    clear_pending_input: true,
                    release_writer: self.writer_held,
                    retry_idempotent_reads: retryable,
                    mutation_disposition: if mutation_in_flight {
                        Mutation::QueryByCommandId
                    } else {
                        Mutation::None
                    },
                    requires_explicit_action: !retryable,
                }
            }
            Event::Retry if self.phase == Phase::Degraded => neutral(RemoteRouteState {
                phase: Phase::Connecting,
                writer_held: false,
                ..self
            }),
            Event::Cancel
                if matches!(
                    self.phase,
                    Phase::Connecting
                        | Phase::Authenticating
                        | Phase::Online
                        | Phase::Reconnecting
                        | Phase::Degraded
                ) =>
            {
                cleanup(self, Phase::Idle, false)
            }
            Event::Revoke => cleanup(self, Phase::Revoked, true),
            Event::Disable => cleanup(self, Phase::Disabled, true),
            _ => return Err(RemoteRouteTransitionError::InvalidTransition),
        };
        Ok(result)
    }

    pub fn switch_to(
        self,
        target: RemoteRouteKind,
        platform: ControllerPlatform,
        target_available: bool,
        explicitly_confirmed: bool,
    ) -> Result<RouteSwitchDecision, RemoteRouteTransitionError> {
        if !explicitly_confirmed {
            return Err(RemoteRouteTransitionError::ExplicitConfirmationRequired);
        }
        if self.route == target {
            return Err(RemoteRouteTransitionError::SameRoute);
        }
        if !RemoteRoutePolicy::for_kind(target).supports(platform) {
            return Err(RemoteRouteTransitionError::UnsupportedPlatform);
        }
        if !target_available {
            return Err(RemoteRouteTransitionError::TargetUnavailable);
        }
        Ok(RouteSwitchDecision {
            from: self.route,
            to: target,
            disconnect_source: transport_is_active(self.phase),
            clear_pending_input: true,
            release_writer: self.writer_held,
            automatic: false,
        })
    }

    pub const fn mutation_disposition(
        self,
        completion: RemoteRouteMutationCompletion,
    ) -> RemoteRouteMutationDisposition {
        match completion {
            RemoteRouteMutationCompletion::NotSent
                if matches!(self.phase, RemoteRoutePhase::Online) =>
            {
                RemoteRouteMutationDisposition::MaySend
            }
            RemoteRouteMutationCompletion::NotSent
            | RemoteRouteMutationCompletion::Acknowledged
            | RemoteRouteMutationCompletion::Rejected => {
                RemoteRouteMutationDisposition::DoNotReplay
            }
            RemoteRouteMutationCompletion::Unknown => {
                RemoteRouteMutationDisposition::QueryByCommandId
            }
        }
    }
}

fn cleanup(
    state: RemoteRouteState,
    phase: RemoteRoutePhase,
    explicit: bool,
) -> RemoteRouteTransition {
    RemoteRouteTransition {
        state: RemoteRouteState {
            phase,
            writer_held: false,
            ..state
        },
        terminal_allowed: false,
        disconnect_transport: transport_is_active(state.phase),
        clear_pending_input: true,
        release_writer: state.writer_held,
        retry_idempotent_reads: false,
        mutation_disposition: RemoteRouteMutationDisposition::None,
        requires_explicit_action: explicit,
    }
}

const fn transport_is_active(phase: RemoteRoutePhase) -> bool {
    matches!(
        phase,
        RemoteRoutePhase::Connecting
            | RemoteRoutePhase::Authenticating
            | RemoteRoutePhase::Online
            | RemoteRoutePhase::Reconnecting
            | RemoteRoutePhase::Degraded
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouteSwitchDecision {
    pub from: RemoteRouteKind,
    pub to: RemoteRouteKind,
    pub disconnect_source: bool,
    pub clear_pending_input: bool,
    pub release_writer: bool,
    pub automatic: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteRouteTransitionError {
    InvalidTransition,
    ExplicitConfirmationRequired,
    SameRoute,
    UnsupportedPlatform,
    TargetUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Fixture {
        schema_version: u32,
        routes: Vec<RemoteRoutePolicy>,
        transition_cases: Vec<TransitionCase>,
        invalid_transition_cases: Vec<InvalidTransitionCase>,
        mutation_cases: Vec<MutationCase>,
        switch_cases: Vec<SwitchCase>,
    }

    #[derive(Deserialize)]
    struct TransitionCase {
        name: String,
        initial: RemoteRouteState,
        event: EventFixture,
        expected: RemoteRouteTransition,
    }

    #[derive(Deserialize)]
    struct EventFixture {
        kind: String,
        available: Option<bool>,
        retryable: Option<bool>,
        mutation_in_flight: Option<bool>,
    }

    impl EventFixture {
        fn event(&self) -> RemoteRouteEvent {
            match self.kind.as_str() {
                "enable" => RemoteRouteEvent::Enable {
                    available: self.available.unwrap(),
                },
                "connect" => RemoteRouteEvent::Connect,
                "transport_ready" => RemoteRouteEvent::TransportReady,
                "authenticated" => RemoteRouteEvent::Authenticated,
                "failure" => RemoteRouteEvent::Failure {
                    retryable: self.retryable.unwrap(),
                    mutation_in_flight: self.mutation_in_flight.unwrap(),
                },
                "retry" => RemoteRouteEvent::Retry,
                "cancel" => RemoteRouteEvent::Cancel,
                "revoke" => RemoteRouteEvent::Revoke,
                "disable" => RemoteRouteEvent::Disable,
                other => panic!("unsupported fixture event {other}"),
            }
        }
    }

    #[derive(Deserialize)]
    struct MutationCase {
        state: RemoteRouteState,
        completion: RemoteRouteMutationCompletion,
        expected: RemoteRouteMutationDisposition,
    }

    #[derive(Deserialize)]
    struct InvalidTransitionCase {
        name: String,
        initial: RemoteRouteState,
        event: EventFixture,
    }

    #[derive(Deserialize)]
    struct SwitchCase {
        name: String,
        initial: RemoteRouteState,
        target: RemoteRouteKind,
        platform: ControllerPlatform,
        target_available: bool,
        confirmed: bool,
        expected: Option<RouteSwitchDecision>,
        expected_error: Option<String>,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/controller-routes/route-selection-v1.json"
        ))
        .unwrap()
    }

    #[test]
    fn canonical_route_policies_match_fixture() {
        let fixture = fixture();
        assert_eq!(fixture.schema_version, 1);
        assert_eq!(fixture.routes.len(), 4);
        for expected in fixture.routes {
            assert_eq!(RemoteRoutePolicy::for_kind(expected.kind), expected);
        }
    }

    #[test]
    fn canonical_route_transitions_fail_closed() {
        let fixture = fixture();
        for case in fixture.transition_cases {
            assert_eq!(
                case.initial.transition(case.event.event()).unwrap(),
                case.expected,
                "{}",
                case.name
            );
            assert_ne!(
                case.expected.mutation_disposition,
                RemoteRouteMutationDisposition::MaySend
            );
        }
        for case in fixture.invalid_transition_cases {
            assert_eq!(
                case.initial.transition(case.event.event()),
                Err(RemoteRouteTransitionError::InvalidTransition),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn mutation_reconciliation_never_replays() {
        for case in fixture().mutation_cases {
            assert_eq!(
                case.state.mutation_disposition(case.completion),
                case.expected
            );
        }
    }

    #[test]
    fn route_switch_requires_explicit_confirmation() {
        for case in fixture().switch_cases {
            match (
                case.initial.switch_to(
                    case.target,
                    case.platform,
                    case.target_available,
                    case.confirmed,
                ),
                case.expected,
            ) {
                (Ok(actual), Some(expected)) => {
                    assert_eq!(actual, expected, "{}", case.name);
                    assert!(!actual.automatic);
                }
                (Err(error), None) => {
                    assert_eq!(format!("{error:?}"), case.expected_error.unwrap())
                }
                result => panic!("{}: unexpected switch result {result:?}", case.name),
            }
        }
    }

    #[test]
    fn transport_readiness_does_not_grant_terminal_authority() {
        let state = RemoteRouteState {
            route: RemoteRouteKind::SelfHostedRelay,
            phase: RemoteRoutePhase::Connecting,
            writer_held: false,
        };
        let decision = state.transition(RemoteRouteEvent::TransportReady).unwrap();
        assert_eq!(decision.state.phase, RemoteRoutePhase::Authenticating);
        assert!(!decision.terminal_allowed);
    }
}
