import Combine
import Foundation
@preconcurrency import Network

/// A Multiplex Host announcing itself with Bonjour on this network. Finding one is not trusting
/// it: pairing still needs the code, and the announcement carries no secret.
struct ControllerDiscoveredComputer: Identifiable, Hashable, Sendable {
    let discoveryID: String
    let name: String
    let endpoint: NWEndpoint

    var id: String { discoveryID }
}

/// Where a code pairing connects: a computer found on the network, or an address typed as
/// `host:port`.
enum ControllerPairingTarget: Hashable, Sendable {
    case discovered(ControllerDiscoveredComputer)
    case address(String)

    var displayName: String {
        switch self {
        case .discovered(let computer):
            computer.name
        case .address(let text):
            (try? ControllerTypedAddress(text).host) ?? text
        }
    }
}

protocol ControllerComputerLookup: Sendable {
    /// The Bonjour endpoint of the Host announcing `discoveryID`, if it appears within `timeout`.
    func endpoint(discoveryID: String, within timeout: Duration) async -> NWEndpoint?
}

@MainActor
final class ControllerComputerBrowser: ObservableObject, ControllerComputerLookup {
    static let serviceType = "_multiplex._tcp"
    static let serviceDomain = "local."
    private static let maxComputers = 32
    private static let maxNameScalars = 63

    @Published private(set) var computers: [ControllerDiscoveredComputer] = []
    /// True when browsing stopped or is waiting, usually because Local Network access is off.
    @Published private(set) var isUnavailable = false

    private var browser: NWBrowser?
    private var visibleClients = 0
    private var lookupClients = 0

    /// Starts browsing for a screen that lists computers. Balance with `stop()`.
    func start() {
        visibleClients += 1
        startBrowser()
    }

    func stop() {
        visibleClients = max(0, visibleClients - 1)
        stopBrowserIfUnused()
    }

    nonisolated func endpoint(discoveryID: String, within timeout: Duration) async -> NWEndpoint? {
        await lookUp(discoveryID: discoveryID, timeout: timeout)
    }

    private func lookUp(discoveryID: String, timeout: Duration) async -> NWEndpoint? {
        lookupClients += 1
        startBrowser()
        defer {
            lookupClients -= 1
            stopBrowserIfUnused()
        }
        let deadline = ContinuousClock.now + timeout
        while true {
            if let match = computers.first(where: { $0.discoveryID == discoveryID }) {
                return match.endpoint
            }
            guard ContinuousClock.now < deadline, !Task.isCancelled else { return nil }
            try? await Task.sleep(for: .milliseconds(200))
        }
    }

    private func startBrowser() {
        guard browser == nil else { return }
        let parameters = NWParameters()
        parameters.includePeerToPeer = false
        let browser = NWBrowser(
            for: .bonjourWithTXTRecord(type: Self.serviceType, domain: Self.serviceDomain),
            using: parameters
        )
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            MainActor.assumeIsolated { self?.apply(results: results) }
        }
        browser.stateUpdateHandler = { [weak self] state in
            MainActor.assumeIsolated { self?.apply(state: state) }
        }
        self.browser = browser
        isUnavailable = false
        browser.start(queue: .main)
    }

    private func stopBrowserIfUnused() {
        guard visibleClients == 0, lookupClients == 0, let browser else { return }
        browser.browseResultsChangedHandler = nil
        browser.stateUpdateHandler = nil
        browser.cancel()
        self.browser = nil
        computers = []
    }

    private func apply(results: Set<NWBrowser.Result>) {
        var found: [String: ControllerDiscoveredComputer] = [:]
        for result in results {
            guard case .service(let name, _, _, _) = result.endpoint,
                  case .bonjour(let record) = result.metadata,
                  record["v"] == "1",
                  let id = record["id"],
                  Self.isDiscoveryID(id),
                  !name.isEmpty,
                  name.unicodeScalars.count <= Self.maxNameScalars,
                  !name.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
                  found[id] == nil else { continue }
            found[id] = ControllerDiscoveredComputer(
                discoveryID: id,
                name: name,
                endpoint: result.endpoint
            )
        }
        computers = Array(
            found.values
                .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
                .prefix(Self.maxComputers)
        )
    }

    private func apply(state: NWBrowser.State) {
        switch state {
        case .ready:
            isUnavailable = false
        case .waiting:
            isUnavailable = true
        case .failed:
            isUnavailable = true
            browser?.cancel()
            browser = nil
            computers = []
        default:
            break
        }
    }

    private static func isDiscoveryID(_ value: String) -> Bool {
        value.utf8.count == 32
            && value.utf8.allSatisfy { (0x30...0x39).contains($0) || (0x61...0x66).contains($0) }
    }
}

