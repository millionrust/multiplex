package com.multiplex.mobile.controller

import android.content.res.Configuration
import androidx.compose.foundation.Image
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
import androidx.compose.foundation.layout.height
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
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.text.KeyboardOptions
import androidx.activity.compose.BackHandler
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Add
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
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
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
import androidx.compose.ui.graphics.asImageBitmap
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
import com.multiplex.mobile.ui.SlateTheme
import com.multiplex.mobile.ui.SlateTokens

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ControllerApp(viewModel: ControllerViewModel, modifier: Modifier = Modifier) {
    var showEnrollment by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf(false) }
    var showEnrollmentMenu by remember { mutableStateOf(false) }
    var showHostMenu by remember { mutableStateOf(false) }
    if (showEnrollment) {
        com.multiplex.mobile.replication.EnrollmentScreen(onBack = { showEnrollment = false }, modifier = modifier)
        return
    }
    val state by viewModel.state.collectAsState()
    var showPairing by remember { mutableStateOf(false) }
    var showOfferPairing by remember { mutableStateOf(false) }
    var showScanner by remember { mutableStateOf(false) }
    var showHostDetails by remember { mutableStateOf(false) }
    var confirmForget by remember { mutableStateOf(false) }
    var showSshConfiguration by remember { mutableStateOf(false) }
    var showRelayConfiguration by remember { mutableStateOf(false) }
    var pendingRoute by remember { mutableStateOf<ControllerRemoteRouteKind?>(null) }
    // Which computer's page is open, separate from which one the connection has selected: the
    // list is the launch surface, and it stays that until a computer is opened from it.
    var openHostId by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf<String?>(null) }
    var hostTab by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf(HostPageTab.Terminals) }
    var showNewTerminal by remember { mutableStateOf(false) }
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
    BackHandler(enabled = activeTerminal == null && openHostId != null) { openHostId = null }

    val screenViewer = viewModel.screens.viewer
    BackHandler(enabled = screenViewer != null) { viewModel.closeScreen() }
    if (screenViewer != null) {
        RemoteScreenView(
            model = screenViewer,
            reconnecting = viewModel.screens.reconnecting,
            onClose = viewModel::closeScreen,
            modifier = modifier,
        )
        return
    }
    // The connection carries one session at a time, so the preview runs only while a computer's
    // page is on screen, and only once the fleet has finished loading.
    LaunchedEffect(state.selectedHostId, state.connection, activeTerminal != null, hostTab, openHostId) {
        val showingScreen = hostTab == HostPageTab.Screen
        if (activeTerminal == null && showingScreen && viewModel.canWatchSelectedHost()) {
            viewModel.startScreenPreview()
        } else {
            viewModel.stopScreenPreview()
        }
    }
    DisposableEffect(Unit) { onDispose { viewModel.stopScreenPreview() } }

    com.multiplex.mobile.ui.MultiplexMaterialTheme {
        Scaffold(
            modifier = modifier.fillMaxSize(),
            topBar = {
                if (!focusedLandscapeTerminal) TopAppBar(
                    title = {
                        val openHost = state.hosts.firstOrNull { it.id == openHostId }
                        if (activeTerminal != null) {
                            Text(
                                isolated(activeTerminal.sessionTitle),
                                fontWeight = FontWeight.SemiBold,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        } else if (openHost != null) {
                            Column {
                                Text(
                                    isolated(openHost.displayName),
                                    fontWeight = FontWeight.SemiBold,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                                Text(connectionLabel(state.connection), style = MaterialTheme.typography.labelMedium)
                            }
                        } else {
                            // One line, the name of what is below it, as design/remote-screens/
                            // android.html has it. The app's own name and a second line of chrome
                            // were repeating what the launcher and the tab bar already say.
                            Text(stringResource(com.multiplex.mobile.R.string.controller_computers))
                        }
                    },
                    navigationIcon = {
                        if (activeTerminal == null && openHostId != null) {
                            IconButton(onClick = { openHostId = null }) {
                                Icon(
                                    Icons.AutoMirrored.Outlined.ArrowBack,
                                    contentDescription = stringResource(
                                        com.multiplex.mobile.R.string.controller_back_to_computers,
                                    ),
                                )
                            }
                        }
                    },
                    // A bar carries the actions of what it is titled. The computers list is where
                    // a computer is added, so pairing and enrollment stay there; one computer's page
                    // carries only that computer's, behind a single overflow, and the name gets the
                    // width it was being squeezed out of.
                    actions = {
                        if (activeTerminal != null) {
                            IconButton(onClick = viewModel::detachTerminal) {
                                Icon(
                                    Icons.Outlined.Close,
                                    contentDescription = stringResource(com.multiplex.mobile.R.string.detach),
                                )
                            }
                        } else if (openHostId != null) {
                            if (state.selectedHostId != null) {
                                Box {
                                    IconButton(onClick = { showHostMenu = true }) {
                                        Icon(
                                            Icons.Outlined.MoreVert,
                                            stringResource(com.multiplex.mobile.R.string.more_actions),
                                        )
                                    }
                                    DropdownMenu(
                                        expanded = showHostMenu,
                                        onDismissRequest = { showHostMenu = false },
                                    ) {
                                        DropdownMenuItem(
                                            text = { Text(stringResource(com.multiplex.mobile.R.string.details)) },
                                            onClick = { showHostMenu = false; showHostDetails = true },
                                        )
                                    }
                                }
                            }
                        } else {
                            IconButton(onClick = { showPairing = true }) {
                                Icon(
                                    Icons.Outlined.Add,
                                    stringResource(com.multiplex.mobile.R.string.pair_computer),
                                )
                            }
                            Box {
                                IconButton(onClick = { showEnrollmentMenu = true }) {
                                    Icon(
                                        Icons.Outlined.MoreVert,
                                        stringResource(com.multiplex.mobile.R.string.more_actions),
                                    )
                                }
                                DropdownMenu(
                                    expanded = showEnrollmentMenu,
                                    onDismissRequest = { showEnrollmentMenu = false },
                                ) {
                                    DropdownMenuItem(
                                        text = { Text(stringResource(com.multiplex.mobile.R.string.enrollment_title)) },
                                        onClick = { showEnrollmentMenu = false; showEnrollment = true },
                                    )
                                }
                            }
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = MaterialTheme.colorScheme.background,
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
                            onSelect = { id -> viewModel.selectHost(id); openHostId = id },
                            modifier = Modifier.width(340.dp).fillMaxHeight(),
                            lastPictures = viewModel.screens.lastPictures,
                        )
                        VerticalDivider(modifier = Modifier.fillMaxHeight())
                        FleetDetail(
                            state,
                            viewModel::retry,
                            viewModel::attachSession,
                            modifier = Modifier.weight(1f),
                            screens = viewModel.screens,
                            canWatch = viewModel.canWatchSelectedHost(),
                            onOpenScreen = viewModel::openScreen,
                            tab = hostTab,
                            onSelectTab = { hostTab = it },
                            canCreateSession = viewModel.canCreateSessionOnSelectedHost(),
                            onNewTerminal = { showNewTerminal = true },
                        )
                    }
                } else if (openHostId == null) {
                    // The computers are the launch surface: nothing connects until one is opened.
                    HostList(
                        state = state,
                        onSelect = { id -> viewModel.selectHost(id); openHostId = id },
                        modifier = Modifier.fillMaxSize(),
                        lastPictures = viewModel.screens.lastPictures,
                    )
                } else {
                    FleetDetail(
                        state,
                        viewModel::retry,
                        viewModel::attachSession,
                        modifier = Modifier.fillMaxSize(),
                        screens = viewModel.screens,
                        canWatch = viewModel.canWatchSelectedHost(),
                        onOpenScreen = viewModel::openScreen,
                        tab = hostTab,
                        onSelectTab = { hostTab = it },
                        canCreateSession = viewModel.canCreateSessionOnSelectedHost(),
                        onNewTerminal = { showNewTerminal = true },
                    )
                }
            }
        }
    }

    if (showNewTerminal) {
        NewTerminalDialog(
            onDismiss = { showNewTerminal = false },
            onStart = { folder, shell, name ->
                showNewTerminal = false
                viewModel.createSession(folder, shell, name)
            },
        )
    }
    if (showPairing) {
        PairComputerDialog(
            viewModel = viewModel,
            connection = state.connection,
            hosts = state.hosts,
            onDismiss = {
                viewModel.cancelPairing()
                showPairing = false
            },
            onComplete = { showPairing = false },
            onOtherWays = {
                viewModel.cancelPairing()
                showPairing = false
                showOfferPairing = true
            },
        )
    }
    if (showOfferPairing) {
        PairHostDialog(
            viewModel = viewModel,
            connection = state.connection,
            onDismiss = {
                viewModel.cancelPairing()
                showOfferPairing = false
            },
            onComplete = { showOfferPairing = false },
            onScan = { showScanner = true },
        )
    }
    if (showScanner) {
        ControllerQrScannerDialog(
            onResult = { offer ->
                viewModel.pairingOffer.value = offer
                showScanner = false
                showOfferPairing = true
            },
            onDismiss = { showScanner = false },
        )
    }
    if (showHostDetails) {
        val selected = state.hosts.firstOrNull { it.id == state.selectedHostId }
        if (selected != null) {
            HostDetailsDialog(
                host = selected,
                state = state,
                onDismiss = { showHostDetails = false },
                onReconnect = {
                    showHostDetails = false
                    viewModel.retry()
                },
                onForget = {
                    showHostDetails = false
                    confirmForget = true
                },
                onSelectRoute = { pendingRoute = it },
                onConfigureSsh = { showHostDetails = false; showSshConfiguration = true },
                onConfigureRelay = { showHostDetails = false; showRelayConfiguration = true },
            )
        }
    }
    if (confirmForget) {
        AlertDialog(
            onDismissRequest = { confirmForget = false },
            title = { Text(stringResource(com.multiplex.mobile.R.string.forget_host_title)) },
            text = { Text(stringResource(com.multiplex.mobile.R.string.forget_host_explanation)) },
            confirmButton = {
                Button(onClick = {
                    confirmForget = false
                    viewModel.forgetSelectedHost()
                }) { Text(stringResource(com.multiplex.mobile.R.string.forget_on_device)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmForget = false }) { Text(stringResource(com.multiplex.mobile.R.string.cancel)) }
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
            title = { Text(stringResource(com.multiplex.mobile.R.string.switch_controller_route_title)) },
            text = {
                Text(
                    stringResource(
                        com.multiplex.mobile.R.string.switch_controller_route_message,
                        controllerRouteTitle(target),
                    ),
                )
            },
            confirmButton = {
                Button(onClick = {
                    viewModel.selectControllerRoute(target, explicitlyConfirmed = true)
                    pendingRoute = null
                }) { Text(stringResource(com.multiplex.mobile.R.string.switch_route)) }
            },
            dismissButton = {
                TextButton(onClick = { pendingRoute = null }) {
                    Text(stringResource(com.multiplex.mobile.R.string.cancel))
                }
            },
        )
    }
}

/**
 * What a computer's page is showing.
 *
 * The Controller connection carries one session at a time, so these are two states of the phone
 * rather than two panes: the screen preview runs only while Screen is showing, and a terminal
 * attaches only from Terminals.
 */
internal enum class HostPageTab { Screen, Terminals }

/**
 * What to start, and where.
 *
 * Everything is optional: a person who wants "a terminal, here" says nothing and gets the
 * computer's own defaults, which is what the computer would have opened itself.
 */
@Composable
private fun NewTerminalDialog(
    onDismiss: () -> Unit,
    onStart: (String?, String?, String?) -> Unit,
) {
    var folder by remember { mutableStateOf("") }
    var shell by remember { mutableStateOf("") }
    var name by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(com.multiplex.mobile.R.string.new_terminal_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                OutlinedTextField(
                    value = folder,
                    onValueChange = { folder = it },
                    singleLine = true,
                    label = { Text(stringResource(com.multiplex.mobile.R.string.new_terminal_folder)) },
                    supportingText = {
                        Text(stringResource(com.multiplex.mobile.R.string.new_terminal_folder_hint))
                    },
                )
                OutlinedTextField(
                    value = shell,
                    onValueChange = { shell = it },
                    singleLine = true,
                    label = { Text(stringResource(com.multiplex.mobile.R.string.new_terminal_shell)) },
                    supportingText = {
                        Text(stringResource(com.multiplex.mobile.R.string.new_terminal_shell_hint))
                    },
                )
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    singleLine = true,
                    label = { Text(stringResource(com.multiplex.mobile.R.string.new_terminal_name)) },
                )
            }
        },
        confirmButton = {
            Button(onClick = { onStart(folder, shell, name) }) {
                Text(stringResource(com.multiplex.mobile.R.string.new_terminal_start))
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text(stringResource(com.multiplex.mobile.R.string.cancel))
            }
        },
    )
}

