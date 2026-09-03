import XCTest
@preconcurrency import NIOSSH
@testable import TermiRustMobile

final class MobileVaultImporterTests: XCTestCase {
    private let fixtureEd25519PrivateKey = """
    -----BEGIN OPENSSH PRIVATE KEY-----
    b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
    QyNTUxOQAAACDXdpIxO9R+jxlETproGCGcxOE4SjX6r+jU8UIg2K5ULgAAAJgxZY5/MWWO
    fwAAAAtzc2gtZWQyNTUxOQAAACDXdpIxO9R+jxlETproGCGcxOE4SjX6r+jU8UIg2K5ULg
    AAAECEhzF5gu5ZhIg/P8PEq/33+dXN8wUMwtU7zIlgJLfC2Nd2kjE71H6PGUROmugYIZzE
    4ThKNfqv6NTxQiDYrlQuAAAAD2NsYXdAak1hYy5sb2NhbAECAwQFBg==
    -----END OPENSSH PRIVATE KEY-----
    """

    private let encryptedEnvelopeData = Data("""
    {
      "version": 1,
      "schema_version": 1,
      "cipher": "AES-256-GCM-SIV",
      "kdf": "Argon2id(m=19456,t=3,p=1)",
      "salt": "salt",
      "nonce": "nonce",
      "ciphertext": "ciphertext"
    }
    """.utf8)

    private let plaintextVaultData = Data("""
    {
      "schema_version": 1,
      "export_id": "export-1",
      "created_at_millis": 1,
      "updated_at_millis": 1,
      "source_device_id": "desktop-1",
      "vaults": [],
      "hosts": [{
        "id": "profile-1",
        "label": "Prod",
        "vault_id": null,
        "group": "Ops",
        "tags": ["prod"],
        "host": "prod.example.com",
        "port": 22,
        "username": "ubuntu",
        "auth": {"kind": "private_key", "identity_id": "identity-1", "secret_ref": "termirust-mobile://identity/identity-1/private-key"},
        "jump_host_id": null,
        "startup_directory": "/srv/app",
        "startup_command": "uptime",
        "start_in_files": false,
        "persistent_session": {"enabled": true, "session_name": "tr-prod", "detach_others": false},
        "terminal_scrollback_rows": 20000,
        "color_tag": null,
        "environment": [],
        "known_host_endpoint": "prod.example.com:22"
      }],
      "groups": [],
      "tags": [],
      "identities": [],
      "known_hosts": [{"endpoint": "prod.example.com:22", "public_key": "ssh-ed25519 AAAA", "algorithm": null, "fingerprint": null}],
      "sync": {"revision": null, "last_synced_at_millis": null},
      "devices": [{
        "device_id": "desktop-1",
        "label": "TermiRust Desktop",
        "platform": "desktop",
        "public_key": null,
        "paired_at_millis": 1,
        "last_seen_at_millis": 1,
        "revoked_at_millis": null
      }],
      "device_keys": [{
        "key_id": "vault-key-ios-1",
        "device_id": "ios-1",
        "wrapping_algorithm": "x25519-xsalsa20poly1305",
        "encrypted_vault_key": "base64-wrapped-key",
        "created_at_millis": 1,
        "revoked_at_millis": null
      }]
    }
    """.utf8)

    func testPlaintextVaultDecodesPersistentTmuxHost() throws {
        let vault = try MobileVaultImporter().importPlaintextVaultData(plaintextVaultData)

        XCTAssertEqual(vault.schemaVersion, 1)
        XCTAssertEqual(vault.hosts.first?.persistentSession.sessionName, "tr-prod")
        XCTAssertEqual(
            vault.hosts.first?.auth.secretRef,
            "termirust-mobile://identity/identity-1/private-key"
        )
        XCTAssertEqual(vault.knownHosts.first?.endpoint, "prod.example.com:22")
        XCTAssertEqual(vault.devices.first?.deviceId, "desktop-1")
        XCTAssertEqual(vault.devices.first?.platform, "desktop")
        XCTAssertEqual(vault.devices.first?.pairedAtMillis, 1)
        XCTAssertEqual(vault.deviceKeys.first?.deviceId, "ios-1")
        XCTAssertEqual(vault.activeDeviceKey(for: "ios-1")?.keyId, "vault-key-ios-1")
    }

