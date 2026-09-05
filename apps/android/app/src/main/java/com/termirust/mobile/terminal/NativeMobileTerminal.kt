package com.termirust.mobile.terminal

internal object NativeMobileTerminal {
    private val loaded = runCatching {
        System.loadLibrary("termirust_mobile_ffi")
    }.isSuccess

    fun renderUtf8OrNull(
        input: ByteArray,
        columns: Int,
        rows: Int,
        scrollbackRows: Int,
    ): ByteArray? {
        if (!loaded) {
            return null
        }
        return runCatching {
            renderUtf8(input, columns, rows, scrollbackRows)
        }.getOrNull()
    }

    private external fun renderUtf8(
        input: ByteArray,
        columns: Int,
        rows: Int,
        scrollbackRows: Int,
    ): ByteArray
}
