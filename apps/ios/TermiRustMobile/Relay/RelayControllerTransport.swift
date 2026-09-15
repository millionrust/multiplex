import Crypto
import Foundation
import Security
import TermiRustMobileCrypto

enum RelayControllerTransport {
    static let origin = "termirust://relay-local"
    static let subprotocol = "termirust-relay-v1"

    static func factory(
        hostID: String,
        configuration: ControllerRemoteRouteConfiguration,
        credentials: any ControllerRouteCredentialStoring
    ) throws -> ControllerTransportFactory {
        try configuration.validate()
        guard configuration.kind == .selfHostedRelay else {
            throw ControllerRemoteRouteConfigurationError.unsupportedRoute
        }
        return ControllerTransportFactory { _ in
            try await RelayControllerDuplexConnection.open(
                hostID: hostID,
                configuration: configuration,
                credentials: credentials
            )
        }
    }
}

private final class RelayControllerDuplexConnection: ControllerDuplexConnection, @unchecked Sendable {
    private let session: URLSession
    private let socket: URLSessionWebSocketTask
    private let state: RelayFrameState

    private init(session: URLSession, socket: URLSessionWebSocketTask, routeID: Data) {
        self.session = session
        self.socket = socket
        self.state = RelayFrameState(socket: socket, routeID: routeID)
    }

    static func open(
        hostID: String,
        configuration: ControllerRemoteRouteConfiguration,
        credentials: any ControllerRouteCredentialStoring
    ) async throws -> RelayControllerDuplexConnection {
        let routeID = try decodeFixedBase64(configuration.relayRouteID, count: 32)
        let pin = try decodeSPKIPin(configuration.trustPin)
        let reference = try required(configuration.credential)
        var stored = try required(credentials.load(hostID: hostID, reference: reference))
        defer { stored.resetBytes(in: 0 ..< stored.count) }
        guard let encodedSecret = String(data: stored, encoding: .utf8),
              var admissionSecret = Data(base64Encoded: encodedSecret),
              admissionSecret.count == 32 else {
            throw RelayControllerTransportError.invalidConfiguration
        }
        defer { admissionSecret.resetBytes(in: 0 ..< admissionSecret.count) }
        guard let url = URL(string: configuration.endpoint) else {
            throw RelayControllerTransportError.invalidConfiguration
        }

        let delegate = RelayPinnedSessionDelegate(expectedSPKI: pin)
        let session = URLSession(
            configuration: .ephemeral,
            delegate: delegate,
            delegateQueue: nil
        )
        var request = URLRequest(url: url, timeoutInterval: 10)
        request.setValue(RelayControllerTransport.origin, forHTTPHeaderField: "Origin")
        request.setValue(
            RelayControllerTransport.subprotocol,
            forHTTPHeaderField: "Sec-WebSocket-Protocol"
        )
        let socket = session.webSocketTask(with: request)
        let connection = RelayControllerDuplexConnection(
            session: session,
            socket: socket,
            routeID: routeID
        )
        socket.resume()
        do {
            try await socket.send(URLSessionWebSocketTask.Message.data(
                try NativeRelayProtocol.clientHello(routeID: routeID)
            ))
            let challenge = try await receiveBinary(socket)
            guard let response = socket.response as? HTTPURLResponse,
                  response.value(forHTTPHeaderField: "Sec-WebSocket-Protocol")
                    == RelayControllerTransport.subprotocol else {
                throw RelayControllerTransportError.subprotocolRejected
            }
            let proof = try NativeRelayProtocol.admissionProof(
                routeID: routeID,
                credential: admissionSecret,
                revocationEpoch: try required(configuration.relayRevocationEpoch),
                nowUnixSeconds: UInt64(Date().timeIntervalSince1970),
                challenge: challenge
            )
            try await socket.send(URLSessionWebSocketTask.Message.data(proof))
            let admission = try await receiveBinary(socket)
            _ = try NativeRelayProtocol.admissionConnectionID(result: admission)
            return connection
        } catch {
            connection.cancel()
            throw error
        }
    }

    func send(_ data: Data) async throws {
        try await state.send(data)
    }

    func receive(maximumLength: Int) async throws -> Data {
        try await state.receive(maximumLength: maximumLength)
    }

    func cancel() {
        socket.cancel(with: .goingAway, reason: nil)
        session.invalidateAndCancel()
        Task { await state.close() }
    }

    private static func decodeFixedBase64(_ value: String?, count: Int) throws -> Data {
        guard let value, let data = Data(base64Encoded: value), data.count == count else {
            throw RelayControllerTransportError.invalidConfiguration
        }
        return data
    }

    private static func decodeSPKIPin(_ value: String?) throws -> Data {
        guard let value, value.hasPrefix("sha256/"),
              let pin = Data(base64Encoded: String(value.dropFirst("sha256/".count))),
              pin.count == 32 else {
            throw RelayControllerTransportError.invalidConfiguration
        }
        return pin
    }

