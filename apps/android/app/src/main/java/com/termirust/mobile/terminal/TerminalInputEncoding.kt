package com.termirust.mobile.terminal

fun encodeTerminalInput(input: String, control: Boolean, alt: Boolean): ByteArray {
    val payload = when {
        control -> controlEncodedInput(input) ?: input.encodeToByteArray()
        alt -> input.encodeToByteArray()
        else -> "$input\n".encodeToByteArray()
    }
    return if (alt) byteArrayOf(0x1B) + payload else payload
}

private fun controlEncodedInput(input: String): ByteArray? {
    if (input.length != 1) return null
    val value = input[0].code
    if (value !in 0..127) return null
    val upper = value.toChar().uppercaseChar().code
    if (upper !in 0x40..0x5F) return null
    return byteArrayOf((upper and 0x1F).toByte())
}
