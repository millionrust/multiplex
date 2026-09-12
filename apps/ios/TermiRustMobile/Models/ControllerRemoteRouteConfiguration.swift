import Foundation

enum ControllerRouteCredentialPurpose: String, Codable, Sendable {
    case sshAuthentication = "ssh_authentication"
    case relayAdmission = "relay_admission"
}

enum ControllerSSHAuthenticationKind: String, Codable, Sendable {
    case password
    case privateKey = "private_key"
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
    let sshAuthentication: ControllerSSHAuthenticationKind?
    let relayRouteID: String?
    let relayRevocationEpoch: UInt64?

    private enum CodingKeys: String, CodingKey {
        case kind, endpoint, port, username, credential
        case trustPin = "trust_pin"
        case sshAuthentication = "ssh_authentication"
        case relayRouteID = "relay_route_id"
        case relayRevocationEpoch = "relay_revocation_epoch"
    }

    static func privateNetwork(endpoint: HostRoute) -> Self {
        Self(
            kind: .privateNetwork,
            endpoint: endpoint.address,
            port: endpoint.port,
            username: nil,
            trustPin: nil,
            credential: nil,
            sshAuthentication: nil,
            relayRouteID: nil,
            relayRevocationEpoch: nil
        )
    }

    static func ssh(
        endpoint: String,
        port: UInt16,
        username: String,
        hostKeyPin: String,
        credential: ControllerRouteCredentialReference,
        authentication: ControllerSSHAuthenticationKind
    ) throws -> Self {
        let value = Self(
            kind: .ssh,
            endpoint: endpoint,
            port: port,
            username: username,
            trustPin: hostKeyPin,
            credential: credential,
            sshAuthentication: authentication,
            relayRouteID: nil,
            relayRevocationEpoch: nil
        )
        try value.validate()
        return value
    }

    static func selfHostedRelay(
        endpoint: String,
        spkiPin: String,
        credential: ControllerRouteCredentialReference,
        routeID: String,
        revocationEpoch: UInt64
    ) throws -> Self {
        let value = Self(
            kind: .selfHostedRelay,
            endpoint: endpoint,
            port: nil,
            username: nil,
            trustPin: spkiPin,
            credential: credential,
            sshAuthentication: nil,
            relayRouteID: routeID,
            relayRevocationEpoch: revocationEpoch
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
            guard let port, username == nil, trustPin == nil, credential == nil,
                  sshAuthentication == nil, relayRouteID == nil,
                  relayRevocationEpoch == nil else {
                throw ControllerRemoteRouteConfigurationError.invalidCombination
            }
            _ = try HostRoute(address: endpoint, port: port)
        case .ssh:
            guard let port,
                  let username,
                  let trustPin,
                  let credential,
                  sshAuthentication != nil,
                  credential.route == .ssh,
                  credential.purpose == .sshAuthentication,
                  relayRouteID == nil,
                  relayRevocationEpoch == nil,
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
                  sshAuthentication == nil,
                  let trustPin,
                  let credential,
                  let relayRouteID,
                  relayRevocationEpoch != nil,
                  let decodedRouteID = Data(base64Encoded: relayRouteID),
                  decodedRouteID.count == 32,
                  trustPin.hasPrefix("sha256/"),
                  let decodedPin = Data(base64Encoded: String(trustPin.dropFirst("sha256/".count))),
                  decodedPin.count == 32,
                  credential.route == .selfHostedRelay,
                  credential.purpose == .relayAdmission,
                  !trustPin.isEmpty,
                  trustPin.utf8.count <= 512,
                  !trustPin.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
                  let components = URLComponents(string: endpoint),
                  components.scheme?.lowercased() == "wss",
                  components.host != nil,
                  components.path == "/relay/v1",
                  components.user == nil,
                  components.password == nil,
                  components.query == nil,
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

struct ControllerRelayRoutePackage: Equatable, Sendable {
    let endpoint: String
    let spkiPin: String
    let routeID: String
    let revocationEpoch: UInt64
    let admissionCredential: String

    static func decode(_ text: String) throws -> Self {
        guard let data = text.data(using: .utf8), data.count <= 16 * 1_024 else {
            throw ControllerRelayRoutePackageError.invalidPackage
        }
        let encoded = try JSONDecoder().decode(Encoded.self, from: data)
        guard encoded.schema == "termirust-relay-route",
              encoded.schemaVersion == 1,
              encoded.role == "controller",
              var secret = Data(base64Encoded: encoded.admissionCredential) else {
            throw ControllerRelayRoutePackageError.invalidPackage
        }
        defer { secret.resetBytes(in: 0 ..< secret.count) }
        guard secret.count == 32 else {
            throw ControllerRelayRoutePackageError.invalidPackage
        }
        let reference = try ControllerRouteCredentialReference(
            id: "relay-package-validation",
            route: .selfHostedRelay,
            purpose: .relayAdmission
        )
        _ = try ControllerRemoteRouteConfiguration.selfHostedRelay(
            endpoint: encoded.endpoint,
            spkiPin: encoded.spkiPin,
            credential: reference,
            routeID: encoded.relayRouteID,
            revocationEpoch: encoded.relayRevocationEpoch
        )
        return Self(
            endpoint: encoded.endpoint,
            spkiPin: encoded.spkiPin,
            routeID: encoded.relayRouteID,
            revocationEpoch: encoded.relayRevocationEpoch,
            admissionCredential: encoded.admissionCredential
        )
    }

    private struct Encoded: Decodable {
        let schema: String
        let schemaVersion: UInt64
        let role: String
        let endpoint: String
        let spkiPin: String
        let relayRouteID: String
        let relayRevocationEpoch: UInt64
        let admissionCredential: String

        private enum CodingKeys: String, CodingKey {
            case schema, role, endpoint
            case schemaVersion = "schema_version"
            case spkiPin = "spki_pin"
            case relayRouteID = "relay_route_id"
            case relayRevocationEpoch = "relay_revocation_epoch"
            case admissionCredential = "admission_credential"
        }
    }
}

enum ControllerRelayRoutePackageError: Error, Equatable, Sendable {
    case invalidPackage
}