    private static func required<T>(_ value: T?) throws -> T {
        guard let value else { throw RelayControllerTransportError.invalidConfiguration }
        return value
    }

    private static func receiveBinary(_ socket: URLSessionWebSocketTask) async throws -> Data {
        switch try await socket.receive() {
        case .data(let bytes) where bytes.count > 0 && bytes.count <= RelayFrameState.maximumMessageBytes:
            return bytes
        default:
            throw RelayControllerTransportError.malformedFrame
        }
    }
}

private actor RelayFrameState {
    static let maximumMessageBytes = 1_048_640
    private static let streamChunkBytes = 64 * 1_024
    private static let maximumBufferedBytes = 1 * 1_024 * 1_024

    private let socket: URLSessionWebSocketTask
    private let routeID: Data
    private var sendSequence: UInt64 = 0
    private var receiveSequence: UInt64 = 0
    private var buffered = Data()
    private var closed = false

    init(socket: URLSessionWebSocketTask, routeID: Data) {
        self.socket = socket
        self.routeID = routeID
    }

    func send(_ data: Data) async throws {
        guard !closed else { throw RelayControllerTransportError.closed }
        var offset = 0
        while offset < data.count {
            let end = min(offset + Self.streamChunkBytes, data.count)
            let envelope = try NativeRelayProtocol.encodeEnvelope(
                routeID: routeID,
                sequence: sendSequence,
                payload: data.subdata(in: offset ..< end)
            )
            try await socket.send(.data(envelope))
            sendSequence = try increment(sendSequence)
            offset = end
        }
    }

    func receive(maximumLength: Int) async throws -> Data {
        guard maximumLength > 0 else { return Data() }
        guard !closed else { throw RelayControllerTransportError.closed }
        if buffered.isEmpty {
            let envelope: Data
            switch try await socket.receive() {
            case .data(let bytes) where bytes.count > 0 && bytes.count <= Self.maximumMessageBytes:
                envelope = bytes
            default:
                throw RelayControllerTransportError.malformedFrame
            }
            let payload = try NativeRelayProtocol.decodeEnvelope(
                routeID: routeID,
                expectedSequence: receiveSequence,
                envelope: envelope
            )
            guard payload.count <= Self.maximumBufferedBytes else {
                throw RelayControllerTransportError.receiveBufferExceeded
            }
            receiveSequence = try increment(receiveSequence)
            buffered = payload
        }
        let count = min(maximumLength, buffered.count)
        let result = buffered.prefix(count)
        buffered.removeFirst(count)
        return Data(result)
    }

    func close() {
        closed = true
        buffered.removeAll(keepingCapacity: false)
    }

    private func increment(_ value: UInt64) throws -> UInt64 {
        let (next, overflow) = value.addingReportingOverflow(1)
        guard !overflow else { throw RelayControllerTransportError.sequenceOverflow }
        return next
    }
}

private enum NativeRelayProtocol {
    static func clientHello(routeID: Data) throws -> Data {
        try result(routeID.withUnsafeBytes {
            termirust_mobile_relay_client_hello(
                $0.bindMemory(to: UInt8.self).baseAddress,
                routeID.count
            )
        })
    }

    static func admissionProof(
        routeID: Data,
        credential: Data,
        revocationEpoch: UInt64,
        nowUnixSeconds: UInt64,
        challenge: Data
    ) throws -> Data {
        try routeID.withUnsafeBytes { route in
            try credential.withUnsafeBytes { secret in
                try challenge.withUnsafeBytes { challenge in
                    try result(termirust_mobile_relay_admission_proof(
                        route.bindMemory(to: UInt8.self).baseAddress,
                        routeID.count,
                        secret.bindMemory(to: UInt8.self).baseAddress,
                        credential.count,
                        revocationEpoch,
                        nowUnixSeconds,
                        challenge.bindMemory(to: UInt8.self).baseAddress,
                        challenge.count
                    ))
                }
            }
        }
    }

    static func admissionConnectionID(result admission: Data) throws -> UInt64 {
        let bytes = try admission.withUnsafeBytes { admission in
            try result(termirust_mobile_relay_admission_connection_id(
                admission.bindMemory(to: UInt8.self).baseAddress,
                admission.count
            ))
        }
        guard bytes.count == 8 else { throw RelayControllerTransportError.malformedFrame }
        return bytes.reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
    }

    static func encodeEnvelope(routeID: Data, sequence: UInt64, payload: Data) throws -> Data {
        try routeID.withUnsafeBytes { route in
            try payload.withUnsafeBytes { payload in
                try result(termirust_mobile_relay_encode_envelope(
                    route.bindMemory(to: UInt8.self).baseAddress,
                    routeID.count,
                    sequence,
                    payload.bindMemory(to: UInt8.self).baseAddress,
                    payload.count
                ))
            }
        }
    }

