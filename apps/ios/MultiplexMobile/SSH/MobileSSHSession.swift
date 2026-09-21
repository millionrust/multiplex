import Foundation
import NIOCore
import NIOPosix
@preconcurrency import NIOSSH

enum MobileSSHError: Error, LocalizedError {
    case missingCredential(MobileAuthKind)
    case hostKeyNotPinned
    case hostKeyMismatch
    case invalidChannelType

    var errorDescription: String? {
        switch self {
        case .missingCredential(let kind):
            return "Missing \(kind.rawValue) credential for this host."
        case .hostKeyNotPinned:
            return "No known-host pin exists for this endpoint."
        case .hostKeyMismatch:
            return "Host key mismatch. Connection blocked."
        case .invalidChannelType:
            return "The SSH server opened an unexpected channel type."
        }
    }
}

enum TerminalConnectionState: Equatable {
    case disconnected
    case connecting
    case connected
    case failed(String)
}

protocol MobileSSHConnecting: Sendable {
    func connect(
        host: MobileHost,
        knownHost: MobileKnownHost?,
        onOutput: @escaping @Sendable (Data) -> Void
    ) async throws
    func send(_ bytes: Data) async throws
    func resize(columns: Int, rows: Int) async throws
    func disconnect() async
}

final class DirectSSHSessionClient: MobileSSHConnecting, @unchecked Sendable {
    private let secretStore: SecretStoring?
    private let lock = NSLock()
    private var group: MultiThreadedEventLoopGroup?
    private var parentChannel: Channel?
    private var childChannel: Channel?

    init(secretStore: SecretStoring? = nil) {
        self.secretStore = secretStore
    }

    func connect(
        host: MobileHost,
        knownHost: MobileKnownHost?,
        onOutput: @escaping @Sendable (Data) -> Void
    ) async throws {
        await disconnect()

        guard let knownHost else {
            throw MobileSSHError.hostKeyNotPinned
        }

        guard let secretRef = host.auth.secretRef,
              let credential = try secretStore?.readSecret(account: secretRef),
              !credential.isEmpty else {
            throw MobileSSHError.missingCredential(host.auth.kind)
        }
        let authDelegate = try AuthenticationDelegateBox(authenticationDelegate(for: host, credential: credential))

        try await runBlocking {
            try self.connectBlocking(
                host: host,
                knownHost: knownHost,
                userAuthDelegate: authDelegate,
                onOutput: onOutput
            )
        }
    }

    func send(_ bytes: Data) async throws {
        try await runBlocking {
            let channel = try self.currentChildChannel()
            var buffer = channel.allocator.buffer(capacity: bytes.count)
            buffer.writeBytes(bytes)
            try channel.writeAndFlush(SSHChannelData(type: .channel, data: .byteBuffer(buffer))).wait()
        }
    }

    func resize(columns: Int, rows: Int) async throws {
        try await runBlocking {
            let channel = try self.currentChildChannel()
            let event = SSHChannelRequestEvent.WindowChangeRequest(
                terminalCharacterWidth: columns,
                terminalRowHeight: rows,
                terminalPixelWidth: 0,
                terminalPixelHeight: 0
            )
            let promise = channel.eventLoop.makePromise(of: Void.self)
            channel.pipeline.triggerUserOutboundEvent(event, promise: promise)
            try promise.futureResult.wait()
        }
    }

    func disconnect() async {
        try? await runBlocking {
            self.lock.lock()
            let child = self.childChannel
            let parent = self.parentChannel
            let group = self.group
            self.childChannel = nil
            self.parentChannel = nil
            self.group = nil
            self.lock.unlock()

            try? child?.close().wait()
            try? parent?.close().wait()
            try? group?.syncShutdownGracefully()
        }
    }

    private func connectBlocking(
        host: MobileHost,
        knownHost: MobileKnownHost,
        userAuthDelegate: AuthenticationDelegateBox,
        onOutput: @escaping @Sendable (Data) -> Void
    ) throws {
        let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
        let hostKeyDelegate = try PinnedHostKeyDelegate(knownHost: knownHost)
        var parent: Channel?
        var child: Channel?

        do {
            let bootstrap = ClientBootstrap(group: group)
                .channelInitializer { channel in
                    channel.eventLoop.makeCompletedFuture {
                        let ssh = NIOSSHHandler(
                            role: .client(
                                .init(
                                    userAuthDelegate: userAuthDelegate.delegate,
                                    serverAuthDelegate: hostKeyDelegate
                                )
                            ),
                            allocator: channel.allocator,
                            inboundChildChannelInitializer: nil
                        )
                        try channel.pipeline.syncOperations.addHandler(ssh)
                        try channel.pipeline.syncOperations.addHandler(MobileSSHErrorHandler())
                    }
                }
                .channelOption(ChannelOptions.socket(SocketOptionLevel(SOL_SOCKET), SO_REUSEADDR), value: 1)
                .channelOption(ChannelOptions.socket(SocketOptionLevel(IPPROTO_TCP), TCP_NODELAY), value: 1)

            parent = try bootstrap.connect(host: host.host, port: Int(host.port)).wait()
            let parentChannel = parent!
            let childPromise = parentChannel.eventLoop.makePromise(of: Channel.self)
            let sshHandler = try parentChannel.pipeline.handler(type: NIOSSHHandler.self).wait()
            sshHandler.createChannel(childPromise, channelType: .session) { childChannel, channelType in
                guard channelType == .session else {
                    return childChannel.eventLoop.makeFailedFuture(MobileSSHError.invalidChannelType)
                }
                return childChannel.eventLoop.makeCompletedFuture {
                    try childChannel.pipeline.syncOperations.addHandler(
                        MobileShellOutputHandler(onOutput: onOutput)
                    )
                    try childChannel.pipeline.syncOperations.addHandler(MobileSSHErrorHandler())
                }
            }
            child = try childPromise.futureResult.wait()
            let childChannel = child!

            try requestPTYAndShell(on: childChannel)

            if let startup = TmuxBootstrap(host: host).startupCommand() {
                var buffer = childChannel.allocator.buffer(capacity: startup.utf8.count + 1)
                buffer.writeString(startup)
                buffer.writeString("\n")
                try childChannel.writeAndFlush(SSHChannelData(type: .channel, data: .byteBuffer(buffer))).wait()
            }

            lock.lock()
            self.group = group
            self.parentChannel = parent
            self.childChannel = child
            lock.unlock()
        } catch {
            try? child?.close().wait()
            try? parent?.close().wait()
            try? group.syncShutdownGracefully()
            throw error
        }
    }