@Composable
private fun EmptyFleet(onPair: () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier.padding(28.dp),
        ) {
            Text(stringResource(com.multiplex.mobile.R.string.no_paired_hosts), style = MaterialTheme.typography.headlineSmall)
            Text(
                stringResource(com.multiplex.mobile.R.string.pair_private_network_hint),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Button(onClick = onPair, modifier = Modifier.heightIn(min = 48.dp)) {
                Text(stringResource(com.multiplex.mobile.R.string.pair_computer))
            }
        }
    }
}

@Composable
private fun HostList(
    state: ControllerUiState,
    onSelect: (String) -> Unit,
    modifier: Modifier = Modifier,
    lastPictures: Map<String, android.graphics.Bitmap> = emptyMap(),
) {
    // The bar above says Computers, so the list goes straight to them, as
    // design/remote-screens/android.html has it. A heading repeating the bar and a line telling
    // the reader to tap a card were between them.
    LazyColumn(modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        items(state.hosts, key = { it.id }) { host ->
            HostRow(
                host = host,
                connected = host.id == state.selectedHostId &&
                    state.connection == ControllerConnectionState.ReadyReadOnly,
                glance = state.glances[host.id],
                picture = lastPictures[host.id],
            ) { onSelect(host.id) }
        }
    }
}

