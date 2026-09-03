package com.termirust.mobile.controller

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.termirust.mobile.security.MobileSecretStore
import okhttp3.OkHttpClient
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.security.KeyStore
import java.security.cert.CertificateFactory
import java.util.Base64
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509TrustManager

@RunWith(AndroidJUnit4::class)
class RelayControllerTransportInstrumentedTest {
    @Test
    fun productionRelayTransportEchoesAcrossFreshReconnect() {
        val assets = InstrumentationRegistry.getInstrumentation().context.assets
        val packageText = assets.open("relay-live.json").bufferedReader().use { it.readText() }
        assumeTrue(
            "Run through scripts/test-mobile-android-relay-transport.sh",
            !packageText.contains("\"schema_version\":0"),
        )
        val routePackage = ControllerRelayRoutePackage.decode(packageText)
        val caDer = Base64.getDecoder().decode(
            assets.open("relay-live-ca.txt").bufferedReader().use { it.readText().trim() },
        )
        val clientBuilders = trustedClientBuilders(caDer)
        val reference = ControllerRouteCredentialReference(
            id = "live-relay",
            route = ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
            purpose = ControllerRouteCredentialPurpose.RELAY_ADMISSION,
        )
        val configuration = ControllerRemoteRouteConfiguration(
            kind = ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
            endpoint = routePackage.endpoint,
            trustPin = routePackage.spkiPin,
            credential = reference,
            relayRouteId = routePackage.relayRouteId,
            relayRevocationEpoch = routePackage.relayRevocationEpoch,
        )
        val secrets = MemoryRelaySecretStore()
        val credentials = ControllerRouteCredentialStore(secrets)
        credentials.save("live-relay-host", reference, routePackage.admissionCredential)
        val factory = RelayControllerTransportFactory.create(
            hostId = "live-relay-host",
            configuration = configuration,
            credentials = credentials,
            clientBuilderFactory = clientBuilders,
        )

        assertTrue("mobile relay JNI library must load", NativeRelayProtocol.loaded)
        for (payload in listOf("first-connection", "second-connection")) {
            assertEchoWithRetry(payload.encodeToByteArray(), factory)
        }
    }

    private fun assertEchoWithRetry(payload: ByteArray, factory: ControllerTransportFactory) {
        var lastError: Throwable? = null
        repeat(8) { attempt ->
            try {
                assertEcho(payload, factory)
                return
            } catch (error: Throwable) {
                lastError = error
                Thread.sleep(100L * (attempt + 1))
            }
        }
        throw AssertionError("relay reconnect did not produce an exact echo", lastError)
    }

    private fun assertEcho(payload: ByteArray, factory: ControllerTransportFactory) {
        val transport = factory.open(HostRoute("unused.invalid", 1))
        val reader = Executors.newSingleThreadExecutor()
        try {
            transport.output.write(payload)
            transport.output.flush()
            val response = reader.submit<ByteArray> {
                ByteArray(payload.size).also { bytes ->
                    var offset = 0
                    while (offset < bytes.size) {
                        val read = transport.input.read(bytes, offset, bytes.size - offset)
                        check(read >= 0) { "relay transport closed before the complete echo" }
                        offset += read
                    }
                }
            }.get(10, TimeUnit.SECONDS)
            assertArrayEquals(payload, response)
        } finally {
            transport.close()
            reader.shutdownNow()
        }
    }

    private fun trustedClientBuilders(caDer: ByteArray): RelayHttpClientBuilderFactory {
        val certificate = CertificateFactory.getInstance("X.509")
            .generateCertificate(caDer.inputStream())
        val keyStore = KeyStore.getInstance(KeyStore.getDefaultType()).apply {
            load(null)
            setCertificateEntry("termirust-relay-test-ca", certificate)
        }
        val trustManagerFactory = TrustManagerFactory.getInstance(
            TrustManagerFactory.getDefaultAlgorithm(),
        ).apply { init(keyStore) }
        val trustManager = trustManagerFactory.trustManagers
            .filterIsInstance<X509TrustManager>()
            .single()
        return RelayHttpClientBuilderFactory {
            val context = SSLContext.getInstance("TLS").apply {
                init(null, arrayOf(trustManager), null)
            }
            OkHttpClient.Builder().sslSocketFactory(context.socketFactory, trustManager)
        }
    }
}

private class MemoryRelaySecretStore : MobileSecretStore {
    private val values = mutableMapOf<String, String>()

    override fun saveSecret(account: String, secret: String) {
        values[account] = secret
    }

    override fun readSecret(account: String): String? = values[account]

    override fun deleteSecret(account: String) {
        values.remove(account)
    }
}
