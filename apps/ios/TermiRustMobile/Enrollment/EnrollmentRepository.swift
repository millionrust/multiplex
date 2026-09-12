import Foundation
import Darwin

struct EnrollmentSnapshot: Sendable, Equatable {
    var request: Data?
    var cancellationPending = false
}

protocol EnrollmentRepository: Sendable {
    func load() async throws -> EnrollmentSnapshot
    func prepare() async throws -> EnrollmentSnapshot
    func cancel(_ request: Data) async throws -> EnrollmentSnapshot
}

/// Actor isolation keeps synchronous Rust and Keychain work off the main actor.
actor NativeEnrollmentRepository: EnrollmentRepository {
    private let directory: URL
    private let store: any ReplicationSecureStore

    init(directory: URL? = nil, store: any ReplicationSecureStore = ReplicationKeychainStore()) {
        self.directory = directory ?? FileManager.default.urls(for: .applicationSupportDirectory,
            in: .userDomainMask)[0].appendingPathComponent("replication-product-v1", isDirectory: true)
        self.store = store
    }

    private func guarded<T>(_ action: (MobileReplicationProduct, URL) throws -> T) throws -> T {
        guard directory.isFileURL else { throw MobileReplicationError.Invalid }
        let manager = FileManager.default
        try manager.createDirectory(at: directory, withIntermediateDirectories: true)
        guard try directory.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink != true else {
            throw MobileReplicationError.Invalid
        }
        var root = directory.resolvingSymlinksInPath()
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try root.setResourceValues(values)
        let fd = open(root.appendingPathComponent("ui-operation.lock").path, O_CREAT | O_RDWR | O_NOFOLLOW, 0o600)
        guard fd >= 0 else { throw MobileReplicationError.Unavailable }
        defer { close(fd) }
        guard flock(fd, LOCK_EX | LOCK_NB) == 0 else { throw MobileReplicationError.Busy }
        defer { flock(fd, LOCK_UN) }
        return try action(MobileReplicationProduct(privateDirectory: root.path, store: store), root)
    }

    private func journal(_ root: URL) -> URL { root.appendingPathComponent("reviewed-cancellation.json") }

    private func reviewed(_ root: URL) throws -> Data? {
        let fd = open(journal(root).path, O_RDONLY | O_NOFOLLOW)
        guard fd >= 0 else {
            if errno == ENOENT { return nil }
            throw MobileReplicationError.Unavailable
        }
        defer { close(fd) }
        var metadata = stat()
        guard fstat(fd, &metadata) == 0, metadata.st_mode & S_IFMT == S_IFREG,
              (1...4096).contains(metadata.st_size) else { throw MobileReplicationError.Invalid }
        var bytes = [UInt8](repeating: 0, count: 4097)
        var length = 0
        while length < bytes.count {
            let count = bytes.withUnsafeMutableBytes { buffer in
                read(fd, buffer.baseAddress!.advanced(by: length), buffer.count - length)
            }
            if count == 0 { break }
            if count < 0 {
                if errno == EINTR { continue }
                throw MobileReplicationError.Unavailable
            }
            length += count
        }
        guard (1...4096).contains(length) else { throw MobileReplicationError.Invalid }
        return Data(bytes.prefix(length))
    }

    func load() async throws -> EnrollmentSnapshot {
        try guarded { product, root in
            if let request = try reviewed(root) { return EnrollmentSnapshot(request: request, cancellationPending: true) }
            return EnrollmentSnapshot(request: try product.pendingEnrollment()?.canonicalRequest)
        }
    }

    func prepare() async throws -> EnrollmentSnapshot {
        try guarded { product, root in
            guard try reviewed(root) == nil else { throw MobileReplicationError.RecoveryRequired }
            let exchange = root.appendingPathComponent("local-exchange", isDirectory: true)
            try FileManager.default.createDirectory(at: exchange, withIntermediateDirectories: true)
            let attributes = try exchange.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
            guard attributes.isDirectory == true, attributes.isSymbolicLink != true else {
                throw MobileReplicationError.Invalid
            }
            return EnrollmentSnapshot(request: try product.prepareEnrollment(localExchangeDirectory: exchange.path).canonicalRequest)
        }
    }

    func cancel(_ request: Data) async throws -> EnrollmentSnapshot {
        guard (1...4096).contains(request.count) else { throw MobileReplicationError.Invalid }
        return try guarded { product, root in
            if let previous = try reviewed(root), previous != request { throw MobileReplicationError.StaleRequest }
            // Save only the reviewed public request before mutating native custody.
            let path = journal(root)
            try request.write(to: path, options: [.atomic, .completeFileProtection])
            let handle = try FileHandle(forWritingTo: path)
            defer { try? handle.close() }
            try handle.synchronize()
            do { _ = try product.cancelPendingEnrollment(expectedRequest: request) }
            catch MobileReplicationError.StaleRequest {
                try FileManager.default.removeItem(at: path)
                throw MobileReplicationError.StaleRequest
            } catch MobileReplicationError.AlreadyConfigured {
                try FileManager.default.removeItem(at: path)
                throw MobileReplicationError.AlreadyConfigured
            }
            try FileManager.default.removeItem(at: path)
            return EnrollmentSnapshot()
        }
    }
}
