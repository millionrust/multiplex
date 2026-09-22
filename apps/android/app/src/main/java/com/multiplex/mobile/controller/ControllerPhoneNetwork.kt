package com.multiplex.mobile.controller

import android.content.Context
import android.content.SharedPreferences
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import com.multiplex.controller.security.PhoneAddress
import com.multiplex.controller.security.PhoneLink
import com.multiplex.controller.security.PhoneNetwork
import com.multiplex.controller.security.networkFingerprint
import java.net.Inet4Address
import java.security.SecureRandom
import java.util.Base64

/** The phone's current physical network, as the route planner needs it. */
internal fun interface ControllerPhoneNetworkSource {
    fun current(): PhoneNetwork

    companion object {
        /** For transports that do not connect by address: nothing is known about the network. */
        val Unknown = ControllerPhoneNetworkSource { PhoneNetwork(PhoneLink.OTHER, emptyList(), null) }
    }
}

/**
 * Reads the active network from ConnectivityManager. When a VPN such as Tailscale is the active
 * network, the network under it is read instead: the planner wants the Wi-Fi the phone is on, not
 * the tunnel's own address.
 */
internal class AndroidPhoneNetworkSource(
    context: Context,
    private val preferences: SharedPreferences,
) : ControllerPhoneNetworkSource {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)

    override fun current(): PhoneNetwork = runCatching(::read)
        .getOrElse { PhoneNetwork(PhoneLink.OTHER, emptyList(), null) }

    private fun read(): PhoneNetwork {
        val manager = connectivity ?: return OFFLINE
        val network = physicalNetwork(manager) ?: return OFFLINE
        val capabilities = manager.getNetworkCapabilities(network) ?: return OFFLINE
        val link = when {
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> PhoneLink.WIFI
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> PhoneLink.ETHERNET
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> PhoneLink.CELLULAR
            else -> PhoneLink.OTHER
        }
        val properties = manager.getLinkProperties(network)
        val addresses = properties?.linkAddresses.orEmpty().map { address ->
            PhoneAddress(address.address.hostAddress.orEmpty(), address.prefixLength.toUByte())
        }
        val gateway = properties?.routes.orEmpty()
            .firstOrNull { it.isDefaultRoute && it.gateway is Inet4Address }
            ?.gateway
            ?.hostAddress
        return PhoneNetwork(link, addresses, networkFingerprint(salt(), link, addresses, gateway))
    }

    private fun physicalNetwork(manager: ConnectivityManager): Network? {
        val active = manager.activeNetwork
        val activeCapabilities = active?.let(manager::getNetworkCapabilities)
        if (active != null && activeCapabilities?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) != true) {
            return active
        }
        @Suppress("DEPRECATION")
        val candidates = manager.allNetworks.filter { network ->
            val capabilities = manager.getNetworkCapabilities(network) ?: return@filter false
            capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
                !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN)
        }
        return candidates.firstOrNull { network ->
            val capabilities = manager.getNetworkCapabilities(network)
            capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true ||
                capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) == true
        } ?: candidates.firstOrNull()
    }

    // Kept per install so a network's name means nothing outside this phone. Not a secret.
    private fun salt(): ByteArray {
        preferences.getString(SALT_KEY, null)
            ?.let { runCatching { Base64.getDecoder().decode(it) }.getOrNull() }
            ?.takeIf { it.size == SALT_BYTES }
            ?.let { return it }
        val salt = ByteArray(SALT_BYTES).also(SecureRandom()::nextBytes)
        preferences.edit().putString(SALT_KEY, Base64.getEncoder().encodeToString(salt)).apply()
        return salt
    }

    private companion object {
        const val SALT_KEY = "route_network_salt"
        const val SALT_BYTES = 16
        val OFFLINE = PhoneNetwork(PhoneLink.OFFLINE, emptyList(), null)
    }
}
