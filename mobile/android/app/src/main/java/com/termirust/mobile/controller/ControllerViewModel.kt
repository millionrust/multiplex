package com.termirust.mobile.controller

import android.app.Application
import android.os.Build
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.net.SocketException
import java.net.SocketTimeoutException
import java.security.SecureRandom
import java.util.UUID
import kotlin.math.min

class ControllerViewModel(application: Application) : AndroidViewModel(application) {
    private val hostStore = PairedHostStore(application)
    private val cacheStore = ControllerFleetCacheStore(application)
    private val secureBlobs = ControllerSecureBlobStore(application)
    private val routeConnections = AndroidControllerRouteConnections(
        privateNetwork = ControllerConnection(secureBlobs),
        ssh = null,
        selfHostedRelay = null,
    )
    private val routePreferences = application.getSharedPreferences("controller-routes-v1", 0)
    private val routeCoordinator = AndroidControllerRouteCoordinator(routeConnections.availability())
    private val random = SecureRandom()
    private val deviceId = loadDeviceId(application)
    private val _state = MutableStateFlow(ControllerUiState())
    val state: StateFlow<ControllerUiState> = _state.asStateFlow()
    val pairingOffer = MutableStateFlow("")
    val pairingHostName = MutableStateFlow("My Computer")
    val pairingDeviceName = MutableStateFlow("Android ${Build.MODEL}".take(64))
    private var hosts: List<PairedHostRecord> = emptyList()
    private var cache = ControllerCacheDocument()
    private var operation: Job? = null
    private var terminalRender: Job? = null
    private var terminalResize: Job? = null
    private var terminalRuntime: ActiveTerminalRuntime? = null
    private var routeError: String? = null

    init {
        val persisted = routePreferences.getString(SELECTED_ROUTE_KEY, null)
            ?.let { raw -> ControllerRemoteRouteKind.entries.firstOrNull { it.name == raw } }
            ?.takeIf { it in ControllerRemoteRouteKind.androidRoutes }
            ?: ControllerRemoteRouteKind.PRIVATE_NETWORK
        routeCoordinator.restorePersistedSelection(persisted)
        operation = viewModelScope.launch { restore() }
    }

    fun selectControllerRoute(target: ControllerRemoteRouteKind, explicitlyConfirmed: Boolean): Boolean {
        terminalRuntime?.let(::syncRouteWriter)
        val source = routeCoordinator.selected
        val plan = try {
            routeCoordinator.select(target, explicitlyConfirmed)
        } catch (error: AndroidControllerRouteCoordinatorException) {
            routeError = routeSelectionError(error)
            publishRouteState()
            return false
        }
        routeError = null
        routePreferences.edit().putString(SELECTED_ROUTE_KEY, target.name).apply()
        operation?.cancel()
        terminalResize?.cancel()
        terminalResize = null
        terminalRuntime?.let { runtime ->
            if (plan.clearPendingInput) {
                runtime.pendingPaste = null
                runtime.pendingResize = null
                runtime.inputInFlight = null
                runtime.pendingMutations.clear()
            }
            runtime.writer.releaseLocally()
            runtime.reducer.markOffline()
            publishTerminal(runtime, privacyCovered = true)
        }
        operation = viewModelScope.launch {
            if (plan.releaseWriter) terminalRuntime?.let { releaseWriterForLifecycle(it, true) }
            plan.disconnectTransport?.let { route -> routeConnections.disconnect(route) }
            if (source != target) {
                terminalRender?.cancel()
                terminalRender = null
                terminalRuntime = null
                _state.value = _state.value.copy(activeTerminal = null)
            }
            refreshSelected(retry = true)
        }
        publishRouteState()
        return true
    }

    fun beginPairing() {
        operation?.cancel()
        _state.value = _state.value.copy(connection = ControllerConnectionState.Pairing)
        operation = viewModelScope.launch {
            runCatching {
                pairingConnection().beginPairing(
                    offerText = pairingOffer.value,
                    hostName = pairingHostName.value.trim(),
                    deviceName = pairingDeviceName.value.trim(),
                    deviceId = deviceId,
                )
            }.onSuccess { challenge ->
                _state.value = _state.value.copy(
                    connection = ControllerConnectionState.SasReady(
                        challenge.sas,
                        challenge.fingerprintSuffix,
                    ),
                )
            }.onFailure { error ->
                pairingOffer.value = ""
                showFailure(error)
            }
        }
    }

