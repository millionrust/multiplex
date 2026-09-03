package com.termirust.mobile.controller

internal object NativeRelayProtocol {
    val loaded = runCatching { System.loadLibrary("termirust_mobile_ffi") }.isSuccess

    @JvmStatic external fun clientHello(routeId: ByteArray): ByteArray
    @JvmStatic external fun admissionProof(
        routeId: ByteArray,
        credential: ByteArray,
        revocationEpoch: Long,
        nowUnixSeconds: Long,
        challenge: ByteArray,
    ): ByteArray
    @JvmStatic external fun admissionConnectionId(result: ByteArray): ByteArray
    @JvmStatic external fun encodeEnvelope(routeId: ByteArray, sequence: Long, payload: ByteArray): ByteArray
    @JvmStatic external fun decodeEnvelope(routeId: ByteArray, expectedSequence: Long, envelope: ByteArray): ByteArray
}
