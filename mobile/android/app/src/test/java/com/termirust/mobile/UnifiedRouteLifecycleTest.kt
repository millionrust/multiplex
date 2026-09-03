package com.termirust.mobile

import com.termirust.mobile.data.MobileHost
import com.termirust.mobile.data.MobileKnownHost
import com.termirust.mobile.ssh.MobileSshSessionClient
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class UnifiedRouteLifecycleTest {
    @Before
    fun setUp() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
    }

    @Test
    fun directSshBackgroundDropsInputAndDisconnectsWithoutReplay() = runTest {
        val client = LifecycleSshClient()
        val viewModel = MobileHostViewModel(sshClient = client)

        viewModel.onBackground()
        viewModel.sendTerminalBytes("must-not-send".encodeToByteArray())
        advanceUntilIdle()

        assertTrue(viewModel.privacyCovered.value)
        assertEquals(1, client.disconnectCount)
        assertTrue(client.sentBytes.isEmpty())

        viewModel.onForeground()
        advanceUntilIdle()

        assertFalse(viewModel.privacyCovered.value)
        assertTrue(client.sentBytes.isEmpty())
    }
}

private class LifecycleSshClient : MobileSshSessionClient {
    val sentBytes = mutableListOf<ByteArray>()
    var disconnectCount = 0

    override suspend fun connect(
        host: MobileHost,
        knownHost: MobileKnownHost?,
        onOutput: suspend (ByteArray) -> Unit,
    ) = Unit

    override suspend fun send(bytes: ByteArray) {
        sentBytes += bytes.copyOf()
    }

    override suspend fun resize(columns: Int, rows: Int) = Unit

    override suspend fun disconnect() {
        disconnectCount += 1
    }
}
