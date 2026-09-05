import XCTest
@testable import TermiRustMobile

final class UnifiedRouteLifecycleTests: XCTestCase {
    @MainActor
    func testDirectSSHBackgroundDropsInputAndDisconnectsWithoutReplay() async throws {
        let client = LifecycleSSHClient()
        let viewModel = HostListViewModel(
            vaultImporter: MobileVaultImporter(),
            secretStore: LifecycleSecretStore(),
            sshClient: client
        )

        viewModel.suspend()
        XCTAssertTrue(viewModel.privacyCovered)
        viewModel.sendTerminalBytes(Data("must-not-send".utf8))
        try await waitUntil { await client.disconnects() == 1 }
        let backgroundBytes = await client.bytes()
        XCTAssertEqual(backgroundBytes, [])

        viewModel.resume()
        XCTAssertFalse(viewModel.privacyCovered)
        let resumedBytes = await client.bytes()
        XCTAssertEqual(resumedBytes, [])
    }
}

@MainActor
private func waitUntil(
    timeout: Duration = .seconds(2),
    condition: @escaping @MainActor () async -> Bool
) async throws {
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: timeout)
    while clock.now < deadline {
        if await condition() { return }
        try await Task.sleep(for: .milliseconds(10))
    }
    XCTFail("condition did not become true before timeout")
}

private final class LifecycleSecretStore: SecretStoring {
    func saveSecret(_ secret: String, account: String) throws {}
    func readSecret(account: String) throws -> String? { nil }
    func deleteSecret(account: String) throws {}
}

private actor LifecycleSSHClient: MobileSSHConnecting {
    private(set) var sentBytes: [Data] = []
    private(set) var disconnectCount = 0

    func connect(
        host: MobileHost,
        knownHost: MobileKnownHost?,
        onOutput: @escaping @Sendable (Data) -> Void
    ) async throws {}

    func send(_ bytes: Data) async throws { sentBytes.append(bytes) }
    func resize(columns: Int, rows: Int) async throws {}
    func disconnect() async { disconnectCount += 1 }
    func bytes() -> [Data] { sentBytes }
    func disconnects() -> Int { disconnectCount }
}
