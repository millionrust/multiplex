import Foundation
import XCTest
@testable import TermiRustMobile

final class ControllerBindingConformanceTests: XCTestCase {
    func testControllerV1GoldenVectorAcrossGeneratedBinding() throws {
        let vector = try GoldenVector.load()
        let deviceStore = MemorySecureBlobStore()
        let hostStore = MemorySecureBlobStore()
        let device = try ControllerSecurityEngine(blobs: deviceStore)
        let host = try ControllerSecurityEngine(blobs: hostStore)

        try device.storeSecureBlob(keyId: "fixture-device", value: vector.data("device_static_private_hex"))
        try host.storeSecureBlob(keyId: "fixture-host", value: vector.data("host_static_private_hex"))

        let offer = try vector.data("offer_hex")
        let summary = try device.decodeOfferSummary(offerBytes: offer)
        XCTAssertEqual(summary.version, ProtocolVersion(major: 1, minor: 0))
        XCTAssertEqual(summary.capabilityBits, 7)

        let deviceSession = try device.pairingStart(request: PairingStartRequest(
            role: .deviceInitiator,
            offerBytes: offer,
            staticKeyId: "fixture-device",
            ephemeralPrivateKey: try vector.data("device_ephemeral_private_hex"),
            nowMillis: 1_000,
            nowUnixSeconds: 1_000
        ))
        let hostSession = try host.pairingStart(request: PairingStartRequest(
            role: .hostResponder,
            offerBytes: offer,
            staticKeyId: "fixture-host",
            ephemeralPrivateKey: try vector.data("host_ephemeral_private_hex"),
            nowMillis: 1_000,
            nowUnixSeconds: 1_000
        ))

        let message1 = try deviceSession.pairingOutbound(nowMillis: 1_001)
        XCTAssertEqual(message1, try vector.data("message_1_hex"))
        try hostSession.pairingReceive(message: message1, nowMillis: 1_002)

        let message2 = try hostSession.pairingOutbound(nowMillis: 1_003)
        XCTAssertEqual(message2, try vector.data("message_2_hex"))
        try deviceSession.pairingReceive(message: message2, nowMillis: 1_004)

        let message3 = try deviceSession.pairingOutbound(nowMillis: 1_005)
        XCTAssertEqual(message3, try vector.data("message_3_hex"))
        try hostSession.pairingReceive(message: message3, nowMillis: 1_006)

        for session in [deviceSession, hostSession] {
            XCTAssertEqual(try session.sas().value, vector.string("sas_display"))
            XCTAssertEqual(try session.handshakeHash(), try vector.data("handshake_hash_hex"))
            _ = try session.confirmOrReject(
                confirmation: .confirm,
                comparedSas: vector.string("sas_display"),
                revocationEpoch: 4
            )
            XCTAssertEqual(
                try session.authorize(capability: .observeSessions, presentedRevocationEpoch: 4),
                .allow
            )
            XCTAssertEqual(
                try session.authorize(capability: .resize, presentedRevocationEpoch: 4),
                .deny
            )
        }

        let frame = try deviceSession.sealFrame(
            kind: .control,
            capability: .observeSessions,
            revocationEpoch: 4,
            payload: Data("controller-v1-first".utf8)
        )
        XCTAssertEqual(frame, try vector.data("first_frame_hex"))
        let opened = try hostSession.openFrame(frame: frame)
        XCTAssertEqual(opened.sequence, 0)
        XCTAssertEqual(opened.payload, Data("controller-v1-first".utf8))
    }

    func testCallbackSizeCancellationAndDisposalFailuresAreTyped() throws {
        let store = MemorySecureBlobStore()
        let engine = try ControllerSecurityEngine(blobs: store)
        store.failure = .Locked
        assertBindingError(.SecureBlobLocked) {
            _ = try engine.secureBlobStatus(keyId: "device")
        }

        store.failure = nil
        assertBindingError(.SecureBlobInvalid) {
            try engine.storeSecureBlob(keyId: "device", value: Data(repeating: 0, count: 4_097))
        }

        let vector = try GoldenVector.load()
        try engine.storeSecureBlob(keyId: "device", value: vector.data("device_static_private_hex"))
        let session = try engine.pairingStart(request: PairingStartRequest(
            role: .deviceInitiator,
            offerBytes: try vector.data("offer_hex"),
            staticKeyId: "device",
            ephemeralPrivateKey: try vector.data("device_ephemeral_private_hex"),
            nowMillis: 1_000,
            nowUnixSeconds: 1_000
        ))
        try session.cancel()
        assertBindingError(.Disposed) { _ = try session.sas() }
        try session.finish()
        try session.finish()
    }

    private func assertBindingError(
        _ expected: ControllerBindingError,
        operation: () throws -> Void
    ) {
        XCTAssertThrowsError(try operation()) { error in
            XCTAssertEqual(error as? ControllerBindingError, expected)
        }
    }
}

private final class MemorySecureBlobStore: SecureBlobStore, @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: Data] = [:]
    private var storedFailure: SecureBlobError?

    var failure: SecureBlobError? {
        get {
            lock.lock()
            defer { lock.unlock() }
            return storedFailure
        }
        set {
            lock.lock()
            storedFailure = newValue
            lock.unlock()
        }
    }

    func load(keyId: String) throws -> Data? {
        lock.lock()
        defer { lock.unlock() }
        if let storedFailure { throw storedFailure }
        return values[keyId]
    }

    func store(keyId: String, value: Data) throws {
        lock.lock()
        defer { lock.unlock() }
        if let storedFailure { throw storedFailure }
        values[keyId] = value
    }

    func delete(keyId: String) throws {
        lock.lock()
        defer { lock.unlock() }
        if let storedFailure { throw storedFailure }
        values.removeValue(forKey: keyId)
    }
}

private struct GoldenVector {
    private let object: [String: Any]

    static func load() throws -> Self {
        let url = try XCTUnwrap(
            Bundle(for: ControllerBindingConformanceTests.self)
                .url(forResource: "controller-v1", withExtension: "json")
        )
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        )
        return Self(object: object)
    }

    func string(_ key: String) -> String {
        object[key] as? String ?? ""
    }

    func data(_ key: String) throws -> Data {
        try Data(strictHex: XCTUnwrap(object[key] as? String))
    }
}

private extension Data {
    init(strictHex value: String) throws {
        guard value.count.isMultiple(of: 2) else {
            throw HexError.invalid
        }
        var bytes: [UInt8] = []
        bytes.reserveCapacity(value.count / 2)
        var index = value.startIndex
        while index < value.endIndex {
            let next = value.index(index, offsetBy: 2)
            guard let byte = UInt8(value[index..<next], radix: 16) else {
                throw HexError.invalid
            }
            bytes.append(byte)
            index = next
        }
        self.init(bytes)
    }
}

private enum HexError: Error {
    case invalid
}