@Composable
private fun HostRow(
    host: PairedHostRecord,
    connected: Boolean,
    glance: HostGlance?,
    picture: android.graphics.Bitmap?,
    onClick: () -> Unit,
) {
    val hostDescription = stringResource(com.multiplex.mobile.R.string.host_accessibility, isolated(host.displayName))
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .semantics { contentDescription = hostDescription },
        colors = CardDefaults.cardColors(
            containerColor = if (connected) {
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
            // The last picture of that computer, when one has ever arrived; otherwise a plain
            // square, because an empty frame says more honestly that nothing has been seen.
            if (picture != null) {
                Image(
                    bitmap = picture.asImageBitmap(),
                    contentDescription = null,
                    modifier = Modifier.width(64.dp).height(40.dp),
                )
            } else {
                Box(
                    Modifier.size(36.dp).background(
                        MaterialTheme.colorScheme.primary,
                        MaterialTheme.shapes.small,
                    ),
                    contentAlignment = Alignment.Center,
                ) { Text(">", color = MaterialTheme.colorScheme.onPrimary, fontWeight = FontWeight.Bold) }
            }
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
                Text(
                    hostGlanceLabel(glance),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (connected) {
                AssistChip(
                    onClick = onClick,
                    label = { Text(stringResource(com.multiplex.mobile.R.string.controller_host_connected)) },
                )
            }
        }
    }
}

/** How many terminals were open when the phone last looked, and when that was. */
@Composable
private fun hostGlanceLabel(glance: HostGlance?): String {
    if (glance == null) return stringResource(com.multiplex.mobile.R.string.controller_host_never_looked)
    val terminals = if (glance.openTerminals == 1) {
        stringResource(com.multiplex.mobile.R.string.controller_host_one_terminal)
    } else {
        stringResource(com.multiplex.mobile.R.string.controller_host_terminals, glance.openTerminals)
    }
    return "$terminals · " + stringResource(
        com.multiplex.mobile.R.string.controller_host_seen,
        relativeTime(glance.updatedAtMillis),
    )
}

@Composable
private fun FleetDetail(
    state: ControllerUiState,
    onRetry: () -> Unit,
    onOpenSession: (String) -> Unit,
    modifier: Modifier = Modifier,
    screens: ControllerScreenCoordinator? = null,
    canWatch: Boolean = false,
    onOpenScreen: (UInt?) -> Unit = {},
    tab: HostPageTab = HostPageTab.Terminals,
    onSelectTab: (HostPageTab) -> Unit = {},
    canCreateSession: Boolean = false,
    onNewTerminal: () -> Unit = {},
) {
    val openTerminals = state.sessions.filter(ControllerSessionSummary::isOpenTerminal)
    val previousSessions = state.sessions.filterNot(ControllerSessionSummary::isOpenTerminal)
    Column(modifier.fillMaxSize()) {
        ConnectionBanner(state, onRetry)
        HostPageTabs(tab, onSelectTab)
        when (tab) {
            HostPageTab.Screen -> ScreenTab(
                state = state,
                screens = screens,
                canWatch = canWatch,
                onOpenScreen = onOpenScreen,
            )
            HostPageTab.Terminals -> LazyColumn(
                Modifier.fillMaxSize().padding(12.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                item(key = "new-terminal") {
                    if (canCreateSession) {
                        Button(
                            onClick = onNewTerminal,
                            modifier = Modifier.fillMaxWidth(),
                        ) { Text(stringResource(com.multiplex.mobile.R.string.new_terminal)) }
                    } else {
                        Text(
                            stringResource(com.multiplex.mobile.R.string.new_terminal_not_granted),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                if (openTerminals.isEmpty()) {
                    item(key = "no-open-terminals") {
                        Box(
                            Modifier.fillMaxWidth().heightIn(min = 140.dp),
                            contentAlignment = Alignment.Center,
                        ) {
                            Text(
                                if (state.connection.isBusy()) {
                                    stringResource(com.multiplex.mobile.R.string.loading_sessions)
                                } else {
                                    stringResource(com.multiplex.mobile.R.string.no_open_terminals)
                                },
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                } else {
                    item(key = "open-terminals-header") {
                        SessionSectionHeader(stringResource(com.multiplex.mobile.R.string.open_terminals))
                    }
                    items(openTerminals, key = { it.id }) { session ->
                        SessionRow(session, state.cachedReadOnly) { onOpenSession(session.id) }
                    }
                }
                if (previousSessions.isNotEmpty()) {
                    item(key = "previous-sessions-header") {
                        SessionSectionHeader(stringResource(com.multiplex.mobile.R.string.previous_sessions))
                    }
                    items(previousSessions, key = { it.id }) { session ->
                        SessionRow(session, state.cachedReadOnly) { onOpenSession(session.id) }
                    }
                }
            }
        }
    }
}

/** Screen or Terminals: which of the two a computer's page is showing. */
@Composable
private fun HostPageTabs(tab: HostPageTab, onSelectTab: (HostPageTab) -> Unit) {
    SingleChoiceSegmentedButtonRow(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
    ) {
        HostPageTab.entries.forEachIndexed { index, entry ->
            SegmentedButton(
                selected = tab == entry,
                onClick = { onSelectTab(entry) },
                shape = SegmentedButtonDefaults.itemShape(index, HostPageTab.entries.size),
            ) {
                Text(
                    stringResource(
                        when (entry) {
                            HostPageTab.Screen -> com.multiplex.mobile.R.string.controller_tab_screen
                            HostPageTab.Terminals -> com.multiplex.mobile.R.string.controller_tab_terminals
                        },
                    ),
                )
            }
        }
    }
}

/**
 * The computer's screen: its last picture, and the displays it has.
 *
 * The displays come from the preview session, which is the only thing that knows them, so the
 * list fills in once the preview has connected. Before then, and on a computer that never shared
 * its screen, the page says so rather than offering a button that would fail.
 */
@Composable
private fun ScreenTab(
    state: ControllerUiState,
    screens: ControllerScreenCoordinator?,
    canWatch: Boolean,
    onOpenScreen: (UInt?) -> Unit,
) {
    if (!canWatch || screens == null) {
        Box(
            Modifier.fillMaxSize().padding(24.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                stringResource(com.multiplex.mobile.R.string.controller_screen_not_granted),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        return
    }
    val displays = screens.preview?.displays.orEmpty()
    LazyColumn(
        Modifier.fillMaxSize().padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        item(key = "this-computers-screen") {
            ControllerScreenPreviewCard(
                preview = screens.preview,
                lastPicture = state.selectedHostId?.let { screens.lastPictures[it] },
                unavailable = screens.unavailable,
                onOpenScreen = { onOpenScreen(null) },
            )
        }
        if (displays.size > 1) {
            item(key = "displays-header") {
                SessionSectionHeader(stringResource(com.multiplex.mobile.R.string.controller_displays))
            }
            items(displays, key = { it.id.toLong() }) { display ->
                DisplayRow(display) { onOpenScreen(display.id) }
            }
        }
        item(key = "one-at-a-time") {
            Text(
                stringResource(com.multiplex.mobile.R.string.controller_one_at_a_time),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 4.dp, vertical = 8.dp),
            )
        }
    }
}

@Composable
private fun DisplayRow(display: com.multiplex.screens.ScreenSurface, onOpen: () -> Unit) {
    Card(
        Modifier.fillMaxWidth().clickable(onClick = onOpen),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
    ) {
        Row(
            Modifier.fillMaxWidth().padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Text(isolated(display.name), fontWeight = FontWeight.SemiBold, maxLines = 1)
                Text(
                    "${display.width} × ${display.height}",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            TextButton(onClick = onOpen) {
                Text(stringResource(com.multiplex.mobile.R.string.controller_open_display))
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
                    stringResource(com.multiplex.mobile.R.string.controller_route_title),
                    style = MaterialTheme.typography.titleSmall,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    stringResource(com.multiplex.mobile.R.string.controller_route_subtitle),
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
        state.routeAdvice?.let { advice ->
            Text(
                controllerRouteAdvice(advice),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
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
                        label = { Text(stringResource(com.multiplex.mobile.R.string.controller_selected)) },
                    )
                    projection.available -> OutlinedButton(onClick = { onSelect(projection.route) }) {
                        Text(stringResource(com.multiplex.mobile.R.string.use_route))
                    }
                    projection.route == ControllerRemoteRouteKind.SSH ||
                        projection.route == ControllerRemoteRouteKind.SELF_HOSTED_RELAY ->
                        OutlinedButton(onClick = if (projection.route == ControllerRemoteRouteKind.SSH) onConfigureSsh else onConfigureRelay) {
                            Text(stringResource(com.multiplex.mobile.R.string.configure))
                        }
                    else -> Text(
                        stringResource(com.multiplex.mobile.R.string.not_configured),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (projection.route == ControllerRemoteRouteKind.SSH && projection.available) {
                    TextButton(onClick = onConfigureSsh) {
                        Text(stringResource(com.multiplex.mobile.R.string.edit))
                    }
                }
                if (projection.route == ControllerRemoteRouteKind.SELF_HOSTED_RELAY && projection.available) {
                    TextButton(onClick = onConfigureRelay) {
                        Text(stringResource(com.multiplex.mobile.R.string.edit))
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
    val invalidPackageMessage = stringResource(com.multiplex.mobile.R.string.invalid_relay_controller_package)
    val parsedEpoch = epoch.toLongOrNull()
    val canSave = endpoint.startsWith("wss://") && pin.startsWith("sha256/") &&
        routeId.isNotBlank() && parsedEpoch != null && parsedEpoch >= 0 && credential.isNotBlank()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(com.multiplex.mobile.R.string.configure_relay_controller)) },
        text = {
            LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                item {
                    Text(
                        stringResource(com.multiplex.mobile.R.string.relay_controller_configuration_help),
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
                        Text(stringResource(com.multiplex.mobile.R.string.paste_relay_controller_package))
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
                item { OutlinedTextField(endpoint, { endpoint = it }, label = { Text(stringResource(com.multiplex.mobile.R.string.relay_endpoint)) }, singleLine = true) }
                item { OutlinedTextField(pin, { pin = it }, label = { Text(stringResource(com.multiplex.mobile.R.string.relay_spki_pin)) }, singleLine = true) }
                item { OutlinedTextField(routeId, { routeId = it }, label = { Text(stringResource(com.multiplex.mobile.R.string.relay_route_id)) }, singleLine = true) }
                item {
                    OutlinedTextField(
                        epoch,
                        { epoch = it.filter(Char::isDigit) },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.relay_epoch)) },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        credential,
                        { credential = it },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.relay_admission_credential)) },
                        visualTransformation = PasswordVisualTransformation(),
                        singleLine = true,
                    )
                }
                if (configuration != null) {
                    item {
                        TextButton(onClick = { confirmRemove = true }) {
                            Text(stringResource(com.multiplex.mobile.R.string.remove_relay_controller_route), color = MaterialTheme.colorScheme.error)
                        }
                    }
                }
            }
        },
        confirmButton = {
            Button(enabled = canSave, onClick = { onSave(endpoint, pin, routeId, checkNotNull(parsedEpoch), credential) }) {
                Text(stringResource(com.multiplex.mobile.R.string.save))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(com.multiplex.mobile.R.string.cancel)) } },
    )
    if (confirmRemove) {
        AlertDialog(
            onDismissRequest = { confirmRemove = false },
            title = { Text(stringResource(com.multiplex.mobile.R.string.remove_relay_controller_route)) },
            text = { Text(stringResource(com.multiplex.mobile.R.string.remove_relay_controller_route_help)) },
            confirmButton = { TextButton(onClick = onRemove) { Text(stringResource(com.multiplex.mobile.R.string.remove), color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { confirmRemove = false }) { Text(stringResource(com.multiplex.mobile.R.string.cancel)) } },
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
        title = { Text(stringResource(com.multiplex.mobile.R.string.configure_ssh_controller)) },
        text = {
            LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                item {
                    Text(
                        stringResource(com.multiplex.mobile.R.string.ssh_controller_configuration_help),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                item {
                    OutlinedTextField(
                        value = endpoint,
                        onValueChange = { endpoint = it },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.ssh_host)) },
                        singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        value = port,
                        onValueChange = { port = it.filter(Char::isDigit).take(5) },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.ssh_port)) },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        value = username,
                        onValueChange = { username = it },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.ssh_username)) },
                        singleLine = true,
                    )
                }
                item {
                    OutlinedTextField(
                        value = pin,
                        onValueChange = { pin = it },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.ssh_host_key_pin)) },
                        singleLine = true,
                    )
                }
                item {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilterChip(
                            selected = authentication == ControllerSshAuthenticationKind.PRIVATE_KEY,
                            onClick = { authentication = ControllerSshAuthenticationKind.PRIVATE_KEY },
                            label = { Text(stringResource(com.multiplex.mobile.R.string.private_key)) },
                        )
                        FilterChip(
                            selected = authentication == ControllerSshAuthenticationKind.PASSWORD,
                            onClick = { authentication = ControllerSshAuthenticationKind.PASSWORD },
                            label = { Text(stringResource(com.multiplex.mobile.R.string.password)) },
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
                                    stringResource(com.multiplex.mobile.R.string.password)
                                } else {
                                    stringResource(com.multiplex.mobile.R.string.openssh_private_key)
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
                                stringResource(com.multiplex.mobile.R.string.remove_ssh_controller_route),
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
            ) { Text(stringResource(com.multiplex.mobile.R.string.save)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(com.multiplex.mobile.R.string.cancel)) }
        },
    )
    if (confirmRemove) {
        AlertDialog(
            onDismissRequest = { confirmRemove = false },
            title = { Text(stringResource(com.multiplex.mobile.R.string.remove_ssh_controller_route)) },
            text = { Text(stringResource(com.multiplex.mobile.R.string.remove_ssh_controller_route_help)) },
            confirmButton = {
                TextButton(onClick = onRemove) {
                    Text(
                        stringResource(com.multiplex.mobile.R.string.remove),
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmRemove = false }) {
                    Text(stringResource(com.multiplex.mobile.R.string.cancel))
                }
            },
        )
    }
}

@Composable
private fun controllerRouteTitle(route: ControllerRemoteRouteKind): String = when (route) {
    ControllerRemoteRouteKind.LOCAL_IPC -> stringResource(com.multiplex.mobile.R.string.route_local_ipc)
    ControllerRemoteRouteKind.PRIVATE_NETWORK -> stringResource(com.multiplex.mobile.R.string.route_private_network)
    ControllerRemoteRouteKind.SSH -> stringResource(com.multiplex.mobile.R.string.route_ssh)
    ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> stringResource(com.multiplex.mobile.R.string.route_self_hosted_relay)
}

@Composable
private fun controllerRouteDescription(route: ControllerRemoteRouteKind): String = when (route) {
    ControllerRemoteRouteKind.LOCAL_IPC -> stringResource(com.multiplex.mobile.R.string.route_local_ipc_description)
    ControllerRemoteRouteKind.PRIVATE_NETWORK -> stringResource(com.multiplex.mobile.R.string.route_private_network_description)
    ControllerRemoteRouteKind.SSH -> stringResource(com.multiplex.mobile.R.string.route_ssh_description)
    ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> stringResource(com.multiplex.mobile.R.string.route_relay_description)
}

@Composable
private fun controllerRouteStatus(projection: AndroidControllerRouteProjection): String = when {
    !projection.available -> stringResource(com.multiplex.mobile.R.string.not_configured)
    projection.phase == ControllerRemoteRoutePhase.ONLINE -> stringResource(com.multiplex.mobile.R.string.route_status_online)
    projection.phase == ControllerRemoteRoutePhase.DEGRADED -> stringResource(com.multiplex.mobile.R.string.route_status_degraded)
    projection.phase == ControllerRemoteRoutePhase.RECONNECTING -> stringResource(com.multiplex.mobile.R.string.route_status_reconnecting)
    projection.phase == ControllerRemoteRoutePhase.REVOKED -> stringResource(com.multiplex.mobile.R.string.route_status_revoked)
    else -> stringResource(com.multiplex.mobile.R.string.route_status_ready)
}

@Composable
private fun controllerRouteAdvice(advice: String): String = when (advice) {
    "route_advice_tailscale_exit_node" -> stringResource(com.multiplex.mobile.R.string.route_advice_tailscale_exit_node)
    "route_advice_remote_access_off" -> stringResource(com.multiplex.mobile.R.string.route_advice_remote_access_off)
    "route_advice_not_on_network" -> stringResource(com.multiplex.mobile.R.string.route_advice_not_on_network)
    else -> stringResource(com.multiplex.mobile.R.string.route_advice_needs_remote_route)
}

@Composable
private fun controllerRouteError(error: String): String = when (error) {
    "route_confirmation_required" -> stringResource(com.multiplex.mobile.R.string.route_error_confirmation)
    "route_unavailable" -> stringResource(com.multiplex.mobile.R.string.route_error_unavailable)
    "route_already_selected" -> stringResource(com.multiplex.mobile.R.string.route_error_selected)
    "route_degraded" -> stringResource(com.multiplex.mobile.R.string.route_error_degraded)
    "route_configuration_invalid" -> stringResource(com.multiplex.mobile.R.string.route_error_configuration_invalid)
    else -> stringResource(com.multiplex.mobile.R.string.route_error_generic)
}

@Composable
private fun ConnectionBanner(state: ControllerUiState, onRetry: () -> Unit) {
    // The bar above this one already names the connection, so the band says something only when
    // there is more to say than the name: it is still working, it failed, or what is on screen is
    // cached rather than live. A healthy live page used to carry the same words twice.
    val needsRetry = state.connection is ControllerConnectionState.Failed || state.cachedReadOnly
    if (!state.connection.isBusy() && !needsRetry) return
    Surface(color = MaterialTheme.colorScheme.surfaceVariant) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (state.connection.isBusy()) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            Column(Modifier.weight(1f)) {
                Text(connectionLabel(state.connection), fontWeight = FontWeight.SemiBold)
                // How old what is on screen is, live or cached — it used to say only when the
                // data was stale, so a live list looked the same as one from an hour ago.
                freshnessLabelResource(state)?.let { label ->
                    Text(
                        stringResource(label, relativeTime(state.cachedAtMillis)),
                        style = MaterialTheme.typography.labelSmall,
                    )
                }
            }
            if (state.connection is ControllerConnectionState.Failed || state.cachedReadOnly) {
                OutlinedButton(onClick = onRetry) { Text(stringResource(com.multiplex.mobile.R.string.retry)) }
            }
        }
    }
}

