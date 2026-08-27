import Darwin
import Foundation

@main
enum ControllerBindingConformanceRunner {
    static func main() {
        do {
            let fixturePath = try required(CommandLine.arguments.dropFirst().first)
            let vector = try GoldenVector(path: fixturePath)
            try runGoldenVector(vector)
            try runFailureContract(vector)
            print("AT-G20.3-SWIFT OK")
        } catch {
            fputs("AT-G20.3-SWIFT FAIL\n", stderr)
            exit(1)
        }
    }

    private static func runGoldenVector(_ vector: GoldenVector) throws {
        let device = try ControllerSecurityEngine(blobs: MemorySecureBlobStore())
        let host = try ControllerSecurityEngine(blobs: MemorySecureBlobStore())
        try device.storeSecureBlob(keyId: "fixture-device", value: vector.data("device_static_private_hex"))
        try host.storeSecureBlob(keyId: "fixture-host", value: vector.data("host_static_private_hex"))

        let offer = try vector.data("offer_hex")
        let summary = try device.decodeOfferSummary(offerBytes: offer)
        try require(summary.version == ProtocolVersion(major: 1, minor: 0))
        try require(summary.capabilityBits == 7)

        let deviceSession = try device.pairingStart(request: PairingStartRequest(
            role: .deviceInitiator,
            offerBytes: offer,
            staticKeyId: "fixture-device",
            ephemeralPrivateKey: vector.data("device_ephemeral_private_hex"),
            nowMillis: 1_000,
            nowUnixSeconds: 1_000
        ))
        let hostSession = try host.pairingStart(request: PairingStartRequest(
            role: .hostResponder,
            offerBytes: offer,
            staticKeyId: "fixture-host",
            ephemeralPrivateKey: vector.data("host_ephemeral_private_hex"),
            nowMillis: 1_000,
            nowUnixSeconds: 1_000
        ))

        let message1 = try deviceSession.pairingOutbound(nowMillis: 1_001)
        try require(message1 == vector.data("message_1_hex"))
        try hostSession.pairingReceive(message: message1, nowMillis: 1_002)

        let message2 = try hostSession.pairingOutbound(nowMillis: 1_003)
        try require(message2 == vector.data("message_2_hex"))
        try deviceSession.pairingReceive(message: message2, nowMillis: 1_004)

        let message3 = try deviceSession.pairingOutbound(nowMillis: 1_005)
        try require(message3 == vector.data("message_3_hex"))
        try hostSession.pairingReceive(message: message3, nowMillis: 1_006)

        for session in [deviceSession, hostSession] {
            try require(try session.sas().value == vector.string("sas_display"))
            try require(try session.handshakeHash() == vector.data("handshake_hash_hex"))
            _ = try session.confirmOrReject(
                confirmation: .confirm,
                comparedSas: vector.string("sas_display"),
                revocationEpoch: 4
            )
            try require(
                try session.authorize(
                    capability: .observeSessions,
                    presentedRevocationEpoch: 4
                ) == .allow
            )
            try require(
                try session.authorize(
                    capability: .resize,
                    presentedRevocationEpoch: 4
                ) == .deny
            )
        }

        let frame = try deviceSession.sealFrame(
            kind: .control,
            capability: .observeSessions,
            revocationEpoch: 4,
            payload: Data("controller-v1-first".utf8)
        )
        try require(frame == vector.data("first_frame_hex"))
        let opened = try hostSession.openFrame(frame: frame)
        try require(opened.sequence == 0)
        try require(opened.payload == Data("controller-v1-first".utf8))
    }

    private static func runFailureContract(_ vector: GoldenVector) throws {
        let store = MemorySecureBlobStore()
        let engine = try ControllerSecurityEngine(blobs: store)
        store.failure = .Locked
        try requireBindingError(.SecureBlobLocked) {
            _ = try engine.secureBlobStatus(keyId: "device")
        }

        store.failure = nil
        try requireBindingError(.SecureBlobInvalid) {
            try engine.storeSecureBlob(keyId: "device", value: Data(repeating: 0, count: 4_097))
        }

        try engine.storeSecureBlob(keyId: "device", value: vector.data("device_static_private_hex"))
        let session = try engine.pairingStart(request: PairingStartRequest(
            role: .deviceInitiator,
            offerBytes: vector.data("offer_hex"),
            staticKeyId: "device",
            ephemeralPrivateKey: vector.data("device_ephemeral_private_hex"),
            nowMillis: 1_000,
            nowUnixSeconds: 1_000
        ))
        try session.cancel()
        try requireBindingError(.Disposed) { _ = try session.sas() }
        try session.finish()
        try session.finish()
    }

    private static func requireBindingError(
        _ expected: ControllerBindingError,
        operation: () throws -> Void
    ) throws {
        do {
            try operation()
            throw RunnerFailure.failed
        } catch let error as ControllerBindingError {
            try require(error == expected)
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

    init(path: String) throws {
        object = try required(
            JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: path)))
                as? [String: Any]
        )
    }

    func string(_ key: String) throws -> String {
        try required(object[key] as? String)
    }

    func data(_ key: String) throws -> Data {
        try Data(strictHex: string(key))
    }
}

private extension Data {
    init(strictHex value: String) throws {
        try require(value.count.isMultiple(of: 2))
        var bytes: [UInt8] = []
        bytes.reserveCapacity(value.count / 2)
        var index = value.startIndex
        while index < value.endIndex {
            let next = value.index(index, offsetBy: 2)
            bytes.append(try required(UInt8(value[index..<next], radix: 16)))
            index = next
        }
        self.init(bytes)
    }
}

private func required<T>(_ value: T?) throws -> T {
    guard let value else { throw RunnerFailure.failed }
    return value
}

private func require(_ condition: @autoclosure () throws -> Bool) throws {
    guard try condition() else { throw RunnerFailure.failed }
}

private enum RunnerFailure: Error {
    case failed
}