    func testPlaintextVaultDefaultsMissingDeviceKeys() throws {
        var object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: plaintextVaultData) as? [String: Any]
        )
        object.removeValue(forKey: "device_keys")
        let legacyVaultData = try JSONSerialization.data(withJSONObject: object)

        let vault = try MobileVaultImporter().importPlaintextVaultData(legacyVaultData)

        XCTAssertTrue(vault.deviceKeys.isEmpty)
    }

    @MainActor
    func testViewModelGeneratesPairingRequestJson() throws {
        let viewModel = HostListViewModel(
            vaultImporter: MobileVaultImporter(),
            secretStore: FixtureSecretStore(),
            sshClient: FixtureSSHClient(),
            localDeviceId: "ios-1"
        )

        let request = try viewModel.pairingRequestText(label: "Jacob iPhone", nowMillis: 42)
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(request.utf8)) as? [String: Any]
        )

        XCTAssertEqual(object["schema_version"] as? Int, 1)
        XCTAssertEqual(object["request_id"] as? String, "pair-ios-1-42")
        XCTAssertEqual(object["device_id"] as? String, "ios-1")
        XCTAssertEqual(object["label"] as? String, "Jacob iPhone")
        XCTAssertEqual(object["platform"] as? String, "ios")
        XCTAssertEqual(object["created_at_millis"] as? Int, 42)
    }

    @MainActor
    func testViewModelRejectsVaultWhenLocalDeviceIsRevoked() throws {
        var object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: plaintextVaultData) as? [String: Any]
        )
        var devices = try XCTUnwrap(object["devices"] as? [[String: Any]])
        devices.append([
            "device_id": "ios-1",
            "label": "Jacob iPhone",
            "platform": "ios",
            "public_key": NSNull(),
            "revoked_at_millis": 1719356789123
        ])
        object["devices"] = devices
        let revokedVaultData = try JSONSerialization.data(withJSONObject: object)
        let tempURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("json")
        try revokedVaultData.write(to: tempURL)
        defer { try? FileManager.default.removeItem(at: tempURL) }
        let viewModel = HostListViewModel(
            vaultImporter: MobileVaultImporter(),
            secretStore: FixtureSecretStore(),
            sshClient: FixtureSSHClient(),
            localDeviceId: "ios-1"
        )

        viewModel.importPlaintextFixture(from: tempURL)

        XCTAssertNil(viewModel.selectedHost)
        XCTAssertEqual(
            viewModel.importError,
            "This device has been revoked for the imported mobile vault (ios-1). Import blocked."
        )
    }

    @MainActor
    func testViewModelRejectsOlderVaultOverLoadedVault() throws {
        let viewModel = HostListViewModel(
            vaultImporter: MobileVaultImporter(),
            secretStore: FixtureSecretStore(),
            sshClient: FixtureSSHClient()
        )
        let currentURL = try writeVaultFixture(updatedAtMillis: 10, revision: 2)
        let staleURL = try writeVaultFixture(updatedAtMillis: 5, revision: 1)
        defer {
            try? FileManager.default.removeItem(at: currentURL)
            try? FileManager.default.removeItem(at: staleURL)
        }

        viewModel.importPlaintextFixture(from: currentURL)
        XCTAssertEqual(viewModel.vault?.updatedAtMillis, 10)

        viewModel.importPlaintextFixture(from: staleURL)

        XCTAssertEqual(viewModel.vault?.updatedAtMillis, 10)
        XCTAssertEqual(
            viewModel.importError,
            "Imported vault is older than the currently loaded vault. Import blocked to avoid overwriting newer mobile state."
        )
    }

    @MainActor
    func testViewModelResolvesSelectedHostKnownHostPin() throws {
        let viewModel = HostListViewModel(
            vaultImporter: MobileVaultImporter(),
            secretStore: FixtureSecretStore(),
            sshClient: FixtureSSHClient()
        )
        let tempURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("json")
        try plaintextVaultData.write(to: tempURL)
        defer { try? FileManager.default.removeItem(at: tempURL) }

        viewModel.importPlaintextFixture(from: tempURL)

        let host = try XCTUnwrap(viewModel.selectedHost)
        XCTAssertEqual(viewModel.knownHost(for: host)?.endpoint, "prod.example.com:22")
    }

    private func writeVaultFixture(updatedAtMillis: UInt64, revision: UInt64) throws -> URL {
        var object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: plaintextVaultData) as? [String: Any]
        )
        object["updated_at_millis"] = updatedAtMillis
        object["sync"] = ["revision": revision, "last_synced_at_millis": NSNull()]
        let data = try JSONSerialization.data(withJSONObject: object)
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("json")
        try data.write(to: url)
        return url
    }

    func testPlaintextVaultRejectsRevokedSourceDevice() {
        let revokedVaultData = Data(
            String(decoding: plaintextVaultData, as: UTF8.self)
                .replacingOccurrences(of: "\"revoked_at_millis\": null", with: "\"revoked_at_millis\": 1719356789123")
                .utf8
        )

        XCTAssertThrowsError(try MobileVaultImporter().importPlaintextVaultData(revokedVaultData)) { error in
            XCTAssertEqual(error as? MobileVaultImportError, .revokedSourceDevice("desktop-1"))
        }
    }

    func testEncryptedVaultRequiresSharedCryptoDecryptor() throws {
        XCTAssertThrowsError(
            try MobileVaultImporter().importEncryptedVaultData(encryptedEnvelopeData, passphrase: "hunter2")
        ) { error in
            XCTAssertEqual(error as? MobileVaultImportError, .encryptedVaultRequiresSharedCrypto)
        }
    }

    func testEncryptedVaultUsesInjectedDecryptor() throws {
        let importer = MobileVaultImporter(decryptor: FixtureDecryptor(plaintext: plaintextVaultData))

        let vault = try importer.importEncryptedVaultData(encryptedEnvelopeData, passphrase: "hunter2")

        XCTAssertEqual(vault.hosts.first?.host, "prod.example.com")
        XCTAssertEqual(vault.hosts.first?.persistentSession.sessionName, "tr-prod")
    }

    @MainActor
    func testEncryptedVaultImportCachesEncryptedBytesForLaterUnlock() throws {
        let store = FixtureEncryptedVaultStore()
        let viewModel = HostListViewModel(
            vaultImporter: MobileVaultImporter(decryptor: FixtureDecryptor(plaintext: plaintextVaultData)),
            secretStore: FixtureSecretStore(),
            encryptedVaultStore: store,
            sshClient: FixtureSSHClient()
        )
        let tempURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
            .appendingPathExtension("json")
        try encryptedEnvelopeData.write(to: tempURL)
        defer { try? FileManager.default.removeItem(at: tempURL) }

        viewModel.importEncryptedVault(from: tempURL, passphrase: "hunter2")

        XCTAssertTrue(viewModel.hasStoredEncryptedVault)
        XCTAssertEqual(store.saved, encryptedEnvelopeData)

        viewModel.unlockStoredEncryptedVault(passphrase: "hunter2")

        XCTAssertEqual(viewModel.selectedHost?.label, "Prod")
    }

    @MainActor
    func testCredentialSaveUsesExportedSecretReference() throws {
        let host = MobileHost(
            id: "profile-1",
            label: "Prod",
            vaultId: nil,
            group: "Ops",
            tags: [],
            host: "prod.example.com",
            port: 22,
            username: "ubuntu",
            auth: MobileAuthMetadata(kind: .password, identityId: nil, secretRef: "secret-prod-password"),
            jumpHostId: nil,
            startupDirectory: nil,
            startupCommand: nil,
            startInFiles: false,
            persistentSession: MobilePersistentSession(enabled: false, sessionName: nil, detachOthers: false),
            terminalScrollbackRows: nil,
            colorTag: nil,
            environment: [],
            knownHostEndpoint: "prod.example.com:22"
        )
        let store = FixtureSecretStore()
        let viewModel = HostListViewModel(
            vaultImporter: MobileVaultImporter(),
            secretStore: store,
            sshClient: FixtureSSHClient()
        )

        viewModel.saveCredential("super-secret", for: host)

        XCTAssertEqual(store.saved["secret-prod-password"], "super-secret")
        XCTAssertEqual(viewModel.importError, "Credential saved for Prod.")
    }

    func testTmuxBootstrapDoesNotRunStartupCommandOnAttachPath() throws {
        let host = MobileHost(
            id: "profile-1",
            label: "Prod",
            vaultId: nil,
            group: "Ops",
            tags: [],
            host: "prod.example.com",
            port: 22,
            username: "ubuntu",
            auth: MobileAuthMetadata(kind: .privateKey, identityId: nil, secretRef: nil),
            jumpHostId: nil,
            startupDirectory: "/srv/app",
            startupCommand: "uptime",
            startInFiles: false,
            persistentSession: MobilePersistentSession(enabled: true, sessionName: "tr-prod", detachOthers: true),
            terminalScrollbackRows: nil,
            colorTag: nil,
            environment: [],
            knownHostEndpoint: "prod.example.com:22"
        )

        let script = try XCTUnwrap(TmuxBootstrap(host: host).startupCommand())

        XCTAssertTrue(script.contains("tmux has-session -t 'tr-prod'"))
        XCTAssertTrue(script.contains("exec tmux attach-session -d -t 'tr-prod'"))
        XCTAssertTrue(script.contains("exec tmux new-session -s 'tr-prod' -c '/srv/app' -- \"${SHELL:-/bin/sh}\" -lc 'uptime; exec \"${SHELL:-/bin/sh}\" -l'"))
        let attachIndex = try XCTUnwrap(script.range(of: "exec tmux attach-session")?.lowerBound)
        let startupIndex = try XCTUnwrap(script.range(of: "uptime; exec")?.lowerBound)
        XCTAssertLessThan(script.distance(from: script.startIndex, to: attachIndex), script.distance(from: script.startIndex, to: startupIndex))
    }

    func testOpenSSHEd25519PrivateKeyParserExportsExpectedPublicKey() throws {
        let privateKey = try OpenSSHPrivateKeyParser.parse(fixtureEd25519PrivateKey)

        XCTAssertEqual(
            String(openSSHPublicKey: privateKey.publicKey),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINd2kjE71H6PGUROmugYIZzE4ThKNfqv6NTxQiDYrlQu"
        )
    }

    func testPinnedHostKeyDelegateRejectsMismatchedHostKey() throws {
        let privateKey = try OpenSSHPrivateKeyParser.parse(fixtureEd25519PrivateKey)
        let knownHost = MobileKnownHost(
            endpoint: "prod.example.com:22",
            publicKey: String(openSSHPublicKey: privateKey.publicKey),
            algorithm: "ssh-ed25519",
            fingerprint: nil
        )
        let verifier = try PinnedHostKeyDelegate(knownHost: knownHost)
        let mismatchedKey = try NIOSSHPublicKey(
            openSSHPublicKey: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINd2kjE71H6PGUROmugYIZzE4ThKNfqv6NTxQiDYrlQv"
        )

        XCTAssertTrue(verifier.accepts(hostKey: privateKey.publicKey))
        XCTAssertFalse(verifier.accepts(hostKey: mismatchedKey))
    }

    func testOpenSSHPrivateKeyParserRejectsInvalidPEM() {
        XCTAssertThrowsError(try OpenSSHPrivateKeyParser.parse("not a key")) { error in
            XCTAssertEqual(error as? OpenSSHPrivateKeyParserError, .invalidPEM)
        }
    }

    func testTerminalGridEstimationUsesTerminalSizeAndFont() {
        XCTAssertEqual(
            estimateTerminalGrid(size: CGSize(width: 800, height: 600), fontSize: 14),
            TerminalGrid(columns: 92, rows: 31)
        )
        XCTAssertEqual(
            estimateTerminalGrid(size: CGSize(width: 800, height: 600), fontSize: 28),
            TerminalGrid(columns: 46, rows: 15)
        )
        XCTAssertEqual(
            estimateTerminalGrid(size: CGSize(width: 1, height: 1), fontSize: 14),
            TerminalGrid(columns: 20, rows: 6)
        )
    }

    func testTerminalInputEncodingHandlesControlAndOptionModifiers() {
        XCTAssertEqual(Array(encodeTerminalInput("uptime", control: false, option: false)), Array("uptime\n".utf8))
        XCTAssertEqual(Array(encodeTerminalInput("c", control: true, option: false)), [0x03])
        XCTAssertEqual(Array(encodeTerminalInput("D", control: true, option: false)), [0x04])
        XCTAssertEqual(Array(encodeTerminalInput("[", control: true, option: false)), [0x1B])
        XCTAssertEqual(Array(encodeTerminalInput("x", control: false, option: true)), [0x1B, 0x78])
        XCTAssertEqual(Array(encodeTerminalInput("c", control: true, option: true)), [0x1B, 0x03])
    }

    @MainActor
    func testTerminalBufferHandlesCommonTerminalRedrawSequences() {
        let buffer = TerminalBuffer()

        buffer.append("progress 1\rprogress 2\r\n\u{1B}[31mred\u{1B}[0m\r\nabc\u{8}Z")

        XCTAssertEqual(buffer.lines, ["progress 2", "red", "abZ"])
    }

    @MainActor
    func testTerminalBufferClearsScreenOnAnsiClear() {
        let buffer = TerminalBuffer()

        buffer.append("before\n\u{1B}[2J\u{1B}[Hafter")

        XCTAssertEqual(buffer.lines, ["after"])
    }
}

