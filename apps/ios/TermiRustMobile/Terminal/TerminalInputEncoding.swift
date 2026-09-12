import Foundation

func encodeTerminalInput(_ input: String, control: Bool, option: Bool) -> Data {
    let payload: Data
    if control || option {
        payload = control ? controlEncodedInput(input) ?? Data(input.utf8) : Data(input.utf8)
    } else {
        payload = Data((input + "\n").utf8)
    }

    guard option else {
        return payload
    }

    var data = Data([0x1B])
    data.append(payload)
    return data
}

private func controlEncodedInput(_ input: String) -> Data? {
    guard input.unicodeScalars.count == 1, let scalar = input.unicodeScalars.first else {
        return nil
    }
    let value = scalar.value
    guard value < 128 else {
        return nil
    }
    let upper = UInt8(value).uppercasedAscii
    guard upper >= 0x40, upper <= 0x5F else {
        return nil
    }
    return Data([upper & 0x1F])
}

private extension UInt8 {
    var uppercasedAscii: UInt8 {
        if self >= 0x61, self <= 0x7A {
            return self - 0x20
        }
        return self
    }
}
