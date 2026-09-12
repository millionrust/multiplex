package com.termirust.mobile.ssh

import com.termirust.mobile.data.MobileAuthKind
import com.termirust.mobile.data.MobileHost
import com.termirust.mobile.data.MobileKnownHost
import com.hierynomus.sshj.userauth.keyprovider.OpenSSHKeyV1KeyFile
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import net.schmizz.sshj.SSHClient
import net.schmizz.sshj.connection.channel.direct.Session
import net.schmizz.sshj.transport.verification.HostKeyVerifier
import net.schmizz.sshj.transport.verification.OpenSSHKnownHosts
import net.schmizz.sshj.userauth.keyprovider.OpenSSHKeyFile
import net.schmizz.sshj.userauth.password.PasswordFinder
import net.schmizz.sshj.userauth.password.Resource
import java.io.StringReader
import java.security.PublicKey
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

sealed interface TerminalConnectionState {
    data object Disconnected : TerminalConnectionState
    data object Connecting : TerminalConnectionState
    data object Connected : TerminalConnectionState
    data class Failed(val message: String) : TerminalConnectionState
}

interface MobileSshSessionClient {
    suspend fun connect(
        host: MobileHost,
        knownHost: MobileKnownHost?,
        onOutput: suspend (ByteArray) -> Unit,
    )

    suspend fun send(bytes: ByteArray)
    suspend fun resize(columns: Int, rows: Int)
    suspend fun disconnect()
}

fun interface MobileSshSecretProvider {
    suspend fun secretFor(reference: String): CharArray?
}