    private func requestPTYAndShell(on channel: Channel) throws {
        let pty = SSHChannelRequestEvent.PseudoTerminalRequest(
            wantReply: false,
            term: "xterm-256color",
            terminalCharacterWidth: 80,
            terminalRowHeight: 24,
            terminalPixelWidth: 0,
            terminalPixelHeight: 0,
            terminalModes: .init([:])
        )
        channel.pipeline.triggerUserOutboundEvent(pty, promise: nil)
        let shell = SSHChannelRequestEvent.ShellRequest(wantReply: false)
        channel.pipeline.triggerUserOutboundEvent(shell, promise: nil)
    }

    private func authenticationDelegate(
        for host: MobileHost,
        credential: String
    ) throws -> NIOSSHClientUserAuthenticationDelegate {
        switch host.auth.kind {
        case .password:
            return SimplePasswordDelegate(username: host.username, password: credential)
        case .privateKey:
            let privateKey = try OpenSSHPrivateKeyParser.parse(credential)
            return SinglePrivateKeyDelegate(username: host.username, privateKey: privateKey)
        }
    }

    private func currentChildChannel() throws -> Channel {
        lock.lock()
        defer { lock.unlock() }
        guard let childChannel else {
            throw MobileSSHError.missingCredential(.password)
        }
        return childChannel
    }
}

private final class AuthenticationDelegateBox: @unchecked Sendable {
    let delegate: NIOSSHClientUserAuthenticationDelegate

    init(_ delegate: NIOSSHClientUserAuthenticationDelegate) {
        self.delegate = delegate
    }
}

private final class SinglePrivateKeyDelegate: NIOSSHClientUserAuthenticationDelegate, @unchecked Sendable {
    private var authRequest: NIOSSHUserAuthenticationOffer?

    init(username: String, privateKey: NIOSSHPrivateKey) {
        self.authRequest = NIOSSHUserAuthenticationOffer(
            username: username,
            serviceName: "",
            offer: .privateKey(.init(privateKey: privateKey))
        )
    }

    func nextAuthenticationType(
        availableMethods: NIOSSHAvailableUserAuthenticationMethods,
        nextChallengePromise: EventLoopPromise<NIOSSHUserAuthenticationOffer?>
    ) {
        guard let authRequest, availableMethods.contains(.publicKey) else {
            nextChallengePromise.succeed(nil)
            return
        }
        self.authRequest = nil
        nextChallengePromise.succeed(authRequest)
    }
}

final class PinnedHostKeyDelegate: NIOSSHClientServerAuthenticationDelegate, @unchecked Sendable {
    private let expectedKey: NIOSSHPublicKey

    init(knownHost: MobileKnownHost) throws {
        self.expectedKey = try NIOSSHPublicKey(openSSHPublicKey: knownHost.publicKey)
    }

    func validateHostKey(hostKey: NIOSSHPublicKey, validationCompletePromise: EventLoopPromise<Void>) {
        if accepts(hostKey: hostKey) {
            validationCompletePromise.succeed(())
        } else {
            validationCompletePromise.fail(MobileSSHError.hostKeyMismatch)
        }
    }

    func accepts(hostKey: NIOSSHPublicKey) -> Bool {
        hostKey == expectedKey || String(openSSHPublicKey: hostKey) == String(openSSHPublicKey: expectedKey)
    }
}

private final class MobileShellOutputHandler: ChannelInboundHandler, @unchecked Sendable {
    typealias InboundIn = SSHChannelData

    private let onOutput: @Sendable (Data) -> Void

    init(onOutput: @escaping @Sendable (Data) -> Void) {
        self.onOutput = onOutput
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let message = unwrapInboundIn(data)
        guard case .byteBuffer(var buffer) = message.data else {
            return
        }
        if let bytes = buffer.readBytes(length: buffer.readableBytes), !bytes.isEmpty {
            onOutput(Data(bytes))
        }
    }
}

private final class MobileSSHErrorHandler: ChannelInboundHandler {
    typealias InboundIn = Any

    func errorCaught(context: ChannelHandlerContext, error: Error) {
        context.close(promise: nil)
    }
}

private func runBlocking<T>(_ body: @escaping @Sendable () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                continuation.resume(returning: try body())
            } catch {
                continuation.resume(throwing: error)
            }
        }
    }
}
