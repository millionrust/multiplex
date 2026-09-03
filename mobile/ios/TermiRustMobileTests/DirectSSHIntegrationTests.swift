import Foundation
import XCTest
@testable import TermiRustMobile

final class DirectSSHIntegrationTests: XCTestCase {
    func testDirectSSHAttachesToPersistentTmuxSessionAndSurvivesReconnect() async throws {
        guard let config = LiveSSHSmokeConfig.load() else {
            throw XCTSkip(
                "Set TERMIRUST_MOBILE_TEST_SSH_HOST, TERMIRUST_MOBILE_TEST_SSH_PORT, TERMIRUST_MOBILE_TEST_SSH_USER, TERMIRUST_MOBILE_TEST_SSH_KEY, and TERMIRUST_MOBILE_TEST_KNOWN_HOST_KEY to run this live SSH smoke."
            )
        }

        let sessionName = "mobile-ios-smoke"
        let secretRef = "termirust-mobile-test-private-key"
        let host = MobileHost(
            id: "ios-live-ssh",
            label: "iOS Live SSH",
            vaultId: nil,
            group: "",
            tags: [],
            host: config.host,
            port: config.port,
            username: config.username,
            auth: MobileAuthMetadata(
                kind: .privateKey,
                identityId: nil,
                secretRef: secretRef
            ),
            jumpHostId: nil,
            startupDirectory: nil,
            startupCommand: nil,
            startInFiles: false,
            persistentSession: MobilePersistentSession(
                enabled: true,
                sessionName: sessionName,
                detachOthers: false
            ),
            terminalScrollbackRows: nil,
            colorTag: nil,
            environment: [],
            knownHostEndpoint: "\(config.host):\(config.port)"
        )
        let knownHost = MobileKnownHost(
            endpoint: "\(config.host):\(config.port)",
            publicKey: config.knownHostKey,
            algorithm: nil,
            fingerprint: nil
        )

        let firstOutput = OutputCapture()
        let firstClient = DirectSSHSessionClient(
            secretStore: StaticSecretStore(account: secretRef, secret: config.privateKey)
        )
        do {
            try await firstClient.connect(host: host, knownHost: knownHost) { data in
                firstOutput.append(data)
            }
            try await firstClient.send(Data("tmux display-message -p '#S'\n".utf8))
            try await firstClient.send(Data("echo ios-smoke-first > ~/termirust-ios-smoke\n".utf8))
            let attachedToSession = await firstOutput.waitFor(sessionName)
            XCTAssertTrue(
                attachedToSession,
                "first connection did not attach to the expected tmux session"
            )
        } catch {
            await firstClient.disconnect()
            throw error
        }
        await firstClient.disconnect()

        let secondOutput = OutputCapture()
        let secondClient = DirectSSHSessionClient(
            secretStore: StaticSecretStore(account: secretRef, secret: config.privateKey)
        )
        do {
            try await secondClient.connect(host: host, knownHost: knownHost) { data in
                secondOutput.append(data)
            }
            try await secondClient.send(Data("cat ~/termirust-ios-smoke\n".utf8))
            let sawPersistentMarker = await secondOutput.waitFor("ios-smoke-first")
            XCTAssertTrue(
                sawPersistentMarker,
                "reconnect did not see marker created inside the persistent tmux session"
            )
        } catch {
            await secondClient.disconnect()
            throw error
        }
        await secondClient.disconnect()
    }

}

private struct LiveSSHSmokeConfig {
    let host: String
    let port: UInt16
    let username: String
    let privateKey: String
    let knownHostKey: String

    static func load() -> Self? {
        let values = envValues().merging(fileValues()) { envValue, _ in envValue }
        guard let host = values["TERMIRUST_MOBILE_TEST_SSH_HOST"], !host.isEmpty,
              let portText = values["TERMIRUST_MOBILE_TEST_SSH_PORT"],
              let port = UInt16(portText),
              let username = values["TERMIRUST_MOBILE_TEST_SSH_USER"], !username.isEmpty,
              let privateKey = decodedValue(values, "TERMIRUST_MOBILE_TEST_SSH_KEY"),
              !privateKey.isEmpty,
              let knownHostKey = decodedValue(values, "TERMIRUST_MOBILE_TEST_KNOWN_HOST_KEY"),
              !knownHostKey.isEmpty else {
            return nil
        }
        return Self(
            host: host,
            port: port,
            username: username,
            privateKey: privateKey,
            knownHostKey: knownHostKey
        )
    }

    private static func envValues() -> [String: String] {
        ProcessInfo.processInfo.environment.mapValues {
            $0.trimmingCharacters(in: .whitespacesAndNewlines)
        }
    }

    private static func fileValues() -> [String: String] {
        let fileURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent(".termirust-mobile-live-ssh.properties")
        guard let contents = try? String(contentsOf: fileURL, encoding: .utf8) else {
            return [:]
        }
        return contents
            .split(whereSeparator: \.isNewline)
            .reduce(into: [:]) { values, line in
                let text = String(line)
                guard !text.trimmingCharacters(in: .whitespaces).hasPrefix("#"),
                      let separator = line.firstIndex(of: "=") else {
                    return
                }
                let key = line[..<separator].trimmingCharacters(in: .whitespacesAndNewlines)
                let value = line[line.index(after: separator)...].trimmingCharacters(in: .whitespacesAndNewlines)
                values[key] = value
            }
    }

    private static func decodedValue(_ values: [String: String], _ key: String) -> String? {
        if let encoded = values["\(key)_BASE64"],
           let data = Data(base64Encoded: encoded),
           let decoded = String(data: data, encoding: .utf8) {
            return decoded
        }
        return values[key]
    }
}

private final class OutputCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var chunks: [Data] = []

    func append(_ data: Data) {
        lock.lock()
        chunks.append(data)
        lock.unlock()
    }

    func text() -> String {
        lock.lock()
        defer { lock.unlock() }
        return chunks
            .compactMap { String(data: $0, encoding: .utf8) }
            .joined()
    }

    func waitFor(_ needle: String, attempts: Int = 80) async -> Bool {
        for _ in 0..<attempts {
            if text().contains(needle) {
                return true
            }
            try? await Task.sleep(nanoseconds: 250_000_000)
        }
        return false
    }
}

private final class StaticSecretStore: SecretStoring {
    let account: String
    let secret: String

    init(account: String, secret: String) {
        self.account = account
        self.secret = secret
    }

    func saveSecret(_ secret: String, account: String) throws {}

    func readSecret(account: String) throws -> String? {
        account == self.account ? secret : nil
    }

    func deleteSecret(account: String) throws {}
}
