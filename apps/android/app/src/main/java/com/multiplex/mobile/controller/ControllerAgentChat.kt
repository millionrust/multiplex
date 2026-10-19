package com.multiplex.mobile.controller

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * What a phone asks of the coding agent running in a terminal: up to [limit] items, the newest
 * unless [before] names the item to read back from. A [sinceRevision] the computer still has
 * answers `unchanged`, so asking often costs little.
 */
data class AgentChatRequest(
    val before: String? = null,
    val sinceRevision: String? = null,
    val limit: Int = 60,
) {
    fun command(target: GitTarget): JsonObject = buildJsonObject {
        put("kind", "agent_chat")
        put("session_id", target.sessionId.lowercase())
        put("occupant_generation", target.occupantGeneration)
        put("before", before?.let(::JsonPrimitive) ?: JsonNull)
        put("since_revision", sinceRevision?.let(::JsonPrimitive) ?: JsonNull)
        put("limit", limit)
    }
}

enum class AgentProvider(val wire: String, val displayName: String) {
    CLAUDE_CODE("claude_code", "Claude Code"),
    CODEX("codex", "Codex"),
}

enum class AgentChatState(val wire: String) { WORKING("working"), READY("ready"), UNKNOWN("unknown") }

enum class AgentChatItemKind(val wire: String) {
    USER("user"),
    ASSISTANT("assistant"),
    TOOL("tool"),
    NOTICE("notice"),
}

enum class AgentToolState(val wire: String) { RUNNING("running"), SUCCEEDED("succeeded"), FAILED("failed") }

/** Reads an enum by its wire name, taking [fallback] for a name from a newer computer. */
private open class WireEnumSerializer<T : Enum<T>>(
    name: String,
    private val values: Array<T>,
    private val wire: (T) -> String,
    private val fallback: T,
) : KSerializer<T> {
    override val descriptor = PrimitiveSerialDescriptor(name, PrimitiveKind.STRING)
    override fun deserialize(decoder: Decoder): T {
        val raw = decoder.decodeString()
        return values.firstOrNull { wire(it) == raw } ?: fallback
    }
    override fun serialize(encoder: Encoder, value: T) = encoder.encodeString(wire(value))
}

private object AgentProviderSerializer : KSerializer<AgentProvider> {
    override val descriptor = PrimitiveSerialDescriptor("AgentProvider", PrimitiveKind.STRING)
    override fun deserialize(decoder: Decoder): AgentProvider {
        val raw = decoder.decodeString()
        return AgentProvider.entries.firstOrNull { it.wire == raw }
            ?: throw IllegalArgumentException("unknown provider $raw")
    }
    override fun serialize(encoder: Encoder, value: AgentProvider) = encoder.encodeString(value.wire)
}

private object AgentChatStateSerializer : WireEnumSerializer<AgentChatState>(
    "AgentChatState", AgentChatState.entries.toTypedArray(), AgentChatState::wire, AgentChatState.UNKNOWN,
)

private object AgentChatItemKindSerializer : WireEnumSerializer<AgentChatItemKind>(
    "AgentChatItemKind", AgentChatItemKind.entries.toTypedArray(), AgentChatItemKind::wire, AgentChatItemKind.NOTICE,
)

private object AgentToolStateSerializer : WireEnumSerializer<AgentToolState>(
    "AgentToolState", AgentToolState.entries.toTypedArray(), AgentToolState::wire, AgentToolState.SUCCEEDED,
)

@Serializable
data class AgentChatItem(
    val id: String,
    @Serializable(with = AgentChatItemKindSerializer::class) val kind: AgentChatItemKind,
    val title: String? = null,
    val text: String = "",
    @SerialName("tool_state")
    @Serializable(with = AgentToolStateSerializer::class)
    val toolState: AgentToolState? = null,
    val truncated: Boolean = false,
)

@Serializable
data class AgentChat(
    @Serializable(with = AgentProviderSerializer::class) val provider: AgentProvider,
    val title: String? = null,
    @Serializable(with = AgentChatStateSerializer::class) val state: AgentChatState = AgentChatState.UNKNOWN,
    val revision: String,
    val unchanged: Boolean = false,
    val items: List<AgentChatItem> = emptyList(),
    val earlier: Boolean = false,
    @SerialName("history_truncated") val historyTruncated: Boolean = false,
) {
    companion object {
        private val json = Json { ignoreUnknownKeys = true; explicitNulls = false }

        /** Reads one answer, checking it answers [commandId]. An error answer throws its code. */
        fun decodeAnswer(text: String, commandId: String): AgentChat? {
            val element = runCatching { json.parseToJsonElement(text).jsonObject }
                .getOrElse { throw IllegalArgumentException("malformed chat answer") }
            val kind = element["kind"]?.jsonPrimitive?.content
                ?: throw IllegalArgumentException("missing response kind")
            if (kind == "error") {
                throw ControllerConnectionException.HostError(
                    element["code"]?.jsonPrimitive?.content ?: "unknown",
                )
            }
            require(kind == "agent_chat") { "unexpected answer $kind" }
            require(element["command_id"]?.jsonPrimitive?.content.equals(commandId, ignoreCase = true)) {
                "answer for another command"
            }
            val chat = element["chat"] ?: return null
            if (chat is JsonNull) return null
            return json.decodeFromJsonElement(serializer(), chat)
        }
    }
}

/** A message split into prose and fenced code, which the chat draws differently. */
sealed interface ChatSegment {
    data class Prose(val text: String) : ChatSegment
    data class Code(val language: String?, val text: String) : ChatSegment

    companion object {
        fun split(text: String): List<ChatSegment> {
            val segments = mutableListOf<ChatSegment>()
            val prose = mutableListOf<String>()
            var code: MutableList<String>? = null
            var language: String? = null
            fun flushProse() {
                val joined = prose.joinToString("\n").trim('\n')
                if (joined.isNotBlank()) segments += Prose(joined)
                prose.clear()
            }
            for (line in text.split('\n')) {
                val trimmed = line.trim()
                if (trimmed.startsWith("```")) {
                    val open = code
                    if (open != null) {
                        segments += Code(language, open.joinToString("\n"))
                        code = null
                        language = null
                    } else {
                        flushProse()
                        language = trimmed.removePrefix("```").trim().ifEmpty { null }
                        code = mutableListOf()
                    }
                } else if (code != null) {
                    code.add(line)
                } else {
                    prose += line
                }
            }
            // A fence the cut-off text never closed is still code.
            code?.let { segments += Code(language, it.joinToString("\n")) }
            flushProse()
            return segments
        }
    }
}
