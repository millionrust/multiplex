import Foundation

private struct PairedHostDocument: Codable {
    static let currentSchemaVersion = 1

    let schemaVersion: Int
    var hosts: [PairedHostRecord]

    init(hosts: [PairedHostRecord]) {
        self.schemaVersion = Self.currentSchemaVersion
        self.hosts = hosts
    }
}

actor PairedHostStore {
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
        self.fileURL = directory.appendingPathComponent("paired-hosts-v1.json")
    }

    func load() throws -> [PairedHostRecord] {
        guard FileManager.default.fileExists(atPath: fileURL.path) else { return [] }
        let data = try Data(contentsOf: fileURL, options: .mappedIfSafe)
        guard data.count <= 256 * 1_024 else { throw PairedHostStoreError.resourceLimit }
        let document = try decoder.decode(PairedHostDocument.self, from: data)
        guard document.schemaVersion == PairedHostDocument.currentSchemaVersion,
              document.hosts.count <= ControllerCacheLimits.maxHosts,
              Set(document.hosts.map(\.id)).count == document.hosts.count,
              document.hosts.allSatisfy({
                  $0.schemaVersion == PairedHostRecord.currentSchemaVersion
                      && $0.hostStaticPublicKey.count == 32
              }) else {
            throw PairedHostStoreError.invalidDocument
        }
        return document.hosts.sorted { $0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending }
    }

    func upsert(_ record: PairedHostRecord) throws -> [PairedHostRecord] {
        var hosts = try load()
        if let index = hosts.firstIndex(where: { $0.id == record.id }) {
            hosts[index] = record
        } else {
            guard hosts.count < ControllerCacheLimits.maxHosts else {
                throw PairedHostStoreError.resourceLimit
            }
            hosts.append(record)
        }
        try save(hosts)
        return try load()
    }

    func remove(id: String) throws -> [PairedHostRecord] {
        let hosts = try load().filter { $0.id != id }
        try save(hosts)
        return hosts
    }

    private func save(_ hosts: [PairedHostRecord]) throws {
        let data = try encoder.encode(PairedHostDocument(hosts: hosts))
        guard data.count <= 256 * 1_024 else { throw PairedHostStoreError.resourceLimit }
        try data.write(to: fileURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    private var encoder: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        encoder.outputFormatting = [.sortedKeys]
        return encoder
    }

    private var decoder: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .millisecondsSince1970
        return decoder
    }
}

enum PairedHostStoreError: Error, Equatable {
    case resourceLimit
    case invalidDocument
}
