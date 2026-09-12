package com.termirust.mobile.controller

import com.hierynomus.sshj.userauth.keyprovider.OpenSSHKeyV1KeyFile
import com.termirust.mobile.security.MobileSecretStore
import net.schmizz.sshj.SSHClient
import net.schmizz.sshj.connection.channel.direct.Session
import net.schmizz.sshj.transport.verification.HostKeyVerifier
import net.schmizz.sshj.transport.verification.OpenSSHKnownHosts
import net.schmizz.sshj.userauth.keyprovider.OpenSSHKeyFile
import net.schmizz.sshj.userauth.password.PasswordFinder
import net.schmizz.sshj.userauth.password.Resource
import java.io.InputStream
import java.io.OutputStream
import java.io.StringReader
import java.security.PublicKey

internal class SshControllerTransportFactory(
    private val hostId: String,
    private val configuration: ControllerRemoteRouteConfiguration,
    private val credentialStore: ControllerRouteCredentialStore,
) : ControllerTransportFactory {
    init {
        configuration.validate()
        require(configuration.kind == ControllerRemoteRouteKind.SSH)
    }

    override fun open(route: HostRoute): ControllerDuplexTransport {
        val reference = checkNotNull(configuration.credential)
        val credential = credentialStore.read(hostId, reference)?.toCharArray()
            ?: error("SSH Controller credential is unavailable.")
        val client = SSHClient().apply {
            connectTimeout = CONNECT_TIMEOUT_MILLIS
            timeout = READ_TIMEOUT_MILLIS
            addHostKeyVerifier(ControllerSshHostKeyVerifier(configuration))
        }
        var session: Session? = null
        var command: Session.Command? = null
        try {
            client.connect(configuration.endpoint, checkNotNull(configuration.port))
            authenticate(client, credential)
            session = client.startSession()
            command = session.exec(REMOTE_COMMAND)
            return SshControllerTransport(client, session, command)
        } catch (error: Throwable) {
            runCatching { command?.close() }
            runCatching { session?.close() }
            runCatching { client.disconnect() }
            runCatching { client.close() }
            throw error
        } finally {
            credential.fill('\u0000')
        }
    }

    private fun authenticate(client: SSHClient, credential: CharArray) {
        when (checkNotNull(configuration.sshAuthentication)) {
            ControllerSshAuthenticationKind.PASSWORD ->
                client.authPassword(checkNotNull(configuration.username), credential)
            ControllerSshAuthenticationKind.PRIVATE_KEY -> {
                val privateKey = credential.concatToString()
                val finder = NoPassphraseFinder
                val provider = runCatching {
                    OpenSSHKeyV1KeyFile().apply { init(privateKey, null, finder) }
                }.getOrElse {
                    OpenSSHKeyFile().apply { init(privateKey, null, finder) }
                }
                client.authPublickey(checkNotNull(configuration.username), provider)
            }
        }
    }

    companion object {
        const val REMOTE_COMMAND = "termirust controller-bridge --stdio"
        private const val CONNECT_TIMEOUT_MILLIS = 10_000
        private const val READ_TIMEOUT_MILLIS = 30_000
    }
}

internal class SshControllerConnection(
    blobStore: ControllerSecureBlobStore,
    hostId: String,
    configuration: ControllerRemoteRouteConfiguration,
    secrets: MobileSecretStore,
) : ControllerConnecting {
    private val delegate = ControllerConnection(
        blobStore = blobStore,
        transportFactory = SshControllerTransportFactory(
            hostId,
            configuration,
            ControllerRouteCredentialStore(secrets),
        ),
    )

    override suspend fun beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceId: java.util.UUID,
    ): ControllerPairingChallenge = error("Pair on the private-network route before configuring SSH Controller.")

    override suspend fun finishPairing(matches: Boolean): PairedHostRecord =
        error("Pair on the private-network route before configuring SSH Controller.")

    override suspend fun fetchSessions(
        host: PairedHostRecord,
        progress: suspend (ControllerConnectionState) -> Unit,
    ) = delegate.fetchSessions(host, progress)

    override suspend fun attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        onEvent: suspend (ReadOnlyWireEvent) -> Unit,
    ) = delegate.attachReadOnly(host, cursor, viewport, onEvent)

    override suspend fun attachInteractive(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        onEvent: suspend (ReadOnlyWireEvent) -> Unit,
    ) = delegate.attachInteractive(host, cursor, viewport, onEvent)

    override suspend fun requestWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: java.util.UUID,
    ) = delegate.requestWriter(host, identity, commandId)

    override suspend fun releaseWriter(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: java.util.UUID,
    ) = delegate.releaseWriter(host, identity, commandId)

    override suspend fun sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: java.util.UUID,
        bytes: ByteArray,
    ) = delegate.sendInput(host, identity, commandId, bytes)

    override suspend fun sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: java.util.UUID,
        viewport: TerminalViewport,
    ) = delegate.sendResize(host, identity, commandId, viewport)

    override suspend fun cancel() = delegate.cancel()
    override fun close() = delegate.close()
}

private class SshControllerTransport(
    private val client: SSHClient,
    private val session: Session,
    private val command: Session.Command,
) : ControllerDuplexTransport {
    override val input: InputStream = command.inputStream
    override val output: OutputStream = command.outputStream

    override fun close() {
        runCatching { command.close() }
        runCatching { session.close() }
        runCatching { client.disconnect() }
        runCatching { client.close() }
    }
}

private class ControllerSshHostKeyVerifier(
    configuration: ControllerRemoteRouteConfiguration,
) : HostKeyVerifier {
    private val endpoint = configuration.endpoint
    private val port = checkNotNull(configuration.port)
    private val expected = checkNotNull(configuration.trustPin).trim()
    private val knownHosts = expected.takeIf { it.startsWith("ssh-") || it.startsWith("ecdsa-") }
        ?.let { key ->
            val host = if (port == 22) endpoint else "[$endpoint]:$port"
            OpenSSHKnownHosts(StringReader("$host $key\n"))
        }

    override fun verify(hostname: String, port: Int, key: PublicKey): Boolean {
        if (hostname != endpoint || port != this.port) return false
        if (knownHosts?.verify(hostname, port, key) == true) return true
        return net.schmizz.sshj.common.SecurityUtils.getFingerprint(key) == expected
    }

    override fun findExistingAlgorithms(hostname: String, port: Int): MutableList<String> =
        knownHosts?.findExistingAlgorithms(hostname, port)?.toMutableList() ?: mutableListOf()
}

private object NoPassphraseFinder : PasswordFinder {
    override fun reqPassword(resource: Resource<*>?): CharArray? = null
    override fun shouldRetry(resource: Resource<*>?): Boolean = false
}
