package com.termirust.mobile.controller

import com.termirust.mobile.security.MobileSecretStore
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import java.net.URI

@Serializable
enum class ControllerRouteCredentialPurpose {
    @SerialName("ssh_authentication") SSH_AUTHENTICATION,
    @SerialName("relay_admission") RELAY_ADMISSION,
}

@Serializable
data class ControllerRouteCredentialReference(
    val id: String,
    val route: ControllerRemoteRouteKind,
    val purpose: ControllerRouteCredentialPurpose,
) {
    init {
        require(id.isSafeRouteIdentifier())
        require(
            (route == ControllerRemoteRouteKind.SSH && purpose == ControllerRouteCredentialPurpose.SSH_AUTHENTICATION) ||
                (route == ControllerRemoteRouteKind.SELF_HOSTED_RELAY && purpose == ControllerRouteCredentialPurpose.RELAY_ADMISSION),
        )
    }
}

@Serializable
data class ControllerRemoteRouteConfiguration(
    val kind: ControllerRemoteRouteKind,
    val endpoint: String,
    val port: Int? = null,
    val username: String? = null,
    @SerialName("trust_pin") val trustPin: String? = null,
    val credential: ControllerRouteCredentialReference? = null,
) {
    fun validate() {
        require(endpoint.isNotBlank() && endpoint.toByteArray().size <= 2_048 && endpoint.none(Char::isISOControl))
        when (kind) {
            ControllerRemoteRouteKind.LOCAL_IPC -> error("local IPC is not an Android route configuration")
            ControllerRemoteRouteKind.PRIVATE_NETWORK -> {
                require(port in 1..65_535 && username == null && trustPin == null && credential == null)
                HostRoute(endpoint, port!!)
            }
            ControllerRemoteRouteKind.SSH -> {
                require(port in 1..65_535)
                require(!username.isNullOrBlank() && username.toByteArray().size <= 255 && username.none(Char::isISOControl))
                require(!trustPin.isNullOrBlank() && trustPin.toByteArray().size <= 512 && trustPin.none(Char::isISOControl))
                require(credential?.route == kind && credential.purpose == ControllerRouteCredentialPurpose.SSH_AUTHENTICATION)
                HostRoute(endpoint, port!!)
            }
            ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> {
                require(port == null && username == null)
                require(!trustPin.isNullOrBlank() && trustPin.toByteArray().size <= 512 && trustPin.none(Char::isISOControl))
                require(credential?.route == kind && credential.purpose == ControllerRouteCredentialPurpose.RELAY_ADMISSION)
                val uri = runCatching { URI(endpoint) }.getOrElse { throw IllegalArgumentException("invalid relay endpoint", it) }
                require(uri.scheme.equals("wss", ignoreCase = true) && !uri.host.isNullOrBlank())
                require(uri.userInfo == null && uri.fragment == null)
            }
        }
    }
}

class ControllerRouteCredentialStore(private val secrets: MobileSecretStore) {
    fun save(hostId: String, reference: ControllerRouteCredentialReference, secret: String) {
        require(secret.isNotEmpty() && secret.toByteArray().size <= 64 * 1_024)
        secrets.saveSecret(account(hostId, reference), secret)
    }

    fun read(hostId: String, reference: ControllerRouteCredentialReference): String? =
        secrets.readSecret(account(hostId, reference))

    fun delete(hostId: String, reference: ControllerRouteCredentialReference) {
        secrets.deleteSecret(account(hostId, reference))
    }

    internal fun account(hostId: String, reference: ControllerRouteCredentialReference): String {
        require(hostId.isSafeRouteIdentifier())
        return listOf(
            "controller-route-v1",
            hostId,
            reference.route.name.lowercase(),
            reference.purpose.name.lowercase(),
            reference.id,
        ).joinToString(":")
    }
}

private fun String.isSafeRouteIdentifier() =
    isNotBlank() && toByteArray().size <= 128 && all { it.isLetterOrDigit() || it in "-_." }
