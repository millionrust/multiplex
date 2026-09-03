import Foundation
import XCTest
@testable import TermiRustMobile

final class AppleRelayControllerTransportLiveTests: XCTestCase {
    func testLiveRelayEchoSurvivesFreshTransportReconnect() async throws {
        let package = try livePackage()
        let reference = try ControllerRouteCredentialReference(
            id: "live-relay",
            route: .selfHostedRelay,
            purpose: .relayAdmission
        )
        let configuration = try ControllerRemoteRouteConfiguration.selfHostedRelay(
            endpoint: package.endpoint,
            spkiPin: package.spkiPin,
            credential: reference,
            routeID: package.routeID,
            revocationEpoch: package.revocationEpoch
        )
        let credentials = LiveRelayCredentialStore(
            secret: Data(package.admissionCredential.utf8)
        )
        let factory = try RelayControllerTransport.factory(
            hostID: "live-relay-host",
            configuration: configuration,
            credentials: credentials
        )
        let route = try HostRoute(address: "unused.invalid", port: 1)

        for payload in [Data("first-connection".utf8), Data("second-connection".utf8)] {
            try await assertEcho(payload, factory: factory, route: route)
        }
    }

    private func assertEcho(
        _ payload: Data,
        factory: ControllerTransportFactory,
        route: HostRoute
    ) async throws {
        var lastError: Error?
        for attempt in 0 ..< 8 {
            do {
                let transport = try await factory.open(route)
                defer { transport.cancel() }
                try await transport.send(payload)
                let echoed = try await readExactly(payload.count, from: transport)
                XCTAssertEqual(echoed, payload)
                return
            } catch {
                lastError = error
                try await Task.sleep(for: .milliseconds(100 * (attempt + 1)))
            }
        }
        throw lastError ?? LiveRelayTestError.reconnectExhausted
    }

    private func livePackage() throws -> ControllerRelayRoutePackage {
        guard let encoded = ProcessInfo.processInfo.environment["TERMIRUST_MOBILE_RELAY_PACKAGE"],
              let data = Data(base64Encoded: encoded),
              let text = String(data: data, encoding: .utf8) else {
            throw XCTSkip("Run through scripts/test-mobile-controller-relay-transport.sh")
        }
        return try ControllerRelayRoutePackage.decode(text)
    }

    private func readExactly(
        _ count: Int,
        from transport: any ControllerDuplexConnection
    ) async throws -> Data {
        var result = Data()
        while result.count < count {
            result.append(try await transport.receive(maximumLength: count - result.count))
        }
        return result
    }
}

private enum LiveRelayTestError: Error {
    case reconnectExhausted
}

private final class LiveRelayCredentialStore:
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
