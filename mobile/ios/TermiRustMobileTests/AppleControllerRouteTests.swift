import Foundation
import XCTest
@testable import TermiRustMobile

final class AppleControllerRouteTests: XCTestCase {
    func testCanonicalApplePoliciesRequireInnerAuthenticationAndNeverFallback() {
        for route in ControllerRemoteRouteKind.appleCases {
            let policy = ControllerRemoteRoutePolicy.canonical(route)
            XCTAssertTrue(policy.supports(.appleMobile))
            XCTAssertTrue(policy.trustLayers.contains(.controllerAuthentication))
            XCTAssertEqual(policy.capabilities, ControllerRemoteCapability.allCases)
            XCTAssertFalse(policy.allowsAutomaticSwitch)
            XCTAssertFalse(policy.allowsOfflineMutations)
        }
        XCTAssertFalse(
            ControllerRemoteRoutePolicy.canonical(.localIPC).supports(.appleMobile)
        )
    }

    func testEveryAppleRoutePassesNormalDegradedReconnectCancelAndRevoke() throws {
        for route in ControllerRemoteRouteKind.appleCases {
            var coordinator = AppleControllerRouteCoordinator(availability: .all)
            try connectOnline(route, coordinator: &coordinator)
            try coordinator.setWriterHeld(true)

            let reconnect = try coordinator.failed(
                route,
                retryable: true,
                mutationInFlight: true
            )
            XCTAssertEqual(coordinator.selected, route)
            XCTAssertEqual(reconnect.disconnectTransport, route)
            XCTAssertTrue(reconnect.releaseWriter)
            XCTAssertTrue(reconnect.retryIdempotentReads)
            XCTAssertEqual(reconnect.mutationDisposition, .queryByCommandID)
            _ = try coordinator.transportReady(route)
            _ = try coordinator.authenticated(route)

            let degraded = try coordinator.failed(
                route,
                retryable: false,
                mutationInFlight: false
            )
            XCTAssertEqual(degraded.disconnectTransport, route)
            XCTAssertFalse(degraded.retryIdempotentReads)
            XCTAssertEqual(projection(route, in: coordinator).recovery, .retry)
            XCTAssertEqual(try coordinator.retrySelected().startTransport, route)

            let cancel = try coordinator.cancelSelected()
            XCTAssertEqual(cancel.disconnectTransport, route)
            XCTAssertTrue(cancel.clearPendingInput)
            _ = try coordinator.connectSelected()
            _ = try coordinator.transportReady(route)
            _ = try coordinator.authenticated(route)
            _ = try coordinator.revokeSelected()
            XCTAssertEqual(projection(route, in: coordinator).phase, .revoked)
            XCTAssertEqual(projection(route, in: coordinator).recovery, .reauthorize)
        }
    }

    func testEveryAppleRouteSwitchRequiresConfirmationAndDoesNotStartTarget() throws {
        for source in ControllerRemoteRouteKind.appleCases {
            for target in ControllerRemoteRouteKind.appleCases where source != target {
                var coordinator = AppleControllerRouteCoordinator(availability: .all)
                try connectOnline(source, coordinator: &coordinator)
                XCTAssertThrowsError(try coordinator.select(target, explicitlyConfirmed: false)) {
                    XCTAssertEqual(
                        $0 as? AppleControllerRouteCoordinatorError,
                        .transition(.explicitConfirmationRequired)
                    )
                }
                XCTAssertEqual(coordinator.selected, source)

                let plan = try coordinator.select(target, explicitlyConfirmed: true)
                XCTAssertEqual(plan.disconnectTransport, source)
                XCTAssertNil(plan.startTransport)
                XCTAssertTrue(plan.clearPendingInput)
                XCTAssertEqual(coordinator.selected, target)
            }
        }
    }

    func testUnavailableTargetNeverFallsBackAndRepeatedLossIsIdempotent() throws {
        var coordinator = AppleControllerRouteCoordinator(
            availability: AppleControllerRouteAvailability(
                privateNetwork: true,
                ssh: false,
                selfHostedRelay: false
            )
        )
        _ = try coordinator.select(.privateNetwork, explicitlyConfirmed: true)
        XCTAssertThrowsError(try coordinator.select(.ssh, explicitlyConfirmed: true)) {
            XCTAssertEqual(
                $0 as? AppleControllerRouteCoordinatorError,
                .transition(.targetUnavailable)
            )
        }
        XCTAssertEqual(coordinator.selected, .privateNetwork)
        _ = try coordinator.connectSelected()
        XCTAssertNotNil(try coordinator.setAvailable(.privateNetwork, available: false))
        XCTAssertNil(try coordinator.setAvailable(.privateNetwork, available: false))
        XCTAssertEqual(coordinator.selected, .privateNetwork)
        XCTAssertEqual(projection(.privateNetwork, in: coordinator).phase, .unavailable)
    }

