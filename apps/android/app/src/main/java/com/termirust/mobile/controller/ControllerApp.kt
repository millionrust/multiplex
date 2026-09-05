package com.termirust.mobile.controller

import android.content.res.Configuration
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
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.text.KeyboardOptions
import androidx.activity.compose.BackHandler
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.KeyboardHide
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalConfiguration
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.roundToInt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ControllerApp(viewModel: ControllerViewModel, modifier: Modifier = Modifier) {
    val state by viewModel.state.collectAsState()
    var showPairing by remember { mutableStateOf(false) }
    var showScanner by remember { mutableStateOf(false) }
    var showHostDetails by remember { mutableStateOf(false) }
    var confirmForget by remember { mutableStateOf(false) }
    var showSshConfiguration by remember { mutableStateOf(false) }
    var showRelayConfiguration by remember { mutableStateOf(false) }
    var pendingRoute by remember { mutableStateOf<ControllerRemoteRouteKind?>(null) }
    val activeTerminal = state.activeTerminal
    val configuration = LocalConfiguration.current
    val windowDensity = LocalDensity.current
    val keyboardPresented = WindowInsets.ime.getBottom(windowDensity) > 0
    val focusedLandscapeTerminal = activeTerminal != null &&
        ControllerTerminalLayout.usesFocusedLandscape(
            configuration.orientation == Configuration.ORIENTATION_LANDSCAPE,
            keyboardPresented,
        )

    BackHandler(enabled = activeTerminal != null) { viewModel.detachTerminal() }

    MaterialTheme {
        Scaffold(
            modifier = modifier.fillMaxSize(),
            topBar = {
                if (!focusedLandscapeTerminal) TopAppBar(
                    title = {
                        if (activeTerminal != null) {
                            Text(
                                isolated(activeTerminal.sessionTitle),
                                fontWeight = FontWeight.SemiBold,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        } else Column {
                            Text(stringResource(com.termirust.mobile.R.string.app_name), fontWeight = FontWeight.Bold)
                            Text(stringResource(com.termirust.mobile.R.string.controller_fleet), style = MaterialTheme.typography.labelMedium)
                        }
                    },
                    actions = {
                        if (activeTerminal != null) {
                            IconButton(onClick = viewModel::detachTerminal) {
                                Icon(
                                    Icons.Outlined.Close,
                                    contentDescription = stringResource(com.termirust.mobile.R.string.detach),
                                )
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
                            onConfigureSsh = { showSshConfiguration = true },
                            onConfigureRelay = { showRelayConfiguration = true },
                            modifier = Modifier.weight(1f),
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
                            onConfigureSsh = { showSshConfiguration = true },
                            onConfigureRelay = { showRelayConfiguration = true },
                            modifier = Modifier.weight(1f),
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
    if (showSshConfiguration) {
        val selectedHost = state.hosts.firstOrNull { it.id == state.selectedHostId }
        SshControllerConfigurationDialog(
            configuration = viewModel.selectedSshConfiguration(),
            suggestedEndpoint = selectedHost?.route?.address.orEmpty(),
            onSave = { endpoint, port, username, pin, authentication, secret ->
                if (viewModel.configureSshRoute(
                        endpoint,
                        port,
                        username,
                        pin,
                        authentication,
                        secret,
                    )
                ) {
                    showSshConfiguration = false
                }
            },
            onRemove = {
                viewModel.removeSshRoute()
                showSshConfiguration = false
            },
            onDismiss = { showSshConfiguration = false },
        )
    }
    if (showRelayConfiguration) {
        RelayControllerConfigurationDialog(
            configuration = viewModel.selectedRelayConfiguration(),
            onSave = { endpoint, pin, routeId, epoch, credential ->
                if (viewModel.configureRelayRoute(endpoint, pin, routeId, epoch, credential)) {
                    showRelayConfiguration = false
                }
            },
            onRemove = {
                viewModel.removeRelayRoute()
                showRelayConfiguration = false
            },
            onDismiss = { showRelayConfiguration = false },
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
    onConfigureSsh: () -> Unit,
    onConfigureRelay: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val openTerminals = state.sessions.filter(ControllerSessionSummary::isOpenTerminal)
    val previousSessions = state.sessions.filterNot(ControllerSessionSummary::isOpenTerminal)
    Column(modifier.fillMaxSize()) {
        ConnectionBanner(state, onRetry)
        ControllerRouteSelector(state, onSelectRoute, onConfigureSsh, onConfigureRelay)
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
    onConfigureSsh: () -> Unit,
    onConfigureRelay: () -> Unit,
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
        state.routeProjections.filter { it.route != ControllerRemoteRouteKind.LOCAL_IPC }.forEach { projection ->
            ControllerRouteRow(projection, onSelect, onConfigureSsh, onConfigureRelay)
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
    onConfigureSsh: () -> Unit,
    onConfigureRelay: () -> Unit,
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
            Column(horizontalAlignment = Alignment.End) {
                when {
                    projection.selected -> AssistChip(
                        onClick = {},
                        label = { Text(stringResource(com.termirust.mobile.R.string.controller_selected)) },
                    )
                    projection.available -> OutlinedButton(onClick = { onSelect(projection.route) }) {
                        Text(stringResource(com.termirust.mobile.R.string.use_route))
                    }
                    projection.route == ControllerRemoteRouteKind.SSH ||
                        projection.route == ControllerRemoteRouteKind.SELF_HOSTED_RELAY ->
                        OutlinedButton(onClick = if (projection.route == ControllerRemoteRouteKind.SSH) onConfigureSsh else onConfigureRelay) {
                            Text(stringResource(com.termirust.mobile.R.string.configure))
                        }
                    else -> Text(
                        stringResource(com.termirust.mobile.R.string.not_configured),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (projection.route == ControllerRemoteRouteKind.SSH && projection.available) {
                    TextButton(onClick = onConfigureSsh) {
                        Text(stringResource(com.termirust.mobile.R.string.edit))
                    }
                }
                if (projection.route == ControllerRemoteRouteKind.SELF_HOSTED_RELAY && projection.available) {
                    TextButton(onClick = onConfigureRelay) {
                        Text(stringResource(com.termirust.mobile.R.string.edit))
                    }
                }
            }
        }
    }
}

@Composable
private fun RelayControllerConfigurationDialog(
    configuration: ControllerRemoteRouteConfiguration?,
    onSave: (String, String, String, Long, String) -> Unit,
    onRemove: () -> Unit,
    onDismiss: () -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    var endpoint by remember(configuration) { mutableStateOf(configuration?.endpoint.orEmpty()) }
    var pin by remember(configuration) { mutableStateOf(configuration?.trustPin.orEmpty()) }
    var routeId by remember(configuration) { mutableStateOf(configuration?.relayRouteId.orEmpty()) }
    var epoch by remember(configuration) { mutableStateOf(configuration?.relayRevocationEpoch?.toString() ?: "0") }
    var credential by remember { mutableStateOf("") }
    var packageError by remember { mutableStateOf<String?>(null) }
    var confirmRemove by remember { mutableStateOf(false) }
    val invalidPackageMessage = stringResource(com.termirust.mobile.R.string.invalid_relay_controller_package)
    val parsedEpoch = epoch.toLongOrNull()
    val canSave = endpoint.startsWith("wss://") && pin.startsWith("sha256/") &&
        routeId.isNotBlank() && parsedEpoch != null && parsedEpoch >= 0 && credential.isNotBlank()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(com.termirust.mobile.R.string.configure_relay_controller)) },
        text = {
            LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                item {
                    Text(
                        stringResource(com.termirust.mobile.R.string.relay_controller_configuration_help),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                item {
                    OutlinedButton(
                        onClick = {
                            runCatching {
                                ControllerRelayRoutePackage.decode(
                                    clipboard.getText()?.text
                                        ?: error("clipboard does not contain text"),
                                )
                            }.onSuccess { importedPackage ->
                                endpoint = importedPackage.endpoint
                                pin = importedPackage.spkiPin
                                routeId = importedPackage.relayRouteId
                                epoch = importedPackage.relayRevocationEpoch.toString()
                                credential = importedPackage.admissionCredential
                                packageError = null
                            }.onFailure { packageError = invalidPackageMessage }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text(stringResource(com.termirust.mobile.R.string.paste_relay_controller_package))
                    }
                }
                packageError?.let { message ->
                    item {
                        Text(
                            message,
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
                item { OutlinedTextField(endpoint, { endpoint = it }, label = { Text(stringResource(com.termirust.mobile.R.string.relay_endpoint)) }, singleLine = true) }
                item { OutlinedTextField(pin, { pin = it }, label = { Text(stringResource(com.termirust.mobile.R.string.relay_spki_pin)) }, singleLine = true) }
                item { OutlinedTextField(routeId, { routeId = it }, label = { Text(stringResource(com.termirust.mobile.R.string.relay_route_id)) }, singleLine = true) }
                item {
                    OutlinedTextField(
                        epoch,
                        { epoch = it.filter(Char::isDigit) },
                        label = { Text(stringResource(com.termirust.mobile.R.string.relay_epoch)) },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        credential,
                        { credential = it },
                        label = { Text(stringResource(com.termirust.mobile.R.string.relay_admission_credential)) },
                        visualTransformation = PasswordVisualTransformation(),
                        singleLine = true,
                    )
                }
                if (configuration != null) {
                    item {
                        TextButton(onClick = { confirmRemove = true }) {
                            Text(stringResource(com.termirust.mobile.R.string.remove_relay_controller_route), color = MaterialTheme.colorScheme.error)
                        }
                    }
                }
            }
        },
        confirmButton = {
            Button(enabled = canSave, onClick = { onSave(endpoint, pin, routeId, checkNotNull(parsedEpoch), credential) }) {
                Text(stringResource(com.termirust.mobile.R.string.save))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(com.termirust.mobile.R.string.cancel)) } },
    )
    if (confirmRemove) {
        AlertDialog(
            onDismissRequest = { confirmRemove = false },
            title = { Text(stringResource(com.termirust.mobile.R.string.remove_relay_controller_route)) },
            text = { Text(stringResource(com.termirust.mobile.R.string.remove_relay_controller_route_help)) },
            confirmButton = { TextButton(onClick = onRemove) { Text(stringResource(com.termirust.mobile.R.string.remove), color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { confirmRemove = false }) { Text(stringResource(com.termirust.mobile.R.string.cancel)) } },
        )
    }
}

@Composable
private fun SshControllerConfigurationDialog(
    configuration: ControllerRemoteRouteConfiguration?,
    suggestedEndpoint: String,
    onSave: (String, Int, String, String, ControllerSshAuthenticationKind, String) -> Unit,
    onRemove: () -> Unit,
    onDismiss: () -> Unit,
) {
    var endpoint by remember(configuration, suggestedEndpoint) {
        mutableStateOf(configuration?.endpoint ?: suggestedEndpoint)
    }
    var port by remember(configuration) { mutableStateOf(configuration?.port?.toString() ?: "22") }
    var username by remember(configuration) { mutableStateOf(configuration?.username.orEmpty()) }
    var pin by remember(configuration) { mutableStateOf(configuration?.trustPin.orEmpty()) }
    var authentication by remember(configuration) {
        mutableStateOf(configuration?.sshAuthentication ?: ControllerSshAuthenticationKind.PRIVATE_KEY)
    }
    var secret by remember { mutableStateOf("") }
    var confirmRemove by remember { mutableStateOf(false) }
    val parsedPort = port.toIntOrNull()
    val canSave = endpoint.isNotBlank() && parsedPort in 1..65_535 && username.isNotBlank() &&
        pin.isNotBlank() && secret.isNotEmpty()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(com.termirust.mobile.R.string.configure_ssh_controller)) },
        text = {
            LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                item {
                    Text(
                        stringResource(com.termirust.mobile.R.string.ssh_controller_configuration_help),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                item {
                    OutlinedTextField(
                        value = endpoint,
                        onValueChange = { endpoint = it },
                        label = { Text(stringResource(com.termirust.mobile.R.string.ssh_host)) },
                        singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        value = port,
                        onValueChange = { port = it.filter(Char::isDigit).take(5) },
                        label = { Text(stringResource(com.termirust.mobile.R.string.ssh_port)) },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        value = username,
                        onValueChange = { username = it },
                        label = { Text(stringResource(com.termirust.mobile.R.string.ssh_username)) },
                        singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        value = pin,
                        onValueChange = { pin = it },
                        label = { Text(stringResource(com.termirust.mobile.R.string.ssh_host_key_pin)) },
                        singleLine = true,
                    )
                }
                item {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilterChip(
                            selected = authentication == ControllerSshAuthenticationKind.PRIVATE_KEY,
                            onClick = { authentication = ControllerSshAuthenticationKind.PRIVATE_KEY },
                            label = { Text(stringResource(com.termirust.mobile.R.string.private_key)) },
                        )
                        FilterChip(
                            selected = authentication == ControllerSshAuthenticationKind.PASSWORD,
                            onClick = { authentication = ControllerSshAuthenticationKind.PASSWORD },
                            label = { Text(stringResource(com.termirust.mobile.R.string.password)) },
                        )
                    }
                }
                item {
                    OutlinedTextField(
                        value = secret,
                        onValueChange = { secret = it },
                        label = {
                            Text(
                                if (authentication == ControllerSshAuthenticationKind.PASSWORD) {
                                    stringResource(com.termirust.mobile.R.string.password)
                                } else {
                                    stringResource(com.termirust.mobile.R.string.openssh_private_key)
                                },
                            )
                        },
                        visualTransformation = PasswordVisualTransformation(),
                        minLines = if (authentication == ControllerSshAuthenticationKind.PRIVATE_KEY) 3 else 1,
                        maxLines = 6,
                    )
                }
                if (configuration != null) {
                    item {
                        TextButton(onClick = { confirmRemove = true }) {
                            Text(
                                stringResource(com.termirust.mobile.R.string.remove_ssh_controller_route),
                                color = MaterialTheme.colorScheme.error,
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            Button(
                enabled = canSave,
                onClick = {
                    onSave(endpoint, checkNotNull(parsedPort), username, pin, authentication, secret)
                },
            ) { Text(stringResource(com.termirust.mobile.R.string.save)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(com.termirust.mobile.R.string.cancel)) }
        },
    )
    if (confirmRemove) {
        AlertDialog(
            onDismissRequest = { confirmRemove = false },
            title = { Text(stringResource(com.termirust.mobile.R.string.remove_ssh_controller_route)) },
            text = { Text(stringResource(com.termirust.mobile.R.string.remove_ssh_controller_route_help)) },
            confirmButton = {
                TextButton(onClick = onRemove) {
                    Text(
                        stringResource(com.termirust.mobile.R.string.remove),
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmRemove = false }) {
                    Text(stringResource(com.termirust.mobile.R.string.cancel))
                }
            },
        )
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
    !projection.available -> stringResource(com.termirust.mobile.R.string.not_configured)
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
    "route_configuration_invalid" -> stringResource(com.termirust.mobile.R.string.route_error_configuration_invalid)
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
    var keyboardRequest by remember { mutableLongStateOf(0L) }
    var showKeyboard by remember { mutableStateOf(false) }
    var controlModifier by remember { mutableStateOf(false) }
    var altModifier by remember { mutableStateOf(false) }
    var showOptions by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val density = LocalDensity.current
    val terminalPreferences = remember(context) {
        context.getSharedPreferences("controller_terminal", android.content.Context.MODE_PRIVATE)
    }
    var terminalFontSize by remember {
        mutableDoubleStateOf(
            terminalPreferences.getFloat("font_size", 14f).toDouble().coerceIn(
                TerminalAcceptance.MINIMUM_FONT_SIZE,
                TerminalAcceptance.MAXIMUM_FONT_SIZE,
            ),
        )
    }
    var usesDesktopWidth by remember {
        mutableStateOf(terminalPreferences.getBoolean("desktop_width", false))
    }
    var displayedFontSize by remember { mutableDoubleStateOf(terminalFontSize) }
    var displayedColumns by remember { mutableIntStateOf(40) }
    var displayedRows by remember { mutableIntStateOf(24) }
    val clipboard = LocalClipboardManager.current
    val uriHandler = LocalUriHandler.current
    val listState = rememberLazyListState()
    val horizontalState = rememberScrollState()
    val configuration = LocalConfiguration.current
    val keyboardPresented = WindowInsets.ime.getBottom(density) > 0
    val focusedLandscape = ControllerTerminalLayout.usesFocusedLandscape(
        configuration.orientation == Configuration.ORIENTATION_LANDSCAPE,
        keyboardPresented,
    )
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
    val statusColor = when (terminal.attachState) {
        ReadOnlyAttachState.Live -> androidx.compose.ui.graphics.Color(0xff22c55e)
        is ReadOnlyAttachState.Gap, is ReadOnlyAttachState.Failed ->
            androidx.compose.ui.graphics.Color(0xfff59e0b)
        ReadOnlyAttachState.Offline, ReadOnlyAttachState.Exited ->
            MaterialTheme.colorScheme.onSurfaceVariant
        else -> MaterialTheme.colorScheme.primary
    }
    fun setTerminalFontSize(value: Double) {
        terminalFontSize = value.coerceIn(
            TerminalAcceptance.MINIMUM_FONT_SIZE,
            TerminalAcceptance.MAXIMUM_FONT_SIZE,
        )
        terminalPreferences.edit().putFloat("font_size", terminalFontSize.toFloat()).apply()
    }
    fun setDesktopWidth(enabled: Boolean) {
        usesDesktopWidth = enabled
        terminalPreferences.edit().putBoolean("desktop_width", enabled).apply()
    }
    fun setKeyboardPresented(presented: Boolean) {
        showKeyboard = presented
        keyboardRequest += 1
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
    LaunchedEffect(
        terminal.outputSequence,
        followOutput,
        lines,
        terminal.screen.cursorRow,
        terminal.screen.scrollbackRows,
        displayedRows,
    ) {
        val target = ControllerTerminalFollowTarget.row(
            lines,
            terminal.screen.cursorRow,
            terminal.screen.scrollbackRows,
        )
        if (followOutput && target != null) {
            listState.scrollToItem(
                ControllerTerminalFollowTarget.firstVisibleRow(target, displayedRows),
            )
        }
    }
    LaunchedEffect(terminalSurfaceSize, terminalFontSize, density.fontScale, usesDesktopWidth) {
        if (terminalSurfaceSize == IntSize.Zero) return@LaunchedEffect
        val layout = TerminalAcceptance.layout(
            width = with(density) { terminalSurfaceSize.width.toDp().value.toDouble() },
            height = with(density) { terminalSurfaceSize.height.toDp().value.toDouble() },
            requestedFontSize = terminalFontSize,
            textScale = density.fontScale.toDouble(),
        )
        displayedFontSize = layout.displayedFontSize
        displayedColumns = ControllerTerminalWidth.columns(layout.columns, usesDesktopWidth)
        displayedRows = layout.rows
        onViewportChanged(displayedColumns, displayedRows)
    }
    LaunchedEffect(
        terminal.outputSequence,
        followOutput,
        usesDesktopWidth,
        terminal.screen.cursorColumn,
        terminal.screen.cursorVisible,
        displayedFontSize,
        terminalSurfaceSize,
    ) {
        if (!followOutput || !usesDesktopWidth || !terminal.screen.cursorVisible ||
            terminalSurfaceSize == IntSize.Zero
        ) return@LaunchedEffect
        val cellWidth = with(density) { (displayedFontSize * 0.62).dp.toPx() }
        val padding = with(density) { 12.dp.toPx() }
        val cursorStart = padding + terminal.screen.cursorColumn * cellWidth
        val cursorEnd = cursorStart + cellWidth
        val visibleStart = horizontalState.value.toFloat()
        val visibleEnd = visibleStart + terminalSurfaceSize.width
        val next = when {
            cursorStart < visibleStart + padding -> cursorStart - padding
            cursorEnd > visibleEnd - padding -> cursorEnd - terminalSurfaceSize.width + padding
            else -> null
        }
        if (next != null) {
            horizontalState.scrollTo(next.roundToInt().coerceIn(0, horizontalState.maxValue))
        }
    }
    Column(Modifier.fillMaxSize().background(androidx.compose.ui.graphics.Color.Black)) {
        if (!focusedLandscape) Surface(color = MaterialTheme.colorScheme.surfaceVariant) {
            BoxWithConstraints {
                val compactStatus = maxWidth < 600.dp || density.fontScale >= 1.6f
                Column(Modifier.fillMaxWidth()) {
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(start = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(7.dp),
                    ) {
                        Box(
                            Modifier
                                .size(7.dp)
                                .background(statusColor, CircleShape),
                        )
                        Text(
                            isolated(terminal.hostTitle),
                            style = MaterialTheme.typography.labelLarge,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        if (!compactStatus) {
                            Text(
                                terminalStatus(terminal),
                                style = MaterialTheme.typography.labelSmall,
                                color = statusColor,
                                maxLines = 1,
                            )
                        }
                        Text(
                            writerLabel(terminal),
                            style = MaterialTheme.typography.labelSmall,
                            color = if (terminal.writerLease == WriterLeaseState.Held) {
                                androidx.compose.ui.graphics.Color(0xff22c55e)
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                            maxLines = 1,
                        )
                        when {
                            terminal.writerLease == WriterLeaseState.Held ->
                                TextButton(onClick = onReleaseControl) {
                                    Text(stringResource(com.termirust.mobile.R.string.release_control))
                                }
                            terminal.supportsWriter && terminal.attachState == ReadOnlyAttachState.Live &&
                                terminal.writerLease !is WriterLeaseState.Requesting ->
                                TextButton(onClick = onRequestControl) {
                                    Text(stringResource(com.termirust.mobile.R.string.control))
                                }
                        }
                        if (terminal.attachState is ReadOnlyAttachState.Offline ||
                            terminal.attachState is ReadOnlyAttachState.Gap ||
                            terminal.attachState is ReadOnlyAttachState.Failed
                        ) {
                            TextButton(onClick = onRetry) {
                                Text(stringResource(com.termirust.mobile.R.string.retry))
                            }
                        }
                        Box {
                            IconButton(onClick = { showOptions = true }) {
                                Icon(
                                    Icons.Outlined.MoreVert,
                                    contentDescription = stringResource(com.termirust.mobile.R.string.terminal_options),
                                )
                            }
                            DropdownMenu(
                                expanded = showOptions,
                                onDismissRequest = { showOptions = false },
                            ) {
                                DropdownMenuItem(
                                    text = {
                                        Text(
                                            stringResource(
                                                if (followOutput) com.termirust.mobile.R.string.stop_following_output
                                                else com.termirust.mobile.R.string.follow_output,
                                            ),
                                        )
                                    },
                                    onClick = {
                                        followOutput = !followOutput
                                        showOptions = false
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(com.termirust.mobile.R.string.phone_width)) },
                                    onClick = {
                                        setDesktopWidth(false)
                                        showOptions = false
                                    },
                                    trailingIcon = {
                                        if (!usesDesktopWidth) Icon(Icons.Outlined.Check, contentDescription = null)
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(com.termirust.mobile.R.string.desktop_width)) },
                                    onClick = {
                                        setDesktopWidth(true)
                                        showOptions = false
                                    },
                                    trailingIcon = {
                                        if (usesDesktopWidth) Icon(Icons.Outlined.Check, contentDescription = null)
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(com.termirust.mobile.R.string.decrease_terminal_text)) },
                                    onClick = {
                                        setTerminalFontSize(terminalFontSize - 1)
                                        showOptions = false
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(com.termirust.mobile.R.string.increase_terminal_text)) },
                                    onClick = {
                                        setTerminalFontSize(terminalFontSize + 1)
                                        showOptions = false
                                    },
                                )
                                urls.forEach { url ->
                                    DropdownMenuItem(
                                        text = { Text(url, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                                        onClick = {
                                            showOptions = false
                                            uriHandler.openUri(url)
                                        },
                                    )
                                }
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
        BoxWithConstraints(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .onSizeChanged { terminalSurfaceSize = it },
        ) {
            val terminalContentWidth = if (usesDesktopWidth) {
                maxOf(maxWidth, (displayedColumns * displayedFontSize * 0.62 + 24).dp)
            } else {
                maxWidth
            }
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
                        Row(
                            Modifier
                                .fillMaxSize()
                                .horizontalScroll(horizontalState, enabled = usesDesktopWidth),
                        ) {
                            LazyColumn(
                                state = listState,
                                modifier = Modifier
                                    .width(terminalContentWidth)
                                    .fillMaxHeight()
                                    .padding(12.dp),
                            ) {
                                items(cells.size) { index ->
                                    val cursorColumn = ControllerTerminalCursor.column(
                                        rowIndex = index,
                                        cells = cells[index],
                                        cursorRow = terminal.screen.cursorRow,
                                        cursorColumn = terminal.screen.cursorColumn,
                                        scrollbackRows = terminal.screen.scrollbackRows,
                                        visible = terminal.screen.cursorVisible,
                                    )
                                    Text(
                                        styledTerminalRow(cells[index], cursorColumn),
                                        fontFamily = FontFamily.Monospace,
                                        fontSize = terminalFontSize.sp,
                                        maxLines = 1,
                                        softWrap = false,
                                        modifier = Modifier.heightIn(
                                            min = (displayedFontSize * 1.35).dp,
                                        ),
                                    )
                                }
                            }
                        }
                    }
                }
            }
            if (!terminal.privacyCovered) {
                ControllerTerminalInputView(
                    enabled = canInput,
                    keyboardRequest = keyboardRequest,
                    showKeyboard = showKeyboard,
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
                    if (focusedLandscape) {
                        Text(
                            stringResource(com.termirust.mobile.R.string.you_control),
                            color = androidx.compose.ui.graphics.Color(0xff22c55e),
                            fontWeight = FontWeight.SemiBold,
                        )
                        TextButton(onClick = onReleaseControl) {
                            Text(stringResource(com.termirust.mobile.R.string.release_control))
                        }
                    }
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
                    Button(onClick = { setKeyboardPresented(!keyboardPresented) }) {
                        Icon(
                            if (keyboardPresented) Icons.Outlined.KeyboardHide else Icons.Outlined.Keyboard,
                            contentDescription = null,
                        )
                        Spacer(Modifier.width(6.dp))
                        Text(
                            stringResource(
                                if (keyboardPresented) com.termirust.mobile.R.string.hide_keyboard
                                else com.termirust.mobile.R.string.show_keyboard,
                            ),
                        )
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

internal fun styledTerminalRow(
    cells: List<BoundedTerminalCell>,
    cursorColumn: Int?,
): AnnotatedString =
    buildAnnotatedString {
        val displayCells = cells.toMutableList()
        if (cursorColumn != null) {
            while (displayCells.size <= cursorColumn) displayCells += BoundedTerminalCell.blank()
        }
        for ((column, cell) in displayCells.withIndex()) {
            if (cell.width == TerminalCellWidth.CONTINUATION) continue
            val colors = resolvedTerminalColors(cell.style)
            withStyle(
                SpanStyle(
                    color = if (column == cursorColumn) {
                        androidx.compose.ui.graphics.Color.Black
                    } else {
                        colors.first
                    },
                    background = if (column == cursorColumn) {
                        androidx.compose.ui.graphics.Color(0xff22c55e)
                    } else {
                        colors.second
                    },
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

private enum class PairingOfferPasteError { Empty, TooLarge }

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
    val clipboard = LocalClipboardManager.current
    val liveSas = connection as? ControllerConnectionState.SasReady
    var retainedSas by remember { mutableStateOf<ControllerConnectionState.SasReady?>(null) }
    var awaitingCompletion by remember { mutableStateOf(false) }
    var pairingOfferPasteError by remember { mutableStateOf<PairingOfferPasteError?>(null) }
    LaunchedEffect(liveSas) {
        if (liveSas != null) retainedSas = liveSas
    }
    LaunchedEffect(connection, awaitingCompletion) {
        if (!awaitingCompletion || connection is ControllerConnectionState.Pairing ||
            connection is ControllerConnectionState.SasReady
        ) return@LaunchedEffect
        awaitingCompletion = false
        retainedSas = null
        if (connection !is ControllerConnectionState.Failed) onComplete()
    }
    val sas = liveSas ?: retainedSas?.takeIf { awaitingCompletion }
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
                    if (connection is ControllerConnectionState.Failed && offer.isBlank()) {
                        Text(
                            stringResource(com.termirust.mobile.R.string.pairing_new_offer_required),
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                    Text(stringResource(com.termirust.mobile.R.string.pairing_offer_hint))
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        OutlinedButton(
                            onClick = onScan,
                            modifier = Modifier.weight(1f),
                            contentPadding = PaddingValues(horizontal = 12.dp, vertical = 8.dp),
                        ) {
                            Text(stringResource(com.termirust.mobile.R.string.scan_qr_code))
                        }
                        Button(
                            onClick = {
                                val clipboardOffer = clipboard.getText()?.text
                                    ?.trim()
                                    .orEmpty()
                                if (clipboardOffer.isEmpty()) {
                                    pairingOfferPasteError = PairingOfferPasteError.Empty
                                } else if (clipboardOffer.toByteArray().size > 4 * 1_024) {
                                    pairingOfferPasteError = PairingOfferPasteError.TooLarge
                                } else {
                                    viewModel.pairingOffer.value = clipboardOffer
                                    pairingOfferPasteError = null
                                }
                            },
                            modifier = Modifier.weight(1f),
                            contentPadding = PaddingValues(horizontal = 12.dp, vertical = 8.dp),
                        ) {
                            Text(stringResource(com.termirust.mobile.R.string.paste_offer))
                        }
                    }
                    OutlinedTextField(
                        value = offer,
                        onValueChange = { if (it.toByteArray().size <= 4 * 1_024) viewModel.pairingOffer.value = it },
                        label = { Text(stringResource(com.termirust.mobile.R.string.pairing_offer)) },
                        minLines = 4,
                        maxLines = 8,
                        modifier = Modifier.fillMaxWidth(),
                        trailingIcon = if (offer.isNotEmpty()) {
                            {
                                IconButton(
                                    onClick = {
                                        viewModel.pairingOffer.value = ""
                                        pairingOfferPasteError = null
                                    },
                                ) {
                                    Icon(
                                        Icons.Outlined.Close,
                                        contentDescription = stringResource(com.termirust.mobile.R.string.clear),
                                    )
                                }
                            }
                        } else {
                            null
                        },
                    )
                    pairingOfferPasteError?.let { error ->
                        Text(
                            stringResource(
                                if (error == PairingOfferPasteError.TooLarge) com.termirust.mobile.R.string.pairing_offer_too_large
                                else com.termirust.mobile.R.string.pairing_clipboard_empty,
                            ),
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    if (offer.isNotBlank()) {
                        Text(
                            stringResource(com.termirust.mobile.R.string.pairing_offer_ready),
                            color = androidx.compose.ui.graphics.Color(0xff22c55e),
                            style = MaterialTheme.typography.labelMedium,
                        )
                    }
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
                    enabled = offer.isNotBlank() && hostName.isNotBlank() && deviceName.isNotBlank() &&
                        connection !is ControllerConnectionState.Pairing,
                ) { Text(stringResource(com.termirust.mobile.R.string.continue_action)) }
            } else {
                Button(onClick = {
                    retainedSas = sas
                    awaitingCompletion = true
                    viewModel.finishPairing(true)
                }, enabled = connection !is ControllerConnectionState.Pairing) {
                    Text(stringResource(com.termirust.mobile.R.string.codes_match))
                }
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
