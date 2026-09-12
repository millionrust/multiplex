package com.termirust.mobile.controller

import java.io.Closeable
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket

internal interface ControllerDuplexTransport : Closeable {
    val input: InputStream
    val output: OutputStream
}

internal fun interface ControllerTransportFactory {
    fun open(route: HostRoute): ControllerDuplexTransport
}

internal object TcpControllerTransportFactory : ControllerTransportFactory {
    override fun open(route: HostRoute): ControllerDuplexTransport {
        val socket = Socket()
        try {
            socket.connect(InetSocketAddress(route.address, route.port), CONNECT_TIMEOUT_MILLIS)
            socket.soTimeout = READ_TIMEOUT_MILLIS
            socket.tcpNoDelay = true
            return SocketControllerTransport(socket)
        } catch (error: Throwable) {
            runCatching { socket.close() }
            throw error
        }
    }

    private const val CONNECT_TIMEOUT_MILLIS = 10_000
    private const val READ_TIMEOUT_MILLIS = 30_000
}

private class SocketControllerTransport(
    private val socket: Socket,
) : ControllerDuplexTransport {
    override val input: InputStream = socket.getInputStream()
    override val output: OutputStream = socket.getOutputStream()

    override fun close() {
        socket.close()
    }
}
