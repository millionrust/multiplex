import Crypto
import Foundation
import NIOCore
import NIOPosix
@preconcurrency import NIOSSH

enum SSHControllerTransport {
    static let remoteCommand = "termirust controller-bridge --stdio"

    static func factory(
        hostID: String,
        configuration: ControllerRemoteRouteConfiguration,
        credentials: any ControllerRouteCredentialStoring
    ) throws -> ControllerTransportFactory {
        try configuration.validate()
        guard configuration.kind == .ssh else {
            throw ControllerRemoteRouteConfigurationError.unsupportedRoute
        }
        return ControllerTransportFactory { _ in
            try await SSHControllerDuplexConnection.open(
                hostID: hostID,
                configuration: configuration,
                credentials: credentials
            )
        }
    }
}

private final class SSHControllerDuplexConnection: ControllerDuplexConnection, @unchecked Sendable {
    private let group: MultiThreadedEventLoopGroup
    private let parent: Channel
    private let child: Channel
    private let reads: SSHControllerReadBuffer

    private init(
        group: MultiThreadedEventLoopGroup,
        parent: Channel,
        child: Channel,
        reads: SSHControllerReadBuffer
    ) {
        self.group = group
        self.parent = parent
        self.child = child
        self.reads = reads
    }

    static func open(
        hostID: String,
        configuration: ControllerRemoteRouteConfiguration,
        credentials: any ControllerRouteCredentialStoring
    ) async throws -> SSHControllerDuplexConnection {
        try await sshControllerBlocking {
            let reference = try required(configuration.credential)
            var credential = try required(credentials.load(hostID: hostID, reference: reference))
            defer { credential.resetBytes(in: 0 ..< credential.count) }
            let username = try required(configuration.username)
            let authentication = try required(configuration.sshAuthentication)
            let authDelegate: NIOSSHClientUserAuthenticationDelegate
            switch authentication {
            case .password:
                authDelegate = SimplePasswordDelegate(
                    username: username,
                    password: String(decoding: credential, as: UTF8.self)
                )
            case .privateKey:
                authDelegate = SingleControllerPrivateKeyDelegate(
                    username: username,
                    privateKey: try OpenSSHPrivateKeyParser.parse(
                        String(decoding: credential, as: UTF8.self)
                    )
                )
            }
            let auth = SSHControllerAuthenticationBox(authDelegate)
            let hostKey = try SSHControllerHostKeyDelegate(pin: required(configuration.trustPin))
            let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
            var parent: Channel?
            var child: Channel?
            do {
                let bootstrap = ClientBootstrap(group: group)
                    .connectTimeout(.seconds(10))
                    .channelInitializer { channel in
                        channel.eventLoop.makeCompletedFuture {
                            try channel.pipeline.syncOperations.addHandler(NIOSSHHandler(
                                role: .client(.init(
                                    userAuthDelegate: auth.delegate,
                                    serverAuthDelegate: hostKey
                                )),
                                allocator: channel.allocator,
                                inboundChildChannelInitializer: nil
                            ))
                        }
                    }
                    .channelOption(
                        ChannelOptions.socket(SocketOptionLevel(IPPROTO_TCP), TCP_NODELAY),
                        value: 1
                    )
                parent = try bootstrap.connect(
                    host: configuration.endpoint,
                    port: Int(try required(configuration.port))
                ).wait()
                let parentChannel = try required(parent)
                let reads = SSHControllerReadBuffer()
                let childPromise = parentChannel.eventLoop.makePromise(of: Channel.self)
                let handler = try parentChannel.pipeline.handler(type: NIOSSHHandler.self).wait()
                handler.createChannel(childPromise, channelType: .session) { channel, type in
                    guard type == .session else {
                        return channel.eventLoop.makeFailedFuture(MobileSSHError.invalidChannelType)
                    }
                    return channel.pipeline.addHandlers(
                        SSHControllerOutputHandler(reads: reads),
                        SSHControllerChannelErrorHandler(reads: reads)
                    )
                }
                child = try childPromise.futureResult.wait()
                let childChannel = try required(child)
                let promise = childChannel.eventLoop.makePromise(of: Void.self)
                childChannel.pipeline.triggerUserOutboundEvent(
                    SSHChannelRequestEvent.ExecRequest(
                        command: SSHControllerTransport.remoteCommand,
                        wantReply: true
                    ),
                    promise: promise
                )
                try promise.futureResult.wait()
                return SSHControllerDuplexConnection(
                    group: group,
                    parent: parentChannel,
                    child: childChannel,
                    reads: reads
                )
            } catch {
                try? child?.close().wait()
                try? parent?.close().wait()
                try? group.syncShutdownGracefully()
                throw error
            }
        }
    }

    func send(_ data: Data) async throws {
        try await sshControllerBlocking {
            var buffer = self.child.allocator.buffer(capacity: data.count)
            buffer.writeBytes(data)
            try self.child.writeAndFlush(
                SSHChannelData(type: .channel, data: .byteBuffer(buffer))
            ).wait()
        }
    }

    func receive(maximumLength: Int) async throws -> Data {
        try await reads.receive(maximumLength: maximumLength)
    }

    func cancel() {
        Task { await reads.close() }
        child.close(promise: nil)
        parent.close(promise: nil)
        group.shutdownGracefully { _ in }
    }
}

