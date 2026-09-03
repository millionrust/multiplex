package com.termirust.mobile.controller

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.activity.compose.BackHandler
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ControllerApp(viewModel: ControllerViewModel, modifier: Modifier = Modifier) {
    val state by viewModel.state.collectAsState()
    var showPairing by remember { mutableStateOf(false) }
    var showScanner by remember { mutableStateOf(false) }
    var showHostDetails by remember { mutableStateOf(false) }
    var confirmForget by remember { mutableStateOf(false) }
    var pendingRoute by remember { mutableStateOf<ControllerRemoteRouteKind?>(null) }
    val activeTerminal = state.activeTerminal

    BackHandler(enabled = activeTerminal != null) { viewModel.detachTerminal() }

    MaterialTheme {
        Scaffold(
            modifier = modifier.fillMaxSize(),
            topBar = {
                TopAppBar(
                    title = {
                        Column {
                            Text(stringResource(com.termirust.mobile.R.string.app_name), fontWeight = FontWeight.Bold)
                            Text(stringResource(com.termirust.mobile.R.string.controller_fleet), style = MaterialTheme.typography.labelMedium)
                        }
                    },
                    actions = {
                        if (activeTerminal != null) {
                            TextButton(onClick = viewModel::detachTerminal) {
                                Text(stringResource(com.termirust.mobile.R.string.detach))
                            }
                        } else {
                            TextButton(onClick = { showPairing = true }) { Text(stringResource(com.termirust.mobile.R.string.pair_host)) }
                        }
                        if (activeTerminal == null && state.selectedHostId != null) {
                            TextButton(onClick = { showHostDetails = true }) { Text(stringResource(com.termirust.mobile.R.string.details)) }
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = MaterialTheme.colorScheme.surface,
                    ),
                    modifier = Modifier.statusBarsPadding(),
                )
            },
        ) { padding ->
            BoxWithConstraints(
                Modifier
                    .fillMaxSize()
                    .padding(padding)
                    .navigationBarsPadding(),
            ) {
                if (activeTerminal != null) {
                    ControllerTerminalScreen(
                        terminal = activeTerminal,
                        onRetry = viewModel::retryTerminal,
                        onRequestControl = viewModel::requestTerminalControl,
                        onReleaseControl = viewModel::releaseTerminalControl,
                        onBytes = viewModel::sendTerminalBytes,
                        onPaste = viewModel::requestTerminalPaste,
                        onConfirmPaste = viewModel::confirmTerminalPaste,
                        onCancelPaste = viewModel::cancelTerminalPaste,
                        onViewportChanged = viewModel::updateTerminalViewport,
                    )
                } else if (state.hosts.isEmpty()) {
                    EmptyFleet(onPair = { showPairing = true })
                } else if (maxWidth >= 840.dp) {
                    Row(Modifier.fillMaxSize()) {
                        HostList(
                            state = state,
                            onSelect = viewModel::selectHost,
                            modifier = Modifier.width(340.dp).fillMaxHeight(),
                        )
                        VerticalDivider(modifier = Modifier.fillMaxHeight())
                        FleetDetail(
                            state,
                            viewModel::retry,
                            viewModel::attachSession,
                            onSelectRoute = { pendingRoute = it },
                            Modifier.weight(1f),
                        )
                    }
                } else {
                    Column(Modifier.fillMaxSize()) {
                        CompactHostStrip(state, viewModel::selectHost)
                        HorizontalDivider()
                        FleetDetail(
                            state,
                            viewModel::retry,
                            viewModel::attachSession,
                            onSelectRoute = { pendingRoute = it },
                            Modifier.weight(1f),
                        )
                    }
                }
            }
        }
    }

    if (showPairing) {
        PairHostDialog(
            viewModel = viewModel,
            connection = state.connection,
            onDismiss = {
                viewModel.cancelPairing()
                showPairing = false
            },
            onComplete = { showPairing = false },
            onScan = { showScanner = true },
        )
    }
    if (showScanner) {
        ControllerQrScannerDialog(
            onResult = { offer ->
                viewModel.pairingOffer.value = offer
                showScanner = false
                showPairing = true
            },
            onDismiss = { showScanner = false },
        )
    }
    if (showHostDetails) {
        val selected = state.hosts.firstOrNull { it.id == state.selectedHostId }
        if (selected != null) {
            HostDetailsDialog(
                host = selected,
                onDismiss = { showHostDetails = false },
                onReconnect = {
                    showHostDetails = false
                    viewModel.retry()
                },
                onForget = {
                    showHostDetails = false
                    confirmForget = true
                },
            )
        }
    }
    if (confirmForget) {
        AlertDialog(
            onDismissRequest = { confirmForget = false },
            title = { Text(stringResource(com.termirust.mobile.R.string.forget_host_title)) },
            text = { Text(stringResource(com.termirust.mobile.R.string.forget_host_explanation)) },
            confirmButton = {
                Button(onClick = {
                    confirmForget = false
                    viewModel.forgetSelectedHost()
                }) { Text(stringResource(com.termirust.mobile.R.string.forget_on_device)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmForget = false }) { Text(stringResource(com.termirust.mobile.R.string.cancel)) }
            },
        )
    }
    pendingRoute?.let { target ->
        AlertDialog(
            onDismissRequest = { pendingRoute = null },
            title = { Text(stringResource(com.termirust.mobile.R.string.switch_controller_route_title)) },
            text = {
                Text(
                    stringResource(
                        com.termirust.mobile.R.string.switch_controller_route_message,
                        controllerRouteTitle(target),
                    ),
                )
            },
            confirmButton = {
                Button(onClick = {
                    viewModel.selectControllerRoute(target, explicitlyConfirmed = true)
                    pendingRoute = null
                }) { Text(stringResource(com.termirust.mobile.R.string.switch_route)) }
            },
            dismissButton = {
                TextButton(onClick = { pendingRoute = null }) {
                    Text(stringResource(com.termirust.mobile.R.string.cancel))
                }
            },
        )
    }
}

@Composable
private fun EmptyFleet(onPair: () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier.padding(28.dp),
        ) {
            Text(stringResource(com.termirust.mobile.R.string.no_paired_hosts), style = MaterialTheme.typography.headlineSmall)
            Text(
                stringResource(com.termirust.mobile.R.string.pair_private_network_hint),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Button(onClick = onPair, modifier = Modifier.size(width = 160.dp, height = 48.dp)) {
                Text(stringResource(com.termirust.mobile.R.string.pair_host))
            }
        }
    }
}

