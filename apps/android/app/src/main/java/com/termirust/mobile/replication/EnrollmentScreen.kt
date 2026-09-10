package com.termirust.mobile.replication

import android.content.Context
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.FileUpload
import androidx.compose.material.icons.outlined.FileDownload
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.compose.viewModel
import com.termirust.mobile.R
import kotlinx.coroutines.launch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.ensureActive

@Composable
fun EnrollmentScreen(onBack: () -> Unit, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val repository = remember(context) { NativeEnrollmentRepository(context.applicationContext) }
    EnrollmentScreen(onBack, modifier, repository)
}

@Composable
internal fun EnrollmentScreen(onBack: () -> Unit, modifier: Modifier, repository: NativeEnrollmentRepository) {
    var showHosts by rememberSaveable { mutableStateOf(false) }
    if (showHosts) {
        HostImportScreen(onBack = { showHosts = false }, repository = repository)
        return
    }
    val context = LocalContext.current
    val factory = remember(repository) { object : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T =
            EnrollmentViewModel(repository) as T
    } }
    val model: EnrollmentViewModel = viewModel(factory = factory)
    val state by model.state.collectAsState()
    var exportBytes by rememberSaveable { mutableStateOf<ByteArray?>(null) }
    var exporting by remember { mutableStateOf(false) }
    var notice by remember { mutableStateOf<Int?>(null) }
    val scope = rememberCoroutineScope()
    val transfer = remember(context) { EnrollmentTransfer(context) }
    var reading by remember { mutableStateOf(false) }
    var readSignal by remember { mutableStateOf<android.os.CancellationSignal?>(null) }
    DisposableEffect(model) {
        onDispose { readSignal?.cancel(); model.discardBundleReview() }
    }
    val importBundle = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null && !reading) {
            val signal = android.os.CancellationSignal()
            readSignal = signal
            reading = true
            notice = null
            scope.launch {
                try {
                    val bytes = withContext(Dispatchers.IO) {
                        ReplicationDocumentReader(context.contentResolver).read(uri, ReplicationTransferKind.ENROLLMENT_BUNDLE, signal)
                    }
                    coroutineContext.ensureActive()
                    model.reviewBundle(bytes)
                } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
                catch (_: Exception) { notice = R.string.enrollment_bundle_read_failed }
                finally { signal.cancel(); readSignal = null; reading = false }
            }
        }
    }
    val label = stringResource(R.string.enrollment_title)
    val credentials = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { model.reload() }
    val export = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/json")) { uri ->
        val bytes = exportBytes
        exportBytes = null
        if (uri != null && bytes != null) {
            exporting = true
            scope.launch {
            notice = try {
                transfer.export(uri, bytes)
                R.string.enrollment_exported
            } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
            catch (_: Exception) { R.string.enrollment_export_failed }
            finally { exporting = false }
            }
        }
    }
    LaunchedEffect(Unit) { model.reload() }
    BackHandler { onBack() }
    EnrollmentContent(state.copy(loading = state.loading || exporting || exportBytes != null || reading), onBack, model::prepare, model::reload, model::cancel,
        onCopy = { bytes ->
            notice = try {
                transfer.copy(bytes)
                R.string.enrollment_copied
            } catch (_: Exception) { R.string.enrollment_export_failed }
        },
        onExport = { bytes -> exportBytes = bytes.copyOf(); export.launch("termirust-enrollment.json") },
        onCredentials = {
            val keyguard = context.getSystemService(Context.KEYGUARD_SERVICE) as android.app.KeyguardManager
            @Suppress("DEPRECATION")
            val intent = keyguard.createConfirmDeviceCredentialIntent(label, null)
                ?: android.content.Intent(android.provider.Settings.ACTION_SECURITY_SETTINGS)
            credentials.launch(intent)
        }, notice = notice, modifier = modifier,
        onSelectBundle = { model.discardBundleReview(); importBundle.launch(arrayOf("application/json", "application/octet-stream")) },
        onDiscardBundle = model::discardBundleReview, onAcceptBundle = model::acceptBundle,
        onOpenHosts = { showHosts = true }, onRecover = model::recover)
}

