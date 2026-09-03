package com.termirust.mobile.controller

data class AndroidControllerRouteAvailability(
    val privateNetwork: Boolean,
    val ssh: Boolean,
    val selfHostedRelay: Boolean,
) {
    fun isAvailable(route: ControllerRemoteRouteKind) = when (route) {
        ControllerRemoteRouteKind.LOCAL_IPC -> false
        ControllerRemoteRouteKind.PRIVATE_NETWORK -> privateNetwork
        ControllerRemoteRouteKind.SSH -> ssh
        ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> selfHostedRelay
    }

    fun replacing(route: ControllerRemoteRouteKind, available: Boolean) = when (route) {
        ControllerRemoteRouteKind.LOCAL_IPC -> this
        ControllerRemoteRouteKind.PRIVATE_NETWORK -> copy(privateNetwork = available)
        ControllerRemoteRouteKind.SSH -> copy(ssh = available)
        ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> copy(selfHostedRelay = available)
    }
}

enum class AndroidControllerRouteRecovery { NONE, ENABLE, CONFIGURE, RETRY, REAUTHORIZE, CANCEL }

data class AndroidControllerRouteProjection(
    val route: ControllerRemoteRouteKind,
    val selected: Boolean,
    val available: Boolean,
    val phase: ControllerRemoteRoutePhase,
    val terminalAllowed: Boolean,
    val trustLayers: List<ControllerRemoteTrustLayer>,
    val capabilities: List<ControllerRemoteCapability>,
    val recovery: AndroidControllerRouteRecovery,
)

data class AndroidControllerRoutePlan(
    val startTransport: ControllerRemoteRouteKind? = null,
    val disconnectTransport: ControllerRemoteRouteKind? = null,
    val clearPendingInput: Boolean = false,
    val releaseWriter: Boolean = false,
    val retryIdempotentReads: Boolean = false,
    val mutationDisposition: ControllerRemoteMutationDisposition? = null,
    val requiresExplicitAction: Boolean = false,
)

enum class AndroidControllerRouteCoordinatorError {
    NO_SELECTED_ROUTE,
    ROUTE_NOT_SELECTED,
    WRITER_REQUIRES_ONLINE_ROUTE,
}

class AndroidControllerRouteCoordinatorException(
    val reason: AndroidControllerRouteCoordinatorError? = null,
    val transition: ControllerRemoteRouteError? = null,
) : IllegalStateException((reason ?: transition).toString())

class AndroidControllerRouteCoordinator(initialAvailability: AndroidControllerRouteAvailability) {
    private var availability = initialAvailability
    private val states = ControllerRemoteRouteKind.androidRoutes.associateWithTo(mutableMapOf()) { route ->
        ControllerRemoteRouteState(
            route,
            if (initialAvailability.isAvailable(route)) ControllerRemoteRoutePhase.IDLE else ControllerRemoteRoutePhase.UNAVAILABLE,
        )
    }
    var selected: ControllerRemoteRouteKind? = null
        private set

    val projections: List<AndroidControllerRouteProjection>
        get() = ControllerRemoteRouteKind.androidRoutes.map { route ->
            val state = state(route)
            val policy = ControllerRemoteRoutePolicy.canonical(route)
            AndroidControllerRouteProjection(
                route,
                selected == route,
                availability.isAvailable(route),
                state.phase,
                selected == route && state.phase == ControllerRemoteRoutePhase.ONLINE,
                policy.trustLayers,
                policy.capabilities,
                recovery(state.phase),
            )
        }

    fun restorePersistedSelection(route: ControllerRemoteRouteKind) {
        if (route !in ControllerRemoteRouteKind.androidRoutes) transitionError(ControllerRemoteRouteError.UNSUPPORTED_PLATFORM)
        selected = route
    }