@Composable
private fun HostList(
    state: ControllerUiState,
    onSelect: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    LazyColumn(modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item {
            Text(
                stringResource(com.termirust.mobile.R.string.paired_hosts),
                style = MaterialTheme.typography.titleSmall,
                modifier = Modifier.padding(horizontal = 8.dp, vertical = 6.dp),
            )
        }
        items(state.hosts, key = { it.id }) { host ->
            HostRow(host, host.id == state.selectedHostId) { onSelect(host.id) }
        }
    }
}

@Composable
private fun CompactHostStrip(state: ControllerUiState, onSelect: (String) -> Unit) {
    LazyRow(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items(state.hosts, key = { it.id }) { host ->
            CompactHostChip(host, host.id == state.selectedHostId) { onSelect(host.id) }
        }
    }
}

@Composable
private fun CompactHostChip(host: PairedHostRecord, selected: Boolean, onClick: () -> Unit) {
    val hostDescription = stringResource(com.termirust.mobile.R.string.host_accessibility, isolated(host.displayName))
    Card(
        modifier = Modifier
            .width(220.dp)
            .clickable(onClick = onClick)
            .semantics { contentDescription = hostDescription },
        colors = CardDefaults.cardColors(
            containerColor = if (selected) {
                MaterialTheme.colorScheme.secondaryContainer
            } else {
                MaterialTheme.colorScheme.surfaceVariant
            },
        ),
    ) {
        Column(Modifier.padding(horizontal = 14.dp, vertical = 10.dp)) {
            Text(
                isolated(host.displayName),
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                isolated("${host.route.address}:${host.route.port}"),
                style = MaterialTheme.typography.labelSmall,
                fontFamily = FontFamily.Monospace,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun HostRow(host: PairedHostRecord, selected: Boolean, onClick: () -> Unit) {
    val hostDescription = stringResource(com.termirust.mobile.R.string.host_accessibility, isolated(host.displayName))
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .semantics { contentDescription = hostDescription },
        colors = CardDefaults.cardColors(
            containerColor = if (selected) {
                MaterialTheme.colorScheme.secondaryContainer
            } else {
                MaterialTheme.colorScheme.surfaceVariant
            },
        ),
    ) {
        Row(
            Modifier.fillMaxWidth().padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(
                Modifier.size(36.dp).background(
                    MaterialTheme.colorScheme.primary,
                    MaterialTheme.shapes.small,
                ),
                contentAlignment = Alignment.Center,
            ) { Text(">", color = MaterialTheme.colorScheme.onPrimary, fontWeight = FontWeight.Bold) }
            Column(Modifier.weight(1f)) {
                Text(isolated(host.displayName), fontWeight = FontWeight.SemiBold, maxLines = 2)
                Text(
                    isolated("${host.route.address}:${host.route.port}"),
                    style = MaterialTheme.typography.labelSmall,
                    fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (selected) AssistChip(onClick = onClick, label = { Text(stringResource(com.termirust.mobile.R.string.controller_selected)) })
        }
    }
}

@Composable
private fun FleetDetail(
    state: ControllerUiState,
    onRetry: () -> Unit,
    onOpenSession: (String) -> Unit,
    onSelectRoute: (ControllerRemoteRouteKind) -> Unit,
    modifier: Modifier = Modifier,
) {
    val openTerminals = state.sessions.filter(ControllerSessionSummary::isOpenTerminal)
    val previousSessions = state.sessions.filterNot(ControllerSessionSummary::isOpenTerminal)
    Column(modifier.fillMaxSize()) {
        ConnectionBanner(state, onRetry)
        ControllerRouteSelector(state, onSelectRoute)
        LazyColumn(
            Modifier.fillMaxSize().padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (openTerminals.isEmpty()) {
                item(key = "no-open-terminals") {
                    Box(
                        Modifier.fillMaxWidth().heightIn(min = 140.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            if (state.connection.isBusy()) {
                                stringResource(com.termirust.mobile.R.string.loading_sessions)
                            } else {
                                stringResource(com.termirust.mobile.R.string.no_open_terminals)
                            },
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            } else {
                item(key = "open-terminals-header") {
                    SessionSectionHeader(stringResource(com.termirust.mobile.R.string.open_terminals))
                }
                items(openTerminals, key = { it.id }) { session ->
                    SessionRow(session, state.cachedReadOnly) { onOpenSession(session.id) }
                }
            }
            if (previousSessions.isNotEmpty()) {
                item(key = "previous-sessions-header") {
                    SessionSectionHeader(stringResource(com.termirust.mobile.R.string.previous_sessions))
                }
                items(previousSessions, key = { it.id }) { session ->
                    SessionRow(session, state.cachedReadOnly) { onOpenSession(session.id) }
                }
            }
        }
    }
}

@Composable
private fun SessionSectionHeader(title: String) {
    Text(
        title,
        style = MaterialTheme.typography.titleSmall,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(horizontal = 4.dp, vertical = 6.dp),
    )
}

@Composable
private fun ControllerRouteSelector(
    state: ControllerUiState,
    onSelect: (ControllerRemoteRouteKind) -> Unit,
) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(
                    stringResource(com.termirust.mobile.R.string.controller_route_title),
                    style = MaterialTheme.typography.titleSmall,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    stringResource(com.termirust.mobile.R.string.controller_route_subtitle),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        state.routeProjections.filter { it.selected || it.available }.forEach { projection ->
            ControllerRouteRow(projection, onSelect)
        }
        state.routeError?.let { error ->
            Text(
                controllerRouteError(error),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }
    }
    HorizontalDivider()
}

@Composable
private fun ControllerRouteRow(
    projection: AndroidControllerRouteProjection,
    onSelect: (ControllerRemoteRouteKind) -> Unit,
) {
    Surface(
        color = if (projection.selected) {
            MaterialTheme.colorScheme.secondaryContainer
        } else {
            MaterialTheme.colorScheme.surfaceVariant
        },
        shape = MaterialTheme.shapes.small,
    ) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Text(controllerRouteTitle(projection.route), fontWeight = FontWeight.SemiBold)
                Text(
                    controllerRouteDescription(projection.route),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    controllerRouteStatus(projection),
                    style = MaterialTheme.typography.labelSmall,
                    color = if (projection.available) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.error
                    },
                )
            }
            when {
                projection.selected -> AssistChip(
                    onClick = {},
                    label = { Text(stringResource(com.termirust.mobile.R.string.controller_selected)) },
                )
                projection.available -> OutlinedButton(onClick = { onSelect(projection.route) }) {
                    Text(stringResource(com.termirust.mobile.R.string.use_route))
                }
                else -> Text(
                    stringResource(com.termirust.mobile.R.string.not_configured),
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun controllerRouteTitle(route: ControllerRemoteRouteKind): String = when (route) {
    ControllerRemoteRouteKind.LOCAL_IPC -> stringResource(com.termirust.mobile.R.string.route_local_ipc)
    ControllerRemoteRouteKind.PRIVATE_NETWORK -> stringResource(com.termirust.mobile.R.string.route_private_network)
    ControllerRemoteRouteKind.SSH -> stringResource(com.termirust.mobile.R.string.route_ssh)
    ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> stringResource(com.termirust.mobile.R.string.route_self_hosted_relay)
}

@Composable
private fun controllerRouteDescription(route: ControllerRemoteRouteKind): String = when (route) {
    ControllerRemoteRouteKind.LOCAL_IPC -> stringResource(com.termirust.mobile.R.string.route_local_ipc_description)
    ControllerRemoteRouteKind.PRIVATE_NETWORK -> stringResource(com.termirust.mobile.R.string.route_private_network_description)
    ControllerRemoteRouteKind.SSH -> stringResource(com.termirust.mobile.R.string.route_ssh_description)
    ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> stringResource(com.termirust.mobile.R.string.route_relay_description)
}

@Composable
private fun controllerRouteStatus(projection: AndroidControllerRouteProjection): String = when {
    !projection.available -> stringResource(com.termirust.mobile.R.string.route_status_unavailable)
    projection.phase == ControllerRemoteRoutePhase.ONLINE -> stringResource(com.termirust.mobile.R.string.route_status_online)
    projection.phase == ControllerRemoteRoutePhase.DEGRADED -> stringResource(com.termirust.mobile.R.string.route_status_degraded)
    projection.phase == ControllerRemoteRoutePhase.RECONNECTING -> stringResource(com.termirust.mobile.R.string.route_status_reconnecting)
    projection.phase == ControllerRemoteRoutePhase.REVOKED -> stringResource(com.termirust.mobile.R.string.route_status_revoked)
    else -> stringResource(com.termirust.mobile.R.string.route_status_ready)
}

@Composable
private fun controllerRouteError(error: String): String = when (error) {
    "route_confirmation_required" -> stringResource(com.termirust.mobile.R.string.route_error_confirmation)
    "route_unavailable" -> stringResource(com.termirust.mobile.R.string.route_error_unavailable)
    "route_already_selected" -> stringResource(com.termirust.mobile.R.string.route_error_selected)
    "route_degraded" -> stringResource(com.termirust.mobile.R.string.route_error_degraded)
    else -> stringResource(com.termirust.mobile.R.string.route_error_generic)
}

@Composable
private fun ConnectionBanner(state: ControllerUiState, onRetry: () -> Unit) {
    Surface(color = MaterialTheme.colorScheme.surfaceVariant) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (state.connection.isBusy()) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            Column(Modifier.weight(1f)) {
                Text(connectionLabel(state.connection), fontWeight = FontWeight.SemiBold)
                if (state.cachedReadOnly) {
                    Text(
                        stringResource(com.termirust.mobile.R.string.cached_updated, relativeTime(state.cachedAtMillis)),
                        style = MaterialTheme.typography.labelSmall,
                    )
                }
            }
            if (state.connection is ControllerConnectionState.Failed || state.cachedReadOnly) {
                OutlinedButton(onClick = onRetry) { Text(stringResource(com.termirust.mobile.R.string.retry)) }
            }
        }
    }
}

@Composable
private fun SessionRow(session: ControllerSessionSummary, cached: Boolean, onOpen: () -> Unit) {
    val canOpen = !cached && session.occupantGeneration != null &&
        (session.capabilities.isEmpty() || ControllerSessionCapability.ATTACH_OUTPUT in session.capabilities)
    val description = stringResource(com.termirust.mobile.R.string.monitor_session, isolated(session.title))
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .then(if (canOpen) Modifier.clickable(onClick = onOpen) else Modifier)
            .semantics { if (canOpen) contentDescription = description },
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Column(
            Modifier.fillMaxWidth().padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    isolated(session.title),
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.weight(1f),
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                AssistChip(onClick = {}, label = { Text(if (cached) stringResource(com.termirust.mobile.R.string.cached) else session.lifecycle) })
            }
            val location = listOfNotNull(session.project, session.group).joinToString(" / ")
            if (location.isNotEmpty()) {
                Text(isolated(location), style = MaterialTheme.typography.bodySmall)
            }
            val origin = when (session.origin) {
                ControllerSessionOrigin.TERMINAL -> stringResource(com.termirust.mobile.R.string.session_origin_terminal)
                ControllerSessionOrigin.MANAGED_AGENT -> stringResource(com.termirust.mobile.R.string.session_origin_managed_agent)
                ControllerSessionOrigin.OBSERVED_AGENT -> stringResource(com.termirust.mobile.R.string.session_origin_observed_agent)
                ControllerSessionOrigin.UNKNOWN -> stringResource(com.termirust.mobile.R.string.session_origin_unknown)
            }
            val access = if (ControllerSessionCapability.SEND_INPUT in session.capabilities) {
                stringResource(com.termirust.mobile.R.string.session_control_available)
            } else {
                stringResource(com.termirust.mobile.R.string.view_only)
            }
            Text(
                listOfNotNull(origin, session.runtime, access).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(session.activity ?: stringResource(com.termirust.mobile.R.string.no_recent_activity), style = MaterialTheme.typography.labelMedium)
                if (session.unreadCount > 0) Text(stringResource(com.termirust.mobile.R.string.unread), color = MaterialTheme.colorScheme.primary)
                if (session.hasWriter) Text(stringResource(com.termirust.mobile.R.string.writer_active), color = MaterialTheme.colorScheme.tertiary)
            }
        }
    }
}

@Composable
private fun ControllerTerminalScreen(
    terminal: ControllerTerminalUiState,
    onRetry: () -> Unit,
    onRequestControl: () -> Unit,
    onReleaseControl: () -> Unit,
    onBytes: (ByteArray) -> Unit,
    onPaste: (String) -> Unit,
    onConfirmPaste: () -> Unit,
    onCancelPaste: () -> Unit,
    onViewportChanged: (Int, Int) -> Unit,
) {
    var followOutput by remember { mutableStateOf(true) }
    var focusRequest by remember { mutableStateOf(0L) }
    var controlModifier by remember { mutableStateOf(false) }
    var altModifier by remember { mutableStateOf(false) }
    var showLinks by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val density = LocalDensity.current
    val terminalPreferences = remember(context) {
        context.getSharedPreferences("controller_terminal", android.content.Context.MODE_PRIVATE)
    }
    var terminalFontSize by remember {
        mutableStateOf(
            terminalPreferences.getFloat("font_size", 14f).toDouble().coerceIn(
                TerminalAcceptance.MINIMUM_FONT_SIZE,
                TerminalAcceptance.MAXIMUM_FONT_SIZE,
            ),
        )
    }
    val clipboard = LocalClipboardManager.current
    val uriHandler = LocalUriHandler.current
    val listState = rememberLazyListState()
    val lines = terminal.screen.lines
    val cells = terminal.screen.contentCells
    val urls = remember(lines) { TerminalInteraction.visibleHttpUrls(lines.joinToString("\n")) }
    var terminalSurfaceSize by remember { mutableStateOf(IntSize.Zero) }
    val privacyAccessibility = stringResource(com.termirust.mobile.R.string.terminal_privacy_accessibility)
    val terminalOutputAccessibility = stringResource(
        com.termirust.mobile.R.string.terminal_output_accessibility,
        terminal.sessionTitle,
        TerminalAcceptance.accessibleOutput(lines),
    )
    val canInput = terminal.writerLease == WriterLeaseState.Held &&
        terminal.attachState == ReadOnlyAttachState.Live && !terminal.privacyCovered
    fun setTerminalFontSize(value: Double) {
        terminalFontSize = value.coerceIn(
            TerminalAcceptance.MINIMUM_FONT_SIZE,
            TerminalAcceptance.MAXIMUM_FONT_SIZE,
        )
        terminalPreferences.edit().putFloat("font_size", terminalFontSize.toFloat()).apply()
    }
    val submitKey: (TerminalInputKey) -> Unit = { key ->
        TerminalInteraction.encode(
            key,
            modifiers = TerminalInputModifiers(control = controlModifier, alt = altModifier),
            applicationCursor = terminal.screen.applicationCursor,
        )?.let(onBytes)
        controlModifier = false
        altModifier = false
    }
    LaunchedEffect(terminal.outputSequence, followOutput, lines.size) {
        if (followOutput && lines.isNotEmpty()) listState.scrollToItem(lines.lastIndex)
    }
    LaunchedEffect(terminalSurfaceSize, terminalFontSize, density.fontScale) {
        if (terminalSurfaceSize == IntSize.Zero) return@LaunchedEffect
        val layout = TerminalAcceptance.layout(
            width = with(density) { terminalSurfaceSize.width.toDp().value.toDouble() },
            height = with(density) { terminalSurfaceSize.height.toDp().value.toDouble() },
            requestedFontSize = terminalFontSize,
            textScale = density.fontScale.toDouble(),
        )
        onViewportChanged(layout.columns, layout.rows)
    }
    Column(Modifier.fillMaxSize().background(androidx.compose.ui.graphics.Color.Black)) {
        Surface(color = MaterialTheme.colorScheme.surfaceVariant) {
            BoxWithConstraints {
                val compactStatus = maxWidth < 600.dp || density.fontScale >= 1.6f
                Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp)) {
                    if (compactStatus) {
                        Row(
                            Modifier.fillMaxWidth(),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.SpaceBetween,
                        ) {
                            AssistChip(onClick = {}, label = { Text(stringResource(com.termirust.mobile.R.string.device_session)) })
                            Text(
                                stringResource(com.termirust.mobile.R.string.terminal_sequence, terminal.outputSequence),
                                style = MaterialTheme.typography.labelSmall,
                                fontFamily = FontFamily.Monospace,
                            )
                        }
                        AssistChip(onClick = {}, label = { Text(writerLabel(terminal)) })
                        Text(isolated(terminal.sessionTitle), fontWeight = FontWeight.SemiBold, maxLines = 1)
                        Text(
                            "${isolated(terminal.hostTitle)} · ${terminalStatus(terminal)}",
                            style = MaterialTheme.typography.labelSmall,
                        )
                    } else {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            AssistChip(onClick = {}, label = { Text(stringResource(com.termirust.mobile.R.string.device_session)) })
                            AssistChip(onClick = {}, label = { Text(writerLabel(terminal)) })
                            Column(Modifier.weight(1f)) {
                                Text(isolated(terminal.sessionTitle), fontWeight = FontWeight.SemiBold, maxLines = 1)
                                Text(
                                    "${isolated(terminal.hostTitle)} · ${terminalStatus(terminal)}",
                                    style = MaterialTheme.typography.labelSmall,
                                )
                            }
                            Text(
                                stringResource(com.termirust.mobile.R.string.terminal_sequence, terminal.outputSequence),
                                style = MaterialTheme.typography.labelSmall,
                                fontFamily = FontFamily.Monospace,
                            )
                        }
                    }
                    Row(
                        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        TextButton(onClick = { followOutput = !followOutput }) {
                            Text(
                                stringResource(
                                    if (followOutput) com.termirust.mobile.R.string.following_output
                                    else com.termirust.mobile.R.string.follow_output,
                                ),
                            )
                        }
                        if (urls.isNotEmpty()) {
                            Box {
                                TextButton(onClick = { showLinks = true }) {
                                    Text(stringResource(com.termirust.mobile.R.string.open_link))
                                }
                                DropdownMenu(
                                    expanded = showLinks,
                                    onDismissRequest = { showLinks = false },
                                ) {
                                    urls.forEach { url ->
                                        DropdownMenuItem(
                                            text = { Text(url, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                                            onClick = {
                                                showLinks = false
                                                uriHandler.openUri(url)
                                            },
                                        )
                                    }
                                }
                            }
                        }
                        TerminalKey(
                            "A-",
                            accessibilityLabel = stringResource(com.termirust.mobile.R.string.decrease_terminal_text),
                        ) { setTerminalFontSize(terminalFontSize - 1) }
                        TerminalKey(
                            "A+",
                            accessibilityLabel = stringResource(com.termirust.mobile.R.string.increase_terminal_text),
                        ) { setTerminalFontSize(terminalFontSize + 1) }
                        when {
                            terminal.writerLease == WriterLeaseState.Held ->
                                OutlinedButton(onClick = onReleaseControl) {
                                    Text(stringResource(com.termirust.mobile.R.string.release_control))
                                }
                            terminal.supportsWriter && terminal.attachState == ReadOnlyAttachState.Live &&
                                terminal.writerLease !is WriterLeaseState.Requesting ->
                                Button(onClick = onRequestControl) {
                                    Text(stringResource(com.termirust.mobile.R.string.request_control))
                                }
                        }
                        if (terminal.attachState is ReadOnlyAttachState.Offline ||
                            terminal.attachState is ReadOnlyAttachState.Gap ||
                            terminal.attachState is ReadOnlyAttachState.Failed
                        ) {
                            OutlinedButton(onClick = onRetry) {
                                Text(stringResource(com.termirust.mobile.R.string.retry))
                            }
                        }
                    }
                    if (terminal.screen.truncation != null) {
                        Text(
                            stringResource(com.termirust.mobile.R.string.terminal_truncated),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                    terminal.writerMessage?.let { code ->
                        Text(
                            stringResource(
                                com.termirust.mobile.R.string.terminal_control_warning,
                                writerMessage(code),
                            ),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                }
            }
        }
        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .onSizeChanged { terminalSurfaceSize = it },
        ) {
            if (terminal.privacyCovered) {
                Box(
                    Modifier
                        .fillMaxSize()
                        .background(MaterialTheme.colorScheme.background)
                        .clearAndSetSemantics {
                            contentDescription = privacyAccessibility
                        },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        stringResource(com.termirust.mobile.R.string.terminal_privacy_cover),
                        style = MaterialTheme.typography.titleMedium,
                        modifier = Modifier.padding(24.dp),
                    )
                }
            } else if (lines.all(String::isEmpty)) {
                Text(
                    terminalEmptyText(terminal.attachState),
                    color = androidx.compose.ui.graphics.Color.White.copy(alpha = 0.62f),
                    modifier = Modifier.padding(16.dp),
                )
            } else {
                Box(
                    Modifier.fillMaxSize().clearAndSetSemantics {
                        contentDescription = terminalOutputAccessibility
                    },
                ) {
                    SelectionContainer {
                        LazyColumn(
                            state = listState,
                            modifier = Modifier.fillMaxSize().padding(12.dp),
                        ) {
                            items(cells.size) { index ->
                                Text(
                                    styledTerminalRow(cells[index]),
                                    fontFamily = FontFamily.Monospace,
                                    fontSize = terminalFontSize.sp,
                                    maxLines = 1,
                                    softWrap = false,
                                    modifier = Modifier.heightIn(
                                        min = (terminalFontSize * density.fontScale * 1.35).dp,
                                    ),
                                )
                            }
                        }
                    }
                }
            }
            if (!terminal.privacyCovered) {
                ControllerTerminalInputView(
                    enabled = canInput,
                    focusRequest = focusRequest,
                    applicationCursor = terminal.screen.applicationCursor,
                    onBytes = onBytes,
                    modifier = Modifier.size(2.dp),
                )
            }
        }
        if (canInput) {
            Surface(color = MaterialTheme.colorScheme.surfaceVariant) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .horizontalScroll(rememberScrollState())
                        .padding(horizontal = 8.dp, vertical = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    TerminalKey("Esc") { submitKey(TerminalInputKey.ESCAPE) }
                    TerminalKey("Ctrl", selected = controlModifier) { controlModifier = !controlModifier }
                    TerminalKey("Alt", selected = altModifier) { altModifier = !altModifier }
                    TerminalKey("Tab") { submitKey(TerminalInputKey.TAB) }
                    TerminalKey("←") { submitKey(TerminalInputKey.LEFT) }
                    TerminalKey("↑") { submitKey(TerminalInputKey.UP) }
                    TerminalKey("↓") { submitKey(TerminalInputKey.DOWN) }
                    TerminalKey("→") { submitKey(TerminalInputKey.RIGHT) }
                    TextButton(onClick = { clipboard.getText()?.text?.let(onPaste) }) {
                        Text(stringResource(com.termirust.mobile.R.string.paste))
                    }
                    Button(onClick = { focusRequest += 1 }) {
                        Text(stringResource(com.termirust.mobile.R.string.show_keyboard))
                    }
                }
            }
        }
    }
    if (terminal.pendingPasteBytes > 0) {
        AlertDialog(
            onDismissRequest = onCancelPaste,
            title = { Text(stringResource(com.termirust.mobile.R.string.paste_confirmation_title)) },
            text = {
                Text(stringResource(com.termirust.mobile.R.string.paste_confirmation_message, terminal.pendingPasteBytes))
            },
            confirmButton = {
                Button(onClick = onConfirmPaste) { Text(stringResource(com.termirust.mobile.R.string.send_paste)) }
            },
            dismissButton = {
                TextButton(onClick = onCancelPaste) { Text(stringResource(com.termirust.mobile.R.string.cancel)) }
            },
        )
    }
}

private fun styledTerminalRow(cells: List<BoundedTerminalCell>): AnnotatedString =
    buildAnnotatedString {
        for (cell in cells) {
            if (cell.width == TerminalCellWidth.CONTINUATION) continue
            val colors = resolvedTerminalColors(cell.style)
            withStyle(
                SpanStyle(
                    color = colors.first,
                    background = colors.second,
                    fontWeight = if (cell.style.bold) FontWeight.Bold else FontWeight.Normal,
                    fontStyle = if (cell.style.italic) FontStyle.Italic else FontStyle.Normal,
                    textDecoration = if (cell.style.underline) {
                        TextDecoration.Underline
                    } else {
                        TextDecoration.None
                    },
                ),
            ) {
                append(cell.text)
            }
        }
    }

private fun resolvedTerminalColors(
    style: TerminalCellStyle,
): Pair<androidx.compose.ui.graphics.Color, androidx.compose.ui.graphics.Color> {
    val foreground = terminalColor(
        style.foreground,
        androidx.compose.ui.graphics.Color(0xffe6e6e6),
    )
    val background = terminalColor(style.background, androidx.compose.ui.graphics.Color.Black)
    val resolvedForeground = if (style.inverse) background else foreground
    val resolvedBackground = if (style.inverse) foreground else background
    return (if (style.dim) resolvedForeground.copy(alpha = 0.55f) else resolvedForeground) to
        resolvedBackground
}

private fun terminalColor(
    color: TerminalCellColor,
    fallback: androidx.compose.ui.graphics.Color,
): androidx.compose.ui.graphics.Color = when (color) {
    TerminalCellColor.Default -> fallback
    is TerminalCellColor.Indexed -> ansiColor(color.value)
    is TerminalCellColor.Rgb -> androidx.compose.ui.graphics.Color(
        red = color.red,
        green = color.green,
        blue = color.blue,
    )
}

private fun ansiColor(index: Int): androidx.compose.ui.graphics.Color {
    val base = listOf(
        0x000000, 0xcc0000, 0x00cc00, 0xcccc00,
        0x0000cc, 0xcc00cc, 0x00cccc, 0xbfbfbf,
        0x808080, 0xff0000, 0x00ff00, 0xffff00,
        0x5959ff, 0xff00ff, 0x00ffff, 0xffffff,
    )
    if (index in base.indices) {
        return androidx.compose.ui.graphics.Color(0xff000000 or base[index].toLong())
    }
    if (index in 16..231) {
        val cube = index - 16
        val levels = listOf(0, 95, 135, 175, 215, 255)
        return androidx.compose.ui.graphics.Color(
            red = levels[cube / 36],
            green = levels[(cube / 6) % 6],
            blue = levels[cube % 6],
        )
    }
    val gray = 8 + (index - 232).coerceIn(0, 23) * 10
    return androidx.compose.ui.graphics.Color(red = gray, green = gray, blue = gray)
}

@Composable
private fun TerminalKey(
    label: String,
    selected: Boolean = false,
    accessibilityLabel: String = label,
    onClick: () -> Unit,
) {
    val modifier = Modifier
        .size(width = 48.dp, height = 48.dp)
        .semantics { contentDescription = accessibilityLabel }
    if (selected) {
        Button(
            onClick = onClick,
            modifier = modifier,
            contentPadding = PaddingValues(0.dp),
        ) { Text(label) }
    } else {
        OutlinedButton(
            onClick = onClick,
            modifier = modifier,
            contentPadding = PaddingValues(0.dp),
        ) { Text(label) }
    }
}

@Composable
private fun writerLabel(terminal: ControllerTerminalUiState): String = when (terminal.writerLease) {
    WriterLeaseState.None -> if (terminal.hasWriterElsewhere) {
        stringResource(com.termirust.mobile.R.string.controlled_elsewhere)
    } else stringResource(com.termirust.mobile.R.string.view_only)
    is WriterLeaseState.Requesting -> stringResource(com.termirust.mobile.R.string.requesting_control)
    WriterLeaseState.Held -> stringResource(com.termirust.mobile.R.string.you_control)
    WriterLeaseState.Busy -> stringResource(com.termirust.mobile.R.string.controlled_elsewhere)
    WriterLeaseState.Lost -> stringResource(com.termirust.mobile.R.string.control_lost)
}

@Composable
private fun writerMessage(code: String): String = stringResource(
    when (code) {
        "control_unavailable" -> com.termirust.mobile.R.string.writer_error_control_unavailable
        "controlled_elsewhere" -> com.termirust.mobile.R.string.writer_error_controlled_elsewhere
        "input_pressure" -> com.termirust.mobile.R.string.writer_error_input_pressure
        "paste_too_large" -> com.termirust.mobile.R.string.writer_error_paste_too_large
        "resize_rejected" -> com.termirust.mobile.R.string.writer_error_resize_rejected
        "completion_unknown" -> com.termirust.mobile.R.string.writer_error_unknown
        "connection_failed_no_replay" -> com.termirust.mobile.R.string.writer_error_connection
        else -> com.termirust.mobile.R.string.writer_error_rejected
    },
)

@Composable
private fun terminalStatus(terminal: ControllerTerminalUiState): String = when (val state = terminal.attachState) {
    ReadOnlyAttachState.Detached -> stringResource(com.termirust.mobile.R.string.terminal_detached)
    ReadOnlyAttachState.Authenticating -> stringResource(com.termirust.mobile.R.string.terminal_authenticating)
    ReadOnlyAttachState.Snapshot -> stringResource(com.termirust.mobile.R.string.terminal_snapshot)
    ReadOnlyAttachState.Replaying -> stringResource(com.termirust.mobile.R.string.terminal_replaying)
    ReadOnlyAttachState.Live -> stringResource(
        if (terminal.hasWriterElsewhere) com.termirust.mobile.R.string.terminal_live_writer
        else com.termirust.mobile.R.string.terminal_live,
    )
    is ReadOnlyAttachState.Gap -> stringResource(com.termirust.mobile.R.string.terminal_gap, state.expected, state.received)
    ReadOnlyAttachState.Exited -> stringResource(com.termirust.mobile.R.string.terminal_exited)
    ReadOnlyAttachState.Offline -> stringResource(com.termirust.mobile.R.string.terminal_offline)
    is ReadOnlyAttachState.Failed -> stringResource(com.termirust.mobile.R.string.terminal_failed)
}

@Composable
private fun terminalEmptyText(state: ReadOnlyAttachState): String = when (state) {
    ReadOnlyAttachState.Authenticating, ReadOnlyAttachState.Snapshot, ReadOnlyAttachState.Replaying ->
        stringResource(com.termirust.mobile.R.string.terminal_waiting)
    ReadOnlyAttachState.Live -> stringResource(com.termirust.mobile.R.string.terminal_no_visible_output)
    ReadOnlyAttachState.Offline -> stringResource(com.termirust.mobile.R.string.terminal_no_offline_screen)
    is ReadOnlyAttachState.Failed -> stringResource(com.termirust.mobile.R.string.terminal_render_failed)
    else -> stringResource(com.termirust.mobile.R.string.terminal_no_output)
}

@Composable
private fun PairHostDialog(
    viewModel: ControllerViewModel,
    connection: ControllerConnectionState,
    onDismiss: () -> Unit,
    onComplete: () -> Unit,
    onScan: () -> Unit,
) {
    val offer by viewModel.pairingOffer.collectAsState()
    val hostName by viewModel.pairingHostName.collectAsState()
    val deviceName by viewModel.pairingDeviceName.collectAsState()
    val sas = connection as? ControllerConnectionState.SasReady
    val spokenSas = sas?.sas?.toCharArray()?.joinToString(" ")
    val sasDescription = spokenSas?.let {
        stringResource(com.termirust.mobile.R.string.security_code_accessibility, it)
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (sas == null) stringResource(com.termirust.mobile.R.string.pair_host) else stringResource(com.termirust.mobile.R.string.compare_security_code)) },
        text = {
            if (sas == null) {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(stringResource(com.termirust.mobile.R.string.pairing_offer_hint))
                    OutlinedButton(onClick = onScan, modifier = Modifier.fillMaxWidth()) {
                        Text(stringResource(com.termirust.mobile.R.string.scan_qr_code))
                    }
                    OutlinedTextField(
                        value = offer,
                        onValueChange = { if (it.toByteArray().size <= 4 * 1_024) viewModel.pairingOffer.value = it },
                        label = { Text(stringResource(com.termirust.mobile.R.string.pairing_offer)) },
                        minLines = 4,
                        maxLines = 8,
                    )
                    OutlinedTextField(
                        value = hostName,
                        onValueChange = { viewModel.pairingHostName.value = it.take(256) },
                        label = { Text(stringResource(com.termirust.mobile.R.string.host_name)) },
                        singleLine = true,
                    )
                    OutlinedTextField(
                        value = deviceName,
                        onValueChange = { viewModel.pairingDeviceName.value = it.take(64) },
                        label = { Text(stringResource(com.termirust.mobile.R.string.this_device)) },
                        singleLine = true,
                    )
                    if (connection is ControllerConnectionState.Failed) {
                        Text(stringResource(com.termirust.mobile.R.string.pairing_failed, connection.code), color = MaterialTheme.colorScheme.error)
                    }
                }
            } else {
                Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                    Text(stringResource(com.termirust.mobile.R.string.confirm_security_code))
                    Text(
                        sas.sas,
                        style = MaterialTheme.typography.headlineMedium,
                        fontFamily = FontFamily.Monospace,
                        fontWeight = FontWeight.Bold,
                        modifier = Modifier.semantics {
                            contentDescription = requireNotNull(sasDescription)
                        },
                    )
                    Text(stringResource(com.termirust.mobile.R.string.fingerprint_ending, sas.fingerprintSuffix))
                }
            }
        },
        confirmButton = {
            if (sas == null) {
                Button(
                    onClick = viewModel::beginPairing,
                    enabled = offer.isNotBlank() && connection !is ControllerConnectionState.Pairing,
                ) { Text(stringResource(com.termirust.mobile.R.string.continue_action)) }
            } else {
                Button(onClick = {
                    viewModel.finishPairing(true)
                    onComplete()
                }) { Text(stringResource(com.termirust.mobile.R.string.codes_match)) }
            }
        },
        dismissButton = {
            TextButton(onClick = {
                if (sas != null) viewModel.finishPairing(false)
                onDismiss()
            }) { Text(if (sas == null) stringResource(com.termirust.mobile.R.string.cancel) else stringResource(com.termirust.mobile.R.string.reject)) }
        },
    )
}

@Composable
private fun HostDetailsDialog(
    host: PairedHostRecord,
    onDismiss: () -> Unit,
    onReconnect: () -> Unit,
    onForget: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(isolated(host.displayName)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(com.termirust.mobile.R.string.route_value, isolated(host.route.address), host.route.port))
                Text(stringResource(com.termirust.mobile.R.string.fingerprint_ending, host.id.takeLast(12)), fontFamily = FontFamily.Monospace)
                Text(stringResource(com.termirust.mobile.R.string.capabilities_value, capabilityLabels(host.capabilityBits).joinToString()))
                Text(stringResource(com.termirust.mobile.R.string.forget_not_revoke))
            }
        },
        confirmButton = { Button(onClick = onReconnect) { Text(stringResource(com.termirust.mobile.R.string.reconnect)) } },
        dismissButton = {
            Row {
                TextButton(onClick = onForget) { Text(stringResource(com.termirust.mobile.R.string.forget)) }
                TextButton(onClick = onDismiss) { Text(stringResource(com.termirust.mobile.R.string.close)) }
            }
        },
    )
}

