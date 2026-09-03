import Foundation
import Crypto
@preconcurrency import NIOSSH

enum OpenSSHPrivateKeyParserError: Error, LocalizedError, Equatable {
    case invalidPEM
    case invalidEnvelope
    case encryptedKeysUnsupported
    case unsupportedKeyType(String)
    case checkintsMismatch

    var errorDescription: String? {
        switch self {
        case .invalidPEM:
            return "The private key is not a valid OpenSSH private key PEM."
        case .invalidEnvelope:
            return "The private key has an invalid OpenSSH envelope."
        case .encryptedKeysUnsupported:
            return "Encrypted private keys are not supported yet. Import an unencrypted Ed25519 OpenSSH key for this prototype."
        case .unsupportedKeyType(let keyType):
            return "Unsupported private key type: \(keyType). Only unencrypted OpenSSH Ed25519 keys are supported in this prototype."
        case .checkintsMismatch:
            return "The OpenSSH private key integrity check failed."
        }
    }
}

enum OpenSSHPrivateKeyParser {
    static func parse(_ pem: String) throws -> NIOSSHPrivateKey {
        let base64 = pem
            .split(whereSeparator: \.isNewline)
            .filter { !$0.hasPrefix("-----") }
            .joined()
        guard let data = Data(base64Encoded: base64) else {
            throw OpenSSHPrivateKeyParserError.invalidPEM
        }

        var reader = SSHDataReader(data: data)
        guard reader.readCString() == "openssh-key-v1" else {
            throw OpenSSHPrivateKeyParserError.invalidEnvelope
        }
        let cipherName = try reader.readString()
        let kdfName = try reader.readString()
        _ = try reader.readData()
        guard cipherName == "none", kdfName == "none" else {
            throw OpenSSHPrivateKeyParserError.encryptedKeysUnsupported
        }
        guard try reader.readUInt32() == 1 else {
            throw OpenSSHPrivateKeyParserError.invalidEnvelope
        }
        _ = try reader.readData()
        let privateBlob = try reader.readData()

        var privateReader = SSHDataReader(data: privateBlob)
        let check1 = try privateReader.readUInt32()
        let check2 = try privateReader.readUInt32()
        guard check1 == check2 else {
            throw OpenSSHPrivateKeyParserError.checkintsMismatch
        }

        let keyType = try privateReader.readString()
        guard keyType == "ssh-ed25519" else {
            throw OpenSSHPrivateKeyParserError.unsupportedKeyType(keyType)
        }
        let publicKey = try privateReader.readData()
        let privateKey = try privateReader.readData()
        guard publicKey.count == 32, privateKey.count == 64 else {
            throw OpenSSHPrivateKeyParserError.invalidEnvelope
        }

        let seed = privateKey.prefix(32)
        let signingKey = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
        guard signingKey.publicKey.rawRepresentation == publicKey else {
            throw OpenSSHPrivateKeyParserError.invalidEnvelope
        }
        return NIOSSHPrivateKey(ed25519Key: signingKey)
    }
}

private struct SSHDataReader {
    private let data: Data
    private var offset = 0

    init(data: Data) {
        self.data = data
    }

    mutating func readCString() -> String? {
        guard let end = data[offset...].firstIndex(of: 0) else {
            return nil
        }
        let value = String(data: data[offset..<end], encoding: .utf8)
        offset = data.index(after: end)
        return value
    }

    mutating func readUInt32() throws -> UInt32 {
        guard offset + 4 <= data.count else {
            throw OpenSSHPrivateKeyParserError.invalidEnvelope
        }
        let value = data[offset..<offset + 4].reduce(UInt32(0)) { ($0 << 8) | UInt32($1) }
        offset += 4
        return value
    }

    mutating func readData() throws -> Data {
        let length = Int(try readUInt32())
        guard length >= 0, offset + length <= data.count else {
            throw OpenSSHPrivateKeyParserError.invalidEnvelope
        }
        let value = data[offset..<offset + length]
        offset += length
        return Data(value)
    }

    mutating func readString() throws -> String {
        let data = try readData()
        guard let value = String(data: data, encoding: .utf8) else {
            throw OpenSSHPrivateKeyParserError.invalidEnvelope
        }
        return value
    }
}