@OptIn(ExperimentalMaterial3Api::class, androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
fun EnrollmentContent(
    state: EnrollmentState,
    onBack: () -> Unit,
    onPrepare: () -> Unit,
    onReload: () -> Unit,
    onCancel: (ByteArray) -> Unit,
    onCopy: (ByteArray) -> Unit,
    onExport: (ByteArray) -> Unit,
    modifier: Modifier = Modifier,
    onCredentials: () -> Unit = {},
    notice: Int? = null,
    onSelectBundle: () -> Unit = {},
    onDiscardBundle: () -> Unit = {},
    onAcceptBundle: (String) -> Unit = {},
    onOpenHosts: () -> Unit = {},
    onRecover: () -> Unit = {},
) {
    var reviewed by remember { mutableStateOf<ByteArray?>(null) }
    var reviewRecovery by remember { mutableStateOf(false) }
    val canRecover = state.problem == EnrollmentProblem.RECOVERY && !state.loading && !state.snapshot.cancellationPending
    LaunchedEffect(canRecover) { if (!canRecover) reviewRecovery = false }
    val request = state.snapshot.request
    val ready = request != null && !state.loading && state.problem == null && !state.snapshot.cancellationPending
    Scaffold(modifier.fillMaxSize(), topBar = {
        TopAppBar(title = { Text(stringResource(R.string.enrollment_title)) }, navigationIcon = {
            IconButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.enrollment_back))
            }
        }, actions = {
            IconButton(onClick = onReload, enabled = !state.loading) {
                Icon(Icons.Outlined.Refresh, stringResource(R.string.enrollment_reload))
            }
        })
    }) { padding ->
        Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.TopCenter) {
            Column(Modifier.widthIn(max = 640.dp).fillMaxWidth().verticalScroll(rememberScrollState())
                .padding(24.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                Text(stringResource(when {
                    state.loading -> R.string.enrollment_working
                    state.snapshot.configured -> R.string.enrollment_configured_title
                    state.snapshot.cancellationPending -> R.string.enrollment_cancel_pending
                    state.problem != null -> R.string.enrollment_unknown
                    request != null -> R.string.enrollment_pending
                    else -> R.string.enrollment_not_prepared
                }), style = MaterialTheme.typography.titleLarge)
                if (state.problem != EnrollmentProblem.CONFIGURED && !state.snapshot.configured) Text(stringResource(R.string.enrollment_not_enrolled), style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (state.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
                state.problem?.let { problem ->
                    Text(stringResource(when (problem) {
                        EnrollmentProblem.LOCKED -> R.string.enrollment_locked
                        EnrollmentProblem.BUSY -> R.string.enrollment_busy
                        EnrollmentProblem.INVALID -> R.string.enrollment_invalid
                        EnrollmentProblem.CHANGED -> R.string.enrollment_changed
                        EnrollmentProblem.RECOVERY -> R.string.enrollment_recovery
                        EnrollmentProblem.CONFIGURED -> R.string.enrollment_configured
                        EnrollmentProblem.UNAVAILABLE -> R.string.enrollment_unavailable
                    }), color = MaterialTheme.colorScheme.error)
                }
                if (state.problem == EnrollmentProblem.LOCKED) {
                    OutlinedButton(onClick = onCredentials, enabled = !state.loading, modifier = Modifier.fillMaxWidth()) {
                        Text(stringResource(R.string.enrollment_credentials))
                    }
                }
                if (canRecover) {
                    OutlinedButton(onClick = { reviewRecovery = true }, modifier = Modifier.fillMaxWidth()) {
                        Icon(Icons.Outlined.Refresh, null); Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.enrollment_review_recovery))
                    }
                }
                if (state.snapshot.configured) {
                    Text(stringResource(R.string.enrollment_configured_detail), style = MaterialTheme.typography.bodyMedium)
                    OutlinedButton(onClick = onOpenHosts, enabled = !state.loading && state.problem == null, modifier = Modifier.fillMaxWidth()) {
                        Icon(Icons.Outlined.FileDownload, null); Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.host_import_title))
                    }
                } else if (request == null) {
                    Button(onClick = onPrepare, enabled = !state.loading && state.problem == null,
                        modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.enrollment_prepare)) }
                } else {
                    OutlinedButton(onClick = { onCopy(request.copyOf()) }, enabled = ready, modifier = Modifier.fillMaxWidth()) {
                        Icon(Icons.Outlined.ContentCopy, null); Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.enrollment_copy))
                    }
                    OutlinedButton(onClick = { onExport(request.copyOf()) }, enabled = ready, modifier = Modifier.fillMaxWidth()) {
                        Icon(Icons.Outlined.FileUpload, null); Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.enrollment_export))
                    }
                    OutlinedButton(onClick = onSelectBundle, enabled = ready, modifier = Modifier.fillMaxWidth()) {
                        Icon(Icons.Outlined.FileDownload, null); Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.enrollment_select_bundle))
                    }
                    TextButton(onClick = { reviewed = request.copyOf() }, enabled = !state.loading,
                        modifier = Modifier.fillMaxWidth()) {
                        Text(stringResource(if (state.snapshot.cancellationPending) R.string.enrollment_retry_cancel else R.string.enrollment_cancel),
                            color = MaterialTheme.colorScheme.error)
                    }
                }
                notice?.let { Text(stringResource(it), style = MaterialTheme.typography.bodyMedium) }
            }
        }
    }
    if (reviewRecovery && canRecover) {
        AlertDialog(onDismissRequest = { reviewRecovery = false },
            title = { Text(stringResource(R.string.enrollment_review_recovery)) },
            text = { Text(stringResource(R.string.enrollment_recovery_detail)) },
            confirmButton = { TextButton(onClick = { reviewRecovery = false; onRecover() }) {
                Text(stringResource(R.string.enrollment_confirm_recovery))
            } },
            dismissButton = { TextButton(onClick = { reviewRecovery = false }) {
                Text(stringResource(R.string.enrollment_bundle_back))
            } })
    }
    state.bundleReview?.let { bundle ->
        var code by remember(bundle) { mutableStateOf("") }
        AlertDialog(onDismissRequest = onDiscardBundle,
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            text = {
                val bringIntoView = remember { BringIntoViewRequester() }
                var focused by remember { mutableStateOf(false) }
                val keyboardBottom = WindowInsets.ime.getBottom(LocalDensity.current)
                LaunchedEffect(keyboardBottom, focused) {
                    if (focused) {
                        withFrameNanos { }
                        bringIntoView.bringIntoView()
                    }
                }
                Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(stringResource(R.string.enrollment_bundle_title),
                        style = MaterialTheme.typography.headlineSmall,
                        modifier = Modifier.semantics { heading() })
                    Text(stringResource(R.string.enrollment_bundle_workspace, bundle.workspace))
                    Text(stringResource(R.string.enrollment_bundle_recipient, bundle.recipient))
                    Text(stringResource(R.string.enrollment_bundle_code, bundle.code))
                    OutlinedTextField(value = code, onValueChange = { if (it.length <= 13) code = it },
                        label = { Text(stringResource(R.string.enrollment_desktop_code)) }, singleLine = true,
                        modifier = Modifier.fillMaxWidth().bringIntoViewRequester(bringIntoView)
                            .onFocusChanged { focused = it.isFocused })
                }
            },
            confirmButton = { TextButton(onClick = { onAcceptBundle(code) }, enabled = !state.loading && code == bundle.code) {
                Text(stringResource(R.string.enrollment_accept_bundle))
            } },
            dismissButton = { TextButton(onClick = onDiscardBundle) { Text(stringResource(R.string.enrollment_bundle_back)) } })
    }
    reviewed?.let { bytes ->
        AlertDialog(onDismissRequest = { reviewed = null },
            title = { Text(stringResource(R.string.enrollment_cancel_title)) },
            text = { Text(stringResource(R.string.enrollment_cancel_detail)) },
            confirmButton = { TextButton(onClick = { reviewed = null; onCancel(bytes) }) {
                Text(stringResource(R.string.enrollment_confirm_cancel))
            } },
            dismissButton = { TextButton(onClick = { reviewed = null }) { Text(stringResource(R.string.enrollment_keep)) } })
    }
}
