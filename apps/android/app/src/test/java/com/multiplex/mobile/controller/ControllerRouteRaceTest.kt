package com.multiplex.mobile.controller

import com.multiplex.controller.security.AttemptResult
import com.multiplex.controller.security.PhoneAddress
import com.multiplex.controller.security.PhoneLink
import com.multiplex.controller.security.PhoneNetwork
import com.multiplex.controller.security.PlannedAttempt
import com.multiplex.controller.security.RememberedRoute
import com.multiplex.controller.security.RouteAddress
import com.multiplex.controller.security.RouteKind
import com.multiplex.controller.security.RoutePlan
import com.multiplex.controller.security.planRoutes
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.contentOrNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.net.ConnectException
import java.net.SocketTimeoutException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class ControllerRouteRaceTest {
    @Test
    fun everySharedRoutePlanCaseCrossesTheBindingExactly() {
        val stream = requireNotNull(
            ControllerRouteRaceTest::class.java.classLoader?.getResourceAsStream("route-plan-v1.json"),
        )
        val document = stream.bufferedReader().use { Json.parseToJsonElement(it.readText()).jsonObject }
        assertEquals(1, document.getValue("schema_version").jsonPrimitive.int)
        val cases = document.getValue("cases").jsonArray.map { it.jsonObject }
        assertTrue(cases.size >= 9)
        cases.forEach { case ->
            val name = case.getValue("name").jsonPrimitive.content
            val network = PhoneNetwork(
                link(case.getValue("link").jsonPrimitive.content),
                case.getValue("phone_addresses").jsonArray.map {
                    PhoneAddress(
                        it.jsonObject.getValue("address").jsonPrimitive.content,
                        it.jsonObject.getValue("prefix_length").jsonPrimitive.int.toUByte(),
                    )
                },
                case.getValue("fingerprint").jsonPrimitive.contentOrNull,
            )
            val plan = planRoutes(
                routes(case, "saved"),
                routes(case, "discovered"),
                network,
                case.getValue("remembered").jsonArray.map {
                    RememberedRoute(
                        it.jsonObject.getValue("fingerprint").jsonPrimitive.content,
                        address(it.jsonObject.getValue("route").jsonObject),
                    )
                },
            )
            val expected = case.getValue("expected").jsonObject
            assertEquals(name, expected.getValue("deadline_millis").jsonPrimitive.int.toUInt(), plan.deadlineMillis)
            val attempts = expected.getValue("attempts").jsonArray.map { it.jsonObject }
            assertEquals(name, attempts.size, plan.attempts.size)
            attempts.zip(plan.attempts).forEach { (want, got) ->
                assertEquals(name, address(want), got.route)
                assertEquals(name, kind(want.getValue("kind").jsonPrimitive.content), got.kind)
                assertEquals(name, want.getValue("tier").jsonPrimitive.int.toUByte(), got.tier)
                assertEquals(name, want.getValue("start_after_millis").jsonPrimitive.int.toUInt(), got.startAfterMillis)
                assertEquals(name, want.getValue("timeout_millis").jsonPrimitive.int.toUInt(), got.timeoutMillis)
            }
        }
    }

    @Test
    fun aHangingFirstRouteLosesAndIsClosedWithNothingWritten() = runBlocking {
        val release = CountDownLatch(1)
        val factory = FakeFactory { route ->
            if (route.address == SLOW) release.await(5, TimeUnit.SECONDS)
        }
        val result = ControllerRouteRace.race(plan(attempt(SLOW, 0, 2_000), attempt(FAST, 50, 2_000)), factory)

        assertEquals(HostRoute(FAST, PORT), result.route)
        assertSame(factory.opened.getValue(FAST), result.transport)
        assertEquals(
            listOf(AttemptResult.CANCELLED, AttemptResult.CONNECTED),
            result.outcomes.map { it.result },
        )
        release.countDown()
        val loser = factory.awaitOpened(SLOW)
        assertTrue(loser.awaitClosed())
        assertEquals(0, loser.written.size())
    }

    @Test
    fun aRefusedRouteLetsTheNextStartBeforeItsTime() = runBlocking {
        val factory = FakeFactory { route ->
            if (route.address == SLOW) throw ConnectException("connect failed: ECONNREFUSED (Connection refused)")
        }
        val started = System.nanoTime()
        val result = ControllerRouteRace.race(plan(attempt(SLOW, 0, 2_000), attempt(FAST, 5_000, 2_000)), factory)
        val elapsedMillis = (System.nanoTime() - started) / 1_000_000

        assertEquals(HostRoute(FAST, PORT), result.route)
        assertTrue("took $elapsedMillis ms", elapsedMillis < 2_000)
        assertEquals(
            listOf(AttemptResult.REFUSED, AttemptResult.CONNECTED),
            result.outcomes.map { it.result },
        )
    }

    @Test
    fun whenEveryRouteFailsTheLastFailureIsReportedWithEveryOutcome() = runBlocking {
        val factory = FakeFactory { route ->
            when (route.address) {
                SLOW -> throw ConnectException("connect failed: ECONNREFUSED (Connection refused)")
                else -> throw java.net.NoRouteToHostException("No route to host")
            }
        }
        val result = ControllerRouteRace.race(plan(attempt(SLOW, 0, 1_000), attempt(FAST, 10, 1_000)), factory)

        assertNull(result.transport)
        assertTrue(result.failure is java.net.NoRouteToHostException)
        assertEquals(
            listOf(AttemptResult.REFUSED, AttemptResult.UNREACHABLE),
            result.outcomes.map { it.result },
        )
    }

    @Test
    fun theRaceGivesUpAtItsDeadline() = runBlocking {
        val release = CountDownLatch(1)
        val factory = FakeFactory { release.await(5, TimeUnit.SECONDS) }
        val started = System.nanoTime()
        val result = ControllerRouteRace.race(
            plan(attempt(SLOW, 0, 5_000), attempt(FAST, 20, 5_000), deadline = 150),
            factory,
        )
        val elapsedMillis = (System.nanoTime() - started) / 1_000_000
        release.countDown()

        assertNull(result.transport)
        assertTrue(result.failure is SocketTimeoutException)
        assertTrue("took $elapsedMillis ms", elapsedMillis < 1_500)
        assertEquals(
            listOf(AttemptResult.TIMED_OUT, AttemptResult.TIMED_OUT),
            result.outcomes.map { it.result },
        )
        assertTrue(factory.awaitOpened(SLOW).awaitClosed())
        assertTrue(factory.awaitOpened(FAST).awaitClosed())
    }

    @Test
    fun anAttemptPastItsOwnLimitCountsAsTimedOut() = runBlocking {
        val release = CountDownLatch(1)
        val factory = FakeFactory { route ->
            if (route.address == SLOW) release.await(5, TimeUnit.SECONDS)
        }
        val result = ControllerRouteRace.race(plan(attempt(SLOW, 0, 50), attempt(FAST, 3_000, 2_000)), factory)
        release.countDown()

        assertEquals(HostRoute(FAST, PORT), result.route)
        assertEquals(
            listOf(AttemptResult.TIMED_OUT, AttemptResult.CONNECTED),
            result.outcomes.map { it.result },
        )
    }

    @Test
    fun platformErrorsMapToOutcomes() {
        assertEquals(AttemptResult.TIMED_OUT, ControllerRouteRace.resultOf(SocketTimeoutException()))
        assertEquals(
            AttemptResult.UNREACHABLE,
            ControllerRouteRace.resultOf(ConnectException("connect failed: ENETUNREACH (Network is unreachable)")),
        )
        assertEquals(
            AttemptResult.UNREACHABLE,
            ControllerRouteRace.resultOf(ConnectException("connect failed: EHOSTUNREACH (No route to host)")),
        )
        assertEquals(
            AttemptResult.REFUSED,
            ControllerRouteRace.resultOf(ConnectException("connect failed: ECONNREFUSED (Connection refused)")),
        )
        assertEquals(AttemptResult.FAILED, ControllerRouteRace.resultOf(java.io.IOException("reset")))
    }

    private class FakeTransport : ControllerDuplexTransport {
        val written = ByteArrayOutputStream()
        private val closed = CountDownLatch(1)
        override val input: InputStream = InputStream.nullInputStream()
        override val output: OutputStream = written

        override fun close() {
            closed.countDown()
        }

        fun awaitClosed(): Boolean = closed.await(5, TimeUnit.SECONDS)
    }

    private class FakeFactory(private val behave: (HostRoute) -> Unit) : ControllerTransportFactory {
        val opened = ConcurrentHashMap<String, FakeTransport>()
        private val openedSignals = ConcurrentHashMap<String, CountDownLatch>()

        override fun open(route: HostRoute): ControllerDuplexTransport {
            behave(route)
            return FakeTransport().also {
                opened[route.address] = it
                signal(route.address).countDown()
            }
        }

        fun awaitOpened(address: String): FakeTransport {
            assertTrue(signal(address).await(5, TimeUnit.SECONDS))
            return opened.getValue(address)
        }

        private fun signal(address: String) = openedSignals.computeIfAbsent(address) { CountDownLatch(1) }
    }

    private companion object {
        const val SLOW = "192.168.1.20"
        const val FAST = "100.101.102.103"
        const val PORT = 7_420

        fun attempt(address: String, start: Int, timeout: Int) = PlannedAttempt(
            RouteAddress(address, PORT.toUShort()),
            RouteKind.OTHER_PRIVATE,
            3u,
            start.toUInt(),
            timeout.toUInt(),
        )

        fun plan(vararg attempts: PlannedAttempt, deadline: Int = 5_000) =
            RoutePlan(attempts.toList(), deadline.toUInt())

        fun link(name: String) = when (name) {
            "wifi" -> PhoneLink.WIFI
            "ethernet" -> PhoneLink.ETHERNET
            "cellular" -> PhoneLink.CELLULAR
            "other" -> PhoneLink.OTHER
            else -> PhoneLink.OFFLINE
        }

        fun kind(name: String) = when (name) {
            "local_network" -> RouteKind.LOCAL_NETWORK
            "tailscale" -> RouteKind.TAILSCALE
            else -> RouteKind.OTHER_PRIVATE
        }

        fun address(value: JsonObject) = RouteAddress(
            value.getValue("address").jsonPrimitive.content,
            value.getValue("port").jsonPrimitive.int.toUShort(),
        )

        fun routes(case: JsonObject, key: String) = case.getValue(key).jsonArray.map { address(it.jsonObject) }
    }
}