class DirectSshSessionClient(
    private val secretProvider: MobileSshSecretProvider = MobileSshSecretProvider { null },
    private val defaultColumns: Int = 80,
    private val defaultRows: Int = 24,
) : MobileSshSessionClient {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var ssh: SSHClient? = null
    private var session: Session? = null
    private var shell: Session.Shell? = null
    private var readerJob: Job? = null

    override suspend fun connect(
        host: MobileHost,
        knownHost: MobileKnownHost?,
        onOutput: suspend (ByteArray) -> Unit,
    ) {
        disconnect()

        val pin = requireNotNull(knownHost) {
            "No known-host pin exists for ${host.knownHostEndpoint ?: "${host.host}:${host.port}"}."
        }
        val client = SSHClient()
        client.addHostKeyVerifier(PinnedKnownHostVerifier(host, pin))

        withContext(Dispatchers.IO) {
            client.connect(host.host, host.port)
            authenticate(client, host)

            val openedSession = client.startSession()
            openedSession.allocatePTY("xterm-256color", defaultColumns, defaultRows, 0, 0, emptyMap())
            val openedShell = openedSession.startShell()

            ssh = client
            session = openedSession
            shell = openedShell
            readerJob = scope.launch {
                val buffer = ByteArray(8192)
                val input = openedShell.inputStream
                while (isActive && openedShell.isOpen) {
                    val read = input.read(buffer)
                    if (read < 0) {
                        break
                    }
                    if (read > 0) {
                        onOutput(buffer.copyOf(read))
                    }
                }
            }

            TmuxBootstrap(host).startupCommand()?.let { startup ->
                openedShell.outputStream.write(startup.encodeToByteArray())
                openedShell.outputStream.write('\n'.code)
                openedShell.outputStream.flush()
            }
            waitForPersistentSessionAttachment(client, host)
        }
    }

    override suspend fun send(bytes: ByteArray) {
        withContext(Dispatchers.IO) {
            val currentShell = requireNotNull(shell) { "SSH session is not connected." }
            currentShell.outputStream.write(bytes)
            currentShell.outputStream.flush()
        }
    }

    override suspend fun resize(columns: Int, rows: Int) {
        withContext(Dispatchers.IO) {
            shell?.changeWindowDimensions(columns, rows, 0, 0)
        }
    }

    override suspend fun disconnect() {
        withContext(Dispatchers.IO) {
            readerJob?.cancel()
            readerJob = null
            runCatching { shell?.close() }
            runCatching { session?.close() }
            runCatching { ssh?.disconnect() }
            runCatching { ssh?.close() }
            shell = null
            session = null
            ssh = null
        }
    }

    fun close() {
        scope.cancel()
    }

    private suspend fun authenticate(client: SSHClient, host: MobileHost) {
        val secretRef = host.auth.secretRef
            ?: error("No ${host.auth.kind.name.lowercase()} secret is stored for ${host.label}.")
        val secret = secretProvider.secretFor(secretRef)
            ?: error("Missing mobile secret '$secretRef' for ${host.label}.")

        try {
            when (host.auth.kind) {
                MobileAuthKind.Password -> client.authPassword(host.username, secret)
                MobileAuthKind.PrivateKey -> {
                    val privateKey = secret.concatToString()
                    val passphraseFinder = StaticPasswordFinder(null)
                    val provider = runCatching {
                        OpenSSHKeyV1KeyFile().apply {
                            init(privateKey, null, passphraseFinder)
                        }
                    }.getOrElse {
                        OpenSSHKeyFile().apply {
                            init(privateKey, null, passphraseFinder)
                        }
                    }
                    client.authPublickey(host.username, provider)
                }
            }
        } finally {
            secret.fill('\u0000')
        }
    }

    private fun waitForPersistentSessionAttachment(client: SSHClient, host: MobileHost) {
        if (!host.persistentSession.enabled || !remoteTmuxAvailable(client)) {
            return
        }

        val session = shellSingleQuote(TmuxBootstrap(host).sessionName())
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(TMUX_ATTACH_TIMEOUT_SECONDS)
        do {
            val attachedClients = runRemoteCommand(
                client,
                "tmux display-message -p -t $session '#{session_attached}' 2>/dev/null",
            ).output.trim().toIntOrNull() ?: 0
            if (attachedClients > 0) {
                return
            }
            Thread.sleep(TMUX_ATTACH_POLL_MILLIS)
        } while (System.nanoTime() < deadline)

        error("Timed out waiting for tmux session '${TmuxBootstrap(host).sessionName()}' to attach.")
    }

    private fun remoteTmuxAvailable(client: SSHClient): Boolean =
        runCatching {
            runRemoteCommand(client, "command -v tmux >/dev/null 2>&1").exitStatus == 0
        }.getOrDefault(false)

    private fun runRemoteCommand(client: SSHClient, commandText: String): RemoteCommandResult =
        client.startSession().use { probeSession ->
            probeSession.exec(commandText).use { command ->
                val output = command.inputStream.bufferedReader().use { it.readText() }
                command.join(REMOTE_COMMAND_TIMEOUT_SECONDS, TimeUnit.SECONDS)
                RemoteCommandResult(command.exitStatus, output)
            }
        }

    private data class RemoteCommandResult(
        val exitStatus: Int?,
        val output: String,
    )

    private companion object {
        const val TMUX_ATTACH_TIMEOUT_SECONDS = 5L
        const val TMUX_ATTACH_POLL_MILLIS = 50L
        const val REMOTE_COMMAND_TIMEOUT_SECONDS = 2L
    }
}

private class StaticPasswordFinder(
    private val passphrase: CharArray?,
) : PasswordFinder {
    private val retried = AtomicBoolean(false)

    override fun reqPassword(resource: Resource<*>?): CharArray? = passphrase

    override fun shouldRetry(resource: Resource<*>?): Boolean =
        passphrase != null && retried.compareAndSet(false, true)
}

internal class PinnedKnownHostVerifier(
    host: MobileHost,
    knownHost: MobileKnownHost,
) : HostKeyVerifier {
    private val verifier: OpenSSHKnownHosts? = knownHost.publicKey
        .takeIf { it.isNotBlank() }
        ?.let { publicKey ->
            val target = if (host.port == 22) host.host else "[${host.host}]:${host.port}"
            OpenSSHKnownHosts(StringReader("$target $publicKey\n"))
        }
    private val expectedFingerprint = knownHost.fingerprint?.trim()?.takeIf { it.isNotEmpty() }

    override fun verify(hostname: String, port: Int, key: PublicKey): Boolean {
        if (verifier?.verify(hostname, port, key) == true) {
            return true
        }
        val actualFingerprint = net.schmizz.sshj.common.SecurityUtils.getFingerprint(key)
        return expectedFingerprint != null && expectedFingerprint == actualFingerprint
    }

    override fun findExistingAlgorithms(hostname: String, port: Int): MutableList<String> =
        verifier?.findExistingAlgorithms(hostname, port)?.toMutableList() ?: mutableListOf()
}
