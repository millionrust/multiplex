package com.multiplex.mobile.controller

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

enum class GitSection { CHANGES, HISTORY, BRANCHES, WORKTREES }

sealed interface GitLoadState {
    data object Loading : GitLoadState
    data object Loaded : GitLoadState
    data object NotRepository : GitLoadState
    /** [code] is the computer's error code, turned into words by the screen. */
    data class Failed(val code: String) : GitLoadState
}

data class GitUiState(
    val sessionTitle: String,
    val canSwitch: Boolean,
    val load: GitLoadState = GitLoadState.Loading,
    val repository: GitRepository? = null,
    val section: GitSection = GitSection.CHANGES,
    val commits: List<GitCommit> = emptyList(),
    val historyNextOffset: Int? = null,
    val historyLoading: Boolean = false,
    val historyError: String? = null,
    /** Another worktree of the repository, by path, or null for the terminal's own. */
    val worktree: String? = null,
    val switchingBranch: String? = null,
    /** What the last switch did: a code to put into words, or Git's own message. */
    val notice: GitNotice? = null,
)

sealed interface GitNotice {
    data class Switched(val branch: String) : GitNotice
    data class GitSaid(val message: String) : GitNotice
    data class Failed(val code: String) : GitNotice
}

/**
 * The Git repository a terminal's shell is in: its changes, history, branches and worktrees,
 * read from the computer on a connection of its own so the terminal stays attached.
 */
class ControllerGitModel(
    private val scope: CoroutineScope,
    private val host: PairedHostRecord,
    private val target: GitTarget,
    sessionTitle: String,
    canSwitch: Boolean,
    private val connection: ControllerConnecting?,
) {
    private val _state = MutableStateFlow(GitUiState(sessionTitle = sessionTitle, canSwitch = canSwitch))
    val state: StateFlow<GitUiState> = _state.asStateFlow()
    private var loadJob: Job? = null
    private var historyJob: Job? = null
    private var generation = 0

    /** Reads the overview and the first page of history again. */
    fun load() {
        loadJob?.cancel()
        historyJob?.cancel()
        val generation = ++generation
        _state.update {
            it.copy(
                load = if (it.repository == null) GitLoadState.Loading else it.load,
                historyLoading = false,
            )
        }
        loadJob = scope.launch {
            try {
                val answer = request(GitRequest.Overview(_state.value.worktree)) as? GitAnswer.Overview
                    ?: throw IllegalArgumentException("unexpected answer")
                if (generation != this@ControllerGitModel.generation) return@launch
                _state.update {
                    it.copy(
                        repository = answer.repository,
                        load = if (answer.repository == null) GitLoadState.NotRepository else GitLoadState.Loaded,
                        commits = emptyList(),
                        historyNextOffset = 0,
                        historyError = null,
                    )
                }
                if (answer.repository != null) loadMoreHistory()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (generation != this@ControllerGitModel.generation) return@launch
                _state.update { it.copy(load = GitLoadState.Failed(errorCode(error))) }
            }
        }
    }

    fun loadMoreHistory() {
        val current = _state.value
        val offset = current.historyNextOffset ?: return
        if (current.historyLoading || current.load != GitLoadState.Loaded) return
        val generation = generation
        _state.update { it.copy(historyLoading = true, historyError = null) }
        historyJob = scope.launch {
            try {
                val page = request(GitRequest.Log(_state.value.worktree, offset, HISTORY_PAGE)) as? GitAnswer.Log
                    ?: throw IllegalArgumentException("unexpected answer")
                if (generation != this@ControllerGitModel.generation) return@launch
                _state.update { state ->
                    val known = state.commits.mapTo(HashSet(), GitCommit::oid)
                    state.copy(
                        commits = state.commits + page.commits.filterNot { it.oid in known },
                        historyNextOffset = page.nextOffset,
                        historyLoading = false,
                    )
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (generation != this@ControllerGitModel.generation) return@launch
                _state.update { it.copy(historyLoading = false, historyError = errorCode(error)) }
            }
        }
    }

    fun selectSection(section: GitSection) = _state.update { it.copy(section = section) }

    /** Shows another worktree of the same repository, or the terminal's own. */
    fun showWorktree(worktree: GitWorktree) {
        _state.update {
            it.copy(worktree = if (worktree.terminal) null else worktree.path, repository = null, commits = emptyList())
        }
        load()
    }

    fun switchTo(branch: GitBranch) {
        val current = _state.value
        if (!current.canSwitch || current.switchingBranch != null || branch.current) return
        _state.update { it.copy(switchingBranch = branch.name, notice = null) }
        scope.launch {
            val notice = try {
                val result = (request(GitRequest.Switch(_state.value.worktree, branch.name)) as? GitAnswer.Switched)
                    ?.result ?: throw IllegalArgumentException("unexpected answer")
                if (result.switched) GitNotice.Switched(branch.localName) else GitNotice.GitSaid(result.message)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                GitNotice.Failed(errorCode(error))
            }
            _state.update { it.copy(switchingBranch = null, notice = notice) }
            load()
        }
    }

    fun dismissNotice() = _state.update { it.copy(notice = null) }

    /** A commit's changes, or the uncommitted ones for null, narrowed to [path] when given. */
    suspend fun diff(commit: String?, path: String?): GitDiff =
        (request(GitRequest.Diff(_state.value.worktree, commit, path)) as? GitAnswer.Diff)?.diff
            ?: throw IllegalArgumentException("unexpected answer")

    suspend fun stop() {
        loadJob?.cancel()
        historyJob?.cancel()
        connection?.cancel()
    }

    private suspend fun request(request: GitRequest): GitAnswer {
        val connection = connection
            ?: throw ControllerConnectionException.HostError("git_route_unavailable")
        return connection.git(host, target, request)
    }

    companion object {
        const val HISTORY_PAGE = 40

        fun errorCode(error: Throwable): String = when (error) {
            is ControllerConnectionException.HostError -> error.code
            is ControllerConnectionException.CapabilityDenied -> "capability_denied"
            is java.io.IOException -> "unreachable"
            else -> "malformed"
        }
    }
}
