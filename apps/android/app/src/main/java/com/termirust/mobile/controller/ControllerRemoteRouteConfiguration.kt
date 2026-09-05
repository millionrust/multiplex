package com.termirust.mobile.controller

import android.content.Context
import com.termirust.mobile.security.MobileSecretStore
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.net.URI
import java.util.Base64

@Serializable
enum class ControllerRouteCredentialPurpose {
    @SerialName("ssh_authentication") SSH_AUTHENTICATION,
    @SerialName("relay_admission") RELAY_ADMISSION,
}

@Serializable
enum class ControllerSshAuthenticationKind {
    @SerialName("password") PASSWORD,
    @SerialName("private_key") PRIVATE_KEY,
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
    @SerialName("ssh_authentication") val sshAuthentication: ControllerSshAuthenticationKind? = null,
    @SerialName("relay_route_id") val relayRouteId: String? = null,
    @SerialName("relay_revocation_epoch") val relayRevocationEpoch: Long? = null,
) {
    fun validate() {
        require(endpoint.isNotBlank() && endpoint.toByteArray().size <= 2_048 && endpoint.none(Char::isISOControl))
        when (kind) {
            ControllerRemoteRouteKind.LOCAL_IPC -> error("local IPC is not an Android route configuration")
            ControllerRemoteRouteKind.PRIVATE_NETWORK -> {
                require(
                    port in 1..65_535 && username == null && trustPin == null &&
                        credential == null && sshAuthentication == null &&
                        relayRouteId == null && relayRevocationEpoch == null,
                )
                HostRoute(endpoint, port!!)
            }
            ControllerRemoteRouteKind.SSH -> {
                require(port in 1..65_535)
                require(!username.isNullOrBlank() && username.toByteArray().size <= 255 && username.none(Char::isISOControl))
                require(!trustPin.isNullOrBlank() && trustPin.toByteArray().size <= 512 && trustPin.none(Char::isISOControl))
                require(credential?.route == kind && credential.purpose == ControllerRouteCredentialPurpose.SSH_AUTHENTICATION)
                require(sshAuthentication != null)
                require(relayRouteId == null && relayRevocationEpoch == null)
                HostRoute(endpoint, port!!)
            }
            ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> {
                require(port == null && username == null && sshAuthentication == null)
                val relayPin = requireNotNull(trustPin)
                require(relayPin.isNotBlank() && relayPin.toByteArray().size <= 512 && relayPin.none(Char::isISOControl))
                require(relayPin.startsWith("sha256/"))
                val pin = runCatching { Base64.getDecoder().decode(relayPin.removePrefix("sha256/")) }
                    .getOrElse { throw IllegalArgumentException("invalid relay SPKI pin", it) }
                require(pin.size == 32)
                require(credential?.route == kind && credential.purpose == ControllerRouteCredentialPurpose.RELAY_ADMISSION)
                require(relayRevocationEpoch != null && relayRevocationEpoch >= 0)
                val encodedRouteId = requireNotNull(relayRouteId)
                val routeId = runCatching { Base64.getDecoder().decode(encodedRouteId) }
                    .getOrElse { throw IllegalArgumentException("invalid relay route ID", it) }
                require(routeId.size == 32)
                val uri = runCatching { URI(endpoint) }.getOrElse { throw IllegalArgumentException("invalid relay endpoint", it) }
                require(uri.scheme.equals("wss", ignoreCase = true) && !uri.host.isNullOrBlank())
                require(uri.path == "/relay/v1")
                require(uri.userInfo == null && uri.query == null && uri.fragment == null)
            }
        }
    }
}

@Serializable
data class ControllerRelayRoutePackage(
    val schema: String,
    @SerialName("schema_version") val schemaVersion: Int,
    val role: String,
    val endpoint: String,
    @SerialName("spki_pin") val spkiPin: String,
    @SerialName("relay_route_id") val relayRouteId: String,
    @SerialName("relay_revocation_epoch") val relayRevocationEpoch: Long,
    @SerialName("admission_credential") val admissionCredential: String,
) {
    fun validate() {
        require(schema == "termirust-relay-route" && schemaVersion == 1 && role == "controller")
        val credentialBytes = runCatching { Base64.getDecoder().decode(admissionCredential) }
            .getOrElse { throw IllegalArgumentException("invalid relay admission credential", it) }
        require(credentialBytes.size == 32)
        ControllerRemoteRouteConfiguration(
            kind = ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
            endpoint = endpoint,
            trustPin = spkiPin,
            credential = ControllerRouteCredentialReference(
                id = "relay-package-validation",
                route = ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
                purpose = ControllerRouteCredentialPurpose.RELAY_ADMISSION,
            ),
            relayRouteId = relayRouteId,
            relayRevocationEpoch = relayRevocationEpoch,
        ).validate()
    }

    companion object {
        private val json = Json { ignoreUnknownKeys = false }

        fun decode(text: String): ControllerRelayRoutePackage {
            require(text.toByteArray().size <= 16 * 1_024)
            return json.decodeFromString<ControllerRelayRoutePackage>(text).also { it.validate() }
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

class ControllerRouteConfigurationStore(context: Context) {
    private val preferences = context.getSharedPreferences("controller-route-configurations-v1", 0)
    private val json = kotlinx.serialization.json.Json {
        ignoreUnknownKeys = false
        encodeDefaults = true
        explicitNulls = true
    }

    fun save(hostId: String, configuration: ControllerRemoteRouteConfiguration) {
        require(hostId.isSafeRouteIdentifier())
        configuration.validate()
        preferences.edit().putString(key(hostId, configuration.kind), json.encodeToString(configuration)).apply()
    }

    fun load(hostId: String, route: ControllerRemoteRouteKind): ControllerRemoteRouteConfiguration? {
        require(hostId.isSafeRouteIdentifier())
        if (route == ControllerRemoteRouteKind.LOCAL_IPC) return null
        val encoded = preferences.getString(key(hostId, route), null) ?: return null
        return runCatching {
            json.decodeFromString<ControllerRemoteRouteConfiguration>(encoded).also {
                require(it.kind == route)
                it.validate()
            }
        }.getOrNull()
    }

    fun delete(hostId: String, route: ControllerRemoteRouteKind) {
        require(hostId.isSafeRouteIdentifier())
        preferences.edit().remove(key(hostId, route)).apply()
    }

    private fun key(hostId: String, route: ControllerRemoteRouteKind) = "$hostId:${route.name.lowercase()}"
}

private fun String.isSafeRouteIdentifier() =
    isNotBlank() && toByteArray().size <= 128 && all { it.isLetterOrDigit() || it in "-_." }
