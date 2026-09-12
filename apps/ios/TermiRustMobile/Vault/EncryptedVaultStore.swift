import Foundation

protocol EncryptedVaultStoring {
    var hasEncryptedVault: Bool { get }
    func saveEncryptedVault(_ data: Data) throws
    func readEncryptedVault() throws -> Data?
    func clearEncryptedVault() throws
}

struct FileEncryptedVaultStore: EncryptedVaultStoring {
    private let fileURL: URL

    init(fileManager: FileManager = .default) throws {
        let directory = try fileManager.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        ).appendingPathComponent("TermiRustMobile", isDirectory: true)
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        self.fileURL = directory.appendingPathComponent("last-mobile-vault.encrypted.json")
    }

    var hasEncryptedVault: Bool {
        FileManager.default.fileExists(atPath: fileURL.path)
    }

    func saveEncryptedVault(_ data: Data) throws {
        try data.write(to: fileURL, options: [.atomic, .completeFileProtection])
    }

    func readEncryptedVault() throws -> Data? {
        guard hasEncryptedVault else {
            return nil
        }
        return try Data(contentsOf: fileURL)
    }

    func clearEncryptedVault() throws {
        guard hasEncryptedVault else {
            return
        }
        try FileManager.default.removeItem(at: fileURL)
    }
}
