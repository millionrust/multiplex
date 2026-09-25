package com.multiplex.mobile.controller

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The shape a shipped build writes to `controller-hosts-v1.json`, field for field.
 *
 * `PairedHostStore.load` decodes with `ignoreUnknownKeys = false` and answers a failure with an
 * empty list, so renaming a field, or adding one without a default, would not fail anywhere: every
 * paired computer would simply be gone at the next launch. This is the record a real pairing wrote,
 * with its key material and address replaced.
 */
class PairedHostDocumentOnDiskTest {
    private val json = Json { ignoreUnknownKeys = false; encodeDefaults = true; explicitNulls = true }

    @Test
    fun a_record_a_shipped_build_wrote_still_loads() {
        val document = json.decodeFromString<PairedHostDocument>(ON_DISK)
        document.hosts.forEach(PairedHostRecord::validate)
        assertEquals(1, document.hosts.size)
        val host = document.hosts.single()
        assertEquals("Multiplex 6CDC74", host.displayName)
        assertEquals(HostRoute("198.51.100.4", 50198), host.route)
        assertEquals(listOf(host.route), host.routes)
        assertEquals("6cdc7440c386dad2812492863417b937", host.discoveryId)
        assertEquals(495, host.capabilityBits)
        assertEquals(1, host.routeMemory.size)
    }

    private companion object {
        const val ON_DISK = """
            {
              "schema_version": 1,
              "hosts": [
                {
                  "schema_version": 1,
                  "id": "7af476b0d584b33aa380e2fd0426c446082667f87b05b0ecdbd3ba652cf2e655",
                  "display_name": "Multiplex 6CDC74",
                  "route": { "address": "198.51.100.4", "port": 50198 },
                  "host_static_public_key": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                  "device_static_key_id": "controller.device.00000000-0000-4000-8000-000000000000.7af476b0d584b33a",
                  "device_id": "00000000-0000-4000-8000-000000000000",
                  "identity_generation": 1,
                  "revocation_epoch": 0,
                  "session_generation": 1,
                  "capability_bits": 495,
                  "paired_at_millis": 1790329223592,
                  "routes": [ { "address": "198.51.100.4", "port": 50198 } ],
                  "discovery_id": "6cdc7440c386dad2812492863417b937",
                  "route_memory": [
                    {
                      "fingerprint": "3266efdaad0db8b6347499bd97f30595",
                      "address": "198.51.100.4",
                      "port": 50198
                    }
                  ]
                }
              ]
            }
        """
    }
}