private struct FixtureDecryptor: MobileVaultDecrypting {
    let plaintext: Data

    func decrypt(encryptedVaultData: Data, passphrase: String) throws -> Data {
        XCTAssertFalse(encryptedVaultData.isEmpty)
        XCTAssertEqual(passphrase, "hunter2")
        return plaintext
    }
}

private final class FixtureSecretStore: SecretStoring {
    var saved: [String: String] = [:]

    func saveSecret(_ secret: String, account: String) throws {
        saved[account] = secret
    }

    func readSecret(account: String) throws -> String? {
        saved[account]
    }

    func deleteSecret(account: String) throws {
        saved.removeValue(forKey: account)
    }
}

private final class FixtureEncryptedVaultStore: EncryptedVaultStoring {
    var saved: Data?

    var hasEncryptedVault: Bool {
        saved != nil
    }

    func saveEncryptedVault(_ data: Data) throws {
        saved = data
    }

    func readEncryptedVault() throws -> Data? {
        saved
    }

    func clearEncryptedVault() throws {
        saved = nil
    }
}

private final class FixtureSSHClient: MobileSSHConnecting, @unchecked Sendable {
    func connect(
        host: MobileHost,
        knownHost: MobileKnownHost?,
        onOutput: @escaping @Sendable (Data) -> Void
    ) async throws {}

    func send(_ bytes: Data) async throws {}

    func resize(columns: Int, rows: Int) async throws {}

    func disconnect() async {}
}