/// An address typed as `host:port`, for reaching a computer over Tailscale or another VPN:
/// a MagicDNS name, an IPv4 address, or `[IPv6]:port`.
struct ControllerTypedAddress: Sendable {
    let host: String
    let port: UInt16

    init(_ text: String) throws {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let host: Substring
        let port: Substring
        if trimmed.hasPrefix("[") {
            guard let close = trimmed.firstIndex(of: "]"),
                  trimmed[trimmed.index(after: close)...].hasPrefix(":") else {
                throw ControllerPairingError.invalidAddress
            }
            host = trimmed[trimmed.index(after: trimmed.startIndex)..<close]
            port = trimmed[trimmed.index(close, offsetBy: 2)...]
        } else {
            guard let colon = trimmed.lastIndex(of: ":") else {
                throw ControllerPairingError.invalidAddress
            }
            host = trimmed[..<colon]
            port = trimmed[trimmed.index(after: colon)...]
            guard !host.contains(":") else { throw ControllerPairingError.invalidAddress }
        }
        guard !host.isEmpty,
              host.utf8.count <= 253,
              let portNumber = UInt16(port),
              portNumber > 0 else {
            throw ControllerPairingError.invalidAddress
        }
        let isLiteral = IPv4Address(String(host)) != nil || IPv6Address(String(host)) != nil
        guard isLiteral || host.unicodeScalars.allSatisfy({
            CharacterSet.alphanumerics.contains($0) && $0.isASCII || $0 == "." || $0 == "-"
        }) else {
            throw ControllerPairingError.invalidAddress
        }
        self.host = String(host)
        self.port = portNumber
    }

    /// The private addresses this entry reaches. A name is resolved first, and only the
    /// private, CGNAT, and unique local addresses it resolves to are kept.
    func resolvePrivateRoutes() async throws -> [HostRoute] {
        let addresses: [String]
        if IPv4Address(host) != nil || IPv6Address(host) != nil {
            addresses = [host]
        } else {
            addresses = await Self.resolve(host)
            guard !addresses.isEmpty else { throw ControllerPairingError.invalidAddress }
        }
        var routes: [HostRoute] = []
        for address in addresses {
            let route = try HostRoute(address: address, port: port)
            if route.isPrivateNetworkRoute, !routes.contains(route) { routes.append(route) }
        }
        guard !routes.isEmpty else { throw ControllerPairingError.publicRouteRejected }
        return Array(routes.prefix(HostRoute.maxRoutesPerHost))
    }

    private static func resolve(_ host: String) async -> [String] {
        await Task.detached(priority: .userInitiated) {
            var hints = addrinfo()
            hints.ai_family = AF_UNSPEC
            hints.ai_socktype = SOCK_STREAM
            var result: UnsafeMutablePointer<addrinfo>?
            guard getaddrinfo(host, nil, &hints, &result) == 0, let first = result else {
                return []
            }
            defer { freeaddrinfo(first) }
            var addresses: [String] = []
            var cursor: UnsafeMutablePointer<addrinfo>? = first
            while let info = cursor {
                var buffer = [CChar](repeating: 0, count: Int(NI_MAXHOST))
                if let address = info.pointee.ai_addr,
                   getnameinfo(
                       address,
                       info.pointee.ai_addrlen,
                       &buffer,
                       socklen_t(buffer.count),
                       nil,
                       0,
                       NI_NUMERICHOST
                   ) == 0 {
                    let bytes = buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }
                    let text = String(decoding: bytes, as: UTF8.self)
                    if let unscoped = text.split(separator: "%", maxSplits: 1).first {
                        addresses.append(String(unscoped))
                    }
                }
                cursor = info.pointee.ai_next
            }
            return addresses
        }.value
    }
}