@Composable
private fun SessionRow(session: ControllerSessionSummary, cached: Boolean, onOpen: () -> Unit) {
    val canOpen = !cached && session.occupantGeneration != null &&
        (session.capabilities.isEmpty() || ControllerSessionCapability.ATTACH_OUTPUT in session.capabilities)
    val description = stringResource(com.multiplex.mobile.R.string.monitor_session, isolated(session.title))
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
                AssistChip(
                    onClick = {},
                    label = {
                        Text(
                            if (cached) {
                                stringResource(com.multiplex.mobile.R.string.cached)
                            } else {
                                lifecycleLabel(session.lifecycle)
                            },
                        )
                    },
                )
            }
            val location = listOfNotNull(session.project, session.group).joinToString(" / ")
            if (location.isNotEmpty()) {
                Text(isolated(location), style = MaterialTheme.typography.bodySmall)
            }
            val origin = when (session.origin) {
                ControllerSessionOrigin.TERMINAL -> stringResource(com.multiplex.mobile.R.string.session_origin_terminal)
                ControllerSessionOrigin.MANAGED_AGENT -> stringResource(com.multiplex.mobile.R.string.session_origin_managed_agent)
                ControllerSessionOrigin.OBSERVED_AGENT -> stringResource(com.multiplex.mobile.R.string.session_origin_observed_agent)
                ControllerSessionOrigin.UNKNOWN -> stringResource(com.multiplex.mobile.R.string.session_origin_unknown)
            }
            val access = if (ControllerSessionCapability.SEND_INPUT in session.capabilities) {
                stringResource(com.multiplex.mobile.R.string.session_control_available)
            } else {
                stringResource(com.multiplex.mobile.R.string.view_only)
            }
            Text(
                listOfNotNull(origin, session.runtime, access).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(
                    stringResource(activityLabelResource(session.activity)),
                    style = MaterialTheme.typography.labelMedium,
                )
                if (session.unreadCount > 0) Text(stringResource(com.multiplex.mobile.R.string.unread), color = MaterialTheme.colorScheme.primary)
                if (session.hasWriter) Text(stringResource(com.multiplex.mobile.R.string.writer_active), color = MaterialTheme.colorScheme.tertiary)
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
    onViewportChanged: (Int, Int, Boolean) -> Unit,
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
        mutableStateOf(terminalPreferences.getBoolean("desktop_width", true))
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
    val privacyAccessibility = stringResource(com.multiplex.mobile.R.string.terminal_privacy_accessibility)
    val terminalOutputAccessibility = stringResource(
        com.multiplex.mobile.R.string.terminal_output_accessibility,
        terminal.sessionTitle,
        TerminalAcceptance.accessibleOutput(lines),
    )
    val canInput = terminal.writerLease == WriterLeaseState.Held &&
        terminal.attachState == ReadOnlyAttachState.Live && !terminal.privacyCovered
    val statusColor = when (terminal.attachState) {
        ReadOnlyAttachState.Live -> terminalSlateColor(SlateTokens.colorStatusDone(TerminalSlateTheme))
        is ReadOnlyAttachState.Gap, is ReadOnlyAttachState.Failed ->
            terminalSlateColor(SlateTokens.colorStatusAttention(TerminalSlateTheme))
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
        onViewportChanged(displayedColumns, displayedRows, usesDesktopWidth)
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
    Column(Modifier.fillMaxSize().background(terminalSlateColor(SlateTokens.colorBgTerminal(TerminalSlateTheme)))) {
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
                                terminalSlateColor(SlateTokens.colorStatusDone(TerminalSlateTheme))
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                            maxLines = 1,
                        )
                        TerminalControlAffordance(
                            terminal = terminal,
                            onRequestControl = onRequestControl,
                            onReleaseControl = onReleaseControl,
                        )
                        if (terminal.attachState is ReadOnlyAttachState.Offline ||
                            terminal.attachState is ReadOnlyAttachState.Gap ||
                            terminal.attachState is ReadOnlyAttachState.Failed
                        ) {
                            TextButton(onClick = onRetry) {
                                Text(stringResource(com.multiplex.mobile.R.string.retry))
                            }
                        }
                        Box {
                            IconButton(onClick = { showOptions = true }) {
                                Icon(
                                    Icons.Outlined.MoreVert,
                                    contentDescription = stringResource(com.multiplex.mobile.R.string.terminal_options),
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
                                                if (followOutput) com.multiplex.mobile.R.string.stop_following_output
                                                else com.multiplex.mobile.R.string.follow_output,
                                            ),
                                        )
                                    },
                                    onClick = {
                                        followOutput = !followOutput
                                        showOptions = false
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(com.multiplex.mobile.R.string.phone_width)) },
                                    onClick = {
                                        setDesktopWidth(false)
                                        showOptions = false
                                    },
                                    trailingIcon = {
                                        if (!usesDesktopWidth) Icon(Icons.Outlined.Check, contentDescription = null)
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(com.multiplex.mobile.R.string.desktop_width)) },
                                    onClick = {
                                        setDesktopWidth(true)
                                        showOptions = false
                                    },
                                    trailingIcon = {
                                        if (usesDesktopWidth) Icon(Icons.Outlined.Check, contentDescription = null)
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(com.multiplex.mobile.R.string.decrease_terminal_text)) },
                                    onClick = {
                                        setTerminalFontSize(terminalFontSize - 1)
                                        showOptions = false
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text(stringResource(com.multiplex.mobile.R.string.increase_terminal_text)) },
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
                            stringResource(com.multiplex.mobile.R.string.terminal_truncated),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                    terminal.writerMessage?.let { code ->
                        Text(
                            stringResource(
                                com.multiplex.mobile.R.string.terminal_control_warning,
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
                maxOf(maxWidth, ((terminal.screen.cells.maxOfOrNull { it.size } ?: displayedColumns) * displayedFontSize * 0.62 + 24).dp)
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
                        stringResource(com.multiplex.mobile.R.string.terminal_privacy_cover),
                        style = MaterialTheme.typography.titleMedium,
                        modifier = Modifier.padding(24.dp),
                    )
                }
            } else if (lines.all(String::isEmpty)) {
                Text(
                    terminalEmptyText(terminal.attachState),
                    color = terminalSlateColor(SlateTokens.colorTextMuted(TerminalSlateTheme)),
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
                            stringResource(com.multiplex.mobile.R.string.you_control),
                            color = terminalSlateColor(SlateTokens.colorStatusDone(TerminalSlateTheme)),
                            fontWeight = FontWeight.SemiBold,
                        )
                        TextButton(onClick = onReleaseControl) {
                            Text(stringResource(com.multiplex.mobile.R.string.release_control))
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
                        Text(stringResource(com.multiplex.mobile.R.string.paste))
                    }
                    Button(onClick = { setKeyboardPresented(!keyboardPresented) }) {
                        Icon(
                            if (keyboardPresented) Icons.Outlined.KeyboardHide else Icons.Outlined.Keyboard,
                            contentDescription = null,
                        )
                        Spacer(Modifier.width(6.dp))
                        Text(
                            stringResource(
                                if (keyboardPresented) com.multiplex.mobile.R.string.hide_keyboard
                                else com.multiplex.mobile.R.string.show_keyboard,
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
            title = { Text(stringResource(com.multiplex.mobile.R.string.paste_confirmation_title)) },
            text = {
                Text(stringResource(com.multiplex.mobile.R.string.paste_confirmation_message, terminal.pendingPasteBytes))
            },
            confirmButton = {
                Button(onClick = onConfirmPaste) { Text(stringResource(com.multiplex.mobile.R.string.send_paste)) }
            },
            dismissButton = {
                TextButton(onClick = onCancelPaste) { Text(stringResource(com.multiplex.mobile.R.string.cancel)) }
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
                        terminalSlateColor(SlateTokens.colorBgTerminal(TerminalSlateTheme))
                    } else {
                        colors.first
                    },
                    background = if (column == cursorColumn) {
                        terminalSlateColor(SlateTokens.colorTerminalCursor(TerminalSlateTheme))
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
        terminalSlateColor(SlateTokens.colorTerminalFg(TerminalSlateTheme)),
    )
    val background = terminalColor(
        style.background,
        terminalSlateColor(SlateTokens.colorBgTerminal(TerminalSlateTheme)),
    )
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

// The controller terminal is always drawn dark, so it uses the Slate Dark terminal tokens.
private val TerminalSlateTheme = SlateTheme.Dark

private fun terminalSlateColor(argb: Long) = androidx.compose.ui.graphics.Color(argb)

private fun ansiColor(index: Int): androidx.compose.ui.graphics.Color {
    val named = when (index) {
        0 -> SlateTokens.colorTerminalAnsiBlack(TerminalSlateTheme)
        1 -> SlateTokens.colorTerminalAnsiRed(TerminalSlateTheme)
        2 -> SlateTokens.colorTerminalAnsiGreen(TerminalSlateTheme)
        3 -> SlateTokens.colorTerminalAnsiYellow(TerminalSlateTheme)
        4 -> SlateTokens.colorTerminalAnsiBlue(TerminalSlateTheme)
        5 -> SlateTokens.colorTerminalAnsiMagenta(TerminalSlateTheme)
        6 -> SlateTokens.colorTerminalAnsiCyan(TerminalSlateTheme)
        7 -> SlateTokens.colorTerminalAnsiWhite(TerminalSlateTheme)
        8 -> SlateTokens.colorTerminalAnsiBrightBlack(TerminalSlateTheme)
        9 -> SlateTokens.colorTerminalAnsiBrightRed(TerminalSlateTheme)
        10 -> SlateTokens.colorTerminalAnsiBrightGreen(TerminalSlateTheme)
        11 -> SlateTokens.colorTerminalAnsiBrightYellow(TerminalSlateTheme)
        12 -> SlateTokens.colorTerminalAnsiBrightBlue(TerminalSlateTheme)
        13 -> SlateTokens.colorTerminalAnsiBrightMagenta(TerminalSlateTheme)
        14 -> SlateTokens.colorTerminalAnsiBrightCyan(TerminalSlateTheme)
        15 -> SlateTokens.colorTerminalAnsiBrightWhite(TerminalSlateTheme)
        else -> null
    }
    if (named != null) {
        return terminalSlateColor(named)
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

/**
 * The one control the person acts on, and the one place it says why they cannot.
 *
 * A terminal the computer never granted input for used to show nothing at all here, so the
 * absence of a button was the only explanation on offer. It now says View only and, held down,
 * says what would have to change.
 */
@Composable
private fun TerminalControlAffordance(
    terminal: ControllerTerminalUiState,
    onRequestControl: () -> Unit,
    onReleaseControl: () -> Unit,
) {
    when {
        terminal.writerLease == WriterLeaseState.Held ->
            FilledTonalButton(onClick = onReleaseControl) {
                Text(stringResource(com.multiplex.mobile.R.string.release_control))
            }
        terminal.writerLease is WriterLeaseState.Requesting ->
            TextButton(onClick = {}, enabled = false) {
                Text(stringResource(com.multiplex.mobile.R.string.asking_for_control))
            }
        terminal.supportsWriter ->
            TextButton(
                onClick = onRequestControl,
                enabled = terminal.attachState == ReadOnlyAttachState.Live,
            ) {
                Text(stringResource(com.multiplex.mobile.R.string.take_control))
            }
        else -> {
            val why = stringResource(com.multiplex.mobile.R.string.control_not_granted_why)
            Text(
                stringResource(com.multiplex.mobile.R.string.control_not_granted),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.semantics { contentDescription = why },
            )
        }
    }
}

@Composable
private fun writerLabel(terminal: ControllerTerminalUiState): String = when (terminal.writerLease) {
    WriterLeaseState.None -> if (terminal.hasWriterElsewhere) {
        stringResource(com.multiplex.mobile.R.string.controlled_elsewhere)
    } else stringResource(com.multiplex.mobile.R.string.view_only)
    is WriterLeaseState.Requesting -> stringResource(com.multiplex.mobile.R.string.requesting_control)
    WriterLeaseState.Held -> stringResource(com.multiplex.mobile.R.string.you_control)
    WriterLeaseState.Busy -> stringResource(com.multiplex.mobile.R.string.controlled_elsewhere)
    WriterLeaseState.Lost -> stringResource(com.multiplex.mobile.R.string.control_lost)
}

@Composable
private fun writerMessage(code: String): String = stringResource(
    when (code) {
        "control_unavailable" -> com.multiplex.mobile.R.string.writer_error_control_unavailable
        "controlled_elsewhere" -> com.multiplex.mobile.R.string.writer_error_controlled_elsewhere
        "input_pressure" -> com.multiplex.mobile.R.string.writer_error_input_pressure
        "paste_too_large" -> com.multiplex.mobile.R.string.writer_error_paste_too_large
        "resize_rejected" -> com.multiplex.mobile.R.string.writer_error_resize_rejected
        "completion_unknown" -> com.multiplex.mobile.R.string.writer_error_unknown
        "connection_failed_no_replay" -> com.multiplex.mobile.R.string.writer_error_connection
        else -> com.multiplex.mobile.R.string.writer_error_rejected
    },
)

@Composable
private fun terminalStatus(terminal: ControllerTerminalUiState): String = when (val state = terminal.attachState) {
    ReadOnlyAttachState.Detached -> stringResource(com.multiplex.mobile.R.string.terminal_detached)
    ReadOnlyAttachState.Authenticating -> stringResource(com.multiplex.mobile.R.string.terminal_authenticating)
    ReadOnlyAttachState.Snapshot -> stringResource(com.multiplex.mobile.R.string.terminal_snapshot)
    ReadOnlyAttachState.Replaying -> stringResource(com.multiplex.mobile.R.string.terminal_replaying)
    ReadOnlyAttachState.Live -> stringResource(
        if (terminal.hasWriterElsewhere) com.multiplex.mobile.R.string.terminal_live_writer
        else com.multiplex.mobile.R.string.terminal_live,
    )
    is ReadOnlyAttachState.Gap -> stringResource(com.multiplex.mobile.R.string.terminal_gap, state.expected, state.received)
    ReadOnlyAttachState.Exited -> stringResource(com.multiplex.mobile.R.string.terminal_exited)
    ReadOnlyAttachState.Offline -> stringResource(com.multiplex.mobile.R.string.terminal_offline)
    is ReadOnlyAttachState.Failed -> stringResource(com.multiplex.mobile.R.string.terminal_failed)
}

@Composable
private fun terminalEmptyText(state: ReadOnlyAttachState): String = when (state) {
    ReadOnlyAttachState.Authenticating, ReadOnlyAttachState.Snapshot, ReadOnlyAttachState.Replaying ->
        stringResource(com.multiplex.mobile.R.string.terminal_waiting)
    ReadOnlyAttachState.Live -> stringResource(com.multiplex.mobile.R.string.terminal_no_visible_output)
    ReadOnlyAttachState.Offline -> stringResource(com.multiplex.mobile.R.string.terminal_no_offline_screen)
    is ReadOnlyAttachState.Failed -> stringResource(com.multiplex.mobile.R.string.terminal_render_failed)
    else -> stringResource(com.multiplex.mobile.R.string.terminal_no_output)
}

@Composable
private fun PairComputerDialog(
    viewModel: ControllerViewModel,
    connection: ControllerConnectionState,
    hosts: List<PairedHostRecord>,
    onDismiss: () -> Unit,
    onComplete: () -> Unit,
    onOtherWays: () -> Unit,
) {
    val computers by viewModel.discoveredComputers.collectAsState()
    val address by viewModel.pairingAddress.collectAsState()
    val code by viewModel.pairingCode.collectAsState()
    val deviceName by viewModel.pairingDeviceName.collectAsState()
    var target by remember { mutableStateOf<CodePairingTarget?>(null) }
    var enteringAddress by remember { mutableStateOf(false) }
    var addressInvalid by remember { mutableStateOf(false) }
    var attempted by remember { mutableStateOf(false) }
    var hostsAtAttempt by remember { mutableStateOf<List<PairedHostRecord>?>(null) }
    val pairing = connection is ControllerConnectionState.Pairing
    DisposableEffect(viewModel) {
        viewModel.startDiscovery()
        onDispose { viewModel.stopDiscovery() }
    }
    // A saved record is the only proof pairing finished; the connection state moves on to the
    // first fleet refresh, which can fail for unrelated reasons.
    LaunchedEffect(hosts, hostsAtAttempt) {
        val before = hostsAtAttempt ?: return@LaunchedEffect
        if (hosts != before) {
            hostsAtAttempt = null
            onComplete()
        }
    }
    LaunchedEffect(connection, hostsAtAttempt) {
        if (connection is ControllerConnectionState.Failed) hostsAtAttempt = null
    }
    val selected = target
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(
                stringResource(
                    if (selected == null) com.multiplex.mobile.R.string.pair_computer
                    else com.multiplex.mobile.R.string.enter_pairing_code,
                ),
            )
        },
        text = {
            Column(
                Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                if (selected == null) {
                    Text(stringResource(com.multiplex.mobile.R.string.pair_computer_hint))
                    if (!enteringAddress) {
                        Text(
                            stringResource(com.multiplex.mobile.R.string.discovered_computers),
                            style = MaterialTheme.typography.titleSmall,
                            fontWeight = FontWeight.SemiBold,
                        )
                        if (computers.isEmpty()) {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                                Text(
                                    stringResource(com.multiplex.mobile.R.string.searching_computers),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        } else {
                            computers.forEach { computer ->
                                DiscoveredComputerRow(computer) {
                                    viewModel.pairingCode.value = ""
                                    attempted = false
                                    target = CodePairingTarget.Discovered(computer)
                                }
                            }
                        }
                        OutlinedButton(
                            onClick = { enteringAddress = true },
                            modifier = Modifier.fillMaxWidth(),
                        ) { Text(stringResource(com.multiplex.mobile.R.string.enter_address)) }
                    } else {
                        OutlinedTextField(
                            value = address,
                            onValueChange = {
                                viewModel.pairingAddress.value = it.take(300)
                                addressInvalid = false
                            },
                            label = { Text(stringResource(com.multiplex.mobile.R.string.computer_address)) },
                            singleLine = true,
                            isError = addressInvalid,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                            modifier = Modifier.fillMaxWidth(),
                        )
                        Text(
                            stringResource(com.multiplex.mobile.R.string.computer_address_hint),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (addressInvalid) {
                            Text(
                                stringResource(com.multiplex.mobile.R.string.pairing_address_invalid),
                                color = MaterialTheme.colorScheme.error,
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                    TextButton(onClick = onOtherWays) {
                        Text(stringResource(com.multiplex.mobile.R.string.other_ways_to_pair))
                    }
                } else {
                    Text(
                        if (selected is CodePairingTarget.Discovered) {
                            stringResource(
                                com.multiplex.mobile.R.string.pairing_code_hint,
                                isolated(selected.computer.serviceName),
                            )
                        } else {
                            stringResource(com.multiplex.mobile.R.string.pairing_code_hint_address)
                        },
                    )
                    OutlinedTextField(
                        value = code,
                        // Pasted codes often carry spaces or a trailing newline.
                        onValueChange = { viewModel.pairingCode.value = it.filter { char -> char in '0'..'9' }.take(6) },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.pairing_code)) },
                        singleLine = true,
                        enabled = !pairing,
                        textStyle = MaterialTheme.typography.headlineSmall.copy(
                            fontFamily = FontFamily.Monospace,
                            letterSpacing = 6.sp,
                        ),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = deviceName,
                        onValueChange = { viewModel.pairingDeviceName.value = it.take(64) },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.this_device)) },
                        singleLine = true,
                        enabled = !pairing,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    if (pairing) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                            Text(stringResource(com.multiplex.mobile.R.string.state_pairing))
                        }
                    }
                    if (attempted && connection is ControllerConnectionState.Failed) {
                        Text(
                            codePairingError(connection.code),
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                }
            }
        },
        confirmButton = {
            if (selected != null) {
                Button(
                    onClick = {
                        attempted = true
                        hostsAtAttempt = hosts
                        viewModel.pairWithCode(selected)
                    },
                    enabled = code.length == 6 && deviceName.isNotBlank() && !pairing,
                ) { Text(stringResource(com.multiplex.mobile.R.string.pair_action)) }
            } else if (enteringAddress) {
                Button(
                    onClick = {
                        if (runCatching { ControllerNetworkAddresses.parseEndpoint(address) }.isSuccess) {
                            viewModel.pairingCode.value = ""
                            attempted = false
                            target = CodePairingTarget.Address(address.trim())
                        } else {
                            addressInvalid = true
                        }
                    },
                    enabled = address.isNotBlank(),
                ) { Text(stringResource(com.multiplex.mobile.R.string.continue_action)) }
            }
        },
        dismissButton = {
            TextButton(onClick = {
                when {
                    selected != null -> {
                        if (pairing) viewModel.cancelPairing()
                        hostsAtAttempt = null
                        attempted = false
                        target = null
                    }
                    enteringAddress -> {
                        enteringAddress = false
                        addressInvalid = false
                    }
                    else -> onDismiss()
                }
            }) {
                Text(
                    stringResource(
                        if (selected == null && !enteringAddress) com.multiplex.mobile.R.string.cancel
                        else com.multiplex.mobile.R.string.back,
                    ),
                )
            }
        },
    )
}

@Composable
private fun DiscoveredComputerRow(computer: DiscoveredController, onClick: () -> Unit) {
    val description = stringResource(
        com.multiplex.mobile.R.string.pair_discovered_computer_accessibility,
        isolated(computer.serviceName),
    )
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .semantics { contentDescription = description },
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
    ) {
        Column(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 14.dp, vertical = 10.dp)) {
            Text(
                isolated(computer.serviceName),
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                isolated(computer.routes.first().address),
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
private fun codePairingError(code: String): String = when (code) {
    "pairing_code_rejected" -> stringResource(com.multiplex.mobile.R.string.pairing_code_rejected)
    "pairing_address_invalid" -> stringResource(com.multiplex.mobile.R.string.pairing_address_invalid)
    "pairing_address_unresolved" -> stringResource(com.multiplex.mobile.R.string.pairing_address_unresolved)
    "pairing_address_not_private" -> stringResource(com.multiplex.mobile.R.string.pairing_address_not_private)
    "offline", "timeout" -> stringResource(com.multiplex.mobile.R.string.pairing_unreachable)
    else -> stringResource(com.multiplex.mobile.R.string.pairing_failed, code)
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
        stringResource(com.multiplex.mobile.R.string.security_code_accessibility, it)
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (sas == null) stringResource(com.multiplex.mobile.R.string.pair_host) else stringResource(com.multiplex.mobile.R.string.compare_security_code)) },
        text = {
            if (sas == null) {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (connection is ControllerConnectionState.Failed && offer.isBlank()) {
                        Text(
                            stringResource(com.multiplex.mobile.R.string.pairing_new_offer_required),
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                    Text(stringResource(com.multiplex.mobile.R.string.pairing_offer_hint))
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        OutlinedButton(
                            onClick = onScan,
                            modifier = Modifier.weight(1f),
                            contentPadding = PaddingValues(horizontal = 12.dp, vertical = 8.dp),
                        ) {
                            Text(stringResource(com.multiplex.mobile.R.string.scan_qr_code))
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
                            Text(stringResource(com.multiplex.mobile.R.string.paste_offer))
                        }
                    }
                    OutlinedTextField(
                        value = offer,
                        onValueChange = { if (it.toByteArray().size <= 4 * 1_024) viewModel.pairingOffer.value = it },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.pairing_offer)) },
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
                                        contentDescription = stringResource(com.multiplex.mobile.R.string.clear),
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
                                if (error == PairingOfferPasteError.TooLarge) com.multiplex.mobile.R.string.pairing_offer_too_large
                                else com.multiplex.mobile.R.string.pairing_clipboard_empty,
                            ),
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    if (offer.isNotBlank()) {
                        Text(
                            stringResource(com.multiplex.mobile.R.string.pairing_offer_ready),
                            color = terminalSlateColor(SlateTokens.colorStatusDone(TerminalSlateTheme)),
                            style = MaterialTheme.typography.labelMedium,
                        )
                    }
                    OutlinedTextField(
                        value = hostName,
                        onValueChange = { viewModel.pairingHostName.value = it.take(256) },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.host_name)) },
                        singleLine = true,
                    )
                    OutlinedTextField(
                        value = deviceName,
                        onValueChange = { viewModel.pairingDeviceName.value = it.take(64) },
                        label = { Text(stringResource(com.multiplex.mobile.R.string.this_device)) },
                        singleLine = true,
                    )
                    if (connection is ControllerConnectionState.Failed) {
                        Text(stringResource(com.multiplex.mobile.R.string.pairing_failed, connection.code), color = MaterialTheme.colorScheme.error)
                    }
                }
            } else {
                Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                    Text(stringResource(com.multiplex.mobile.R.string.confirm_security_code))
                    Text(
                        sas.sas,
                        style = MaterialTheme.typography.headlineMedium,
                        fontFamily = FontFamily.Monospace,
                        fontWeight = FontWeight.Bold,
                        modifier = Modifier.semantics {
                            contentDescription = requireNotNull(sasDescription)
                        },
                    )
                    Text(stringResource(com.multiplex.mobile.R.string.fingerprint_ending, sas.fingerprintSuffix))
                }
            }
        },
        confirmButton = {
            if (sas == null) {
                Button(
                    onClick = viewModel::beginPairing,
                    enabled = offer.isNotBlank() && hostName.isNotBlank() && deviceName.isNotBlank() &&
                        connection !is ControllerConnectionState.Pairing,
                ) { Text(stringResource(com.multiplex.mobile.R.string.continue_action)) }
            } else {
                Button(onClick = {
                    retainedSas = sas
                    awaitingCompletion = true
                    viewModel.finishPairing(true)
                }, enabled = connection !is ControllerConnectionState.Pairing) {
                    Text(stringResource(com.multiplex.mobile.R.string.codes_match))
                }
            }
        },
        dismissButton = {
            TextButton(onClick = {
                if (sas != null) viewModel.finishPairing(false)
                onDismiss()
            }) { Text(if (sas == null) stringResource(com.multiplex.mobile.R.string.cancel) else stringResource(com.multiplex.mobile.R.string.reject)) }
        },
    )
}