    fun finishPairing(matches: Boolean) {
        operation?.cancel()
        _state.value = _state.value.copy(connection = ControllerConnectionState.Pairing)
        operation = viewModelScope.launch {
            runCatching { pairingConnection().finishPairing(matches) }
                .onSuccess { host ->
                    if (routeCoordinator.selected != ControllerRemoteRouteKind.PRIVATE_NETWORK) {
                        runCatching {
                            routeCoordinator.select(
                                ControllerRemoteRouteKind.PRIVATE_NETWORK,
                                explicitlyConfirmed = true,
                            )
                        }
                        routePreferences.edit()
                            .putString(SELECTED_ROUTE_KEY, ControllerRemoteRouteKind.PRIVATE_NETWORK.name)
                            .apply()
                    }
                    hosts = hostStore.upsert(host)
                    routePreferences.edit().putString(SELECTED_HOST_KEY, host.id).apply()
                    pairingOffer.value = ""
                    _state.value = makeState(
                        selectedHostId = host.id,
                        connectionState = ControllerConnectionState.PairedOffline,
                    )
                    refreshSelected(retry = true)
                }
                .onFailure { error ->
                    pairingOffer.value = ""
                    showFailure(error)
                }
        }
    }

    fun cancelPairing() {
        operation?.cancel()
        operation = viewModelScope.launch { pairingConnection().cancel() }
        _state.value = makeState(
            selectedHostId = _state.value.selectedHostId,
            connectionState = if (hosts.isEmpty()) {
                ControllerConnectionState.Unpaired
            } else {
                ControllerConnectionState.PairedOffline
            },
        )
    }

    fun selectHost(hostId: String) {
        if (hosts.none { it.id == hostId }) return
        operation?.cancel()
        routePreferences.edit().putString(SELECTED_HOST_KEY, hostId).apply()
        _state.value = makeState(hostId, ControllerConnectionState.PairedOffline)
        operation = viewModelScope.launch { refreshSelected(retry = true) }
    }

    fun retry() {
        operation?.cancel()
        operation = viewModelScope.launch { refreshSelected(retry = true) }
    }

    fun onForeground() {
        val terminal = terminalRuntime
        operation?.cancel()
        operation = if (terminal != null) {
            viewModelScope.launch {
                connectionFor(terminal.route).cancel()
                terminal.writer.setForeground(true)
                runTerminal(terminal)
            }
        } else {
            viewModelScope.launch { refreshSelected(retry = true) }
        }
    }

    fun onBackground() {
        operation?.cancel()
        terminalRuntime?.let { runtime ->
            val decision = TerminalAcceptance.backgroundDecision(
                runtime.writer.lease == WriterLeaseState.Held,
            )
            terminalResize?.cancel()
            terminalResize = null
            if (decision.clearPendingResize) runtime.pendingResize = null
            if (decision.clearPendingInput) runtime.pendingPaste = null
            runtime.writer.setForeground(false)
            runtime.reducer.markOffline()
            publishTerminal(runtime, privacyCovered = decision.coverPrivacy)
            operation = viewModelScope.launch {
                releaseWriterForLifecycle(runtime, decision.releaseWriter)
                connectionFor(runtime.route).cancel()
                if (routeCoordinator.selected == runtime.route) {
                    runCatching { routeCoordinator.cancelSelected() }
                    publishRouteState()
                }
            }
            return
        }
        operation = viewModelScope.launch { cancelSelectedRoute() }
        val selected = _state.value.selectedHostId
        _state.value = makeState(selected, ControllerConnectionState.PairedOffline)
    }

    fun attachSession(sessionId: String) {
        val host = selectedHost() ?: return
        val session = _state.value.sessions.firstOrNull { it.id == sessionId } ?: return
        val generation = session.occupantGeneration ?: return
        val identity = ReadOnlyAttachIdentity(
            host.id,
            UUID.fromString(session.id),
            generation,
            session.hostInstanceId?.let(UUID::fromString),
        )
        val viewport = TerminalViewport(120, 40)
        val runtime = ActiveTerminalRuntime(
            route = selectedRoute(),
            host = host,
            session = session,
            reducer = ReadOnlyAttachReducer(identity),
            writer = WriterControlReducer(identity),
            terminal = BoundedControllerTerminal(viewport),
            queue = BoundedTerminalFrameQueue(),
            viewport = viewport,
        )
        operation?.cancel()
        terminalRuntime = runtime
        publishTerminal(runtime)
        operation = viewModelScope.launch {
            connectionFor(runtime.route).cancel()
            runTerminal(runtime)
        }
    }

    fun retryTerminal() {
        val runtime = terminalRuntime ?: return
        if (operation?.isActive == true) return
        operation = viewModelScope.launch {
            connectionFor(runtime.route).cancel()
            runTerminal(runtime)
        }
    }

    fun requestTerminalControl() {
        val runtime = terminalRuntime ?: return
        if (!runtime.supportsWriter || !runtime.writer.isForeground) return
        runtime.acquireAfterAttach = true
        if (runtime.interactive && runtime.reducer.state == ReadOnlyAttachState.Live) {
            runtime.acquireAfterAttach = false
            issueAcquire(runtime)
            return
        }
        runtime.interactive = true
        operation?.cancel()
        operation = viewModelScope.launch {
            connectionFor(runtime.route).cancel()
            runTerminal(runtime)
        }
    }