    fun select(target: ControllerRemoteRouteKind, explicitlyConfirmed: Boolean): AndroidControllerRoutePlan {
        if (target !in ControllerRemoteRouteKind.androidRoutes) transitionError(ControllerRemoteRouteError.UNSUPPORTED_PLATFORM)
        val sourceRoute = selected
        if (sourceRoute == null) {
            if (!explicitlyConfirmed) transitionError(ControllerRemoteRouteError.EXPLICIT_CONFIRMATION_REQUIRED)
            if (!availability.isAvailable(target)) transitionError(ControllerRemoteRouteError.TARGET_UNAVAILABLE)
            selected = target
            return AndroidControllerRoutePlan()
        }
        val decision = try {
            state(sourceRoute).switchTo(target, ControllerRemotePlatform.ANDROID, availability.isAvailable(target), explicitlyConfirmed)
        } catch (error: ControllerRemoteRouteException) {
            transitionError(error.reason)
        }
        states[sourceRoute] = ControllerRemoteRouteState(
            sourceRoute,
            inactivePhase(state(sourceRoute).phase, availability.isAvailable(sourceRoute)),
        )
        states[target] = ControllerRemoteRouteState(target, selectablePhase(state(target).phase))
        selected = target
        return AndroidControllerRoutePlan(
            disconnectTransport = sourceRoute.takeIf { decision.disconnectSource },
            clearPendingInput = decision.clearPendingInput,
            releaseWriter = decision.releaseWriter,
        )
    }

    fun connectSelected(): AndroidControllerRoutePlan {
        val route = selectedRoute()
        return plan(route, apply(route, ControllerRemoteRouteEvent.Connect)).copy(startTransport = route)
    }

    fun transportReady(route: ControllerRemoteRouteKind) = applySelected(route, ControllerRemoteRouteEvent.TransportReady)
    fun authenticated(route: ControllerRemoteRouteKind) = applySelected(route, ControllerRemoteRouteEvent.Authenticated)
    fun failed(route: ControllerRemoteRouteKind, retryable: Boolean, mutationInFlight: Boolean) =
        applySelected(route, ControllerRemoteRouteEvent.Failure(retryable, mutationInFlight))

    fun retrySelected(): AndroidControllerRoutePlan {
        val route = selectedRoute()
        return plan(route, apply(route, ControllerRemoteRouteEvent.Retry)).copy(startTransport = route)
    }

    fun cancelSelected() = selectedPlan(ControllerRemoteRouteEvent.Cancel)
    fun revokeSelected() = selectedPlan(ControllerRemoteRouteEvent.Revoke)
    fun disableSelected() = selectedPlan(ControllerRemoteRouteEvent.Disable)

    fun enableSelected(): AndroidControllerRoutePlan {
        val route = selectedRoute()
        return plan(route, apply(route, ControllerRemoteRouteEvent.Enable(availability.isAvailable(route))))
    }

    fun authorizationRestored(route: ControllerRemoteRouteKind): AndroidControllerRoutePlan {
        requireSelected(route)
        return plan(route, apply(route, ControllerRemoteRouteEvent.AuthorizationRestored(availability.isAvailable(route))))
    }

    fun setWriterHeld(held: Boolean) {
        val route = selectedRoute()
        val current = state(route)
        if (held && current.phase != ControllerRemoteRoutePhase.ONLINE) {
            coordinatorError(AndroidControllerRouteCoordinatorError.WRITER_REQUIRES_ONLINE_ROUTE)
        }
        states[route] = current.copy(writerHeld = held)
    }

    fun setAvailable(route: ControllerRemoteRouteKind, available: Boolean): AndroidControllerRoutePlan? {
        if (route == ControllerRemoteRouteKind.LOCAL_IPC || availability.isAvailable(route) == available) return null
        availability = availability.replacing(route, available)
        val current = state(route)
        if (!available) {
            if (current.phase == ControllerRemoteRoutePhase.DISABLED || current.phase == ControllerRemoteRoutePhase.REVOKED) return null
            if (selected == route) return plan(route, apply(route, ControllerRemoteRouteEvent.AvailabilityLost))
            states[route] = ControllerRemoteRouteState(route, ControllerRemoteRoutePhase.UNAVAILABLE)
        } else if (current.phase == ControllerRemoteRoutePhase.UNAVAILABLE) {
            states[route] = current.transition(ControllerRemoteRouteEvent.Enable(true)).state
        }
        return null
    }

