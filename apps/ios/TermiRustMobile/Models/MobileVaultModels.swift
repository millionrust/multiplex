import Foundation

let mobileVaultSchemaVersion = 1

struct EncryptedMobileVaultEnvelope: Decodable, Equatable {
    let version: Int
    let schemaVersion: Int
    let cipher: String
    let kdf: String
    let salt: String
    let nonce: String
    let ciphertext: String

    enum CodingKeys: String, CodingKey {
        case version
        case schemaVersion = "schema_version"
        case cipher
        case kdf
        case salt
        case nonce
        case ciphertext
    }
}

struct MobileVaultExport: Codable, Hashable {
    let schemaVersion: Int
    let exportId: String
    let createdAtMillis: UInt64
    let updatedAtMillis: UInt64
    let sourceDeviceId: String
    let vaults: [MobileVault]
    let hosts: [MobileHost]
    let groups: [MobileGroup]
    let tags: [String]
    let identities: [MobileIdentityMetadata]
    let knownHosts: [MobileKnownHost]
    let sync: MobileSyncMetadata
    let devices: [MobileDeviceRecord]
    let deviceKeys: [MobileDeviceVaultKey]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case exportId = "export_id"
        case createdAtMillis = "created_at_millis"
        case updatedAtMillis = "updated_at_millis"
        case sourceDeviceId = "source_device_id"
        case vaults
        case hosts
        case groups
        case tags
        case identities
        case knownHosts = "known_hosts"
        case sync
        case devices
        case deviceKeys = "device_keys"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(Int.self, forKey: .schemaVersion)
        exportId = try container.decode(String.self, forKey: .exportId)
        createdAtMillis = try container.decode(UInt64.self, forKey: .createdAtMillis)
        updatedAtMillis = try container.decode(UInt64.self, forKey: .updatedAtMillis)
        sourceDeviceId = try container.decode(String.self, forKey: .sourceDeviceId)
        vaults = try container.decodeIfPresent([MobileVault].self, forKey: .vaults) ?? []
        hosts = try container.decodeIfPresent([MobileHost].self, forKey: .hosts) ?? []
        groups = try container.decodeIfPresent([MobileGroup].self, forKey: .groups) ?? []
        tags = try container.decodeIfPresent([String].self, forKey: .tags) ?? []
        identities = try container.decodeIfPresent([MobileIdentityMetadata].self, forKey: .identities) ?? []
        knownHosts = try container.decodeIfPresent([MobileKnownHost].self, forKey: .knownHosts) ?? []
        sync = try container.decodeIfPresent(MobileSyncMetadata.self, forKey: .sync) ?? MobileSyncMetadata()
        devices = try container.decodeIfPresent([MobileDeviceRecord].self, forKey: .devices) ?? []
        deviceKeys = try container.decodeIfPresent([MobileDeviceVaultKey].self, forKey: .deviceKeys) ?? []
    }

    var sourceDeviceRecord: MobileDeviceRecord? {
        devices.first { $0.deviceId == sourceDeviceId }
    }

    var sourceDeviceRevoked: Bool {
        sourceDeviceRecord?.revokedAtMillis != nil
    }

    func isOlder(than other: MobileVaultExport) -> Bool {
        if let revision = sync.revision, let otherRevision = other.sync.revision, revision != otherRevision {
            return revision < otherRevision
        }
        return updatedAtMillis < other.updatedAtMillis
    }

    func isDeviceRevoked(_ deviceId: String) -> Bool {
        let trimmed = deviceId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return false
        }
        return devices.contains { device in
            device.deviceId == trimmed && device.revokedAtMillis != nil
        }
    }

    func activeDeviceKey(for deviceId: String) -> MobileDeviceVaultKey? {
        guard !isDeviceRevoked(deviceId) else {
            return nil
        }
        return deviceKeys.first { key in
            key.deviceId == deviceId && key.revokedAtMillis == nil
        }
    }
}

struct MobileVault: Codable, Hashable, Identifiable {
    let id: String
    let label: String
    let description: String
    let kind: String
}

struct MobileHost: Codable, Hashable, Identifiable {
    let id: String
    let label: String
    let vaultId: String?
    let group: String
    let tags: [String]
    let host: String
    let port: UInt16
    let username: String
    let auth: MobileAuthMetadata
    let jumpHostId: String?
    let startupDirectory: String?
    let startupCommand: String?
    let startInFiles: Bool
    let persistentSession: MobilePersistentSession
    let terminalScrollbackRows: UInt32?
    let colorTag: String?
    let environment: [MobileEnvironmentVariable]
    let knownHostEndpoint: String?

