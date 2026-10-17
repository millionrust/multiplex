package com.multiplex.mobile.controller

import android.text.format.DateUtils
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.outlined.AccountTree
import androidx.compose.material.icons.outlined.CallMerge
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Cloud
import androidx.compose.material.icons.outlined.Commit
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.multiplex.mobile.R

/** A diff to show: a commit's, or the uncommitted changes for a null [commit], narrowed to [path]. */
data class GitRoute(val commit: String?, val path: String?, val title: String)

/**
 * The repository the attached terminal's shell is in, over the terminal like a sheet, so the
 * terminal stays attached underneath. Back leaves a diff before it leaves the page.
 */
@Composable
fun GitDialog(model: ControllerGitModel, onClose: () -> Unit) {
    val routes = remember { mutableStateListOf<GitRoute>() }
    Dialog(
        onDismissRequest = { if (routes.isNotEmpty()) routes.removeAt(routes.lastIndex) else onClose() },
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            val route = routes.lastOrNull()
            if (route == null) {
                GitScreen(model, onClose = onClose, onOpen = { routes.add(it) })
            } else {
                GitDiffScreen(
                    model = model,
                    route = route,
                    onBack = { routes.removeAt(routes.lastIndex) },
                    onOpenFile = { file -> routes.add(GitRoute(route.commit, file.path, file.path)) },
                )
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun GitScreen(model: ControllerGitModel, onClose: () -> Unit, onOpen: (GitRoute) -> Unit) {
    val state by model.state.collectAsState()
    var pendingSwitch by remember { mutableStateOf<GitBranch?>(null) }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
        TopAppBar(
            title = {
                Text(
                    state.repository?.branch ?: stringResource(R.string.git_title),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            },
            navigationIcon = {
                IconButton(onClick = onClose) {
                    Icon(Icons.Outlined.Close, contentDescription = stringResource(R.string.git_close))
                }
            },
            actions = {
                IconButton(onClick = model::load) {
                    Icon(Icons.Outlined.Refresh, contentDescription = stringResource(R.string.git_refresh))
                }
            },
            colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
        )
        val repository = state.repository
        when {
            repository != null -> GitRepositoryPage(state, repository, model, onOpen) { pendingSwitch = it }
            state.load == GitLoadState.NotRepository -> GitMessage(
                title = stringResource(R.string.git_not_repository),
                body = stringResource(R.string.git_not_repository_body),
            )
            state.load is GitLoadState.Failed -> GitMessage(
                title = stringResource(R.string.git_unavailable),
                body = gitErrorText((state.load as GitLoadState.Failed).code),
                onRetry = model::load,
            )
            else -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        }
    }
    pendingSwitch?.let { branch ->
        AlertDialog(
            onDismissRequest = { pendingSwitch = null },
            title = { Text(stringResource(R.string.git_switch_to, branch.localName)) },
            text = { Text(stringResource(R.string.git_switch_body)) },
            confirmButton = {
                TextButton(onClick = { pendingSwitch = null; model.switchTo(branch) }) {
                    Text(stringResource(R.string.git_switch_branch))
                }
            },
            dismissButton = {
                TextButton(onClick = { pendingSwitch = null }) { Text(stringResource(R.string.cancel)) }
            },
        )
    }
}

@Composable
private fun GitRepositoryPage(
    state: GitUiState,
    repository: GitRepository,
    model: ControllerGitModel,
    onOpen: (GitRoute) -> Unit,
    onSwitch: (GitBranch) -> Unit,
) {
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
        item { GitHeader(repository, otherWorktree = state.worktree != null) }
        item {
            val sections = listOf(
                GitSection.CHANGES to R.string.git_changes,
                GitSection.HISTORY to R.string.git_history,
                GitSection.BRANCHES to R.string.git_branches,
                GitSection.WORKTREES to R.string.git_worktrees,
            )
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
                sections.forEachIndexed { index, (section, label) ->
                    SegmentedButton(
                        selected = state.section == section,
                        onClick = { model.selectSection(section) },
                        shape = SegmentedButtonDefaults.itemShape(index, sections.size),
                        icon = {},
                        label = { Text(stringResource(label), maxLines = 1, fontSize = 12.sp) },
                    )
                }
            }
        }
        state.notice?.let { notice ->
            item {
                Text(
                    gitNoticeText(notice),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp)
                        .background(MaterialTheme.colorScheme.surfaceContainerHigh, RoundedCornerShape(10.dp))
                        .clickable(onClick = model::dismissNotice)
                        .padding(12.dp),
                )
            }
        }
        when (state.section) {
            GitSection.CHANGES -> changes(repository, onOpen)
            GitSection.HISTORY -> history(state, model, onOpen)
            GitSection.BRANCHES -> branches(state, repository, onSwitch)
            GitSection.WORKTREES -> worktrees(repository, model)
        }
        item { Spacer(Modifier.height(24.dp)) }
    }
}