    func testPersistedUnavailableSelectionRemainsSelectedWithoutStartingFallback() throws {
        var coordinator = AppleControllerRouteCoordinator(
            availability: AppleControllerRouteAvailability(
                privateNetwork: true,
                ssh: false,
                selfHostedRelay: false
            )
        )
        try coordinator.restorePersistedSelection(.ssh)
        XCTAssertEqual(coordinator.selected, .ssh)
        XCTAssertEqual(projection(.ssh, in: coordinator).phase, .unavailable)
        XCTAssertThrowsError(try coordinator.connectSelected())
        XCTAssertEqual(
            projection(.privateNetwork, in: coordinator).phase,
            .idle,
            "LAN must remain idle rather than becoming an implicit fallback"
        )
    }

    func testConfigurationChangesCannotEraseRevocationOrDisable() throws {
        var coordinator = AppleControllerRouteCoordinator(availability: .all)
        try connectOnline(.ssh, coordinator: &coordinator)
        _ = try coordinator.revokeSelected()
        XCTAssertNil(try coordinator.setAvailable(.ssh, available: false))
        XCTAssertNil(try coordinator.setAvailable(.ssh, available: true))
        XCTAssertEqual(projection(.ssh, in: coordinator).phase, .revoked)
        XCTAssertThrowsError(try coordinator.connectSelected())

        _ = try coordinator.authorizationRestored(.ssh)
        _ = try coordinator.disableSelected()
        _ = try coordinator.setAvailable(.ssh, available: false)
        _ = try coordinator.setAvailable(.ssh, available: true)
        XCTAssertEqual(projection(.ssh, in: coordinator).phase, .disabled)
        _ = try coordinator.enableSelected()
        XCTAssertEqual(try coordinator.connectSelected().startTransport, .ssh)
    }

    func testRouteConfigurationsContainReferencesButNotCredentialMaterial() throws {
        let sshReference = try ControllerRouteCredentialReference(
            id: "ssh-key-1",
            route: .ssh,
            purpose: .sshAuthentication
        )
        let ssh = try ControllerRemoteRouteConfiguration.ssh(
            endpoint: "host.example",
            port: 22,
            username: "deploy",
            hostKeyPin: "SHA256:host-key",
            credential: sshReference
        )
        try ssh.validate()
        let encoded = try JSONEncoder().encode(ssh)
        XCTAssertFalse(String(decoding: encoded, as: UTF8.self).contains("private-key-material"))

        let relayReference = try ControllerRouteCredentialReference(
            id: "relay-token-1",
            route: .selfHostedRelay,
            purpose: .relayAdmission
        )
        let relay = try ControllerRemoteRouteConfiguration.selfHostedRelay(
            endpoint: "wss://relay.example/termirust",
            spkiPin: "sha256/relay-spki",
            credential: relayReference
        )
        try relay.validate()

        XCTAssertThrowsError(
            try ControllerRemoteRouteConfiguration.ssh(
                endpoint: "host.example",
                port: 22,
                username: "deploy",
                hostKeyPin: "SHA256:host-key",
                credential: relayReference
            )
        )
        XCTAssertThrowsError(
            try ControllerRemoteRouteConfiguration.selfHostedRelay(
                endpoint: "ws://relay.example/termirust",
                spkiPin: "sha256/relay-spki",
                credential: relayReference
            )
        )
    }

    private func connectOnline(
        _ route: ControllerRemoteRouteKind,
        coordinator: inout AppleControllerRouteCoordinator
    ) throws {
        _ = try coordinator.select(route, explicitlyConfirmed: true)
        XCTAssertEqual(try coordinator.connectSelected().startTransport, route)
        _ = try coordinator.transportReady(route)
        _ = try coordinator.authenticated(route)
        XCTAssertTrue(projection(route, in: coordinator).terminalAllowed)
    }

    private func projection(
        _ route: ControllerRemoteRouteKind,
        in coordinator: AppleControllerRouteCoordinator
    ) -> AppleControllerRouteProjection {
        coordinator.projections.first { $0.route == route }!
    }
}

private extension AppleControllerRouteAvailability {
    static let all = Self(privateNetwork: true, ssh: true, selfHostedRelay: true)
}
