import Foundation

enum MobileVaultImportError: Error, Equatable, LocalizedError {
    case unsupportedSchema(Int)
    case encryptedVaultRequiresSharedCrypto
    case invalidVault
    case revokedSourceDevice(String)
    case revokedLocalDevice(String)
    case staleVault

    var errorDescription: String? {
        switch self {
        case .unsupportedSchema(let version):
            return "Unsupported mobile vault schema version \(version)."
        case .encryptedVaultRequiresSharedCrypto:
            return "This build is missing TermiRust shared vault crypto. Install the mobile crypto framework before importing encrypted vaults."
        case .invalidVault:
            return "The selected file is not a valid TermiRust mobile vault."
        case .revokedSourceDevice(let deviceId):
            return "This mobile vault was exported by a revoked device (\(deviceId)). Import blocked."
        case .revokedLocalDevice(let deviceId):
            return "This device has been revoked for the imported mobile vault (\(deviceId)). Import blocked."
        case .staleVault:
            return "Imported vault is older than the currently loaded vault. Import blocked to avoid overwriting newer mobile state."
        }
    }
}

protocol MobileVaultDecrypting {
    func decrypt(encryptedVaultData: Data, passphrase: String) throws -> Data
}

struct MobileVaultImporter {
    private let decoder = JSONDecoder()
    private let decryptor: MobileVaultDecrypting?

    init(decryptor: MobileVaultDecrypting? = nil) {
        self.decryptor = decryptor
    }

    func inspectEncryptedEnvelope(_ data: Data) throws -> EncryptedMobileVaultEnvelope {
        let envelope = try decoder.decode(EncryptedMobileVaultEnvelope.self, from: data)
        guard envelope.schemaVersion == mobileVaultSchemaVersion else {
            throw MobileVaultImportError.unsupportedSchema(envelope.schemaVersion)
        }
        return envelope
    }

    func importPlaintextVaultData(_ data: Data) throws -> MobileVaultExport {
        let vault = try decoder.decode(MobileVaultExport.self, from: data)
        guard vault.schemaVersion == mobileVaultSchemaVersion else {
            throw MobileVaultImportError.unsupportedSchema(vault.schemaVersion)
        }
        guard !vault.sourceDeviceRevoked else {
            throw MobileVaultImportError.revokedSourceDevice(vault.sourceDeviceId)
        }
        return vault
    }

    func importEncryptedVaultData(_ data: Data, passphrase: String) throws -> MobileVaultExport {
        _ = try inspectEncryptedEnvelope(data)
        guard let decryptor else {
            throw MobileVaultImportError.encryptedVaultRequiresSharedCrypto
        }
        let plaintext = try decryptor.decrypt(encryptedVaultData: data, passphrase: passphrase)
        return try importPlaintextVaultData(plaintext)
    }
}