    private fun selectedPlan(event: ControllerRemoteRouteEvent): AndroidControllerRoutePlan {
        val route = selectedRoute()
        return plan(route, apply(route, event))
    }

    private fun applySelected(route: ControllerRemoteRouteKind, event: ControllerRemoteRouteEvent): AndroidControllerRoutePlan {
        requireSelected(route)
        return plan(route, apply(route, event))
    }

    private fun apply(route: ControllerRemoteRouteKind, event: ControllerRemoteRouteEvent): ControllerRemoteRouteTransition {
        val transition = try {
            state(route).transition(event)
        } catch (error: ControllerRemoteRouteException) {
            transitionError(error.reason)
        }
        states[route] = transition.state
        return transition
    }

    private fun state(route: ControllerRemoteRouteKind) = states[route]
        ?: transitionError(ControllerRemoteRouteError.UNSUPPORTED_PLATFORM)

    private fun selectedRoute() = selected ?: coordinatorError(AndroidControllerRouteCoordinatorError.NO_SELECTED_ROUTE)

    private fun requireSelected(route: ControllerRemoteRouteKind) {
        if (selected != route) coordinatorError(AndroidControllerRouteCoordinatorError.ROUTE_NOT_SELECTED)
    }

    private fun plan(route: ControllerRemoteRouteKind, transition: ControllerRemoteRouteTransition) = AndroidControllerRoutePlan(
        disconnectTransport = route.takeIf { transition.disconnectTransport },
        clearPendingInput = transition.clearPendingInput,
        releaseWriter = transition.releaseWriter,
        retryIdempotentReads = transition.retryIdempotentReads,
        mutationDisposition = transition.mutationDisposition.takeUnless { it == ControllerRemoteMutationDisposition.NONE },
        requiresExplicitAction = transition.requiresExplicitAction,
    )

    private fun inactivePhase(phase: ControllerRemoteRoutePhase, available: Boolean) = when (phase) {
        ControllerRemoteRoutePhase.REVOKED -> ControllerRemoteRoutePhase.REVOKED
        ControllerRemoteRoutePhase.DISABLED -> ControllerRemoteRoutePhase.DISABLED
        ControllerRemoteRoutePhase.UNAVAILABLE -> ControllerRemoteRoutePhase.UNAVAILABLE
        else -> if (available) ControllerRemoteRoutePhase.IDLE else ControllerRemoteRoutePhase.UNAVAILABLE
    }

    private fun selectablePhase(phase: ControllerRemoteRoutePhase) = when (phase) {
        ControllerRemoteRoutePhase.REVOKED -> ControllerRemoteRoutePhase.REVOKED
        ControllerRemoteRoutePhase.DISABLED -> ControllerRemoteRoutePhase.DISABLED
        ControllerRemoteRoutePhase.UNAVAILABLE -> ControllerRemoteRoutePhase.UNAVAILABLE
        else -> ControllerRemoteRoutePhase.IDLE
    }

    private fun recovery(phase: ControllerRemoteRoutePhase) = when (phase) {
        ControllerRemoteRoutePhase.DISABLED -> AndroidControllerRouteRecovery.ENABLE
        ControllerRemoteRoutePhase.UNAVAILABLE -> AndroidControllerRouteRecovery.CONFIGURE
        ControllerRemoteRoutePhase.DEGRADED -> AndroidControllerRouteRecovery.RETRY
        ControllerRemoteRoutePhase.REVOKED -> AndroidControllerRouteRecovery.REAUTHORIZE
        ControllerRemoteRoutePhase.CONNECTING,
        ControllerRemoteRoutePhase.AUTHENTICATING,
        ControllerRemoteRoutePhase.RECONNECTING,
        -> AndroidControllerRouteRecovery.CANCEL
        ControllerRemoteRoutePhase.IDLE,
        ControllerRemoteRoutePhase.ONLINE,
        -> AndroidControllerRouteRecovery.NONE
    }
}

private fun coordinatorError(error: AndroidControllerRouteCoordinatorError): Nothing =
    throw AndroidControllerRouteCoordinatorException(reason = error)

private fun transitionError(error: ControllerRemoteRouteError): Nothing =
    throw AndroidControllerRouteCoordinatorException(transition = error)