    static func decodeEnvelope(routeID: Data, expectedSequence: UInt64, envelope: Data) throws -> Data {
        try routeID.withUnsafeBytes { route in
            try envelope.withUnsafeBytes { envelope in
                try result(termirust_mobile_relay_decode_envelope(
                    route.bindMemory(to: UInt8.self).baseAddress,
                    routeID.count,
                    expectedSequence,
                    envelope.bindMemory(to: UInt8.self).baseAddress,
                    envelope.count
                ))
            }
        }
    }

    private static func result(_ value: TermiRustMobileResult) throws -> Data {
        defer { termirust_mobile_free_result(value) }
        guard value.ok else {
            throw RelayControllerTransportError.nativeProtocolFailure(
                String(decoding: data(value.error), as: UTF8.self)
            )
        }
        return data(value.data)
    }

    private static func data(_ buffer: TermiRustMobileByteBuffer) -> Data {
        guard let pointer = buffer.ptr, buffer.len > 0 else { return Data() }
        return Data(bytes: pointer, count: buffer.len)
    }
}

private final class RelayPinnedSessionDelegate: NSObject, URLSessionDelegate, @unchecked Sendable {
    private let expectedSPKI: Data

    init(expectedSPKI: Data) {
        self.expectedSPKI = expectedSPKI
    }

    func urlSession(
        _ session: URLSession,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust,
              SecTrustEvaluateWithError(trust, nil),
              let certificates = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
              let certificate = certificates.first,
              let spki = try? SubjectPublicKeyInfo.extract(
                  from: SecCertificateCopyData(certificate) as Data
              ),
              Data(SHA256.hash(data: spki)) == expectedSPKI else {
            completionHandler(.cancelAuthenticationChallenge, nil)
            return
        }
        completionHandler(.useCredential, URLCredential(trust: trust))
    }
}

private enum SubjectPublicKeyInfo {
    private struct Node {
        let tag: UInt8
        let start: Int
        let content: Int
        let end: Int
    }

    static func extract(from certificate: Data) throws -> Data {
        let bytes = [UInt8](certificate)
        let outer = try node(bytes, at: 0)
        guard outer.tag == 0x30, outer.end == bytes.count else { throw RelayControllerTransportError.invalidCertificate }
        let tbs = try node(bytes, at: outer.content)
        guard tbs.tag == 0x30 else { throw RelayControllerTransportError.invalidCertificate }
        var cursor = tbs.content
        if try node(bytes, at: cursor).tag == 0xa0 { cursor = try node(bytes, at: cursor).end }
        for _ in 0 ..< 5 { cursor = try node(bytes, at: cursor).end }
        let spki = try node(bytes, at: cursor)
        guard spki.tag == 0x30, spki.end <= tbs.end else { throw RelayControllerTransportError.invalidCertificate }
        return Data(bytes[spki.start ..< spki.end])
    }

    private static func node(_ bytes: [UInt8], at start: Int) throws -> Node {
        guard start >= 0, start + 2 <= bytes.count else { throw RelayControllerTransportError.invalidCertificate }
        let tag = bytes[start]
        let firstLength = bytes[start + 1]
        let content: Int
        let length: Int
        if firstLength & 0x80 == 0 {
            content = start + 2
            length = Int(firstLength)
        } else {
            let count = Int(firstLength & 0x7f)
            guard count >= 1, count <= 4, start + 2 + count <= bytes.count else {
                throw RelayControllerTransportError.invalidCertificate
            }
            var decoded = 0
            for byte in bytes[(start + 2) ..< (start + 2 + count)] {
                decoded = try Math.add(decoded, Int(byte))
            }
            content = start + 2 + count
            length = decoded
        }
        let (end, overflow) = content.addingReportingOverflow(length)
        guard !overflow, end <= bytes.count else { throw RelayControllerTransportError.invalidCertificate }
        return Node(tag: tag, start: start, content: content, end: end)
    }
}

private enum Math {
    static func add(_ accumulated: Int, _ byte: Int) throws -> Int {
        let (shifted, shiftOverflow) = accumulated.multipliedReportingOverflow(by: 256)
        let (result, addOverflow) = shifted.addingReportingOverflow(byte)
        guard !shiftOverflow, !addOverflow else { throw RelayControllerTransportError.invalidCertificate }
        return result
    }
}

private enum RelayControllerTransportError: Error {
    case invalidConfiguration
    case invalidCertificate
    case subprotocolRejected
    case malformedFrame
    case receiveBufferExceeded
    case sequenceOverflow
    case closed
    case nativeProtocolFailure(String)
}
