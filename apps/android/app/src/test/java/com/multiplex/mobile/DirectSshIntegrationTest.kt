package com.multiplex.mobile

import com.multiplex.mobile.data.MobileAuthKind
import com.multiplex.mobile.data.MobileAuthMetadata
import com.multiplex.mobile.data.MobileHost
import com.multiplex.mobile.data.MobileKnownHost
import com.multiplex.mobile.data.MobilePersistentSession
import com.multiplex.mobile.ssh.DirectSshSessionClient
import com.multiplex.mobile.ssh.MobileSshSecretProvider
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File
import java.util.Base64
import java.util.Properties
import java.util.concurrent.CopyOnWriteArrayList

class DirectSshIntegrationTest {
    @Test
    fun directSshAttachesToPersistentTmuxSessionAndSurvivesReconnect() = runBlocking {
        val config = LiveSshSmokeConfig.load()
        assumeTrue(
            "Set MULTIPLEX_MOBILE_TEST_SSH_HOST, MULTIPLEX_MOBILE_TEST_SSH_PORT, MULTIPLEX_MOBILE_TEST_SSH_USER, MULTIPLEX_MOBILE_TEST_SSH_KEY, and MULTIPLEX_MOBILE_TEST_KNOWN_HOST_KEY to run this live SSH smoke.",
            config != null,
        )
        requireNotNull(config)

        val sessionName = "mobile-android-smoke"
        val secretRef = "termirust-mobile-test-private-key"
        val host = MobileHost(
            id = "android-live-ssh",
            label = "Android Live SSH",
            host = config.host,
            port = config.port,
            username = config.username,
            auth = MobileAuthMetadata(
                kind = MobileAuthKind.PrivateKey,
                secretRef = secretRef,
            ),
            persistentSession = MobilePersistentSession(
                enabled = true,
                sessionName = sessionName,
                detachOthers = false,
            ),
            knownHostEndpoint = "${config.host}:${config.port}",
        )
        val knownHost = MobileKnownHost(
            endpoint = "${config.host}:${config.port}",
            publicKey = config.knownHostKey,
        )

        val firstOutput = CopyOnWriteArrayList<ByteArray>()
        val firstClient = clientWithKey(secretRef, config.privateKey)
        try {
            firstClient.connect(host, knownHost) { firstOutput += it.copyOf() }
            // Split acknowledgement tokens so echoed input cannot satisfy the assertion.
            firstClient.send(
                ("[ \"\$(tmux display-message -p '#S')\" = '$sessionName' ] && " +
                    "export MULTIPLEX_SMOKE_VALUE=android-smoke-first && " +
                    "printf '%s\\n' \"\$MULTIPLEX_SMOKE_VALUE\" > ~/termirust-android-smoke && " +
                    "printf '%s%s\\n' WRITE_ CONFIRMED\n").encodeToByteArray(),
            )

            assertTrue(
                "first connection did not confirm its tmux session and marker write",
                waitForOutput(firstOutput, "WRITE_CONFIRMED"),
            )
        } finally {
            firstClient.disconnect()
            firstClient.close()
        }

        val secondOutput = CopyOnWriteArrayList<ByteArray>()
        val secondClient = clientWithKey(secretRef, config.privateKey)
        try {
            secondClient.connect(host, knownHost) { secondOutput += it.copyOf() }
            secondClient.send(
                ("[ \"\$MULTIPLEX_SMOKE_VALUE\" = android-smoke-first ] && " +
                    "[ \"\$(cat ~/termirust-android-smoke)\" = \"\$MULTIPLEX_SMOKE_VALUE\" ] && " +
                    "printf '%s%s\\n' RECONNECT_ CONFIRMED\n").encodeToByteArray(),
            )

            assertTrue(
                "reconnect did not preserve both the shell environment and marker file",
                waitForOutput(secondOutput, "RECONNECT_CONFIRMED"),
            )
        } finally {
            secondClient.disconnect()
            secondClient.close()
        }
    }

    private fun clientWithKey(secretRef: String, privateKey: String): DirectSshSessionClient =
        DirectSshSessionClient(
            secretProvider = MobileSshSecretProvider { reference ->
                if (reference == secretRef) privateKey.toCharArray() else null
            },
        )

    private suspend fun waitForOutput(
        chunks: List<ByteArray>,
        needle: String,
        attempts: Int = 80,
    ): Boolean {
        repeat(attempts) {
            if (chunks.joinToString(separator = "") { it.decodeToString() }.contains(needle)) {
                return true
            }
            delay(250)
        }
        return false
    }

    private data class LiveSshSmokeConfig(
        val host: String,
        val port: Int,
        val username: String,
        val privateKey: String,
        val knownHostKey: String,
    ) {
        companion object {
            fun load(): LiveSshSmokeConfig? {
                val values = fileValues().toMutableMap()
                values += System.getenv()
                System.getProperties().stringPropertyNames().forEach { name ->
                    System.getProperty(name)?.let { values[name] = it }
                }

                val host = values["MULTIPLEX_MOBILE_TEST_SSH_HOST"]?.trim().orEmpty()
                val port = values["MULTIPLEX_MOBILE_TEST_SSH_PORT"]?.trim()?.toIntOrNull()
                val username = values["MULTIPLEX_MOBILE_TEST_SSH_USER"]?.trim().orEmpty()
                val privateKey = decodedValue(values, "MULTIPLEX_MOBILE_TEST_SSH_KEY")
                val knownHostKey = decodedValue(values, "MULTIPLEX_MOBILE_TEST_KNOWN_HOST_KEY")
                if (host.isBlank() || port == null || username.isBlank() || privateKey.isBlank() || knownHostKey.isBlank()) {
                    return null
                }
                return LiveSshSmokeConfig(host, port, username, privateKey, knownHostKey)
            }

            private fun fileValues(): Map<String, String> {
                val file = File("app/src/test/.termirust-mobile-live-ssh.properties")
                if (!file.isFile) return emptyMap()
                val properties = Properties()
                file.inputStream().use { properties.load(it) }
                return properties.stringPropertyNames().associateWith { properties.getProperty(it) }
            }

            private fun decodedValue(values: Map<String, String>, key: String): String =
                values["${key}_BASE64"]
                    ?.let { Base64.getDecoder().decode(it) }
                    ?.toString(Charsets.UTF_8)
                    ?: values[key].orEmpty()
        }
    }
}
