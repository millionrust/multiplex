package com.multiplex.mobile.controller

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.outlined.ArrowDownward
import androidx.compose.material.icons.outlined.ArrowUpward
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.multiplex.mobile.R
import kotlinx.coroutines.launch

/**
 * A terminal's page with Chat beside it while a coding agent runs there. The terminal is left
 * exactly as it was; Chat replaces it on screen while chosen.
 */
@Composable
fun TerminalWithAgentChat(
    chat: ControllerAgentChatModel?,
    terminal: ControllerTerminalUiState,
    onSendChat: (String) -> Boolean,
    onRequestControl: () -> Unit,
    terminalContent: @Composable () -> Unit,
) {
    val state = chat?.state?.collectAsState()?.value
    var showsChat by rememberSaveable(terminal.sessionId) { mutableStateOf(false) }
    val available = state?.available == true
    LaunchedEffect(available) { if (!available) showsChat = false }
    LaunchedEffect(chat, showsChat) { chat?.setFollowing(showsChat) }
    Column(Modifier.fillMaxSize()) {
        if (chat != null && state != null && available) {
            AgentChatSwitch(state, showsChat) { showsChat = it }
        }
        Box(Modifier.weight(1f)) {
            // A covered terminal draws its own cover; the conversation is covered with it.
            if (showsChat && chat != null && state != null && !terminal.privacyCovered) {
                AgentChatPane(chat, state, terminal, onSendChat, onRequestControl) { showsChat = false }
            } else {
                terminalContent()
            }
        }
    }
}

@Composable
private fun AgentChatSwitch(state: AgentChatUiState, showsChat: Boolean, onChange: (Boolean) -> Unit) {
    val chat = state.chat ?: return
    Row(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.background)
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        val options = listOf(true to R.string.chat_tab, false to R.string.chat_terminal_tab)
        SingleChoiceSegmentedButtonRow(Modifier.weight(1f)) {
            options.forEachIndexed { index, (value, label) ->
                SegmentedButton(
                    selected = showsChat == value,
                    onClick = { onChange(value) },
                    shape = SegmentedButtonDefaults.itemShape(index, options.size),
                    icon = {},
                    label = { Text(stringResource(label), fontSize = 13.sp) },
                )
            }
        }
        AgentStatePill(chat.state)
    }
}

