import Foundation

protocol ControllerRouteConfigurationStoring: Sendable {
    func save(
        hostID: String,
        configuration: ControllerRemoteRouteConfiguration
    ) throws
    func load(
        hostID: String,
        route: ControllerRemoteRouteKind
    ) throws -> ControllerRemoteRouteConfiguration?
    func delete(hostID: String, route: ControllerRemoteRouteKind) throws
}

final class ControllerRouteConfigurationStore:
    ControllerRouteConfigurationStoring, @unchecked Sendable {
    private let defaults: UserDefaults
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func save(
        hostID: String,
        configuration: ControllerRemoteRouteConfiguration
    ) throws {
        try validateHostID(hostID)
        try configuration.validate()
        guard configuration.kind != .localIPC else {
            throw ControllerRemoteRouteConfigurationError.unsupportedRoute
        }
        defaults.set(
            try encoder.encode(configuration),
            forKey: key(hostID: hostID, route: configuration.kind)
        )
    }

    func load(
        hostID: String,
        route: ControllerRemoteRouteKind
    ) throws -> ControllerRemoteRouteConfiguration? {
        try validateHostID(hostID)
        guard route != .localIPC else { return nil }
        guard let data = defaults.data(forKey: key(hostID: hostID, route: route)) else {
            return nil
        }
        let configuration = try decoder.decode(
            ControllerRemoteRouteConfiguration.self,
            from: data
        )
        guard configuration.kind == route else {
            throw ControllerRemoteRouteConfigurationError.invalidCombination
        }
        try configuration.validate()
        return configuration
    }

    func delete(hostID: String, route: ControllerRemoteRouteKind) throws {
        try validateHostID(hostID)
        defaults.removeObject(forKey: key(hostID: hostID, route: route))
    }

    private func key(hostID: String, route: ControllerRemoteRouteKind) -> String {
        "controller-route-configuration-v1.\(hostID).\(route.rawValue)"
    }

    private func validateHostID(_ hostID: String) throws {
        let trimmed = hostID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty,
              trimmed == hostID,
              hostID.utf8.count <= 128,
              hostID.unicodeScalars.allSatisfy({
                  CharacterSet.alphanumerics.contains($0)
                      || CharacterSet(charactersIn: "-_.").contains($0)
              }) else {
            throw ControllerRouteCredentialStoreError.invalidHost
        }
    }
}