    fun releaseTerminalControl() {
        val runtime = terminalRuntime ?: return
        if (runtime.writer.lease != WriterLeaseState.Held) return
        val commandId = UUID.randomUUID()
        runtime.pendingMutations[commandId] = TerminalMutation.RELEASE
        runtime.writer.releaseLocally()
        syncRouteWriter(runtime)
        publishTerminal(runtime)
        viewModelScope.launch {
            runCatching { connectionFor(runtime.route).releaseWriter(runtime.host, runtime.writer.identity, commandId) }
                .onFailure { mutationSendFailed(runtime, commandId) }
        }
    }

    fun sendTerminalBytes(bytes: ByteArray) = enqueueTerminalInput(bytes, PendingInputKind.KEYBOARD, true)

    fun requestTerminalPaste(text: String) {
        val runtime = terminalRuntime ?: return
        val bytes = TerminalInteraction.normalizePaste(text)
        if (bytes.isEmpty()) return
        if (bytes.size > TerminalInteraction.maximumPastePayload(
                bracketed = runtime.terminal.snapshot().bracketedPaste,
            )
        ) {
            runtime.writerMessage = "paste_too_large"
            publishTerminal(runtime)
        } else if (TerminalInteraction.pasteRequiresConfirmation(bytes)) {
            runtime.pendingPaste = bytes
            publishTerminal(runtime)
        } else {
            sendTerminalPaste(runtime, bytes)
        }
    }

    fun confirmTerminalPaste() {
        val runtime = terminalRuntime ?: return
        val bytes = runtime.pendingPaste ?: return
        runtime.pendingPaste = null
        sendTerminalPaste(runtime, bytes)
    }

    fun cancelTerminalPaste() {
        val runtime = terminalRuntime ?: return
        runtime.pendingPaste = null
        publishTerminal(runtime)
    }

    private fun sendTerminalPaste(runtime: ActiveTerminalRuntime, bytes: ByteArray) {
        if (terminalRuntime !== runtime) return
        val prepared = TerminalInteraction.preparePaste(
            bytes,
            bracketed = runtime.terminal.snapshot().bracketedPaste,
        )
        enqueueTerminalInput(prepared, PendingInputKind.PASTE, true)
    }

    fun detachTerminal() {
        val runtime = terminalRuntime
        val held = runtime?.writer?.lease == WriterLeaseState.Held
        operation?.cancel()
        terminalRender?.cancel()
        terminalResize?.cancel()
        terminalRender = null
        operation = viewModelScope.launch {
            if (runtime != null) releaseWriterForLifecycle(runtime, held)
            if (runtime != null) connectionFor(runtime.route).cancel()
            refreshSelected(retry = false)
        }
        runtime?.writer?.releaseLocally()
        runtime?.reducer?.detach()
        terminalRuntime = null
        _state.value = _state.value.copy(activeTerminal = null)
    }

    fun updateTerminalViewport(columns: Int, rows: Int) {
        val runtime = terminalRuntime ?: return
        val viewport = TerminalViewport(columns, rows)
        if (runCatching { TerminalLimits().validate(viewport) }.isFailure) return
        if (runtime.viewport == viewport) return
        if (runtime.reducer.cursor.outputSequence == 0L) {
            runtime.terminal.resize(viewport)
        }
        runtime.viewport = viewport
        runtime.pendingResize = viewport
        if (runtime.supportsResize && runtime.writer.lease == WriterLeaseState.Held) {
            runtime.writerViewportReady = false
            runtime.inputBlockedForResize = true
            terminalResize?.cancel()
            terminalResize = viewModelScope.launch {
                delay(50)
                sendPendingResize(runtime)
            }
        }
        publishTerminal(runtime)
    }

    fun forgetSelectedHost() {
        val host = selectedHost() ?: return
        operation?.cancel()
        operation = viewModelScope.launch {
            cancelSelectedRoute()
            runCatching { secureBlobs.delete(host.deviceStaticKeyId) }
            hosts = hostStore.remove(host.id)
            cache = cacheStore.remove(cache, host.id)
            val next = hosts.maxByOrNull(PairedHostRecord::pairedAtMillis)
            routePreferences.edit().apply {
                if (next == null) remove(SELECTED_HOST_KEY) else putString(SELECTED_HOST_KEY, next.id)
            }.apply()
            _state.value = makeState(
                selectedHostId = next?.id,
                connectionState = if (hosts.isEmpty()) {
                    ControllerConnectionState.Unpaired
                } else {
                    ControllerConnectionState.PairedOffline
                },
            )
        }
    }