@Composable
private fun AgentStatePill(state: AgentChatState) {
    val working = state == AgentChatState.WORKING
    Row(
        Modifier
            .clip(RoundedCornerShape(50))
            .background(MaterialTheme.colorScheme.surfaceContainerHigh)
            .padding(horizontal = 9.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Box(
            Modifier
                .size(6.dp)
                .clip(CircleShape)
                .background(if (working) com.multiplex.mobile.ui.SlateExtras.attention else com.multiplex.mobile.ui.SlateExtras.done),
        )
        Text(
            stringResource(if (working) R.string.chat_working else R.string.chat_ready),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun AgentChatPane(
    chat: ControllerAgentChatModel,
    state: AgentChatUiState,
    terminal: ControllerTerminalUiState,
    onSendChat: (String) -> Boolean,
    onRequestControl: () -> Unit,
    onShowTerminal: () -> Unit,
) {
    val provider = state.chat?.provider?.displayName ?: "Agent"
    val listState = rememberLazyListState()
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val atBottom by remember {
        derivedStateOf {
            val info = listState.layoutInfo
            val last = info.visibleItemsInfo.lastOrNull()
            last == null || last.index >= info.totalItemsCount - 1
        }
    }
    var newerBelow by remember { mutableStateOf(false) }
    val lastId = state.items.lastOrNull()?.id
    // The list is: header, items, working row; the bottom is its last entry.
    val bottomIndex = state.items.size + 1
    LaunchedEffect(Unit) { listState.scrollToItem(bottomIndex) }
    LaunchedEffect(lastId) {
        if (atBottom) listState.animateScrollToItem(bottomIndex) else newerBelow = true
    }
    LaunchedEffect(atBottom) { if (atBottom) newerBelow = false }

    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .imePadding(),
    ) {
        state.problem?.let {
            Text(
                stringResource(R.string.chat_reconnecting),
                style = MaterialTheme.typography.labelSmall,
                color = com.multiplex.mobile.ui.SlateExtras.attention,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.surfaceContainerLow)
                    .padding(horizontal = 14.dp, vertical = 6.dp),
            )
        }
        Box(Modifier.weight(1f)) {
            LazyColumn(
                state = listState,
                modifier = Modifier.fillMaxSize(),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                item(key = "header") { ChatHeader(state, chat, provider) }
                items(state.items, key = { it.id }) { item ->
                    ChatItem(item, provider, expanded = item.id in state.expandedTools) { chat.toggleTool(item.id) }
                }
                item(key = "working") {
                    if (state.chat?.state == AgentChatState.WORKING) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                            Text(
                                stringResource(R.string.chat_agent_working, provider),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    } else {
                        Spacer(Modifier.size(1.dp))
                    }
                }
            }
            if (newerBelow) {
                SmallFloatingActionButton(
                    onClick = {
                        newerBelow = false
                        scope.launch { listState.animateScrollToItem(bottomIndex) }
                    },
                    modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 10.dp),
                ) {
                    Icon(Icons.Outlined.ArrowDownward, contentDescription = stringResource(R.string.chat_latest))
                }
            }
        }
        ChatComposer(state, terminal, provider, chat, onSendChat, onRequestControl, onShowTerminal)
    }
}

@Composable
private fun ChatHeader(state: AgentChatUiState, chat: ControllerAgentChatModel, provider: String) {
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        state.chat?.title?.let {
            Text(it, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
        }
        when {
            state.hasEarlier -> TextButton(onClick = chat::loadEarlier, enabled = !state.loadingEarlier) {
                if (state.loadingEarlier) {
                    CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.size(6.dp))
                }
                Text(stringResource(R.string.chat_earlier))
            }
            state.chat?.historyTruncated == true -> Text(
                stringResource(R.string.chat_history_truncated),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
        if (state.items.isEmpty()) {
            Text(
                stringResource(R.string.chat_empty, provider),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(top = 40.dp),
            )
        }
    }
}

@Composable
private fun ChatItem(item: AgentChatItem, provider: String, expanded: Boolean, onToggle: () -> Unit) {
    when (item.kind) {
        AgentChatItemKind.USER -> Row(Modifier.fillMaxWidth()) {
            Spacer(Modifier.weight(1f).widthIn(min = 36.dp))
            Column(horizontalAlignment = Alignment.End) {
                SelectionContainer {
                    Text(
                        item.text,
                        style = MaterialTheme.typography.bodyLarge,
                        modifier = Modifier
                            .clip(RoundedCornerShape(13.dp, 13.dp, 4.dp, 13.dp))
                            .background(MaterialTheme.colorScheme.surfaceContainerHigh)
                            .padding(horizontal = 13.dp, vertical = 9.dp),
                    )
                }
                Shortened(item)
            }
        }
        AgentChatItemKind.ASSISTANT -> Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(7.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    if (provider == "Codex") "C" else "✳",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.primary,
                    fontWeight = FontWeight.SemiBold,
                    textAlign = TextAlign.Center,
                    modifier = Modifier
                        .size(19.dp)
                        .clip(RoundedCornerShape(5.dp))
                        .background(MaterialTheme.colorScheme.primary.copy(alpha = 0.15f))
                        .padding(top = 2.dp),
                )
                Text(provider, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            ChatSegment.split(item.text).forEach { segment ->
                when (segment) {
                    is ChatSegment.Prose -> SelectionContainer {
                        Text(segment.text, style = MaterialTheme.typography.bodyLarge)
                    }
                    is ChatSegment.Code -> SelectionContainer {
                        Text(
                            segment.text,
                            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace, fontSize = 12.sp),
                            softWrap = false,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(8.dp))
                                .background(Color(0xFF0B0D10))
                                .horizontalScroll(rememberScrollState())
                                .padding(11.dp),
                        )
                    }
                }
            }
            Shortened(item)
        }
        AgentChatItemKind.TOOL -> Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(9.dp))
                .background(MaterialTheme.colorScheme.surfaceContainerLow)
                .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(9.dp))
                .animateContentSize(),
        ) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable(onClick = onToggle)
                    .heightIn(min = 40.dp)
                    .padding(horizontal = 11.dp, vertical = 9.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(
                    Icons.AutoMirrored.Outlined.KeyboardArrowRight,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(16.dp).rotate(if (expanded) 90f else 0f),
                )
                Text(
                    item.title ?: stringResource(R.string.chat_tool),
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                when (item.toolState) {
                    AgentToolState.RUNNING -> CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
                    AgentToolState.FAILED -> Icon(
                        Icons.Outlined.Close,
                        contentDescription = stringResource(R.string.chat_tool_failed),
                        tint = MaterialTheme.colorScheme.error,
                        modifier = Modifier.size(15.dp),
                    )
                    else -> Icon(
                        Icons.Outlined.Check,
                        contentDescription = stringResource(R.string.chat_tool_done),
                        tint = com.multiplex.mobile.ui.SlateExtras.done,
                        modifier = Modifier.size(15.dp),
                    )
                }
            }
            if (expanded) {
                SelectionContainer {
                    Column(Modifier.padding(horizontal = 11.dp).padding(bottom = 10.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        item.title?.let {
                            Text(it, style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace))
                        }
                        Text(
                            item.text.ifEmpty { stringResource(R.string.chat_no_output) },
                            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace, fontSize = 11.5.sp),
                            color = if (item.text.isEmpty()) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
                            softWrap = false,
                            modifier = Modifier.horizontalScroll(rememberScrollState()),
                        )
                        Shortened(item)
                    }
                }
            }
        }
        AgentChatItemKind.NOTICE -> Text(
            item.text,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun Shortened(item: AgentChatItem) {
    if (item.truncated) {
        Text(
            stringResource(R.string.chat_shortened),
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ChatComposer(
    state: AgentChatUiState,
    terminal: ControllerTerminalUiState,
    provider: String,
    chat: ControllerAgentChatModel,
    onSendChat: (String) -> Boolean,
    onRequestControl: () -> Unit,
    onShowTerminal: () -> Unit,
) {
    val canInput = terminal.writerLease == WriterLeaseState.Held &&
        terminal.attachState == ReadOnlyAttachState.Live && !terminal.privacyCovered
    Column(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.background)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (canInput) {
            Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = state.draft,
                    onValueChange = chat::setDraft,
                    placeholder = { Text(stringResource(R.string.chat_placeholder, provider)) },
                    maxLines = 6,
                    modifier = Modifier.weight(1f),
                )
                val empty = state.draft.isBlank()
                FilledIconButton(
                    onClick = { if (onSendChat(state.draft)) chat.setDraft("") },
                    enabled = !empty,
                    colors = IconButtonDefaults.filledIconButtonColors(),
                    modifier = Modifier.padding(bottom = 4.dp),
                ) {
                    Icon(Icons.Outlined.ArrowUpward, contentDescription = stringResource(R.string.chat_send))
                }
            }
            Text(
                stringResource(R.string.chat_send_note),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth(),
            )
        } else {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    stringResource(if (terminal.supportsWriter) R.string.chat_take_control_note else R.string.chat_view_only),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                )
                if (terminal.supportsWriter) {
                    OutlinedButton(onClick = onRequestControl) { Text(stringResource(R.string.chat_take_control)) }
                } else {
                    OutlinedButton(onClick = onShowTerminal) { Text(stringResource(R.string.chat_terminal_tab)) }
                }
            }
        }
    }
}
