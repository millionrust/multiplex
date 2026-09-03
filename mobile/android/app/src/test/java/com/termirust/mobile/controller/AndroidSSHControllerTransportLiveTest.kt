package com.termirust.mobile.controller

import com.termirust.mobile.security.MobileSecretStore
import org.junit.Assume.assumeTrue
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class AndroidSSHControllerTransportLiveTest {
    @Test
    fun pinnedPrivateKeyTransportExecutesFixedBridgeAndRoundTripsBytes() {
        val environment = liveEnvironment()
        assertRoundTrip(environment, ControllerSshAuthenticationKind.PRIVATE_KEY, environment.privateKey)
    }

    @Test
    fun pinnedPasswordTransportExecutesFixedBridgeAndRoundTripsBytes() {
        val environment = liveEnvironment()
        assertRoundTrip(environment, ControllerSshAuthenticationKind.PASSWORD, environment.password)
    }

    @Test
    fun wrongHostKeyIsRejected() {
        val environment = liveEnvironment()
        assertThrows(Exception::class.java) {
            makeTransport(
                environment,
                ControllerSshAuthenticationKind.PRIVATE_KEY,
                environment.privateKey,
                "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
        }
    }

    private fun liveEnvironment(): LiveSSHEnvironment {
        val port = System.getenv("TERMIRUST_MOBILE_CONTROLLER_SSH_PORT")?.toIntOrNull()
        val hostKey = System.getenv("TERMIRUST_MOBILE_CONTROLLER_SSH_HOST_KEY")
        val privateKey = System.getenv("TERMIRUST_MOBILE_CONTROLLER_SSH_PRIVATE_KEY")
        val password = System.getenv("TERMIRUST_MOBILE_CONTROLLER_SSH_PASSWORD")
        assumeTrue(
            "Run through scripts/test-mobile-controller-ssh-transports.sh",
            port != null && hostKey != null && privateKey != null && password != null,
        )
        return LiveSSHEnvironment(
            checkNotNull(port),
            checkNotNull(hostKey),
            checkNotNull(privateKey),
            checkNotNull(password),
        )
    }

    private fun assertRoundTrip(
        environment: LiveSSHEnvironment,
        authentication: ControllerSshAuthenticationKind,
        secret: String,
    ) {
        val transport = makeTransport(environment, authentication, secret, environment.hostKey)
        val payload = "termirust-mobile-controller-ssh\n".toByteArray()
        val reader = Executors.newSingleThreadExecutor()
        try {
            transport.output.write(payload)
            transport.output.flush()
            val response = reader.submit<ByteArray> {
                ByteArray(payload.size).also { bytes ->
                    var offset = 0
                    while (offset < bytes.size) {
                        val count = transport.input.read(bytes, offset, bytes.size - offset)
                        check(count >= 0) { "SSH Controller bridge closed before echoing the payload" }
                        offset += count
                    }
                }
            }.get(10, TimeUnit.SECONDS)
            assertArrayEquals(payload, response)
        } finally {
            transport.close()
            reader.shutdownNow()
        }
    }

    private fun makeTransport(
        environment: LiveSSHEnvironment,
        authentication: ControllerSshAuthenticationKind,
        secret: String,
        hostKey: String,
    ): ControllerDuplexTransport {
        val reference = ControllerRouteCredentialReference(
            id = "live-test",
            route = ControllerRemoteRouteKind.SSH,
            purpose = ControllerRouteCredentialPurpose.SSH_AUTHENTICATION,
        )
        val configuration = ControllerRemoteRouteConfiguration(
            kind = ControllerRemoteRouteKind.SSH,
            endpoint = "127.0.0.1",
            port = environment.port,
            username = "termirust",
            trustPin = hostKey,
            credential = reference,
            sshAuthentication = authentication,
        )
        val secrets = MemoryMobileSecretStore().apply {
            saveSecret(ControllerRouteCredentialStore(this).account("live-host", reference), secret)
        }
        return SshControllerTransportFactory(
            "live-host",
            configuration,
            ControllerRouteCredentialStore(secrets),
        ).open(HostRoute("ignored-by-explicit-ssh-route", 1))
    }
}

private data class LiveSSHEnvironment(
    val port: Int,
    val hostKey: String,
    val privateKey: String,
    val password: String,
)

private class MemoryMobileSecretStore : MobileSecretStore {
    private val secrets = mutableMapOf<String, String>()
    override fun saveSecret(account: String, secret: String) { secrets[account] = secret }
    override fun readSecret(account: String): String? = secrets[account]
    override fun deleteSecret(account: String) { secrets.remove(account) }
}
