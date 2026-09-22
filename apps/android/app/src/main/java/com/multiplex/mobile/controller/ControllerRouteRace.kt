package com.multiplex.mobile.controller

import com.multiplex.controller.security.AttemptOutcome
import com.multiplex.controller.security.AttemptResult
import com.multiplex.controller.security.PlannedAttempt
import com.multiplex.controller.security.RouteAddress
import com.multiplex.controller.security.RoutePlan
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import java.io.IOException
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.SocketTimeoutException
import java.util.concurrent.atomic.AtomicInteger

/** How a race ended: the winning transport and its route, or the failure to report. */
internal class ControllerRouteRaceResult(
    val transport: ControllerDuplexTransport?,
    val route: HostRoute?,
    val outcomes: List<AttemptOutcome>,
    val failure: IOException?,
)

/**
 * Runs a [RoutePlan] from the shared route planner against a transport factory.
 *
 * Attempt i starts at its `startAfterMillis`, or as soon as every attempt already started has
 * failed. The first transport to open wins; every other attempt is abandoned, and a transport
 * that opens after losing is closed at once, before anything is written to it. A lost race is
 * therefore never a failed login on the computer, which counts those per source address.
 */
internal object ControllerRouteRace {
    // Opening a socket blocks, and only closing that socket would end it early; the factory owns
    // the socket until it returns, so a losing attempt finishes here on its own and is closed.
    private val openers = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    suspend fun race(
        plan: RoutePlan,
        factory: ControllerTransportFactory,
        nowMillis: () -> Long = { System.nanoTime() / 1_000_000 },
    ): ControllerRouteRaceResult {
        val attempts = plan.attempts
        if (attempts.isEmpty()) {
            return ControllerRouteRaceResult(null, null, emptyList(), IOException("no route to the Host"))
        }
        val started = nowMillis()
        val results = arrayOfNulls<AttemptResult>(attempts.size)
        val opens = arrayOfNulls<PendingOpen>(attempts.size)
        val startedAt = LongArray(attempts.size)
        val finished = Channel<Int>(Channel.UNLIMITED)
        var next = 0
        var running = 0
        var winner = -1
        var lastFailure: IOException? = null

        fun start(index: Int, now: Long) {
            val open = PendingOpen(attempts[index].route.toHostRoute())
            opens[index] = open
            startedAt[index] = now
            running += 1
            openers.launch { open.openWith(factory) { finished.trySend(index) } }
        }

        try {
            while (true) {
                val now = nowMillis() - started
                while (next < attempts.size &&
                    (attempts[next].startAfterMillis.toLong() <= now || running == 0)
                ) {
                    start(next, now)
                    next += 1
                }
                // An attempt that outlived its own limit has failed, which may let the next start.
                for (index in 0 until next) {
                    val open = opens[index] ?: continue
                    if (results[index] == null && now - startedAt[index] >= attempts[index].timeoutMillis.toLong()) {
                        open.abandon()
                        results[index] = AttemptResult.TIMED_OUT
                        lastFailure = SocketTimeoutException("the Host did not answer in time")
                        running -= 1
                    }
                }
                if (running == 0 && next >= attempts.size) break
                if (running == 0) continue
                if (now >= plan.deadlineMillis.toLong()) break
                val wake = nextWake(attempts, next, opens, results, startedAt, plan.deadlineMillis.toLong())
                val index = withTimeoutOrNull((wake - now).coerceAtLeast(1)) { finished.receive() } ?: continue
                if (results[index] != null) continue
                running -= 1
                val open = checkNotNull(opens[index])
                val error = open.error
                if (error == null) {
                    results[index] = AttemptResult.CONNECTED
                    winner = index
                    break
                }
                results[index] = resultOf(error)
                lastFailure = error as? IOException ?: IOException(error.message, error)
            }
        } finally {
            for (index in attempts.indices) {
                if (index == winner) continue
                opens[index]?.abandon()
                if (opens[index] != null && results[index] == null) {
                    results[index] = if (winner >= 0) AttemptResult.CANCELLED else AttemptResult.TIMED_OUT
                }
            }
        }
        val outcomes = attempts.indices
            .filter { opens[it] != null }
            .map { AttemptOutcome(attempts[it].route, checkNotNull(results[it])) }
        if (winner >= 0) {
            val open = checkNotNull(opens[winner])
            return ControllerRouteRaceResult(open.transport, open.route, outcomes, null)
        }
        return ControllerRouteRaceResult(
            null,
            null,
            outcomes,
            lastFailure ?: SocketTimeoutException("no route to the Host answered in time"),
        )
    }

    private fun nextWake(
        attempts: List<PlannedAttempt>,
        next: Int,
        opens: Array<PendingOpen?>,
        results: Array<AttemptResult?>,
        startedAt: LongArray,
        deadline: Long,
    ): Long {
        var wake = deadline
        if (next < attempts.size) wake = minOf(wake, attempts[next].startAfterMillis.toLong())
        for (index in 0 until next) {
            if (opens[index] != null && results[index] == null) {
                wake = minOf(wake, startedAt[index] + attempts[index].timeoutMillis.toLong())
            }
        }
        return wake
    }

    internal fun resultOf(error: Throwable): AttemptResult {
        val message = error.message.orEmpty()
        return when {
            error is SocketTimeoutException || "ETIMEDOUT" in message -> AttemptResult.TIMED_OUT
            error is NoRouteToHostException ||
                "ENETUNREACH" in message || "EHOSTUNREACH" in message ||
                "Network is unreachable" in message || "No route to host" in message ->
                AttemptResult.UNREACHABLE
            "ECONNREFUSED" in message || "Connection refused" in message -> AttemptResult.REFUSED
            error is ConnectException -> AttemptResult.REFUSED
            else -> AttemptResult.FAILED
        }
    }

    /** One attempt's open, which either the race takes or nobody ever uses. */
    private class PendingOpen(val route: HostRoute) {
        private val state = AtomicInteger(RUNNING)
        @Volatile var transport: ControllerDuplexTransport? = null
            private set
        @Volatile var error: Throwable? = null
            private set

        fun openWith(factory: ControllerTransportFactory, onFinished: () -> Unit) {
            try {
                transport = factory.open(route)
            } catch (failure: Throwable) {
                error = failure
            }
            if (state.compareAndSet(RUNNING, DONE)) {
                onFinished()
            } else {
                closeQuietly()
            }
        }

        /** Gives the attempt up; a transport it opened, now or later, is closed unused. */
        fun abandon() {
            if (!state.compareAndSet(RUNNING, ABANDONED) && state.compareAndSet(DONE, ABANDONED)) {
                closeQuietly()
            }
        }

        private fun closeQuietly() {
            transport?.let { runCatching { it.close() } }
        }

        private companion object {
            const val RUNNING = 0
            const val DONE = 1
            const val ABANDONED = 2
        }
    }
}

internal fun HostRoute.toRouteAddress(): RouteAddress = RouteAddress(address, port.toUShort())

internal fun RouteAddress.toHostRoute(): HostRoute = HostRoute(address, port.toInt())