    enum CodingKeys: String, CodingKey {
        case id
        case label
        case vaultId = "vault_id"
        case group
        case tags
        case host
        case port
        case username
        case auth
        case jumpHostId = "jump_host_id"
        case startupDirectory = "startup_directory"
        case startupCommand = "startup_command"
        case startInFiles = "start_in_files"
        case persistentSession = "persistent_session"
        case terminalScrollbackRows = "terminal_scrollback_rows"
        case colorTag = "color_tag"
        case environment
        case knownHostEndpoint = "known_host_endpoint"
    }
}

struct MobileAuthMetadata: Codable, Hashable {
    let kind: MobileAuthKind
    let identityId: String?
    let secretRef: String?

    enum CodingKeys: String, CodingKey {
        case kind
        case identityId = "identity_id"
        case secretRef = "secret_ref"
    }
}

enum MobileAuthKind: String, Codable, Hashable {
    case password
    case privateKey = "private_key"
}

struct MobilePersistentSession: Codable, Hashable {
    let enabled: Bool
    let sessionName: String?
    let detachOthers: Bool

    enum CodingKeys: String, CodingKey {
        case enabled
        case sessionName = "session_name"
        case detachOthers = "detach_others"
    }
}

struct MobileEnvironmentVariable: Codable, Hashable {
    let name: String
    let value: String
}

struct MobileIdentityMetadata: Codable, Hashable, Identifiable {
    let id: String
    let label: String
    let vaultId: String?
    let kind: String
    let publicKey: String?
    let fingerprint: String?
    let secretRef: String?

    enum CodingKeys: String, CodingKey {
        case id
        case label
        case vaultId = "vault_id"
        case kind
        case publicKey = "public_key"
        case fingerprint
        case secretRef = "secret_ref"
    }
}

struct MobileKnownHost: Codable, Hashable, Identifiable {
    var id: String { endpoint }

    let endpoint: String
    let publicKey: String
    let algorithm: String?
    let fingerprint: String?

    enum CodingKeys: String, CodingKey {
        case endpoint
        case publicKey = "public_key"
        case algorithm
        case fingerprint
    }
}

struct MobileGroup: Codable, Hashable, Identifiable {
    let id: String
    let name: String
}

struct MobileSyncMetadata: Codable, Hashable {
    let revision: UInt64?
    let lastSyncedAtMillis: UInt64?

    enum CodingKeys: String, CodingKey {
        case revision
        case lastSyncedAtMillis = "last_synced_at_millis"
    }

    init(revision: UInt64? = nil, lastSyncedAtMillis: UInt64? = nil) {
        self.revision = revision
        self.lastSyncedAtMillis = lastSyncedAtMillis
    }
}

struct MobileDeviceRecord: Codable, Hashable, Identifiable {
    var id: String { deviceId }

    let deviceId: String
    let label: String
    let platform: String?
    let publicKey: String?
    let pairedAtMillis: UInt64?
    let lastSeenAtMillis: UInt64?
    let revokedAtMillis: UInt64?

    enum CodingKeys: String, CodingKey {
        case deviceId = "device_id"
        case label
        case platform
        case publicKey = "public_key"
        case pairedAtMillis = "paired_at_millis"
        case lastSeenAtMillis = "last_seen_at_millis"
        case revokedAtMillis = "revoked_at_millis"
    }
}

struct MobileDeviceVaultKey: Codable, Hashable, Identifiable {
    var id: String { keyId }

    let keyId: String
    let deviceId: String
    let wrappingAlgorithm: String
    let encryptedVaultKey: String
    let createdAtMillis: UInt64?
    let revokedAtMillis: UInt64?

    enum CodingKeys: String, CodingKey {
        case keyId = "key_id"
        case deviceId = "device_id"
        case wrappingAlgorithm = "wrapping_algorithm"
        case encryptedVaultKey = "encrypted_vault_key"
        case createdAtMillis = "created_at_millis"
        case revokedAtMillis = "revoked_at_millis"
    }
}

struct MobileDevicePairingRequest: Codable, Hashable {
    let schemaVersion: Int
    let requestId: String
    let deviceId: String
    let label: String
    let platform: String
    let publicKey: String?
    let createdAtMillis: UInt64

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case requestId = "request_id"
        case deviceId = "device_id"
        case label
        case platform
        case publicKey = "public_key"
        case createdAtMillis = "created_at_millis"
    }
}
