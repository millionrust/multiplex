package com.termirust.mobile.controller

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import androidx.annotation.RequiresApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.net.UnknownHostException
import java.security.MessageDigest

/** A computer announcing the Multiplex Controller listener on the local network. */
data class DiscoveredController(
    val serviceName: String,
    val discoveryId: String,
    val routes: List<HostRoute>,
)

data class ControllerEndpoint(val host: String, val port: Int)

sealed class ControllerPairingAddressException : Exception() {
    data object Invalid : ControllerPairingAddressException()
    data object Unresolved : ControllerPairingAddressException()
    data object NotPrivate : ControllerPairingAddressException()
}

internal object ControllerNetworkAddresses {
    private val FINGERPRINT_DOMAIN = "termirust-host-fingerprint-v1".encodeToByteArray() + byteArrayOf(0)
    private val IPV4_LITERAL = Regex("^(\\d{1,3})\\.(\\d{1,3})\\.(\\d{1,3})\\.(\\d{1,3})$")
    private val IPV6_LITERAL = Regex("^[0-9A-Fa-f:.]+$")
    private val HOST_NAME = Regex(
        "^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*\\.?$",
    )

    // Matches the Host's private-address rule: RFC 1918, Tailscale CGNAT, and IPv6 ULA.
    fun isPrivate(address: InetAddress): Boolean {
        if (address.isLoopbackAddress || address.isAnyLocalAddress || address.isMulticastAddress) return false
        val bytes = address.address.map(Byte::toInt).map { it and 0xff }
        return when (address) {
            is Inet4Address ->
                bytes[0] == 10 || bytes[0] == 172 && bytes[1] in 16..31 ||
                    bytes[0] == 192 && bytes[1] == 168 || bytes[0] == 100 && bytes[1] in 64..127
            is Inet6Address -> bytes[0] and 0xfe == 0xfc
            else -> false
        }
    }

    fun isPrivateLiteralRoute(route: HostRoute): Boolean =
        literalAddress(route.address)?.let(::isPrivate) == true

    // Parses an IP literal without ever falling back to a DNS lookup.
    fun literalAddress(value: String): InetAddress? {
        IPV4_LITERAL.matchEntire(value)?.let { match ->
            val octets = match.groupValues.drop(1).map { it.toInt() }
            if (octets.any { it > 255 }) return null
            return InetAddress.getByAddress(octets.map(Int::toByte).toByteArray())
        }
        if (':' !in value || !IPV6_LITERAL.matches(value)) return null
        return runCatching { InetAddress.getByName(value) }.getOrNull()?.takeIf { it is Inet6Address }
    }

    fun discoveryId(hostStaticPublicKey: ByteArray): String {
        require(hostStaticPublicKey.size == 32)
        val digest = MessageDigest.getInstance("SHA-256")
        digest.update(FINGERPRINT_DOMAIN)
        digest.update(hostStaticPublicKey)
        return digest.digest().copyOf(16).joinToString("") { "%02x".format(it) }
    }

    // The same label the desktop announces over Bonjour.
    fun defaultHostName(discoveryId: String): String = "Multiplex ${discoveryId.take(6).uppercase()}"

    fun parseEndpoint(text: String): ControllerEndpoint {
        val value = text.trim()
        if (value.isEmpty() || value.length > 300) throw ControllerPairingAddressException.Invalid
        val host: String
        val portText: String
        if (value.startsWith("[")) {
            val close = value.indexOf(']')
            if (close < 0 || value.getOrNull(close + 1) != ':') throw ControllerPairingAddressException.Invalid
            host = value.substring(1, close)
            portText = value.substring(close + 2)
            if (literalAddress(host) !is Inet6Address) throw ControllerPairingAddressException.Invalid
        } else {
            val colon = value.lastIndexOf(':')
            if (colon <= 0 || value.indexOf(':') != colon) throw ControllerPairingAddressException.Invalid
            host = value.substring(0, colon)
            portText = value.substring(colon + 1)
            if (host.length > 253 || !HOST_NAME.matches(host) && !IPV4_LITERAL.matches(host)) {
                throw ControllerPairingAddressException.Invalid
            }
        }
        val port = portText.takeIf { it.length in 1..5 && it.all(Char::isDigit) }?.toInt()
            ?: throw ControllerPairingAddressException.Invalid
        if (port !in 1..65_535) throw ControllerPairingAddressException.Invalid
        return ControllerEndpoint(host, port)
    }

    // Blocking: a host name goes through the system resolver, so call this off the main thread.
    fun resolve(endpoint: ControllerEndpoint): List<HostRoute> {
        val addresses = literalAddress(endpoint.host)?.let(::listOf) ?: try {
            InetAddress.getAllByName(endpoint.host).toList()
        } catch (_: UnknownHostException) {
            throw ControllerPairingAddressException.Unresolved
        } catch (_: SecurityException) {
            throw ControllerPairingAddressException.Unresolved
        }
        val routes = addresses
            .filter(::isPrivate)
            .mapNotNull { address -> address.hostAddress?.substringBefore('%') }
            .distinct()
            .take(ControllerLimits.MAX_HOST_ROUTES)
            .map { HostRoute(it, endpoint.port) }
        if (routes.isEmpty()) throw ControllerPairingAddressException.NotPrivate
        return routes
    }
}

/**
 * Browses `_multiplex._tcp` with [NsdManager]. Several callers can share one browse: each
 * [start] must be balanced by a [stop]. Finding a computer is not trusting it; pairing still
 * needs the code, and a paired Host is still authenticated by its static key.
 */
