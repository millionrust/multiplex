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

/** The terminal a Git request is about: the repository is the one its shell is in now. */
data class GitTarget(val sessionId: String, val occupantGeneration: Long)

/**
 * What a phone asks of a terminal's repository. `worktree` shows another worktree of the same
 * repository, by the path the overview gave.
 */
sealed interface GitRequest {
    val worktree: String?

    data class Overview(override val worktree: String?) : GitRequest
    data class Log(override val worktree: String?, val offset: Int, val limit: Int) : GitRequest
    data class Diff(override val worktree: String?, val commit: String?, val path: String?) : GitRequest
    data class Switch(override val worktree: String?, val branch: String) : GitRequest

    /** Reading needs what watching a terminal needs; switching needs what typing into one does. */
    val requiresInput: Boolean get() = this is Switch

    fun command(target: GitTarget): JsonObject = buildJsonObject {
        put("kind", when (this@GitRequest) {
            is Overview -> "git_overview"
            is Log -> "git_log"
            is Diff -> "git_diff"
            is Switch -> "git_switch"
        })
        put("session_id", target.sessionId.lowercase())
        put("occupant_generation", target.occupantGeneration)
        put("worktree", worktree?.let(::JsonPrimitive) ?: JsonNull)
        when (val request = this@GitRequest) {
            is Overview -> Unit
            is Log -> {
                put("offset", request.offset)
                put("limit", request.limit)
            }
            is Diff -> {
                put("commit", request.commit?.let(::JsonPrimitive) ?: JsonNull)
                put("path", request.path?.let(::JsonPrimitive) ?: JsonNull)
            }
            is Switch -> put("branch", request.branch)
        }
    }
}

sealed interface GitAnswer {
    /** `repository` is null when the terminal's folder is in no repository. */
    data class Overview(val repository: GitRepository?) : GitAnswer
    data class Log(val commits: List<GitCommit>, val nextOffset: Int?) : GitAnswer
    data class Diff(val diff: GitDiff) : GitAnswer
    data class Switched(val result: GitSwitchResult) : GitAnswer

    companion object {
        private val json = Json { ignoreUnknownKeys = true; explicitNulls = false }

        /** Reads one answer, checking it answers [commandId]. An error answer throws its code. */
        fun decode(text: String, commandId: String): GitAnswer {
            val element = runCatching { json.parseToJsonElement(text).jsonObject }
                .getOrElse { throw IllegalArgumentException("malformed git answer") }
            val kind = element["kind"]?.jsonPrimitive?.content
                ?: throw IllegalArgumentException("missing response kind")
            if (kind == "error") {
                throw ControllerConnectionException.HostError(
                    element["code"]?.jsonPrimitive?.content ?: "unknown",
                )
            }
            require(element["command_id"]?.jsonPrimitive?.content.equals(commandId, ignoreCase = true)) {
                "answer for another command"
            }
            return when (kind) {
                "git_overview" -> Overview(json.decodeFromJsonElement(OverviewPayload.serializer(), element).repository)
                "git_log" -> json.decodeFromJsonElement(LogPayload.serializer(), element).let { Log(it.commits, it.nextOffset) }
                "git_diff" -> Diff(json.decodeFromJsonElement(DiffPayload.serializer(), element).diff)
                "git_switched" -> Switched(json.decodeFromJsonElement(GitSwitchResult.serializer(), element))
                else -> throw IllegalArgumentException("unexpected answer $kind")
            }
        }
    }
}

@Serializable private data class OverviewPayload(val repository: GitRepository? = null)
@Serializable private data class LogPayload(
    val commits: List<GitCommit>,
    @SerialName("next_offset") val nextOffset: Int? = null,
)
@Serializable private data class DiffPayload(val diff: GitDiff)