    override fun onCleared() {
        operation?.cancel()
        terminalRender?.cancel()
        terminalResize?.cancel()
        routeConnections.close()
        super.onCleared()
    }

    private suspend fun restore() {
        hosts = hostStore.load()
        cache = cacheStore.load()
        val savedHostId = routePreferences.getString(SELECTED_HOST_KEY, null)
        val selected = hosts.firstOrNull { it.id == savedHostId }
            ?: hosts.maxByOrNull(PairedHostRecord::pairedAtMillis)
        selected?.let { routePreferences.edit().putString(SELECTED_HOST_KEY, it.id).apply() }
        _state.value = makeState(
            selectedHostId = selected?.id,
            connectionState = if (selected == null) {
                ControllerConnectionState.Unpaired
            } else {
                ControllerConnectionState.PairedOffline
            },
        )
        if (selected != null) refreshSelected(retry = true)
    }

    private suspend fun refreshSelected(retry: Boolean) {
        var host = selectedHost() ?: return
        val route = selectedRoute()
        val connection = try {
            connectionFor(route)
        } catch (error: Throwable) {
            routeError = "route_unavailable"
            _state.value = makeState(host.id, ControllerConnectionState.Failed(classify(error)))
            return
        }
        prepareRouteStart(route)
        val started = System.currentTimeMillis()
        var attempt = 0
        while (true) {
            try {
                val snapshot = connection.fetchSessions(host) { state ->
                    if (routeCoordinator.selected != route) throw CancellationException()
                    when (state) {
                        ControllerConnectionState.Authenticating -> markTransportReady(route)
                        ControllerConnectionState.Syncing -> markAuthenticated(route)
                        else -> Unit
                    }
                    _state.value = makeState(host.id, state)
                }
                markAuthenticated(route)
                if (host.capabilityBits != snapshot.capabilityBits) {
                    host = host.copy(capabilityBits = snapshot.capabilityBits)
                    hosts = hostStore.upsert(host)
                }
                cache = cacheStore.saveSnapshot(
                    current = cache,
                    selectedHostId = host.id,
                    host = host,
                    snapshot = snapshot,
                    nowMillis = System.currentTimeMillis(),
                )
                _state.value = makeState(host.id, ControllerConnectionState.ReadyReadOnly)
                return
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                val willRetry = retry && isRetryable(error) && attempt < 7 &&
                    System.currentTimeMillis() - started < 90_000
                markRouteFailure(route, retryable = willRetry, mutationInFlight = false)
                if (!willRetry) {
                    _state.value = makeState(
                        host.id,
                        ControllerConnectionState.Failed(classify(error)),
                    )
                    return
                }
                val cap = min(10_000L, 250L shl attempt)
                delay(Math.floorMod(random.nextLong(), cap + 1))
                attempt += 1
            }
        }
    }

    private fun makeState(
        selectedHostId: String?,
        connectionState: ControllerConnectionState,
    ): ControllerUiState {
        val cached = selectedHostId?.let(cache.hosts::get)
        val online = connectionState == ControllerConnectionState.ReadyReadOnly
        return ControllerUiState(
            hosts = hosts,
            selectedHostId = selectedHostId,
            sessions = cached?.snapshot?.sessions.orEmpty(),
            connection = connectionState,
            cachedAtMillis = cached?.updatedAtMillis,
            cachedReadOnly = cached != null && !online,
            activeTerminal = _state.value.activeTerminal,
            selectedRoute = selectedRoute(),
            routeProjections = routeCoordinator.projections,
            routeError = routeError,
        )
    }

    private suspend fun runTerminal(runtime: ActiveTerminalRuntime) {
        if (terminalRuntime !== runtime) return
        if (runtime.reducer.state != ReadOnlyAttachState.Detached &&
            runtime.reducer.state != ReadOnlyAttachState.Offline
        ) runtime.reducer.markOffline()
        try {
            prepareRouteStart(runtime.route)
            markTransportReady(runtime.route)
            runtime.reducer.beginAuthentication()
            publishTerminal(runtime)
            val consume: suspend (ReadOnlyWireEvent) -> Unit = { event ->
                if (terminalRuntime !== runtime) throw CancellationException()
                withContext(Dispatchers.Main.immediate) {
                    if (terminalRuntime !== runtime) throw CancellationException()
                    markAuthenticated(runtime.route)
                    consumeTerminalEvent(runtime, event)
                }
            }
            if (runtime.interactive) {
                connectionFor(runtime.route).attachInteractive(runtime.host, runtime.reducer.cursor, runtime.viewport, consume)
            } else {
                connectionFor(runtime.route).attachReadOnly(runtime.host, runtime.reducer.cursor, runtime.viewport, consume)
            }
        } catch (error: CancellationException) {
            if (terminalRuntime === runtime && runtime.reducer.state != ReadOnlyAttachState.Detached) {
                runtime.reducer.markOffline()
                publishTerminal(runtime, privacyCovered = _state.value.activeTerminal?.privacyCovered == true)
            }
            throw error
        } catch (error: Throwable) {
            if (terminalRuntime !== runtime) return
            markRouteFailure(
                runtime.route,
                retryable = isRetryable(error),
                mutationInFlight = runtime.pendingMutations.isNotEmpty(),
            )
            if (runtime.writer.lease == WriterLeaseState.Held ||
                runtime.writer.lease is WriterLeaseState.Requesting
            ) {
                runtime.writer.markLeaseLost()
                runtime.pendingMutations.clear()
                runtime.inputInFlight = null
                runtime.writerMessage = "connection_failed_no_replay"
            }
            if (runtime.reducer.state !is ReadOnlyAttachState.Gap) {
                if (error is ControllerConnectionException.HostError && error.code.contains("exit", true)) {
                    runtime.reducer.markExited()
                } else if (error is IllegalArgumentException) {
                    runtime.reducer.fail("malformed_or_resource_limit")
                } else {
                    runtime.reducer.markOffline()
                }
            }
            publishTerminal(runtime)
        }
    }