@Composable
private fun GitHeader(repository: GitRepository, otherWorktree: Boolean) {
    Column(Modifier.fillMaxWidth().padding(top = 4.dp, bottom = 4.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(Icons.Outlined.AccountTree, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
            Text(
                repository.branch ?: stringResource(R.string.git_detached, repository.head ?: "—"),
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            if (repository.ahead > 0) GitBadge("↑${repository.ahead}", MaterialTheme.colorScheme.primary)
            if (repository.behind > 0) GitBadge("↓${repository.behind}", com.multiplex.mobile.ui.SlateExtras.attention)
            if (otherWorktree) GitBadge(stringResource(R.string.git_other_worktree), com.multiplex.mobile.ui.SlateExtras.attention)
        }
        Text(
            repository.upstream?.let { stringResource(R.string.git_tracking, it) } ?: stringResource(R.string.git_no_upstream),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            repository.root,
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

private fun LazyListScope.changes(repository: GitRepository, onOpen: (GitRoute) -> Unit) {
    item {
        ItemGroup {
            if (repository.changes.isEmpty()) {
                GroupItem(
                    title = stringResource(R.string.git_clean),
                    subtitle = stringResource(R.string.git_clean_body),
                ) { GitLead(Icons.Outlined.CheckCircle) }
            } else {
                val allTitle = stringResource(R.string.git_uncommitted)
                GroupItem(
                    title = stringResource(R.string.git_all_changes),
                    subtitle = stringResource(R.string.git_file_count, repository.changes.size),
                    onClick = { onOpen(GitRoute(null, null, allTitle)) },
                    trailing = { Chevron() },
                ) { GitLead(Icons.Outlined.Description, on = true) }
                repository.changes.forEach { change ->
                    GroupItem(
                        title = change.path.substringAfterLast('/'),
                        subtitle = changeMeta(change),
                        divider = true,
                        onClick = { onOpen(GitRoute(null, change.path, change.path)) },
                        trailing = {
                            GitBadge(
                                change.status.letter,
                                when {
                                    change.status == GitChangeStatus.CONFLICTED -> com.multiplex.mobile.ui.SlateExtras.attention
                                    change.staged -> com.multiplex.mobile.ui.SlateExtras.done
                                    else -> MaterialTheme.colorScheme.onSurfaceVariant
                                },
                            )
                        },
                    ) { GitLead(statusIcon(change.status)) }
                }
            }
        }
    }
    if (repository.changesTruncated) item { Footnote(stringResource(R.string.git_more_files)) }
}

private fun LazyListScope.history(state: GitUiState, model: ControllerGitModel, onOpen: (GitRoute) -> Unit) {
    if (state.commits.isEmpty() && !state.historyLoading) {
        item { Footnote(state.historyError?.let { gitErrorText(it) } ?: stringResource(R.string.git_no_commits)) }
        return
    }
    item {
        ItemGroup {
            state.commits.forEachIndexed { index, commit ->
                GroupItem(
                    title = commit.subject,
                    subtitle = "${commit.short} · ${commit.author} · ${relative(commit.unixTime)}",
                    divider = index > 0,
                    onClick = { onOpen(GitRoute(commit.oid, null, commit.short)) },
                    trailing = {
                        commit.refs.firstOrNull()?.let { GitBadge(it, MaterialTheme.colorScheme.primary) } ?: Chevron()
                    },
                ) { GitLead(if (commit.parents > 1) Icons.Outlined.CallMerge else Icons.Outlined.Commit, on = index == 0) }
            }
        }
    }
    item {
        when {
            state.historyLoading -> Box(Modifier.fillMaxWidth().padding(12.dp), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(24.dp))
            }
            state.historyError != null -> Footnote(gitErrorText(state.historyError))
            state.historyNextOffset != null -> {
                // The end of the list asks for the next page, as the iPhone does when it scrolls there.
                LaunchedEffect(state.commits.size) { model.loadMoreHistory() }
            }
        }
    }
}

private fun LazyListScope.branches(state: GitUiState, repository: GitRepository, onSwitch: (GitBranch) -> Unit) {
    branchGroup(R.string.git_local, repository.localBranches, state, onSwitch)
    if (repository.remoteBranches.isNotEmpty()) {
        branchGroup(R.string.git_remote, repository.remoteBranches, state, onSwitch)
    }
    if (repository.branchesTruncated) item { Footnote(stringResource(R.string.git_more_branches)) }
    if (!state.canSwitch) item { Footnote(stringResource(R.string.git_switch_needs_input)) }
}

private fun LazyListScope.branchGroup(
    label: Int,
    branches: List<GitBranch>,
    state: GitUiState,
    onSwitch: (GitBranch) -> Unit,
) {
    item { GroupLabel(stringResource(label)) }
    item {
        ItemGroup {
            branches.forEachIndexed { index, branch ->
                GroupItem(
                    title = branch.name,
                    subtitle = "${branch.commit} · ${branch.subject}",
                    divider = index > 0,
                    trailing = {
                        when {
                            branch.current -> GitBadge(stringResource(R.string.git_current), com.multiplex.mobile.ui.SlateExtras.done)
                            state.switchingBranch == branch.name -> CircularProgressIndicator(Modifier.size(20.dp))
                            state.canSwitch -> TextButton(
                                onClick = { onSwitch(branch) },
                                enabled = state.switchingBranch == null,
                            ) { Text(stringResource(R.string.git_switch)) }
                            else -> Text(
                                relative(branch.unixTime),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    },
                ) { GitLead(if (branch.remote) Icons.Outlined.Cloud else Icons.Outlined.AccountTree, on = branch.current) }
            }
        }
    }
}

private fun LazyListScope.worktrees(repository: GitRepository, model: ControllerGitModel) {
    item {
        ItemGroup {
            repository.worktrees.forEachIndexed { index, worktree ->
                GroupItem(
                    title = worktree.path.trimEnd('/').substringAfterLast('/'),
                    subtitle = worktree.branch ?: stringResource(R.string.git_detached, worktree.head ?: "—"),
                    divider = index > 0,
                    onClick = { model.showWorktree(worktree) },
                    trailing = {
                        Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (worktree.terminal) GitBadge(stringResource(R.string.git_terminal), MaterialTheme.colorScheme.primary)
                            if (worktree.current) {
                                GitBadge(stringResource(R.string.git_shown), com.multiplex.mobile.ui.SlateExtras.done)
                            } else {
                                Chevron()
                            }
                        }
                    },
                ) { GitLead(Icons.Outlined.Folder, on = worktree.current) }
            }
        }
    }
    item { Footnote(stringResource(R.string.git_worktree_note)) }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun GitDiffScreen(
    model: ControllerGitModel,
    route: GitRoute,
    onBack: () -> Unit,
    onOpenFile: (GitDiffFile) -> Unit,
) {
    var diff by remember(route) { mutableStateOf<GitDiff?>(null) }
    var failure by remember(route) { mutableStateOf<String?>(null) }
    LaunchedEffect(route) {
        try {
            diff = model.diff(route.commit, route.path)
        } catch (error: kotlinx.coroutines.CancellationException) {
            throw error
        } catch (error: Throwable) {
            failure = ControllerGitModel.errorCode(error)
        }
    }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
        TopAppBar(
            title = { Text(route.title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            navigationIcon = {
                IconButton(onClick = onBack) {
                    Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = stringResource(R.string.git_back))
                }
            },
            colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
        )
        val loaded = diff
        when {
            loaded != null -> GitDiffBody(loaded, showFiles = route.path == null, onOpenFile = onOpenFile)
            failure != null -> GitMessage(stringResource(R.string.git_no_diff), gitErrorText(failure!!))
            else -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        }
    }
}

@Composable
private fun GitDiffBody(diff: GitDiff, showFiles: Boolean, onOpenFile: (GitDiffFile) -> Unit) {
    val lines = remember(diff.patch) { GitPatchLine.parse(diff.patch) }
    val horizontal = rememberScrollState()
    LazyColumn(Modifier.fillMaxSize()) {
        diff.commit?.let { commit ->
            item {
                SelectionContainer {
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .background(MaterialTheme.colorScheme.surfaceContainerLow)
                            .padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(commit.message, style = MaterialTheme.typography.bodyMedium)
                        Text(
                            "${commit.author} · ${relative(commit.unixTime)}",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Text(
                            commit.oid,
                            style = MaterialTheme.typography.labelSmall.copy(fontFamily = FontFamily.Monospace),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
        if (showFiles && diff.files.size > 1) {
            item {
                Column(Modifier.padding(horizontal = 16.dp)) {
                    GroupLabel(stringResource(R.string.git_file_count, diff.files.size))
                    ItemGroup {
                        diff.files.forEachIndexed { index, file ->
                            GroupItem(
                                title = file.path.substringAfterLast('/'),
                                subtitle = fileCounts(file),
                                divider = index > 0,
                                onClick = if (file.status == GitChangeStatus.UNTRACKED) null else ({ onOpenFile(file) }),
                                trailing = { GitBadge(file.status.letter, MaterialTheme.colorScheme.onSurfaceVariant) },
                            ) { GitLead(statusIcon(file.status)) }
                        }
                    }
                    if (diff.filesTruncated) Footnote(stringResource(R.string.git_more_files))
                    Spacer(Modifier.height(12.dp))
                }
            }
        }
        if (lines.isEmpty()) {
            item {
                Footnote(
                    stringResource(if (diff.files.isEmpty()) R.string.git_no_changes else R.string.git_no_text_changes),
                    Modifier.padding(16.dp),
                )
            }
        } else {
            // One horizontal scroll shared by every line, so long lines move together.
            itemsIndexed(lines, key = { index, _ -> index }) { _, line ->
                Box(
                    Modifier
                        .fillMaxWidth()
                        .background(line.kind.background())
                        .horizontalScroll(horizontal),
                ) {
                    Text(
                        line.text.ifEmpty { " " },
                        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace, fontSize = 11.5.sp),
                        color = line.kind.foreground(),
                        softWrap = false,
                        modifier = Modifier.padding(horizontal = 10.dp).widthIn(min = 0.dp),
                    )
                }
            }
            if (diff.patchTruncated) {
                item {
                    Text(
                        stringResource(R.string.git_patch_truncated),
                        style = MaterialTheme.typography.bodySmall,
                        color = com.multiplex.mobile.ui.SlateExtras.attention,
                        modifier = Modifier.padding(16.dp),
                    )
                }
            }
        }
    }
}

/** One line of a unified diff and how to colour it. */
data class GitPatchLine(val text: String, val kind: Kind) {
    enum class Kind { FILE, HUNK, ADDED, REMOVED, CONTEXT }

    companion object {
        fun parse(patch: String): List<GitPatchLine> {
            if (patch.isEmpty()) return emptyList()
            var inHeader = false
            return patch.removeSuffix("\n").split('\n').map { raw ->
                val text = raw.replace("\t", "    ")
                val kind = when {
                    text.startsWith("diff --git") -> { inHeader = true; Kind.FILE }
                    text.startsWith("@@") -> { inHeader = false; Kind.HUNK }
                    inHeader -> Kind.FILE
                    text.startsWith("+") -> Kind.ADDED
                    text.startsWith("-") -> Kind.REMOVED
                    else -> Kind.CONTEXT
                }
                GitPatchLine(text, kind)
            }
        }
    }
}

@Composable
private fun GitPatchLine.Kind.foreground(): Color = when (this) {
    GitPatchLine.Kind.FILE -> MaterialTheme.colorScheme.onSurface
    GitPatchLine.Kind.HUNK -> MaterialTheme.colorScheme.primary
    GitPatchLine.Kind.ADDED -> com.multiplex.mobile.ui.SlateExtras.done
    GitPatchLine.Kind.REMOVED -> MaterialTheme.colorScheme.error
    GitPatchLine.Kind.CONTEXT -> MaterialTheme.colorScheme.onSurfaceVariant
}

@Composable
private fun GitPatchLine.Kind.background(): Color = when (this) {
    GitPatchLine.Kind.FILE -> MaterialTheme.colorScheme.surfaceContainerHigh
    GitPatchLine.Kind.HUNK -> MaterialTheme.colorScheme.primary.copy(alpha = 0.10f)
    GitPatchLine.Kind.ADDED -> Color(0xFF17301F).copy(alpha = 0.7f)
    GitPatchLine.Kind.REMOVED -> Color(0xFF3A1D1B).copy(alpha = 0.7f)
    GitPatchLine.Kind.CONTEXT -> Color.Transparent
}

@Composable
private fun GitMessage(title: String, body: String, onRetry: (() -> Unit)? = null) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(Icons.Outlined.Warning, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(title, style = MaterialTheme.typography.titleMedium)
        Text(body, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (onRetry != null) OutlinedButton(onClick = onRetry) { Text(stringResource(R.string.retry)) }
    }
}

@Composable
private fun GitLead(icon: ImageVector, on: Boolean = false) {
    ItemLead {
        Icon(
            icon,
            contentDescription = null,
            tint = if (on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.size(20.dp),
        )
    }
}

@Composable
private fun GitBadge(text: String, color: Color) {
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = color,
        maxLines = 1,
        modifier = Modifier
            .background(color.copy(alpha = 0.14f), RoundedCornerShape(50))
            .padding(horizontal = 7.dp, vertical = 2.dp),
    )
}

@Composable
private fun Chevron() {
    Icon(
        Icons.AutoMirrored.Outlined.KeyboardArrowRight,
        contentDescription = null,
        tint = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun Footnote(text: String, modifier: Modifier = Modifier.padding(horizontal = 4.dp, vertical = 10.dp)) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = modifier,
    )
}

private fun statusIcon(status: GitChangeStatus): ImageVector = when (status) {
    GitChangeStatus.CONFLICTED -> Icons.Outlined.Warning
    else -> Icons.Outlined.Description
}

@Composable
private fun changeMeta(change: GitChange): String {
    val state = when {
        change.staged && change.unstaged -> stringResource(R.string.git_staged_and_changed)
        change.staged -> stringResource(R.string.git_staged)
        else -> statusName(change.status)
    }
    change.originalPath?.let { return stringResource(R.string.git_renamed_from, it) + " · " + state }
    val folder = change.path.substringBeforeLast('/', "")
    return if (folder.isEmpty()) state else "$folder · $state"
}

@Composable
private fun statusName(status: GitChangeStatus): String = stringResource(
    when (status) {
        GitChangeStatus.MODIFIED -> R.string.git_status_modified
        GitChangeStatus.ADDED -> R.string.git_status_added
        GitChangeStatus.DELETED -> R.string.git_status_deleted
        GitChangeStatus.RENAMED -> R.string.git_status_renamed
        GitChangeStatus.COPIED -> R.string.git_status_copied
        GitChangeStatus.TYPE_CHANGED -> R.string.git_status_type_changed
        GitChangeStatus.UNTRACKED -> R.string.git_status_untracked
        GitChangeStatus.CONFLICTED -> R.string.git_status_conflicted
    },
)

@Composable
private fun fileCounts(file: GitDiffFile): String {
    val additions = file.additions
    val deletions = file.deletions
    if (additions == null || deletions == null) {
        return stringResource(if (file.status == GitChangeStatus.UNTRACKED) R.string.git_status_untracked else R.string.git_binary)
    }
    return "+$additions −$deletions"
}

private fun relative(unixTime: Long): String =
    if (unixTime <= 0) "" else DateUtils.getRelativeTimeSpanString(
        unixTime * 1000,
        System.currentTimeMillis(),
        DateUtils.MINUTE_IN_MILLIS,
        DateUtils.FORMAT_ABBREV_RELATIVE,
    ).toString()

@Composable
private fun gitNoticeText(notice: GitNotice): String = when (notice) {
    is GitNotice.Switched -> stringResource(R.string.git_switched, notice.branch)
    is GitNotice.GitSaid -> notice.message.ifBlank { stringResource(R.string.git_did_not_switch) }
    is GitNotice.Failed -> gitErrorText(notice.code)
}

@Composable
internal fun gitErrorText(code: String): String = stringResource(
    when (code) {
        "capability_denied" -> R.string.git_error_permission
        "git_not_local" -> R.string.git_error_not_local
        "git_route_unavailable" -> R.string.git_error_route
        "unsupported_command" -> R.string.git_error_update
        "git_unavailable" -> R.string.git_error_not_installed
        "git_timeout" -> R.string.git_error_timeout
        "git_folder_unknown" -> R.string.git_error_folder
        "git_unknown_worktree" -> R.string.git_error_worktree
        "git_unknown_commit" -> R.string.git_error_commit
        "git_unknown_branch" -> R.string.git_error_branch
        "stale_generation", "host_unavailable" -> R.string.git_error_ended
        "unreachable" -> R.string.git_error_unreachable
        else -> R.string.git_error_failed
    },
)