@Composable
private fun HostDetailsDialog(
    host: PairedHostRecord,
    state: ControllerUiState,
    onDismiss: () -> Unit,
    onReconnect: () -> Unit,
    onForget: () -> Unit,
    onSelectRoute: (ControllerRemoteRouteKind) -> Unit,
    onConfigureSsh: () -> Unit,
    onConfigureRelay: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(isolated(host.displayName)) },
        text = {
            Column(
                verticalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.verticalScroll(rememberScrollState()),
            ) {
                // How this phone reaches the computer belongs with the rest of what is known
                // about it, not across the top of the page it is used from.
                ControllerRouteSelector(state, onSelectRoute, onConfigureSsh, onConfigureRelay)
                host.routes.forEach { route ->
                    Text(stringResource(com.multiplex.mobile.R.string.route_value, isolated(route.address), route.port))
                }
                Text(stringResource(com.multiplex.mobile.R.string.fingerprint_ending, host.id.takeLast(12)), fontFamily = FontFamily.Monospace)
                Text(stringResource(com.multiplex.mobile.R.string.capabilities_value, capabilityLabels(host.capabilityBits).joinToString()))
                Text(stringResource(com.multiplex.mobile.R.string.forget_not_revoke))
            }
        },
        confirmButton = { Button(onClick = onReconnect) { Text(stringResource(com.multiplex.mobile.R.string.reconnect)) } },
        dismissButton = {
            Row {
                TextButton(onClick = onForget) { Text(stringResource(com.multiplex.mobile.R.string.forget)) }
                TextButton(onClick = onDismiss) { Text(stringResource(com.multiplex.mobile.R.string.close)) }
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
    ControllerConnectionState.Unpaired -> stringResource(com.multiplex.mobile.R.string.state_not_paired)
    ControllerConnectionState.Pairing -> stringResource(com.multiplex.mobile.R.string.state_pairing)
    is ControllerConnectionState.SasReady -> stringResource(com.multiplex.mobile.R.string.state_waiting_code)
    ControllerConnectionState.PairedOffline -> stringResource(com.multiplex.mobile.R.string.state_host_offline)
    ControllerConnectionState.Connecting -> stringResource(com.multiplex.mobile.R.string.state_connecting)
    ControllerConnectionState.Authenticating -> stringResource(com.multiplex.mobile.R.string.state_authenticating)
    ControllerConnectionState.Syncing -> stringResource(com.multiplex.mobile.R.string.state_syncing)
    ControllerConnectionState.ReadyReadOnly -> stringResource(com.multiplex.mobile.R.string.state_live_read_only)
    ControllerConnectionState.Revoked -> stringResource(com.multiplex.mobile.R.string.state_device_revoked)
    ControllerConnectionState.Incompatible -> stringResource(com.multiplex.mobile.R.string.state_incompatible)
    is ControllerConnectionState.Failed -> stringResource(com.multiplex.mobile.R.string.state_connection_failed, state.code)
}

/**
 * What a computer has granted this phone, in its own words.
 *
 * All eight bits, not the first five: a computer that shares its screen says so here, which is
 * the only place a person can see whether watching or control was granted.
 */
@Composable
private fun capabilityLabels(bits: Int): List<String> {
    val labels = listOf(
        ControllerConnection.OBSERVE_CAPABILITY to com.multiplex.mobile.R.string.capability_fleet,
        ControllerConnection.ATTACH_CAPABILITY to com.multiplex.mobile.R.string.capability_output,
        ControllerConnection.INPUT_CAPABILITY to com.multiplex.mobile.R.string.capability_input,
        ControllerConnection.RESIZE_CAPABILITY to com.multiplex.mobile.R.string.capability_resize,
        ControllerConnection.APPROVAL_CAPABILITY to com.multiplex.mobile.R.string.capability_approvals,
        ControllerConnection.OBSERVE_SCREENS_CAPABILITY to
            com.multiplex.mobile.R.string.capability_watch_screen,
        ControllerConnection.CONTROL_POINTER_CAPABILITY to
            com.multiplex.mobile.R.string.capability_control_pointer,
        ControllerConnection.CONTROL_KEYBOARD_CAPABILITY to
            com.multiplex.mobile.R.string.capability_control_keyboard,
    ).filter { (bit, _) -> bits and bit != 0 }.map { (_, label) -> stringResource(label) }
    return labels.ifEmpty { listOf(stringResource(com.multiplex.mobile.R.string.capability_none)) }
}

/**
 * What a session's state is called, rather than the name it has on the wire.
 *
 * The chip showed the raw value, so a phone said `running_app_attached` at a person. Anything
 * this build does not know reads as Unknown instead of leaking a protocol string. Kept apart
 * from the drawing so the mapping can be checked without a device.
 */
/**
 * How fresh the list on screen is, or nothing when there is no snapshot to date.
 *
 * Cached data said so; live data said nothing at all, which reads the same as a stale list on a
 * phone that has been in a pocket.
 */
internal fun freshnessLabelResource(state: ControllerUiState): Int? = when {
    state.cachedAtMillis == null -> null
    state.cachedReadOnly -> com.multiplex.mobile.R.string.cached_updated
    state.connection == ControllerConnectionState.ReadyReadOnly ->
        com.multiplex.mobile.R.string.live_updated
    else -> com.multiplex.mobile.R.string.cached_updated
}

internal fun lifecycleLabelResource(lifecycle: String): Int = when (lifecycle) {
    "draft" -> com.multiplex.mobile.R.string.lifecycle_draft
    "validating" -> com.multiplex.mobile.R.string.lifecycle_validating
    "starting" -> com.multiplex.mobile.R.string.lifecycle_starting
    "provisioning" -> com.multiplex.mobile.R.string.lifecycle_provisioning
    "attaching" -> com.multiplex.mobile.R.string.lifecycle_attaching
    "replaying" -> com.multiplex.mobile.R.string.lifecycle_replaying
    "live", "running", "running_app_attached" -> com.multiplex.mobile.R.string.lifecycle_live
    "recording_paused" -> com.multiplex.mobile.R.string.lifecycle_recording_paused
    "stopping" -> com.multiplex.mobile.R.string.lifecycle_stopping
    "offline" -> com.multiplex.mobile.R.string.lifecycle_offline
    "orphaned" -> com.multiplex.mobile.R.string.lifecycle_orphaned
    "gap" -> com.multiplex.mobile.R.string.lifecycle_gap
    "permission_denied" -> com.multiplex.mobile.R.string.lifecycle_permission_denied
    "incompatible" -> com.multiplex.mobile.R.string.lifecycle_incompatible
    "failed" -> com.multiplex.mobile.R.string.lifecycle_failed
    "cancelled" -> com.multiplex.mobile.R.string.lifecycle_cancelled
    "exited", "stopped" -> com.multiplex.mobile.R.string.lifecycle_exited
    else -> com.multiplex.mobile.R.string.lifecycle_unknown
}

/**
 * What a session is doing, in words. `activity` is a protocol code, and a code this build does not
 * know — including the "unknown" every session starts as — reads as no activity rather than
 * reaching the screen as itself. A row used to say "unknown".
 */
internal fun activityLabelResource(activity: String?): Int = when (activity) {
    "idle" -> com.multiplex.mobile.R.string.activity_idle
    "busy" -> com.multiplex.mobile.R.string.activity_busy
    "needs_input" -> com.multiplex.mobile.R.string.activity_needs_input
    "done" -> com.multiplex.mobile.R.string.activity_done
    "failed" -> com.multiplex.mobile.R.string.activity_failed
    else -> com.multiplex.mobile.R.string.no_recent_activity
}

@Composable
private fun lifecycleLabel(lifecycle: String): String =
    stringResource(lifecycleLabelResource(lifecycle))

private fun isolated(value: String): String = "\u2068$value\u2069"

@Composable
private fun relativeTime(millis: Long?): String {
    if (millis == null) return stringResource(com.multiplex.mobile.R.string.time_unknown)
    val seconds = ((System.currentTimeMillis() - millis).coerceAtLeast(0)) / 1_000
    return when {
        seconds < 60 -> stringResource(com.multiplex.mobile.R.string.time_just_now)
        seconds < 3_600 -> stringResource(com.multiplex.mobile.R.string.time_minutes, seconds / 60)
        seconds < 86_400 -> stringResource(com.multiplex.mobile.R.string.time_hours, seconds / 3_600)
        else -> stringResource(com.multiplex.mobile.R.string.time_days, seconds / 86_400)
    }
}