    private fun consumeTerminalEvent(runtime: ActiveTerminalRuntime, event: ReadOnlyWireEvent) {
        when (event) {
            is ReadOnlyWireEvent.Snapshot -> {
                if (event.chunk.chunkIndex == 0) {
                    runtime.reducer.beginSnapshot()
                    runtime.terminal.reset(event.chunk.viewport)
                }
                runtime.terminal.process(event.chunk.bytes)
                runtime.reducer.observeSnapshot(event.chunk)
            }
            is ReadOnlyWireEvent.Attached -> {
                runtime.reducer.bindReplayBarrier(event.replayThroughSequence)
                runtime.hasWriterElsewhere = event.hasWriterLease
                if (runtime.acquireAfterAttach) {
                    runtime.acquireAfterAttach = false
                    issueAcquire(runtime)
                }
            }
            is ReadOnlyWireEvent.Output -> {
                runtime.queue.enqueue(event.frame)
                while (true) {
                    val frame = runtime.queue.removeFirstOrNull() ?: break
                    when (runtime.reducer.observe(frame)) {
                        OutputDisposition.DELIVER -> runtime.terminal.process(frame.bytes)
                        OutputDisposition.DUPLICATE -> Unit
                        OutputDisposition.GAP -> throw ControllerConnectionException.SequenceGap
                    }
                }
            }
            is ReadOnlyWireEvent.HostError -> failMutation(runtime, event.commandId, event.code, event.completionUnknown)
            is ReadOnlyWireEvent.Completed -> completeMutation(runtime, event.commandId, event.applied)
        }
        if (event is ReadOnlyWireEvent.Output) {
            scheduleTerminalPublish(runtime)
        } else {
            publishTerminal(runtime)
        }
    }

    private fun scheduleTerminalPublish(runtime: ActiveTerminalRuntime) {
        if (terminalRender?.isActive == true) return
        terminalRender = viewModelScope.launch {
            delay(16)
            publishTerminal(runtime)
        }
    }

    private fun publishTerminal(runtime: ActiveTerminalRuntime, privacyCovered: Boolean = false) {
        if (terminalRuntime !== runtime) return
        _state.value = _state.value.copy(
            activeTerminal = ControllerTerminalUiState(
                hostTitle = runtime.host.displayName,
                sessionTitle = runtime.session.title,
                attachState = runtime.reducer.state,
                screen = runtime.terminal.snapshot(),
                outputSequence = runtime.reducer.cursor.outputSequence,
                hasWriterElsewhere = runtime.hasWriterElsewhere,
                writerLease = runtime.writer.lease,
                writerMessage = runtime.writerMessage,
                pendingPasteBytes = runtime.pendingPaste?.size ?: 0,
                supportsWriter = runtime.supportsWriter,
                supportsResize = runtime.supportsResize,
                privacyCovered = privacyCovered,
            ),
        )
    }

    private fun issueAcquire(runtime: ActiveTerminalRuntime) {
        if (terminalRuntime !== runtime || !runtime.interactive) return
        val commandId = UUID.randomUUID()
        try {
            runtime.writer.beginAcquire(commandId)
        } catch (_: IllegalArgumentException) {
            runtime.writerMessage = "control_unavailable"
            publishTerminal(runtime)
            return
        }
        runtime.pendingMutations[commandId] = TerminalMutation.ACQUIRE
        runtime.writerViewportReady = false
        publishTerminal(runtime)
        viewModelScope.launch {
            runCatching { connectionFor(runtime.route).requestWriter(runtime.host, runtime.writer.identity, commandId) }
                .onFailure { mutationSendFailed(runtime, commandId) }
        }
    }