private actor SSHControllerReadBuffer {
    private static let maximumBufferedBytes = 1 * 1_024 * 1_024
    private var bytes = Data()
    private var waiter: (maximum: Int, continuation: CheckedContinuation<Data, Error>)?
    private var closed = false
    private var terminalError: Error?

    func append(_ data: Data) {
        guard !closed, !data.isEmpty else { return }
        guard data.count <= Self.maximumBufferedBytes,
              bytes.count <= Self.maximumBufferedBytes - data.count else {
            fail(SSHControllerTransportError.receiveBufferExceeded)
            return
        }
        bytes.append(data)
        drain()
    }

    func receive(maximumLength: Int) async throws -> Data {
        guard maximumLength > 0 else { return Data() }
        if !bytes.isEmpty { return take(maximumLength) }
        if let terminalError { throw terminalError }
        if closed { throw ControllerPairingError.connectionClosed }
        guard waiter == nil else { throw ControllerPairingError.connectionClosed }
        return try await withCheckedThrowingContinuation { continuation in
            waiter = (maximumLength, continuation)
            drain()
        }
    }

    func close() {
        closed = true
        waiter?.continuation.resume(
            throwing: terminalError ?? ControllerPairingError.connectionClosed
        )
        waiter = nil
        bytes.removeAll(keepingCapacity: false)
    }

    func fail(_ error: Error) {
        guard !closed else { return }
        terminalError = error
        close()
    }

    private func drain() {
        guard let waiter, !bytes.isEmpty else { return }
        self.waiter = nil
        waiter.continuation.resume(returning: take(waiter.maximum))
    }

    private func take(_ maximum: Int) -> Data {
        let count = min(maximum, bytes.count)
        let result = bytes.prefix(count)
        bytes.removeFirst(count)
        return Data(result)
    }
}

private enum SSHControllerTransportError: Error {
    case receiveBufferExceeded
}

private final class SSHControllerOutputHandler: ChannelInboundHandler, @unchecked Sendable {
    typealias InboundIn = SSHChannelData
    private let reads: SSHControllerReadBuffer

    init(reads: SSHControllerReadBuffer) {
        self.reads = reads
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let message = unwrapInboundIn(data)
        guard case .byteBuffer(var buffer) = message.data,
              let bytes = buffer.readBytes(length: buffer.readableBytes),
              !bytes.isEmpty else { return }
        Task { await reads.append(Data(bytes)) }
    }

    func channelInactive(context: ChannelHandlerContext) {
        Task { await reads.close() }
        context.fireChannelInactive()
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        Task { await reads.fail(error) }
        context.close(promise: nil)
    }
}

private final class SSHControllerChannelErrorHandler: ChannelInboundHandler, @unchecked Sendable {
    typealias InboundIn = Any
    private let reads: SSHControllerReadBuffer

    init(reads: SSHControllerReadBuffer) {
        self.reads = reads
    }

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        Task { await reads.fail(error) }
        context.close(promise: nil)
    }
}

private final class SSHControllerAuthenticationBox: @unchecked Sendable {
    let delegate: NIOSSHClientUserAuthenticationDelegate
    init(_ delegate: NIOSSHClientUserAuthenticationDelegate) { self.delegate = delegate }
}

private final class SingleControllerPrivateKeyDelegate:
    NIOSSHClientUserAuthenticationDelegate, @unchecked Sendable {
    private var offer: NIOSSHUserAuthenticationOffer?

    init(username: String, privateKey: NIOSSHPrivateKey) {
        offer = NIOSSHUserAuthenticationOffer(
            username: username,
            serviceName: "",
            offer: .privateKey(.init(privateKey: privateKey))
        )
    }

    func nextAuthenticationType(
        availableMethods: NIOSSHAvailableUserAuthenticationMethods,
        nextChallengePromise: EventLoopPromise<NIOSSHUserAuthenticationOffer?>
    ) {
        guard let offer, availableMethods.contains(.publicKey) else {
            nextChallengePromise.succeed(nil)
            return
        }
        self.offer = nil
        nextChallengePromise.succeed(offer)
    }
}

private final class SSHControllerHostKeyDelegate:
    NIOSSHClientServerAuthenticationDelegate, @unchecked Sendable {
    private let openSSHKey: NIOSSHPublicKey?
    private let fingerprint: String?

    init(pin: String) throws {
        let trimmed = pin.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("SHA256:") {
            openSSHKey = nil
            fingerprint = trimmed
        } else {
            openSSHKey = try NIOSSHPublicKey(openSSHPublicKey: trimmed)
            fingerprint = nil
        }
    }

    func validateHostKey(
        hostKey: NIOSSHPublicKey,
        validationCompletePromise: EventLoopPromise<Void>
    ) {
        let accepted: Bool
        if let openSSHKey {
            accepted = hostKey == openSSHKey ||
                String(openSSHPublicKey: hostKey) == String(openSSHPublicKey: openSSHKey)
        } else {
            accepted = fingerprint == Self.fingerprint(hostKey)
        }
        accepted
            ? validationCompletePromise.succeed(())
            : validationCompletePromise.fail(MobileSSHError.hostKeyMismatch)
    }

    private static func fingerprint(_ key: NIOSSHPublicKey) -> String? {
        let fields = String(openSSHPublicKey: key).split(separator: " ")
        guard fields.count >= 2, let blob = Data(base64Encoded: String(fields[1])) else { return nil }
        return "SHA256:" + Data(SHA256.hash(data: blob)).base64EncodedString()
            .trimmingCharacters(in: CharacterSet(charactersIn: "="))
    }
}

private func required<T>(_ value: T?) throws -> T {
    guard let value else { throw ControllerRemoteRouteConfigurationError.invalidCombination }
    return value
}

private func sshControllerBlocking<T: Sendable>(
    _ body: @escaping @Sendable () throws -> T
) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global(qos: .userInitiated).async {
            do { continuation.resume(returning: try body()) }
            catch { continuation.resume(throwing: error) }
        }
    }
}
