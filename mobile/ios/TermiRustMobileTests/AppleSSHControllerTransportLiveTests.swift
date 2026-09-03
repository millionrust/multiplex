import Foundation
import XCTest
@testable import TermiRustMobile

final class AppleSSHControllerTransportLiveTests: XCTestCase {
    func testPinnedPrivateKeySSHTransportExecutesFixedBridgeAndRoundTripsBytes() async throws {
        let environment = try liveEnvironment()
        try await assertRoundTrip(
            environment: environment,
            authentication: .privateKey,
            secret: environment.privateKey
        )
    }

    func testPinnedPasswordSSHTransportExecutesFixedBridgeAndRoundTripsBytes() async throws {
        let environment = try liveEnvironment()
        try await assertRoundTrip(
            environment: environment,
            authentication: .password,
            secret: environment.password
        )
    }

    func testWrongSSHHostKeyIsRejected() async throws {
        let environment = try liveEnvironment()
        do {
            _ = try await makeFactory(
                environment: environment,
                authentication: .privateKey,
                secret: environment.privateKey,
                hostKey: "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            ).open(try HostRoute(address: "ignored", port: 1))
            XCTFail("A mismatched SSH host key must never be accepted")
        } catch {
            return
        }
    }

    private func assertRoundTrip(
        environment: LiveSSHEnvironment,
        authentication: ControllerSSHAuthenticationKind,
        secret: String
    ) async throws {
        let factory = try makeFactory(
            environment: environment,
            authentication: authentication,
            secret: secret,
            hostKey: environment.hostKey
        )
        let connection = try await factory.open(
            try HostRoute(address: "ignored-by-explicit-ssh-route", port: 1)
        )
        defer { connection.cancel() }
        let payload = Data("termirust-mobile-controller-ssh\n".utf8)
        try await connection.send(payload)
        let response = try await connection.receive(maximumLength: 4_096)
        XCTAssertEqual(response, payload)
    }

    private func makeFactory(
        environment: LiveSSHEnvironment,
        authentication: ControllerSSHAuthenticationKind,
        secret: String,
        hostKey: String
    ) throws -> ControllerTransportFactory {
        let reference = try ControllerRouteCredentialReference(
            id: "live-test",
            route: .ssh,
            purpose: .sshAuthentication
        )
        let configuration = try ControllerRemoteRouteConfiguration.ssh(
            endpoint: "127.0.0.1",
            port: environment.port,
            username: "termirust",
            hostKeyPin: hostKey,
            credential: reference,
            authentication: authentication
        )
        let credentials = LiveControllerCredentialStore(
            secret: Data(secret.utf8)
        )
        return try SSHControllerTransport.factory(
            hostID: "live-host",
            configuration: configuration,
            credentials: credentials
        )
    }

    private func liveEnvironment() throws -> LiveSSHEnvironment {
        let values = ProcessInfo.processInfo.environment
        guard let portText = values["TERMIRUST_MOBILE_CONTROLLER_SSH_PORT"],
              let port = UInt16(portText),
              let hostKey = values["TERMIRUST_MOBILE_CONTROLLER_SSH_HOST_KEY"],
              let privateKey = values["TERMIRUST_MOBILE_CONTROLLER_SSH_PRIVATE_KEY"],
              let password = values["TERMIRUST_MOBILE_CONTROLLER_SSH_PASSWORD"] else {
            throw XCTSkip("Run through scripts/test-mobile-controller-ssh-transports.sh")
        }
        return LiveSSHEnvironment(
            port: port,
            hostKey: hostKey,
            privateKey: privateKey,
            password: password
        )
    }
}

private struct LiveSSHEnvironment {
    let port: UInt16
    let hostKey: String
    let privateKey: String
    let password: String
}

private final class LiveControllerCredentialStore:
    ControllerRouteCredentialStoring, @unchecked Sendable {
    private let secret: Data

    init(secret: Data) {
        self.secret = secret
    }

    func store(
        _ secret: Data,
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws {}

    func load(
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws -> Data? {
        secret
    }

    func delete(
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws {}
}