    private fun enqueueTerminalInput(bytes: ByteArray, kind: PendingInputKind, confirmed: Boolean) {
        val runtime = terminalRuntime ?: return
        if (runtime.writer.lease != WriterLeaseState.Held || bytes.isEmpty()) return
        try {
            var offset = 0
            while (offset < bytes.size) {
                val end = (offset + ControllerWriterWireCodec.MAX_INPUT_CHUNK_BYTES).coerceAtMost(bytes.size)
                runtime.writer.enqueue(bytes.copyOfRange(offset, end), kind, confirmed)
                offset = end
            }
            runtime.writerMessage = null
            drainInput(runtime)
        } catch (_: IllegalArgumentException) {
            runtime.writerMessage = "input_pressure"
        }
        publishTerminal(runtime)
    }

    private fun drainInput(runtime: ActiveTerminalRuntime) {
        if (runtime.inputInFlight != null || runtime.writer.lease != WriterLeaseState.Held ||
            runtime.inputBlockedForResize || !runtime.writerViewportReady
        ) return
        val pending = runtime.writer.removeFirstOrNull() ?: return
        runtime.inputInFlight = pending.commandId
        runtime.pendingMutations[pending.commandId] = TerminalMutation.INPUT
        viewModelScope.launch {
            runCatching {
                connectionFor(runtime.route).sendInput(runtime.host, runtime.writer.identity, pending.commandId, pending.bytes)
            }.onFailure { mutationSendFailed(runtime, pending.commandId) }
        }
    }

    private fun sendPendingResize(runtime: ActiveTerminalRuntime) {
        val viewport = runtime.pendingResize ?: return
        if (runtime.writer.lease != WriterLeaseState.Held ||
            runtime.reducer.state != ReadOnlyAttachState.Live ||
            runtime.pendingMutations.values.any { it is TerminalMutation.Resize }
        ) return
        runtime.pendingResize = null
        val commandId = UUID.randomUUID()
        runtime.pendingMutations[commandId] = TerminalMutation.Resize(viewport)
        viewModelScope.launch {
            runCatching {
                connectionFor(runtime.route).sendResize(runtime.host, runtime.writer.identity, commandId, viewport)
            }.onFailure { mutationSendFailed(runtime, commandId) }
        }
    }

    private fun completeMutation(runtime: ActiveTerminalRuntime, commandId: UUID, applied: Boolean) {
        var sendNextResize = false
        var drainQueuedInput = false
        when (val mutation = runtime.pendingMutations.remove(commandId)
            ?: return loseWriter(runtime, "stale_response")) {
            TerminalMutation.ACQUIRE -> {
                runCatching { runtime.writer.finishAcquire(commandId, applied) }
                    .onFailure { return loseWriter(runtime, "stale_response") }
                runtime.hasWriterElsewhere = !applied
                runtime.writerMessage = if (applied) null else "controlled_elsewhere"
                if (applied) {
                    sendNextResize = runtime.supportsResize && runtime.pendingResize != null
                    runtime.writerViewportReady = !sendNextResize
                    runtime.inputBlockedForResize = sendNextResize
                    drainQueuedInput = !sendNextResize
                }
            }
            TerminalMutation.RELEASE -> Unit
            TerminalMutation.INPUT -> {
                if (runtime.inputInFlight != commandId || !applied) return loseWriter(runtime, "input_rejected")
                runtime.inputInFlight = null
                drainQueuedInput = true
            }
            is TerminalMutation.Resize -> {
                if (applied) {
                    runCatching { runtime.terminal.resize(mutation.viewport) }
                        .onFailure { return loseWriter(runtime, "resize_rejected") }
                    sendNextResize = runtime.pendingResize != null
                    runtime.writerViewportReady = !sendNextResize
                    runtime.inputBlockedForResize = sendNextResize
                    drainQueuedInput = !sendNextResize
                } else {
                    runtime.pendingResize = mutation.viewport
                    runtime.writerMessage = "resize_rejected"
                }
            }
        }
        syncRouteWriter(runtime)
        publishTerminal(runtime)
        if (sendNextResize) sendPendingResize(runtime)
        if (drainQueuedInput) drainInput(runtime)
    }

    private fun failMutation(
        runtime: ActiveTerminalRuntime,
        commandId: UUID,
        code: String,
        completionUnknown: Boolean,
    ) {
        val mutation = runtime.pendingMutations.remove(commandId) ?: return loseWriter(runtime, "unmatched_error")
        if (mutation is TerminalMutation.Resize) runtime.pendingResize = mutation.viewport
        if (runtime.inputInFlight == commandId) runtime.inputInFlight = null
        if (mutation == TerminalMutation.ACQUIRE && !completionUnknown) {
            runCatching { runtime.writer.finishAcquire(commandId, false) }
            runtime.hasWriterElsewhere = code == "writer_lease_required" || code == "writer_busy"
            runtime.writerMessage = "control_unavailable"
            publishTerminal(runtime)
        } else {
            loseWriter(runtime, if (completionUnknown) "completion_unknown" else code)
        }
    }

