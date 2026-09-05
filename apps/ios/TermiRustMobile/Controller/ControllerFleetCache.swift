import Foundation

struct CachedHostFleet: Codable, Equatable, Sendable {
    let hostFingerprint: String
    let revision: UInt64
    let updateSequence: UInt64
    let updatedAt: Date
    var lastViewedAt: Date
    let sessions: [ControllerSessionSummary]
}

struct ControllerFleetCache: Codable, Equatable, Sendable {
    static let currentSchemaVersion = 1

    let schemaVersion: Int
    private(set) var hosts: [String: CachedHostFleet]

    init(hosts: [String: CachedHostFleet] = [:]) {
        self.schemaVersion = Self.currentSchemaVersion
        self.hosts = hosts
    }

    mutating func replace(
        hostFingerprint: String,
        revision: UInt64,
        updateSequence: UInt64,
        sessions: [ControllerSessionSummary],
        selectedHostFingerprint: String,
        now: Date,
        encoder: JSONEncoder = ControllerFleetCache.encoder()
    ) throws {
        guard !hostFingerprint.isEmpty,
              hostFingerprint.utf8.count <= 128,
              revision > 0,
              updateSequence > 0,
              sessions.count <= ControllerCacheLimits.maxSessionsPerHost else {
            throw ControllerCacheError.resourceLimit
        }
        try sessions.forEach { try $0.validate() }

        if let current = hosts[hostFingerprint] {
            guard revision > current.revision
                    || (revision == current.revision && updateSequence > current.updateSequence)
            else {
                throw ControllerCacheError.staleUpdate
            }
        }

        let previous = hosts
        hosts[hostFingerprint] = CachedHostFleet(
            hostFingerprint: hostFingerprint,
            revision: revision,
            updateSequence: updateSequence,
            updatedAt: now,
            lastViewedAt: now,
            sessions: sessions
        )

        evictWholeHostCaches(
            selectedHostFingerprint: selectedHostFingerprint,
            encoder: encoder
        )
        guard isWithinLimits(encoder: encoder) else {
            hosts = previous
            throw ControllerCacheError.resourceLimit
        }
    }

    mutating func markViewed(hostFingerprint: String, at date: Date) {
        guard var host = hosts[hostFingerprint] else { return }
        host.lastViewedAt = date
        hosts[hostFingerprint] = host
    }

    mutating func remove(hostFingerprint: String) {
        hosts.removeValue(forKey: hostFingerprint)
    }

    private mutating func evictWholeHostCaches(
        selectedHostFingerprint: String,
        encoder: JSONEncoder
    ) {
        let candidates = hosts.values
            .filter { $0.hostFingerprint != selectedHostFingerprint }
            .sorted {
                if $0.lastViewedAt != $1.lastViewedAt {
                    return $0.lastViewedAt < $1.lastViewedAt
                }
                return $0.hostFingerprint < $1.hostFingerprint
            }
        var candidateIndex = 0
        while !isWithinLimits(encoder: encoder), candidateIndex < candidates.count {
            hosts.removeValue(forKey: candidates[candidateIndex].hostFingerprint)
            candidateIndex += 1
        }
    }

    private func isWithinLimits(encoder: JSONEncoder) -> Bool {
        guard hosts.count <= ControllerCacheLimits.maxHosts,
              hosts.values.reduce(0, { $0 + $1.sessions.count })
                <= ControllerCacheLimits.maxSessionsGlobal else {
            return false
        }
        return ((try? encoder.encode(self).count) ?? Int.max)
            <= ControllerCacheLimits.maxEncodedBytes
    }

    static func encoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        encoder.outputFormatting = [.sortedKeys]
        return encoder
    }

    static func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .millisecondsSince1970
        return decoder
    }
}

actor ControllerFleetCacheStore {
    private let fileURL: URL

    init(fileURL: URL? = nil) throws {
        if let fileURL {
            self.fileURL = fileURL
            return
        }
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let directory = support.appendingPathComponent("TermiRust/Controller", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication]
        )
        self.fileURL = directory.appendingPathComponent("fleet-cache-v1.json")
    }

    func load() throws -> ControllerFleetCache {
        guard FileManager.default.fileExists(atPath: fileURL.path) else {
            return ControllerFleetCache()
        }
        let data = try Data(contentsOf: fileURL, options: .mappedIfSafe)
        guard data.count <= ControllerCacheLimits.maxEncodedBytes else {
            throw ControllerCacheError.resourceLimit
        }
        let cache = try ControllerFleetCache.decoder().decode(ControllerFleetCache.self, from: data)
        guard cache.schemaVersion == ControllerFleetCache.currentSchemaVersion else {
            throw ControllerCacheError.newerSchema
        }
        return cache
    }

    func save(_ cache: ControllerFleetCache) throws {
        let data = try ControllerFleetCache.encoder().encode(cache)
        guard data.count <= ControllerCacheLimits.maxEncodedBytes else {
            throw ControllerCacheError.resourceLimit
        }
        try data.write(to: fileURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    func delete() throws {
        guard FileManager.default.fileExists(atPath: fileURL.path) else { return }
        try FileManager.default.removeItem(at: fileURL)
    }
}

enum ControllerCacheError: Error, Equatable {
    case resourceLimit
    case staleUpdate
    case newerSchema
}
