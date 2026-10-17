package com.multiplex.mobile.controller

import java.lang.reflect.Proxy
import java.util.Base64
import java.util.UUID
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ControllerGitTest {
    @Test
    fun answersDecodeFromTheWire() {
        val id = UUID.randomUUID().toString()
        val overview = """
            {"kind":"git_overview","command_id":"$id","session_id":"s",
             "repository":{"folder":"/r/src","root":"/r","branch":"main","head":"abc1234","upstream":"origin/main",
             "ahead":1,"behind":0,"changes":[{"path":"a.txt","original_path":null,"status":"modified","staged":false,"unstaged":true},
             {"path":"b.txt","original_path":null,"status":"something_new","staged":true,"unstaged":false}],
             "changes_truncated":false,
             "branches":[{"name":"main","remote":false,"current":true,"upstream":"origin/main","commit":"abc1234","subject":"s","unix_time":1},
             {"name":"origin/feature","remote":true,"current":false,"upstream":null,"commit":"def5678","subject":"t","unix_time":2}],
             "branches_truncated":false,
             "worktrees":[{"path":"/r","branch":"main","head":"abc1234","current":true,"terminal":true,"locked":false,"prunable":false}]}}
        """.trimIndent()
        val repository = (GitAnswer.decode(overview, id) as GitAnswer.Overview).repository!!
        assertEquals("main", repository.branch)
        assertEquals(listOf(GitChangeStatus.MODIFIED, GitChangeStatus.MODIFIED), repository.changes.map { it.status })
        assertEquals(listOf("main"), repository.localBranches.map { it.name })
        assertEquals("feature", repository.remoteBranches.single().localName)

        assertEquals(
            GitAnswer.Overview(null),
            GitAnswer.decode("""{"kind":"git_overview","command_id":"$id","repository":null}""", id),
        )
        try {
            GitAnswer.decode("""{"kind":"error","command_id":"$id","code":"git_not_local"}""", id)
            fail("an error answer throws")
        } catch (error: ControllerConnectionException.HostError) {
            assertEquals("git_not_local", error.code)
        }
        try {
            GitAnswer.decode("""{"kind":"git_overview","command_id":"${UUID.randomUUID()}","repository":null}""", id)
            fail("an answer to another command is refused")
        } catch (_: IllegalArgumentException) {
        }
    }

    @Test
    fun requestsSayWhatTheyNeed() {
        val target = GitTarget("ABC", 4)
        assertFalse(GitRequest.Overview(null).requiresInput)
        assertTrue(GitRequest.Switch(null, "x").requiresInput)
        val log = GitRequest.Log("/w", 40, 40).command(target)
        assertEquals("git_log", log["kind"]!!.jsonPrimitive.content)
        assertEquals("abc", log["session_id"]!!.jsonPrimitive.content)
        assertEquals(40, log["offset"]!!.jsonPrimitive.int)
        assertEquals("null", GitRequest.Diff(null, null, null).command(target)["commit"].toString())
    }

    @Test
    fun patchLinesAreColouredByWhatTheyAre() {
        val lines = GitPatchLine.parse("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n same\n")
        assertEquals(
            listOf(
                GitPatchLine.Kind.FILE, GitPatchLine.Kind.FILE, GitPatchLine.Kind.FILE, GitPatchLine.Kind.HUNK,
                GitPatchLine.Kind.REMOVED, GitPatchLine.Kind.ADDED, GitPatchLine.Kind.CONTEXT,
            ),
            lines.map { it.kind },
        )
        assertEquals(" same", lines.last().text)
    }

    @Test
    fun loadsPagesAndSwitchesBranch() = runTest {
        val connection = FixtureGitConnection()
        val model = ControllerGitModel(this, host(), GitTarget(UUID.randomUUID().toString(), 3), "Tests", true, connection)
        model.load()
        advanceUntilIdle()
        assertEquals(GitLoadState.Loaded, model.state.value.load)
        assertEquals(2, model.state.value.commits.size)
        assertEquals(2, model.state.value.historyNextOffset)

        model.loadMoreHistory()
        advanceUntilIdle()
        assertEquals(3, model.state.value.commits.size)
        assertNull(model.state.value.historyNextOffset)

        model.switchTo(GitBranch(name = "origin/feature", remote = true))
        advanceUntilIdle()
        assertEquals(GitNotice.Switched("feature"), model.state.value.notice)
        assertTrue(GitRequest.Switch(null, "origin/feature") in connection.requests)
    }

    @Test
    fun aRouteWithoutGitSaysSo() = runTest {
        val model = ControllerGitModel(this, host(), GitTarget(UUID.randomUUID().toString(), 3), "Tests", false, null)
        model.load()
        advanceUntilIdle()
        assertEquals(GitLoadState.Failed("git_route_unavailable"), model.state.value.load)
        model.switchTo(GitBranch(name = "x"))
        advanceUntilIdle()
        assertNull(model.state.value.notice)
    }

    private fun host() = PairedHostRecord(
        id = "host",
        displayName = "Build Mac",
        route = HostRoute("192.168.1.10", 22_222),
        hostStaticPublicKey = Base64.getEncoder().encodeToString(ByteArray(32) { 7 }),
        deviceStaticKeyId = "controller.device.host",
        deviceId = UUID.randomUUID().toString(),
        identityGeneration = 1,
        revocationEpoch = 1,
        sessionGeneration = 1,
        capabilityBits = 6,
        pairedAtMillis = 1,
    )
}

/** Answers Git requests; anything else a test reaches is a mistake. */
private class FixtureGitConnection : ControllerConnecting by unsupportedConnection() {
    val requests = mutableListOf<GitRequest>()

    override suspend fun git(host: PairedHostRecord, target: GitTarget, request: GitRequest): GitAnswer {
        requests += request
        return when (request) {
            is GitRequest.Overview -> GitAnswer.Overview(GitRepository(folder = "/r", root = "/r", branch = "main"))
            is GitRequest.Log -> if (request.offset == 0) {
                GitAnswer.Log(listOf(commit("1"), commit("2")), 2)
            } else {
                GitAnswer.Log(listOf(commit("3")), null)
            }
            is GitRequest.Diff -> throw IllegalStateException("no diff here")
            is GitRequest.Switch -> GitAnswer.Switched(GitSwitchResult(switched = true, branch = request.branch))
        }
    }

    override suspend fun cancel() = Unit

    private fun commit(oid: String) = GitCommit(oid = oid, short = oid, subject = "s", author = "a")
}

private fun unsupportedConnection(): ControllerConnecting =
    Proxy.newProxyInstance(
        ControllerConnecting::class.java.classLoader,
        arrayOf(ControllerConnecting::class.java),
    ) { _, method, _ -> throw UnsupportedOperationException(method.name) } as ControllerConnecting
