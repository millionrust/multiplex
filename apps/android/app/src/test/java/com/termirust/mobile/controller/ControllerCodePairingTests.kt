package com.termirust.mobile.controller

import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class ControllerCodePairingTests {
    private val json = Json { ignoreUnknownKeys = false; encodeDefaults = true; explicitNulls = true }

    @Test
    fun recordsSavedWithOneRouteReadAsARouteList() {
        val legacy = """
            {"schema_version":1,"id":"ab","display_name":"Mac",
            "route":{"address":"192.168.1.9","port":55555},
            "host_static_public_key":"AAAA","device_static_key_id":"controller.device.a",
            "device_id":"d","identity_generation":1,"revocation_epoch":0,
            "session_generation":1,"capability_bits":3,"paired_at_millis":1}
        """.trimIndent()

        val record = json.decodeFromString<PairedHostRecord>(legacy)

        record.validate()
        assertEquals(listOf(HostRoute("192.168.1.9", 55_555)), record.routes)
        assertNull(record.discoveryId)
        assertEquals(record, json.decodeFromString<PairedHostRecord>(json.encodeToString(record)))
    }

    @Test
    fun aWorkingRouteMovesToTheFront() {
        val lan = HostRoute("192.168.1.9", 55_555)
        val tailnet = HostRoute("100.100.1.2", 55_555)
        val record = json.decodeFromString<PairedHostRecord>(
            """
            {"schema_version":1,"id":"ab","display_name":"Mac","route":{"address":"192.168.1.9","port":55555},
            "host_static_public_key":"AAAA","device_static_key_id":"controller.device.a","device_id":"d",
            "identity_generation":1,"revocation_epoch":0,"session_generation":1,"capability_bits":3,
            "paired_at_millis":1}
            """.trimIndent(),
        )

        val moved = record.preferringRoute(tailnet)
        moved.validate()
        assertEquals(tailnet, moved.route)
        assertEquals(listOf(tailnet, lan), moved.routes)
        assertEquals(listOf(lan, tailnet), moved.preferringRoute(lan).routes)
        assertThrows(IllegalArgumentException::class.java) { moved.copy(route = lan).validate() }
    }

    @Test
    fun discoveryIdMatchesTheDesktopAnnouncement() {
        val id = ControllerNetworkAddresses.discoveryId(ByteArray(32) { 7 })

        assertEquals("595b48412448eddcfbd8434a5a598a54", id)
        assertEquals("Multiplex 595B48", ControllerNetworkAddresses.defaultHostName(id))
    }

    @Test
    fun typedAddressesNeedAPortAndAPrivateAddress() {
        assertEquals(
            ControllerEndpoint("mac.tail1234.ts.net", 55_123),
            ControllerNetworkAddresses.parseEndpoint("mac.tail1234.ts.net:55123"),
        )
        assertEquals(
            ControllerEndpoint("fd7a:115c:a1e0::1", 55_555),
            ControllerNetworkAddresses.parseEndpoint("[fd7a:115c:a1e0::1]:55555"),
        )
        listOf("mac", "mac:", ":1", "fd7a::1:55", "[fd7a::1]", "[10.0.0.1]:5", "host:70000", "a b:1").forEach {
            assertThrows(ControllerPairingAddressException.Invalid::class.java) {
                ControllerNetworkAddresses.parseEndpoint(it)
            }
        }
        assertEquals(
            listOf(HostRoute("100.100.1.2", 1)),
            ControllerNetworkAddresses.resolve(ControllerEndpoint("100.100.1.2", 1)),
        )
        assertThrows(ControllerPairingAddressException.NotPrivate::class.java) {
            ControllerNetworkAddresses.resolve(ControllerEndpoint("8.8.8.8", 1))
        }
    }

    @Test
    fun privateRoutesFollowTheHostRule() {
        listOf("10.1.2.3", "172.16.0.1", "192.168.0.1", "100.64.0.1", "100.127.255.255", "fc00::1", "fdff::1")
            .forEach { assertTrue(it, ControllerNetworkAddresses.isPrivateLiteralRoute(HostRoute(it, 1))) }
        listOf("172.32.0.1", "100.128.0.1", "8.8.8.8", "127.0.0.1", "fe80::1", "::1", "999.1.1.1", "localhost")
            .forEach { assertFalse(it, ControllerNetworkAddresses.isPrivateLiteralRoute(HostRoute(it, 1))) }
    }
}
