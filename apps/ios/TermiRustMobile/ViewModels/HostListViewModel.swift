import Foundation
import SwiftUI
import UniformTypeIdentifiers

@MainActor
final class HostListViewModel: ObservableObject {
    @Published private(set) var vault: MobileVaultExport?
    @Published private(set) var connectionState: TerminalConnectionState = .disconnected
    @Published private(set) var hasStoredEncryptedVault: Bool
    @Published private(set) var privacyCovered = false
    @Published var selectedHost: MobileHost?
    @Published var importError: String?

    let terminalBuffer = TerminalBuffer()

    private let vaultImporter: MobileVaultImporter
    private let secretStore: SecretStoring
    private let encryptedVaultStore: EncryptedVaultStoring?
    private let sshClient: MobileSSHConnecting
    private let localDeviceId: String

    init(
        vaultImporter: MobileVaultImporter,
        secretStore: SecretStoring,
        encryptedVaultStore: EncryptedVaultStoring? = nil,
        sshClient: MobileSSHConnecting = DirectSSHSessionClient(),
        localDeviceId: String = ""
    ) {
        self.vaultImporter = vaultImporter
        self.secretStore = secretStore
        self.encryptedVaultStore = encryptedVaultStore
        self.sshClient = sshClient
        self.localDeviceId = localDeviceId
        self.hasStoredEncryptedVault = encryptedVaultStore?.hasEncryptedVault ?? false
    }

    var hosts: [MobileHost] {
        vault?.hosts ?? []
    }

    var localDeviceIdForDisplay: String {
        localDeviceId.isEmpty ? "Unavailable" : localDeviceId
    }

    func pairingRequestText(
        label: String = "iOS Device",
        nowMillis: UInt64 = currentUnixMillis()
    ) throws -> String {
        guard !localDeviceId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw MobilePairingRequestError.missingDeviceId
        }
        let request = MobileDevicePairingRequest(
            schemaVersion: mobileVaultSchemaVersion,
            requestId: "pair-\(localDeviceId)-\(nowMillis)",
            deviceId: localDeviceId,
            label: label,
            platform: "ios",
            publicKey: nil,
            createdAtMillis: nowMillis
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(request)
        guard let text = String(data: data, encoding: .utf8) else {
            throw MobilePairingRequestError.encodingFailed
        }
        return text
    }

    func importPlaintextFixture(from url: URL) {
        do {
            let data = try Data(contentsOf: url)
            try acceptImportedVault(try vaultImporter.importPlaintextVaultData(data))
        } catch {
            importError = error.localizedDescription
        }
    }

    func importEncryptedVault(from url: URL, passphrase: String) {
        do {
            let data = try Data(contentsOf: url)
            try acceptImportedVault(try vaultImporter.importEncryptedVaultData(data, passphrase: passphrase))
            try encryptedVaultStore?.saveEncryptedVault(data)
            hasStoredEncryptedVault = encryptedVaultStore?.hasEncryptedVault ?? false
        } catch {
            importError = error.localizedDescription
        }
    }

    func unlockStoredEncryptedVault(passphrase: String) {
        do {
            guard let data = try encryptedVaultStore?.readEncryptedVault() else {
                hasStoredEncryptedVault = false
                importError = "No stored encrypted vault is available."
                return
            }
            try acceptImportedVault(try vaultImporter.importEncryptedVaultData(data, passphrase: passphrase))
            hasStoredEncryptedVault = encryptedVaultStore?.hasEncryptedVault ?? false
        } catch {
            importError = error.localizedDescription
        }
    }

    func forgetStoredEncryptedVault() {
        do {
            try encryptedVaultStore?.clearEncryptedVault()
            hasStoredEncryptedVault = false
            vault = nil
            selectedHost = nil
            importError = "Stored encrypted vault removed."
        } catch {
            importError = error.localizedDescription
        }
    }

    func saveCredential(_ secret: String, for host: MobileHost) {
        do {
            guard let secretRef = host.auth.secretRef, !secretRef.isEmpty else {
                importError = "This host does not declare a mobile secret reference."
                return
            }
            guard !secret.isEmpty else {
                importError = "Enter the SSH credential before saving."
                return
            }
            try secretStore.saveSecret(secret, account: secretRef)
            importError = "Credential saved for \(host.label)."
        } catch {
            importError = error.localizedDescription
        }
    }

    func deleteCredential(for host: MobileHost) {
        do {
            guard let secretRef = host.auth.secretRef, !secretRef.isEmpty else {
                importError = "This host does not declare a mobile secret reference."
                return
            }
            try secretStore.deleteSecret(account: secretRef)
            importError = "Credential removed for \(host.label)."
        } catch {
            importError = error.localizedDescription
        }
    }

    func sendTerminalInput(_ input: String) {
        guard !privacyCovered else { return }
        Task {
            do {
                try await sshClient.send(Data((input + "\n").utf8))
            } catch {
                terminalBuffer.append(error.localizedDescription)
            }
        }
    }

    func sendTerminalBytes(_ bytes: Data) {
        guard !privacyCovered else { return }
        Task {
            do {
                try await sshClient.send(bytes)
            } catch {
                terminalBuffer.append(error.localizedDescription)
            }
        }
    }

    func disconnect() {
        Task {
            await sshClient.disconnect()
            connectionState = .disconnected
        }
    }

    func resizeTerminal(columns: Int, rows: Int) {
        guard !privacyCovered else { return }
        terminalBuffer.resize(columns: columns, rows: rows)
        Task {
            do {
                try await sshClient.resize(columns: columns, rows: rows)
            } catch {
                terminalBuffer.append(error.localizedDescription)
            }
        }
    }

    func reportStatus(_ message: String) {
        importError = message
    }

    func suspend() {
        privacyCovered = true
        disconnect()
    }

    func resume() {
        privacyCovered = false
    }

    func credentialReference(for host: MobileHost) -> String? {
        host.auth.secretRef
    }

    func knownHost(for host: MobileHost) -> MobileKnownHost? {
        vault?.knownHosts.first { $0.endpoint == host.knownHostEndpoint }
    }

    private func acceptImportedVault(_ imported: MobileVaultExport) throws {
        if imported.isDeviceRevoked(localDeviceId) {
            throw MobileVaultImportError.revokedLocalDevice(localDeviceId)
        }
        if let vault, imported.isOlder(than: vault) {
            throw MobileVaultImportError.staleVault
        }
        vault = imported
        selectedHost = hosts.first
        importError = nil
    }

    func connectSelectedHost() {
        guard !privacyCovered else { return }
        guard let host = selectedHost else {
            return
        }
        let knownHost = knownHost(for: host)
        connectionState = .connecting
        terminalBuffer.clear()
        terminalBuffer.append("Connecting to \(host.username)@\(host.host):\(host.port)")

        Task {
            do {
                try await sshClient.connect(host: host, knownHost: knownHost) { [weak self] data in
                    Task { @MainActor in
                        self?.terminalBuffer.append(data)
                    }
                }
                connectionState = .connected
            } catch {
                connectionState = .failed(error.localizedDescription)
                terminalBuffer.append(error.localizedDescription)
            }
        }
    }
}

enum MobilePairingRequestError: Error, LocalizedError {
    case missingDeviceId
    case encodingFailed

    var errorDescription: String? {
        switch self {
        case .missingDeviceId:
            return "This mobile device does not have a pairing identity yet."
        case .encodingFailed:
            return "Unable to encode the mobile pairing request."
        }
    }
}

private func currentUnixMillis() -> UInt64 {
    UInt64(Date().timeIntervalSince1970 * 1000)
}
