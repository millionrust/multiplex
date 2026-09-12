package com.termirust.mobile.replication

import android.os.CancellationSignal
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.FileDownload
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.compose.viewModel
import com.termirust.mobile.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@Composable
fun HostImportScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    val repository = remember(context) { NativeEnrollmentRepository(context.applicationContext) }
    HostImportScreen(onBack, repository)
}

@Composable
internal fun HostImportScreen(onBack: () -> Unit, repository: HostImportRepository) {
    val context = LocalContext.current
    val factory = remember(repository) { object : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T =
            HostImportViewModel(repository) as T
    } }
    val model: HostImportViewModel = viewModel(factory = factory)
    val state by model.state.collectAsState()
    val scope = rememberCoroutineScope()
    var signal by remember { mutableStateOf<CancellationSignal?>(null) }
    var choosing by remember { mutableStateOf(false) }
    var readFailed by remember { mutableStateOf(false) }
    DisposableEffect(model) { onDispose { signal?.cancel(); model.discard() } }
    LaunchedEffect(model) { model.reload() }
    BackHandler(onBack = onBack)
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) choosing = false
        else {
            val cancellation = CancellationSignal()
            signal = cancellation
            scope.launch {
                try {
                    val bytes = withContext(Dispatchers.IO) {
                        ReplicationDocumentReader(context.contentResolver).read(uri, ReplicationTransferKind.ENCRYPTED_REPLICA, cancellation)
                    }
                    coroutineContext.ensureActive()
                    model.preview(bytes)
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { readFailed = true }
                finally { cancellation.cancel(); signal = null; choosing = false }
            }
        }
    }
    HostImportContent(state.copy(loading = state.loading || choosing), onBack,
        onReload = { readFailed = false; model.reload() },
        onChoose = {
            model.discard(); readFailed = false; choosing = true
            picker.launch(arrayOf("application/json", "application/octet-stream"))
        }, onSelect = model::select, onDiscard = model::discard, onImport = model::apply,
        readFailed = readFailed)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HostImportContent(
    state: HostImportState, onBack: () -> Unit, onReload: () -> Unit,
    onChoose: () -> Unit, onSelect: (String) -> Unit, onDiscard: () -> Unit,
    onImport: () -> Unit, readFailed: Boolean = false,
) {
    Scaffold(topBar = {
        TopAppBar(title = { Text(stringResource(R.string.host_import_title)) }, navigationIcon = {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.enrollment_back)) }
        }, actions = {
            IconButton(onClick = onReload, enabled = !state.loading) { Icon(Icons.Outlined.Refresh, stringResource(R.string.enrollment_reload)) }
            IconButton(onClick = onChoose, enabled = !state.loading && state.problem == null) {
                Icon(Icons.Outlined.FileDownload, stringResource(R.string.host_import_choose))
            }
        })
    }) { padding ->
        Box(Modifier.fillMaxSize().padding(padding), contentAlignment = androidx.compose.ui.Alignment.TopCenter) {
            LazyColumn(Modifier.widthIn(max = 640.dp).fillMaxSize(), contentPadding = PaddingValues(24.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (state.loading) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
                if (readFailed) item { Text(stringResource(R.string.host_import_read_failed), color = MaterialTheme.colorScheme.error) }
                state.problem?.let { problem -> item {
                    Text(stringResource(when (problem) {
                        HostImportProblem.LOCKED -> R.string.enrollment_locked
                        HostImportProblem.MISSING_KEY -> R.string.host_import_missing_key
                        HostImportProblem.CHANGED -> R.string.host_import_changed
                        HostImportProblem.RECOVERY -> R.string.enrollment_recovery
                        HostImportProblem.INVALID -> R.string.host_import_invalid
                        HostImportProblem.UNAVAILABLE -> R.string.enrollment_unavailable
                    }), color = MaterialTheme.colorScheme.error)
                } }
                item { Text(stringResource(if (state.candidates != null) R.string.host_import_candidates else R.string.host_import_title), style = MaterialTheme.typography.titleLarge) }
                val rows = state.candidates ?: state.hosts
                if (rows.isEmpty() && !state.loading && state.problem == null) item {
                    Text(stringResource(if (state.candidates != null) R.string.host_import_no_candidates else R.string.host_import_empty))
                }
                items(rows, key = { it.recordId }) { host ->
                    ListItem(headlineContent = { Text(host.label) }, supportingContent = {
                        Text(stringResource(R.string.host_import_endpoint, host.username, host.host, host.port))
                    }, trailingContent = {
                        if (state.candidates != null) TextButton(onClick = { onSelect(host.recordId) }, enabled = !state.loading && state.problem == null) {
                            Text(stringResource(R.string.host_import_review))
                        }
                    })
                }
                if (state.candidates != null) item {
                    TextButton(onClick = onDiscard, enabled = !state.loading) { Text(stringResource(R.string.host_import_cancel)) }
                }
            }
        }
    }
    state.review?.let { review ->
        AlertDialog(onDismissRequest = onDiscard,
            title = { Text(stringResource(R.string.host_import_review_title)) },
            text = { Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(review.host.label)
                Text(stringResource(R.string.host_import_endpoint, review.host.username, review.host.host, review.host.port))
                Text(stringResource(if (review.changesLocal) R.string.host_import_local_only else R.string.host_import_unchanged))
            } },
            confirmButton = { TextButton(onClick = onImport, enabled = !state.loading && state.problem == null) { Text(stringResource(R.string.host_import_confirm)) } },
            dismissButton = { TextButton(onClick = onDiscard, enabled = !state.loading) { Text(stringResource(R.string.host_import_cancel)) } })
    }
}