    private fun mutationSendFailed(runtime: ActiveTerminalRuntime, commandId: UUID) {
        val mutation = runtime.pendingMutations.remove(commandId) ?: return
        if (mutation is TerminalMutation.Resize) runtime.pendingResize = mutation.viewport
        if (runtime.inputInFlight == commandId) runtime.inputInFlight = null
        loseWriter(runtime, "connection_failed_no_replay")
    }

    private fun loseWriter(runtime: ActiveTerminalRuntime, message: String) {
        runtime.writer.markLeaseLost()
        syncRouteWriter(runtime)
        runtime.writerViewportReady = false
        runtime.inputBlockedForResize = false
        runtime.pendingMutations.clear()
        runtime.inputInFlight = null
        runtime.writerMessage = message
        publishTerminal(runtime)
    }

    private suspend fun releaseWriterForLifecycle(runtime: ActiveTerminalRuntime, wasHeld: Boolean) {
        if (!wasHeld) return
        val commandId = UUID.randomUUID()
        runtime.writer.releaseLocally()
        syncRouteWriter(runtime)
        runCatching { connectionFor(runtime.route).releaseWriter(runtime.host, runtime.writer.identity, commandId) }
    }

    private fun selectedHost(): PairedHostRecord? =
        hosts.firstOrNull { it.id == _state.value.selectedHostId }

    private fun showFailure(error: Throwable) {
        if (error is CancellationException) return
        _state.value = _state.value.copy(connection = ControllerConnectionState.Failed(classify(error)))
    }

    private fun classify(error: Throwable): String = when (error) {
        is ControllerSecretException.Invalidated -> "keystore_invalidated"
        is ControllerSecretException.Corrupt -> "secret_corrupt"
        is ControllerStoreException.ResourceLimit -> "resource_limit"
        is ControllerConnectionException.PairingRejected -> "pairing_rejected"
        is ControllerConnectionException.AcknowledgementUncertain -> "pairing_acknowledgement_uncertain"
        is ControllerConnectionException.SequenceGap -> "sequence_gap"
        is ControllerConnectionException.HostError -> error.code
        is SocketTimeoutException -> "timeout"
        is SocketException -> "offline"
        is IllegalArgumentException -> "invalid_data"
        else -> "operation_failed"
    }

    private fun isRetryable(error: Throwable): Boolean =
        error is SocketTimeoutException || error is SocketException

    private fun loadDeviceId(application: Application): UUID {
        val preferences = application.getSharedPreferences("controller-device", 0)
        val existing = preferences.getString("device_id", null)
        if (existing != null) runCatching { return UUID.fromString(existing) }
        return UUID.randomUUID().also { preferences.edit().putString("device_id", it.toString()).apply() }
    }

    private fun selectedRoute(): ControllerRemoteRouteKind =
        routeCoordinator.selected ?: ControllerRemoteRouteKind.PRIVATE_NETWORK

    private fun pairingConnection(): ControllerConnecting =
        routeConnections.privateNetwork ?: throw ControllerRouteUnavailableException(ControllerRemoteRouteKind.PRIVATE_NETWORK)

    private fun connectionFor(route: ControllerRemoteRouteKind): ControllerConnecting =
        routeConnections.connection(route) ?: throw ControllerRouteUnavailableException(route)

    private suspend fun cancelSelectedRoute() {
        val route = routeCoordinator.selected ?: return
        routeConnections.connection(route)?.cancel()
        runCatching { routeCoordinator.cancelSelected() }
        publishRouteState()
    }

    private fun prepareRouteStart(route: ControllerRemoteRouteKind) {
        if (routeCoordinator.selected != route) {
            throw ControllerRouteUnavailableException(route)
        }
        when (projection(route).phase) {
            ControllerRemoteRoutePhase.IDLE -> routeCoordinator.connectSelected()
            ControllerRemoteRoutePhase.DEGRADED -> routeCoordinator.retrySelected()
            ControllerRemoteRoutePhase.CONNECTING,
            ControllerRemoteRoutePhase.AUTHENTICATING,
            ControllerRemoteRoutePhase.ONLINE,
            ControllerRemoteRoutePhase.RECONNECTING,
            -> Unit
            ControllerRemoteRoutePhase.DISABLED,
            ControllerRemoteRoutePhase.UNAVAILABLE,
            ControllerRemoteRoutePhase.REVOKED,
            -> throw ControllerRouteUnavailableException(route)
        }
        publishRouteState()
    }

