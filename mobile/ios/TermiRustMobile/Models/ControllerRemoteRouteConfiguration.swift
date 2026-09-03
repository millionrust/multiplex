import Foundation

enum ControllerRouteCredentialPurpose: String, Codable, Sendable {
    case sshAuthentication = "ssh_authentication"
    case relayAdmission = "relay_admission"
}

struct ControllerRouteCredentialReference: Codable, Equatable, Hashable, Sendable {
    let id: String
    let route: ControllerRemoteRouteKind
    let purpose: ControllerRouteCredentialPurpose

    init(
        id: String,
        route: ControllerRemoteRouteKind,
        purpose: ControllerRouteCredentialPurpose
    ) throws {
        let trimmed = id.trimmingCharacters(in: .whitespacesAndNewlines)
        guard route != .localIPC,
              !trimmed.isEmpty,
              trimmed.utf8.count <= 128,
              trimmed.unicodeScalars.allSatisfy({
                  CharacterSet.alphanumerics.contains($0) || "-_.".unicodeScalars.contains($0)
              }),
              (route == .ssh && purpose == .sshAuthentication)
                || (route == .selfHostedRelay && purpose == .relayAdmission) else {
            throw ControllerRemoteRouteConfigurationError.invalidCredentialReference
        }
        self.id = trimmed
        self.route = route
        self.purpose = purpose
    }
}

struct ControllerRemoteRouteConfiguration: Codable, Equatable, Sendable {
    let kind: ControllerRemoteRouteKind
    let endpoint: String
    let port: UInt16?
    let username: String?
    let trustPin: String?
    let credential: ControllerRouteCredentialReference?

    static func privateNetwork(endpoint: HostRoute) -> Self {
        Self(
            kind: .privateNetwork,
            endpoint: endpoint.address,
            port: endpoint.port,
            username: nil,
            trustPin: nil,
            credential: nil
        )
    }

    static func ssh(
        endpoint: String,
        port: UInt16,
        username: String,
        hostKeyPin: String,
        credential: ControllerRouteCredentialReference
    ) throws -> Self {
        let value = Self(
            kind: .ssh,
            endpoint: endpoint,
            port: port,
            username: username,
            trustPin: hostKeyPin,
            credential: credential
        )
        try value.validate()
        return value
    }

    static func selfHostedRelay(
        endpoint: String,
        spkiPin: String,
        credential: ControllerRouteCredentialReference
    ) throws -> Self {
        let value = Self(
            kind: .selfHostedRelay,
            endpoint: endpoint,
            port: nil,
            username: nil,
            trustPin: spkiPin,
            credential: credential
        )
        try value.validate()
        return value
    }

    func validate() throws {
        guard !endpoint.isEmpty,
              endpoint.utf8.count <= 2_048,
              !endpoint.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            throw ControllerRemoteRouteConfigurationError.invalidEndpoint
        }
        switch kind {
        case .localIPC:
            throw ControllerRemoteRouteConfigurationError.unsupportedRoute
        case .privateNetwork:
            guard let port, username == nil, trustPin == nil, credential == nil else {
                throw ControllerRemoteRouteConfigurationError.invalidCombination
            }
            _ = try HostRoute(address: endpoint, port: port)
        case .ssh:
            guard let port,
                  let username,
                  let trustPin,
                  let credential,
                  credential.route == .ssh,
                  credential.purpose == .sshAuthentication,
                  !username.isEmpty,
                  username.utf8.count <= 255,
                  !username.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
                  !trustPin.isEmpty,
                  trustPin.utf8.count <= 512,
                  !trustPin.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
            else {
                throw ControllerRemoteRouteConfigurationError.invalidCombination
            }
            _ = try HostRoute(address: endpoint, port: port)
        case .selfHostedRelay:
            guard port == nil,
                  username == nil,
                  let trustPin,
                  let credential,
                  credential.route == .selfHostedRelay,
                  credential.purpose == .relayAdmission,
                  !trustPin.isEmpty,
                  trustPin.utf8.count <= 512,
                  !trustPin.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
                  let components = URLComponents(string: endpoint),
                  components.scheme?.lowercased() == "wss",
                  components.host != nil,
                  components.user == nil,
                  components.password == nil,
                  components.fragment == nil else {
                throw ControllerRemoteRouteConfigurationError.invalidCombination
            }
        }
    }
}

enum ControllerRemoteRouteConfigurationError: Error, Equatable, Sendable {
    case unsupportedRoute
    case invalidEndpoint
    case invalidCredentialReference
    case invalidCombination
}
