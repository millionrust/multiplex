/// The macOS virtual key code (`kVK_*`, Carbon `Events.h`) for a USB HID keyboard-page usage.
///
/// Keys are positional: the host's keyboard layout decides which character a key produces, just
/// as it would for a keyboard plugged into the host. Media keys are not keyboard events on macOS
/// and have no mapping.
pub const fn mac_virtual_keycode(usage: u16) -> Option<u16> {
    Some(match usage {
        0x04 => 0x00, // A
        0x05 => 0x0B, // B
        0x06 => 0x08, // C
        0x07 => 0x02, // D
        0x08 => 0x0E, // E
        0x09 => 0x03, // F
        0x0A => 0x05, // G
        0x0B => 0x04, // H
        0x0C => 0x22, // I
        0x0D => 0x26, // J
        0x0E => 0x28, // K
        0x0F => 0x25, // L
        0x10 => 0x2E, // M
        0x11 => 0x2D, // N
        0x12 => 0x1F, // O
        0x13 => 0x23, // P
        0x14 => 0x0C, // Q
        0x15 => 0x0F, // R
        0x16 => 0x01, // S
        0x17 => 0x11, // T
        0x18 => 0x20, // U
        0x19 => 0x09, // V
        0x1A => 0x0D, // W
        0x1B => 0x07, // X
        0x1C => 0x10, // Y
        0x1D => 0x06, // Z
        0x1E => 0x12, // 1
        0x1F => 0x13, // 2
        0x20 => 0x14, // 3
        0x21 => 0x15, // 4
        0x22 => 0x17, // 5
        0x23 => 0x16, // 6
        0x24 => 0x1A, // 7
        0x25 => 0x1C, // 8
        0x26 => 0x19, // 9
        0x27 => 0x1D, // 0
        0x28 => 0x24, // Return
        0x29 => 0x35, // Escape
        0x2A => 0x33, // Backspace (Delete)
        0x2B => 0x30, // Tab
        0x2C => 0x31, // Space
        0x2D => 0x1B, // - _
        0x2E => 0x18, // = +
        0x2F => 0x21, // [ {
        0x30 => 0x1E, // ] }
        0x31 => 0x2A, // \ |
        0x32 => 0x2A, // Non-US # ~, the same key position as \ on Apple ISO keyboards
        0x33 => 0x29, // ; :
        0x34 => 0x27, // ' "
        0x35 => 0x32, // ` ~
        0x36 => 0x2B, // , <
        0x37 => 0x2F, // . >
        0x38 => 0x2C, // / ?
        0x39 => 0x39, // Caps Lock
        0x3A => 0x7A, // F1
        0x3B => 0x78, // F2
        0x3C => 0x63, // F3
        0x3D => 0x76, // F4
        0x3E => 0x60, // F5
        0x3F => 0x61, // F6
        0x40 => 0x62, // F7
        0x41 => 0x64, // F8
        0x42 => 0x65, // F9
        0x43 => 0x6D, // F10
        0x44 => 0x67, // F11
        0x45 => 0x6F, // F12
        0x49 => 0x72, // Insert, which is Help on Apple keyboards
        0x4A => 0x73, // Home
        0x4B => 0x74, // Page Up
        0x4C => 0x75, // Forward Delete
        0x4D => 0x77, // End
        0x4E => 0x79, // Page Down
        0x4F => 0x7C, // Right Arrow
        0x50 => 0x7B, // Left Arrow
        0x51 => 0x7D, // Down Arrow
        0x52 => 0x7E, // Up Arrow
        0x53 => 0x47, // Num Lock, which is Clear on Apple keypads
        0x54 => 0x4B, // Keypad /
        0x55 => 0x43, // Keypad *
        0x56 => 0x4E, // Keypad -
        0x57 => 0x45, // Keypad +
        0x58 => 0x4C, // Keypad Enter
        0x59 => 0x53, // Keypad 1
        0x5A => 0x54, // Keypad 2
        0x5B => 0x55, // Keypad 3
        0x5C => 0x56, // Keypad 4
        0x5D => 0x57, // Keypad 5
        0x5E => 0x58, // Keypad 6
        0x5F => 0x59, // Keypad 7
        0x60 => 0x5B, // Keypad 8
        0x61 => 0x5C, // Keypad 9
        0x62 => 0x52, // Keypad 0
        0x63 => 0x41, // Keypad .
        0x64 => 0x0A, // Non-US \ |, the § key on Apple ISO keyboards
        0x67 => 0x51, // Keypad =
        0x68 => 0x69, // F13
        0x69 => 0x6B, // F14
        0x6A => 0x71, // F15
        0x6B => 0x6A, // F16
        0x6C => 0x40, // F17
        0x6D => 0x4F, // F18
        0x6E => 0x50, // F19
        0x6F => 0x5A, // F20
        0x75 => 0x72, // Help
        0xE0 => 0x3B, // Left Control
        0xE1 => 0x38, // Left Shift
        0xE2 => 0x3A, // Left Option
        0xE3 => 0x37, // Left Command
        0xE4 => 0x3E, // Right Control
        0xE5 => 0x3C, // Right Shift
        0xE6 => 0x3D, // Right Option
        0xE7 => 0x36, // Right Command
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_follow_the_ansi_layout() {
        let letters = b"abcdefghijklmnopqrstuvwxyz";
        let expected = [
            0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D,
            0x1F, 0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
        ];
        for (offset, (letter, code)) in letters.iter().zip(expected).enumerate() {
            assert_eq!(
                mac_virtual_keycode(0x04 + offset as u16),
                Some(code),
                "{}",
                *letter as char
            );
        }
    }

    #[test]
    fn distinct_keys_never_share_a_code() {
        let mut seen = std::collections::HashMap::new();
        for usage in 0..=0xFFu16 {
            if let Some(code) = mac_virtual_keycode(usage)
                && let Some(previous) = seen.insert(code, usage)
            {
                // Two HID usages name one physical position on Apple keyboards.
                let aliases = [(0x31, 0x32), (0x49, 0x75)];
                assert!(
                    aliases.contains(&(previous, usage)),
                    "{previous:#x} and {usage:#x} both map to {code:#x}"
                );
            }
        }
    }

    #[test]
    fn media_and_unknown_usages_are_unmapped() {
        for usage in [0x00, 0x01, 0x46, 0x47, 0x48, 0x7F, 0x80, 0x81, 0xE8, 0x100] {
            assert_eq!(mac_virtual_keycode(usage), None, "{usage:#x}");
        }
    }
}