    private fun markTransportReady(route: ControllerRemoteRouteKind) {
        if (routeCoordinator.selected == route && projection(route).phase in setOf(
                ControllerRemoteRoutePhase.CONNECTING,
                ControllerRemoteRoutePhase.RECONNECTING,
            )
        ) {
            routeCoordinator.transportReady(route)
            publishRouteState()
        }
    }

    private fun markAuthenticated(route: ControllerRemoteRouteKind) {
        if (routeCoordinator.selected == route &&
            projection(route).phase == ControllerRemoteRoutePhase.AUTHENTICATING
        ) {
            routeCoordinator.authenticated(route)
            routeError = null
            publishRouteState()
        }
    }

    private fun markRouteFailure(
        route: ControllerRemoteRouteKind,
        retryable: Boolean,
        mutationInFlight: Boolean,
    ) {
        if (routeCoordinator.selected != route) return
        val phase = projection(route).phase
        if (phase in setOf(
                ControllerRemoteRoutePhase.CONNECTING,
                ControllerRemoteRoutePhase.AUTHENTICATING,
                ControllerRemoteRoutePhase.ONLINE,
                ControllerRemoteRoutePhase.RECONNECTING,
            )
        ) {
            routeCoordinator.failed(route, retryable, mutationInFlight)
            routeError = if (retryable) null else "route_degraded"
            publishRouteState()
        }
    }

    private fun projection(route: ControllerRemoteRouteKind): AndroidControllerRouteProjection =
        routeCoordinator.projections.first { it.route == route }

    private fun syncRouteWriter(runtime: ActiveTerminalRuntime) {
        if (routeCoordinator.selected != runtime.route ||
            projection(runtime.route).phase != ControllerRemoteRoutePhase.ONLINE
        ) return
        runCatching { routeCoordinator.setWriterHeld(runtime.writer.lease == WriterLeaseState.Held) }
        publishRouteState()
    }

    private fun publishRouteState() {
        _state.value = _state.value.copy(
            selectedRoute = selectedRoute(),
            routeProjections = routeCoordinator.projections,
            routeError = routeError,
        )
    }

    private fun routeSelectionError(error: AndroidControllerRouteCoordinatorException) = when (error.transition) {
        ControllerRemoteRouteError.EXPLICIT_CONFIRMATION_REQUIRED -> "route_confirmation_required"
        ControllerRemoteRouteError.TARGET_UNAVAILABLE -> "route_unavailable"
        ControllerRemoteRouteError.SAME_ROUTE -> "route_already_selected"
        ControllerRemoteRouteError.UNSUPPORTED_PLATFORM -> "route_unsupported"
        ControllerRemoteRouteError.INVALID_TRANSITION, null -> "route_change_failed"
    }

    private companion object {
        const val SELECTED_ROUTE_KEY = "selected_route"
        const val SELECTED_HOST_KEY = "selected_host"
    }
}

private data class ActiveTerminalRuntime(
    val route: ControllerRemoteRouteKind,
    val host: PairedHostRecord,
    val session: ControllerSessionSummary,
    val reducer: ReadOnlyAttachReducer,
    val writer: WriterControlReducer,
    val terminal: BoundedControllerTerminal,
    val queue: BoundedTerminalFrameQueue,
    var viewport: TerminalViewport,
    var hasWriterElsewhere: Boolean = false,
    var interactive: Boolean = false,
    var acquireAfterAttach: Boolean = false,
    var writerMessage: String? = null,
    var pendingPaste: ByteArray? = null,
    var pendingResize: TerminalViewport? = null,
    var writerViewportReady: Boolean = false,
    var inputBlockedForResize: Boolean = false,
    var inputInFlight: UUID? = null,
    val pendingMutations: MutableMap<UUID, TerminalMutation> = mutableMapOf(),
)

private class ControllerRouteUnavailableException(route: ControllerRemoteRouteKind) :
    IllegalStateException("Controller route unavailable: ${route.name.lowercase()}")

private sealed interface TerminalMutation {
    data object ACQUIRE : TerminalMutation
    data object RELEASE : TerminalMutation
    data object INPUT : TerminalMutation
    data class Resize(val viewport: TerminalViewport) : TerminalMutation
}

private val ActiveTerminalRuntime.supportsWriter: Boolean
    get() {
        val hostAllows = host.capabilityBits and ((1 shl 1) or (1 shl 2)) == ((1 shl 1) or (1 shl 2))
        val sessionAllows = session.capabilities.isEmpty() || (
            ControllerSessionCapability.ATTACH_OUTPUT in session.capabilities &&
                ControllerSessionCapability.SEND_INPUT in session.capabilities
            )
        return hostAllows && sessionAllows
    }

private val ActiveTerminalRuntime.supportsResize: Boolean
    get() = host.capabilityBits and (1 shl 3) != 0 &&
        (session.capabilities.isEmpty() || ControllerSessionCapability.RESIZE in session.capabilities)