@Serializable
data class GitRepository(
    val folder: String,
    val root: String,
    val branch: String? = null,
    val head: String? = null,
    val upstream: String? = null,
    val ahead: Int = 0,
    val behind: Int = 0,
    val changes: List<GitChange> = emptyList(),
    @SerialName("changes_truncated") val changesTruncated: Boolean = false,
    val branches: List<GitBranch> = emptyList(),
    @SerialName("branches_truncated") val branchesTruncated: Boolean = false,
    val worktrees: List<GitWorktree> = emptyList(),
) {
    val localBranches: List<GitBranch> get() = branches.filterNot(GitBranch::remote)
    val remoteBranches: List<GitBranch> get() = branches.filter(GitBranch::remote)
}

@Serializable(with = GitChangeStatusSerializer::class)
enum class GitChangeStatus(val wire: String, val letter: String) {
    MODIFIED("modified", "M"),
    ADDED("added", "A"),
    DELETED("deleted", "D"),
    RENAMED("renamed", "R"),
    COPIED("copied", "C"),
    TYPE_CHANGED("type_changed", "T"),
    UNTRACKED("untracked", "?"),
    CONFLICTED("conflicted", "U"),
}

/** Codes from a newer computer read as a modification rather than failing the page. */
private object GitChangeStatusSerializer : KSerializer<GitChangeStatus> {
    override val descriptor = PrimitiveSerialDescriptor("GitChangeStatus", PrimitiveKind.STRING)
    override fun deserialize(decoder: Decoder): GitChangeStatus {
        val raw = decoder.decodeString()
        return GitChangeStatus.entries.firstOrNull { it.wire == raw } ?: GitChangeStatus.MODIFIED
    }
    override fun serialize(encoder: Encoder, value: GitChangeStatus) = encoder.encodeString(value.wire)
}

@Serializable
data class GitChange(
    val path: String,
    @SerialName("original_path") val originalPath: String? = null,
    val status: GitChangeStatus,
    val staged: Boolean = false,
    val unstaged: Boolean = false,
)

@Serializable
data class GitBranch(
    val name: String,
    val remote: Boolean = false,
    val current: Boolean = false,
    val upstream: String? = null,
    val commit: String = "",
    val subject: String = "",
    @SerialName("unix_time") val unixTime: Long = 0,
) {
    /** `origin/feature` becomes the local `feature` git switches to. */
    val localName: String get() = if (remote && '/' in name) name.substringAfter('/') else name
}

@Serializable
data class GitWorktree(
    val path: String,
    val branch: String? = null,
    val head: String? = null,
    /** The worktree shown. */
    val current: Boolean = false,
    /** The worktree the terminal is in. */
    val terminal: Boolean = false,
    val locked: Boolean = false,
    val prunable: Boolean = false,
)

@Serializable
data class GitCommit(
    val oid: String,
    val short: String,
    val subject: String,
    val author: String,
    @SerialName("unix_time") val unixTime: Long = 0,
    val parents: Int = 1,
    val refs: List<String> = emptyList(),
)

@Serializable
data class GitCommitDetail(
    val oid: String,
    val short: String,
    val author: String,
    val email: String = "",
    @SerialName("unix_time") val unixTime: Long = 0,
    val parents: List<String> = emptyList(),
    val message: String = "",
)

@Serializable
data class GitDiff(
    val commit: GitCommitDetail? = null,
    val files: List<GitDiffFile> = emptyList(),
    @SerialName("files_truncated") val filesTruncated: Boolean = false,
    val patch: String = "",
    @SerialName("patch_truncated") val patchTruncated: Boolean = false,
)

@Serializable
data class GitDiffFile(
    val path: String,
    @SerialName("original_path") val originalPath: String? = null,
    val status: GitChangeStatus,
    /** Null for a binary file or one Git does not track yet. */
    val additions: Int? = null,
    val deletions: Int? = null,
)

@Serializable
data class GitSwitchResult(
    val switched: Boolean,
    val branch: String,
    val message: String = "",
)
