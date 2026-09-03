import Foundation

protocol MobileDeviceIdentifying {
    var deviceId: String { get }
}

final class UserDefaultsMobileDeviceIdentityStore: MobileDeviceIdentifying {
    private let defaults: UserDefaults
    private let key: String

    init(defaults: UserDefaults = .standard, key: String = "termirust.mobile.device_id") {
        self.defaults = defaults
        self.key = key
    }

    var deviceId: String {
        if let existing = defaults.string(forKey: key), !existing.isEmpty {
            return existing
        }
        let generated = "ios-\(UUID().uuidString.lowercased())"
        defaults.set(generated, forKey: key)
        return generated
    }
}
