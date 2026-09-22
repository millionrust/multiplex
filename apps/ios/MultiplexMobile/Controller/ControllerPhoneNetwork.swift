import Darwin
import Foundation
@preconcurrency import Network
import Security

/// What the route planner needs to know about the network the phone is on right now.
protocol ControllerPhoneNetworkProviding: Sendable {
    func currentNetwork() -> PhoneNetwork
}

/// A fixed network, for tests and for transports that do not choose between addresses.
struct ControllerFixedPhoneNetwork: ControllerPhoneNetworkProviding {
    let network: PhoneNetwork

    func currentNetwork() -> PhoneNetwork { network }
}

/// Reads the phone's physical network: its kind from the system path, and the phone's own
/// addresses on that interface. A VPN's tunnel is never the physical network, so Tailscale being
/// on does not hide the Wi-Fi the phone is actually on. iOS offers no public way to read the
/// gateway, so the fingerprint is built from the subnets alone.
final class ControllerPhoneNetworkMonitor: ControllerPhoneNetworkProviding, @unchecked Sendable {
    private static let saltDefaultsKey = "multiplex.controller.route_network_salt"
    private static let saltBytes = 16

    private let monitor = NWPathMonitor()
    private let salt: Data

    init(defaults: UserDefaults = .standard) {
        salt = Self.loadSalt(defaults: defaults)
        monitor.start(queue: DispatchQueue(label: "com.multiplex.controller.network-path"))
    }

    deinit {
        monitor.cancel()
    }

    func currentNetwork() -> PhoneNetwork {
        let path = monitor.currentPath
        guard path.status == .satisfied,
              let physical = path.availableInterfaces.first(where: { interface in
                  [.wifi, .wiredEthernet, .cellular].contains(interface.type)
              }) else {
            let link: PhoneLink = path.status == .satisfied ? .other : .offline
            return PhoneNetwork(
                link: link,
                addresses: [],
                fingerprint: networkFingerprint(salt: salt, link: link, addresses: [], gateway: nil)
            )
        }
        let link: PhoneLink
        switch physical.type {
        case .wifi: link = .wifi
        case .wiredEthernet: link = .ethernet
        case .cellular: link = .cellular
        default: link = .other
        }
        let addresses = Self.addresses(onInterface: physical.name)
        return PhoneNetwork(
            link: link,
            addresses: addresses,
            fingerprint: networkFingerprint(salt: salt, link: link, addresses: addresses, gateway: nil)
        )
    }

    /// The IPv4 and IPv6 addresses on `name`, each with the prefix length its netmask gives.
    static func addresses(onInterface name: String) -> [PhoneAddress] {
        var first: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&first) == 0, let first else { return [] }
        defer { freeifaddrs(first) }
        var addresses: [PhoneAddress] = []
        for entry in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let interface = entry.pointee
            let flags = Int32(interface.ifa_flags)
            guard String(cString: interface.ifa_name) == name,
                  flags & IFF_UP != 0,
                  flags & IFF_LOOPBACK == 0,
                  let address = interface.ifa_addr,
                  let mask = interface.ifa_netmask else {
                continue
            }
            let family = Int32(address.pointee.sa_family)
            guard family == AF_INET || family == AF_INET6,
                  let text = numericHost(address),
                  let length = prefixLength(mask, family: family) else {
                continue
            }
            addresses.append(PhoneAddress(address: text, prefixLength: length))
            if addresses.count >= 16 { break }
        }
        return addresses
    }

    private static func numericHost(_ address: UnsafeMutablePointer<sockaddr>) -> String? {
        var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
        let length = socklen_t(address.pointee.sa_len)
        guard getnameinfo(address, length, &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 else {
            return nil
        }
        return String(cString: host)
    }

    /// A netmask may be stored shorter than its address family's `sockaddr`, so only the bytes
    /// it actually has are read.
    private static func prefixLength(_ mask: UnsafeMutablePointer<sockaddr>, family: Int32) -> UInt8? {
        let (offset, count) = family == AF_INET
            ? (MemoryLayout<sockaddr_in>.offset(of: \sockaddr_in.sin_addr) ?? 4, 4)
            : (MemoryLayout<sockaddr_in6>.offset(of: \sockaddr_in6.sin6_addr) ?? 8, 16)
        let stored = max(0, min(Int(mask.pointee.sa_len) - offset, count))
        let raw = UnsafeRawPointer(mask).advanced(by: offset)
        let bits = (0..<stored).reduce(0) { total, index in
            total + raw.load(fromByteOffset: index, as: UInt8.self).nonzeroBitCount
        }
        return UInt8(exactly: bits)
    }

    private static func loadSalt(defaults: UserDefaults) -> Data {
        if let salt = defaults.data(forKey: saltDefaultsKey), salt.count == saltBytes {
            return salt
        }
        var bytes = [UInt8](repeating: 0, count: saltBytes)
        if SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) != errSecSuccess {
            bytes = (0..<saltBytes).map { _ in UInt8.random(in: .min ... .max) }
        }
        let salt = Data(bytes)
        defaults.set(salt, forKey: saltDefaultsKey)
        return salt
    }
}
