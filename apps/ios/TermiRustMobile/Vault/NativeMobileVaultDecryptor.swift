import Foundation
import TermiRustMobileCrypto

enum NativeMobileVaultDecryptorError: Error, LocalizedError {
    case decryptFailed(String)

    var errorDescription: String? {
        switch self {
        case .decryptFailed(let message):
            return message
        }
    }
}

struct NativeMobileVaultDecryptor: MobileVaultDecrypting {
    func decrypt(encryptedVaultData: Data, passphrase: String) throws -> Data {
        var passphraseData = Data(passphrase.utf8)
        defer {
            passphraseData.resetBytes(in: 0..<passphraseData.count)
        }
        let result = encryptedVaultData.withUnsafeBytes { encryptedBytes in
            passphraseData.withUnsafeBytes { passphraseBytes in
                termirust_mobile_decrypt_vault_json(
                    encryptedBytes.bindMemory(to: UInt8.self).baseAddress,
                    encryptedVaultData.count,
                    passphraseBytes.bindMemory(to: UInt8.self).baseAddress,
                    passphraseData.count
                )
            }
        }
        defer {
            termirust_mobile_free_result(result)
        }

        if result.ok {
            return data(from: result.data)
        }

        let message = String(data: data(from: result.error), encoding: .utf8)
            ?? "Unable to decrypt encrypted mobile vault."
        throw NativeMobileVaultDecryptorError.decryptFailed(message)
    }

    private func data(from buffer: TermiRustMobileByteBuffer) -> Data {
        guard let ptr = buffer.ptr, buffer.len > 0 else {
            return Data()
        }
        return Data(bytes: ptr, count: buffer.len)
    }
}
