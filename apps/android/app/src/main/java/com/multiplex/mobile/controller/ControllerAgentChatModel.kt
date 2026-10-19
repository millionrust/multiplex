package com.multiplex.mobile.controller

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class AgentChatUiState(
    val chat: AgentChat? = null,
    val items: List<AgentChatItem> = emptyList(),
    val hasEarlier: Boolean = false,
    val loadingEarlier: Boolean = false,
    /** A code for what went wrong reading the conversation, while it does. */
    val problem: String? = null,
    /** Kept here so it survives switching to the terminal and back. */
    val draft: String = "",
    val expandedTools: Set<String> = emptySet(),
) {
    /** Whether an agent runs in the terminal, so the page offers Chat. */
    val available: Boolean get() = chat != null
}

/**
 * The conversation of the Claude Code or Codex session running in a terminal, read from the
 * computer on a connection of its own so the terminal stays attached. It looks for an agent while
 * the terminal is open and follows the conversation while Chat is showing.
 */
class ControllerAgentChatModel(
    private val scope: CoroutineScope,
    private val host: PairedHostRecord,
    private val target: GitTarget,
    private val connection: ControllerConnecting?,
) {
    private val _state = MutableStateFlow(AgentChatUiState())
    val state: StateFlow<AgentChatUiState> = _state.asStateFlow()
    private var loop: Job? = null
    private var following = false
    private var unsupported = false

    fun start() {
        if (loop?.isActive == true || unsupported) return
        loop = scope.launch {
            while (true) {
                refresh()
                if (unsupported) return@launch
                delay(if (following) FOLLOW_INTERVAL_MILLIS else LOOK_INTERVAL_MILLIS)
            }
        }
    }

    /** Chat is showing: read the conversation often, starting now. */
    fun setFollowing(following: Boolean) {
        if (following == this.following) return
        this.following = following
        if (following && !unsupported) {
            loop?.cancel()
            loop = null
            start()
        }
    }

    suspend fun stop() {
        loop?.cancel()
        loop = null
        connection?.cancel()
    }

    fun pause() {
        loop?.cancel()
        loop = null
    }

    suspend fun refresh() {
        try {
            apply(request(AgentChatRequest(sinceRevision = _state.value.chat?.revision, limit = PAGE)))
            _state.update { it.copy(problem = null) }
        } catch (error: CancellationException) {
            throw error
        } catch (error: ControllerConnectionException.HostError) {
            if (error.code == "unsupported_command" || error.code == "chat_route_unavailable") {
                // A computer or route without the chat: nothing to offer, and nothing to keep asking.
                unsupported = true
                _state.update { it.copy(chat = null, items = emptyList()) }
            } else {
                _state.update { if (it.chat != null) it.copy(problem = error.code) else it }
            }
        } catch (error: Throwable) {
            _state.update { if (it.chat != null) it.copy(problem = ControllerGitModel.errorCode(error)) else it }
        }
    }

    fun loadEarlier() {
        val current = _state.value
        val first = current.items.firstOrNull() ?: return
        if (current.loadingEarlier || !current.hasEarlier) return
        _state.update { it.copy(loadingEarlier = true) }
        scope.launch {
            try {
                val page = request(AgentChatRequest(before = first.id, limit = PAGE))
                _state.update { state ->
                    if (page == null) return@update state.copy(loadingEarlier = false)
                    val known = state.items.mapTo(HashSet(), AgentChatItem::id)
                    state.copy(
                        items = page.items.filterNot { it.id in known } + state.items,
                        hasEarlier = page.earlier,
                        loadingEarlier = false,
                    )
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _state.update { it.copy(loadingEarlier = false, problem = ControllerGitModel.errorCode(error)) }
            }
        }
    }

    fun setDraft(draft: String) = _state.update { it.copy(draft = draft.take(MAX_DRAFT_CHARS)) }

    fun toggleTool(id: String) = _state.update {
        it.copy(expandedTools = if (id in it.expandedTools) it.expandedTools - id else it.expandedTools + id)
    }

    /**
     * Takes in a newest page: what it holds replaces the same items here, and what was read back
     * before it stays in front of it.
     */
    fun apply(answer: AgentChat?) = _state.update { state ->
        when {
            answer == null -> state.copy(chat = null, items = emptyList(), hasEarlier = false)
            answer.unchanged && state.chat != null -> state.copy(chat = answer)
            state.chat?.provider != null && state.chat.provider != answer.provider || answer.items.isEmpty() ->
                state.copy(chat = answer, items = answer.items, hasEarlier = answer.earlier)
            else -> {
                val at = state.items.indexOfFirst { it.id == answer.items.first().id }
                if (at >= 0) {
                    state.copy(
                        chat = answer,
                        items = state.items.subList(0, at) + answer.items,
                        hasEarlier = if (at == 0) answer.earlier else state.hasEarlier,
                    )
                } else {
                    state.copy(chat = answer, items = answer.items, hasEarlier = answer.earlier)
                }
            }
        }
    }

    private suspend fun request(request: AgentChatRequest): AgentChat? {
        val connection = connection
            ?: throw ControllerConnectionException.HostError("chat_route_unavailable")
        return connection.agentChat(host, target, request)
    }

    companion object {
        const val FOLLOW_INTERVAL_MILLIS = 1_500L
        const val LOOK_INTERVAL_MILLIS = 5_000L
        const val PAGE = 60
        const val MAX_DRAFT_CHARS = 16_000
    }
}