class ControllerHostDiscovery(context: Context) : AutoCloseable {
    private val context = context.applicationContext
    private val manager = this.context.getSystemService(NsdManager::class.java)
    private val lock = Any()
    private var users = 0
    private var listener: NsdManager.DiscoveryListener? = null
    private val found = linkedMapOf<String, DiscoveredController>()
    private val pendingResolves = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private val infoCallbacks = mutableMapOf<String, Any>()
    private val _computers = MutableStateFlow<List<DiscoveredController>>(emptyList())
    val computers: StateFlow<List<DiscoveredController>> = _computers.asStateFlow()

    fun start() {
        synchronized(lock) { startLocked() }
    }

    fun stop() {
        synchronized(lock) {
            users = (users - 1).coerceAtLeast(0)
            if (users == 0) stopLocked()
        }
    }

    /** Browses until a computer with [discoveryId] appears or [timeoutMillis] passes. */
    suspend fun find(discoveryId: String, timeoutMillis: Long): DiscoveredController? {
        start()
        return try {
            withTimeoutOrNull(timeoutMillis) {
                computers.first { list -> list.any { it.discoveryId == discoveryId } }
                    .first { it.discoveryId == discoveryId }
            }
        } finally {
            stop()
        }
    }

    override fun close() {
        synchronized(lock) {
            users = 0
            stopLocked()
        }
    }

    private fun startLocked() {
        users += 1
        val manager = manager ?: return
        if (listener != null) return
        val next = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) = Unit
            override fun onDiscoveryStopped(serviceType: String) = Unit

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                synchronized(lock) { if (listener === this) listener = null }
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) = Unit

            override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                synchronized(lock) {
                    if (listener !== this) return
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                        watchService(serviceInfo)
                    } else {
                        pendingResolves.addLast(serviceInfo)
                        resolveNextLocked()
                    }
                }
            }

            override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                synchronized(lock) {
                    if (listener !== this) return
                    forgetServiceLocked(serviceInfo.serviceName)
                }
            }
        }
        listener = next
        try {
            manager.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, next)
        } catch (_: RuntimeException) {
            listener = null
        }
    }

    private fun stopLocked() {
        val current = listener ?: return
        listener = null
        runCatching { manager?.stopServiceDiscovery(current) }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            infoCallbacks.values.forEach(::unwatchService)
        }
        infoCallbacks.clear()
        pendingResolves.clear()
        resolving = false
        found.clear()
        publishLocked()
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun watchService(serviceInfo: NsdServiceInfo) {
        val name = serviceInfo.serviceName
        if (infoCallbacks.containsKey(name)) return
        val callback = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                synchronized(lock) { if (infoCallbacks[name] === this) infoCallbacks.remove(name) }
            }

            override fun onServiceUpdated(serviceInfo: NsdServiceInfo) {
                synchronized(lock) {
                    if (infoCallbacks[name] !== this) return
                    acceptLocked(name, serviceInfo, serviceInfo.hostAddresses)
                }
            }

            override fun onServiceLost() {
                synchronized(lock) {
                    if (infoCallbacks[name] !== this) return
                    found.remove(name)
                    publishLocked()
                }
            }

            override fun onServiceInfoCallbackUnregistered() = Unit
        }
        infoCallbacks[name] = callback
        runCatching { manager?.registerServiceInfoCallback(serviceInfo, context.mainExecutor, callback) }
            .onFailure { infoCallbacks.remove(name) }
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun unwatchService(callback: Any) {
        runCatching { manager?.unregisterServiceInfoCallback(callback as NsdManager.ServiceInfoCallback) }
    }

    private fun forgetServiceLocked(name: String) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            infoCallbacks.remove(name)?.let(::unwatchService)
        }
        pendingResolves.removeAll { it.serviceName == name }
        if (found.remove(name) != null) publishLocked()
    }

    // Before Android 14 NsdManager resolves one service at a time.
    @Suppress("DEPRECATION")
    private fun resolveNextLocked() {
        if (resolving) return
        val next = pendingResolves.removeFirstOrNull() ?: return
        val owner = listener ?: return
        resolving = true
        val resolveListener = object : NsdManager.ResolveListener {
            override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                synchronized(lock) {
                    resolving = false
                    if (listener === owner) resolveNextLocked()
                }
            }

            override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                synchronized(lock) {
                    resolving = false
                    if (listener !== owner) return
                    acceptLocked(serviceInfo.serviceName, serviceInfo, listOfNotNull(serviceInfo.host))
                    resolveNextLocked()
                }
            }
        }
        runCatching { manager?.resolveService(next, resolveListener) }.onFailure {
            resolving = false
            resolveNextLocked()
        }
    }

    private fun acceptLocked(name: String, serviceInfo: NsdServiceInfo, addresses: List<InetAddress>) {
        val attributes = serviceInfo.attributes
        val version = attributes["v"]?.decodeToString()
        val discoveryId = attributes["id"]?.decodeToString()
        val port = serviceInfo.port
        val routes = addresses
            .filter(ControllerNetworkAddresses::isPrivate)
            .mapNotNull { address -> address.hostAddress?.substringBefore('%') }
            .distinct()
            .take(ControllerLimits.MAX_HOST_ROUTES)
        if (version != "1" || discoveryId == null || !DISCOVERY_ID_PATTERN.matches(discoveryId) ||
            port !in 1..65_535 || routes.isEmpty() || name.isBlank()
        ) {
            if (found.remove(name) != null) publishLocked()
            return
        }
        found[name] = DiscoveredController(name.take(64), discoveryId, routes.map { HostRoute(it, port) })
        publishLocked()
    }

    private fun publishLocked() {
        _computers.value = found.values.sortedWith(compareBy({ it.serviceName.lowercase() }, { it.discoveryId }))
    }

    private companion object {
        const val SERVICE_TYPE = "_multiplex._tcp."
    }
}