private fun ControllerConnectionState.isBusy(): Boolean =
    this == ControllerConnectionState.Connecting ||
        this == ControllerConnectionState.Authenticating ||
        this == ControllerConnectionState.Syncing ||
        this == ControllerConnectionState.Pairing

@Composable
private fun connectionLabel(state: ControllerConnectionState): String = when (state) {
    ControllerConnectionState.Unpaired -> stringResource(com.termirust.mobile.R.string.state_not_paired)
    ControllerConnectionState.Pairing -> stringResource(com.termirust.mobile.R.string.state_pairing)
    is ControllerConnectionState.SasReady -> stringResource(com.termirust.mobile.R.string.state_waiting_code)
    ControllerConnectionState.PairedOffline -> stringResource(com.termirust.mobile.R.string.state_host_offline)
    ControllerConnectionState.Connecting -> stringResource(com.termirust.mobile.R.string.state_connecting)
    ControllerConnectionState.Authenticating -> stringResource(com.termirust.mobile.R.string.state_authenticating)
    ControllerConnectionState.Syncing -> stringResource(com.termirust.mobile.R.string.state_syncing)
    ControllerConnectionState.ReadyReadOnly -> stringResource(com.termirust.mobile.R.string.state_live_read_only)
    ControllerConnectionState.Revoked -> stringResource(com.termirust.mobile.R.string.state_device_revoked)
    ControllerConnectionState.Incompatible -> stringResource(com.termirust.mobile.R.string.state_incompatible)
    is ControllerConnectionState.Failed -> stringResource(com.termirust.mobile.R.string.state_connection_failed, state.code)
}

private fun capabilityLabels(bits: Int): List<String> = buildList {
    if (bits and 1 != 0) add("Fleet")
    if (bits and 2 != 0) add("Output")
    if (bits and 4 != 0) add("Input")
    if (bits and 8 != 0) add("Resize")
    if (bits and 16 != 0) add("Approvals")
}

private fun isolated(value: String): String = "\u2068$value\u2069"

@Composable
private fun relativeTime(millis: Long?): String {
    if (millis == null) return stringResource(com.termirust.mobile.R.string.time_unknown)
    val seconds = ((System.currentTimeMillis() - millis).coerceAtLeast(0)) / 1_000
    return when {
        seconds < 60 -> stringResource(com.termirust.mobile.R.string.time_just_now)
        seconds < 3_600 -> stringResource(com.termirust.mobile.R.string.time_minutes, seconds / 60)
        seconds < 86_400 -> stringResource(com.termirust.mobile.R.string.time_hours, seconds / 3_600)
        else -> stringResource(com.termirust.mobile.R.string.time_days, seconds / 86_400)
    }
}
