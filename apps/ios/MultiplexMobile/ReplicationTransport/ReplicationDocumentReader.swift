import Foundation
import Darwin

enum ReplicationTransferKind: Sendable {
    case enrollmentBundle, encryptedReplica
    var maximumBytes: Int { self == .enrollmentBundle ? 192 * 1024 : 8 * 1024 * 1024 }
}

enum ReplicationTransferFailure: Error, Equatable {
    case invalidLocation, empty, tooLarge, denied, unavailable, unsupportedPublication
}

/// Returns untrusted bytes only. Never send a provider URL to the Rust service.
actor ReplicationDocumentReader {
    func read(_ selected: URL, kind: ReplicationTransferKind) throws -> Data {
        guard selected.isFileURL else { throw ReplicationTransferFailure.invalidLocation }
        try Task.checkCancellation()
        let scoped = selected.startAccessingSecurityScopedResource()
        defer { if scoped { selected.stopAccessingSecurityScopedResource() } }
        var coordinationError: NSError?
        var result: Result<Data, Error> = .failure(ReplicationTransferFailure.unavailable)
        NSFileCoordinator().coordinate(readingItemAt: selected, options: [], error: &coordinationError) { coordinated in
            result = Result {
                let values = try coordinated.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
                guard values.isRegularFile == true, values.isSymbolicLink != true else {
                    throw ReplicationTransferFailure.invalidLocation
                }
                let file = try FileHandle(forReadingFrom: coordinated)
                defer { try? file.close() }
                var bytes = Data()
                while true {
                    try Task.checkCancellation()
                    let part = try file.read(upToCount: min(8192, kind.maximumBytes + 1 - bytes.count)) ?? Data()
                    if part.isEmpty { break }
                    bytes.append(part)
                    guard bytes.count <= kind.maximumBytes else { throw ReplicationTransferFailure.tooLarge }
                }
                try Task.checkCancellation()
                guard !bytes.isEmpty else { throw ReplicationTransferFailure.empty }
                return bytes
            }
        }
        do {
            if let coordinationError { throw coordinationError }
            return try result.get()
        } catch let error as ReplicationTransferFailure { throw error }
        catch is CancellationError { throw CancellationError() }
        catch {
            throw Self.classify(error as NSError)
        }
    }

    // Coordination can wrap POSIX access failures in a generic Foundation error.
    static func classify(_ error: NSError) -> ReplicationTransferFailure {
        var current: NSError? = error
        for _ in 0..<8 {
            guard let value = current else { break }
            if (value.domain == NSCocoaErrorDomain && value.code == NSFileReadNoPermissionError)
                || (value.domain == NSPOSIXErrorDomain && [Int(EACCES), Int(EPERM)].contains(value.code)) {
                return .denied
            }
            current = value.userInfo[NSUnderlyingErrorKey] as? NSError
        }
        return .unavailable
    }

    func publish() throws -> Never { throw ReplicationTransferFailure.unsupportedPublication }
}
