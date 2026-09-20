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

/// The PS/2 set 1 scancode for a USB HID keyboard-page usage, and whether it is an extended key.
///
/// Windows takes scancodes rather than virtual keys for injected input, which is what makes the
/// keys positional: the host's own layout decides which character each position produces, exactly
/// as it would for a keyboard plugged into it. A virtual key would instead mean "the key that
/// types this letter *here*", and a viewer on a different layout would get the wrong one.
///
/// The second half of the pair is the `0xE0` prefix, which Windows carries as a separate flag on
/// the event rather than as part of the code.
pub const fn windows_scancode(usage: u16) -> Option<(u16, bool)> {
    const fn plain(code: u16) -> Option<(u16, bool)> {
        Some((code, false))
    }
    const fn extended(code: u16) -> Option<(u16, bool)> {
        Some((code, true))
    }
    match usage {
        0x04 => plain(0x1E), // A
        0x05 => plain(0x30), // B
        0x06 => plain(0x2E), // C
        0x07 => plain(0x20), // D
        0x08 => plain(0x12), // E
        0x09 => plain(0x21), // F
        0x0A => plain(0x22), // G
        0x0B => plain(0x23), // H
        0x0C => plain(0x17), // I
        0x0D => plain(0x24), // J
        0x0E => plain(0x25), // K
        0x0F => plain(0x26), // L
        0x10 => plain(0x32), // M
        0x11 => plain(0x31), // N
        0x12 => plain(0x18), // O
        0x13 => plain(0x19), // P
        0x14 => plain(0x10), // Q
        0x15 => plain(0x13), // R
        0x16 => plain(0x1F), // S
        0x17 => plain(0x14), // T
        0x18 => plain(0x16), // U
        0x19 => plain(0x2F), // V
        0x1A => plain(0x11), // W
        0x1B => plain(0x2D), // X
        0x1C => plain(0x15), // Y
        0x1D => plain(0x2C), // Z
        0x1E => plain(0x02), // 1
        0x1F => plain(0x03), // 2
        0x20 => plain(0x04), // 3
        0x21 => plain(0x05), // 4
        0x22 => plain(0x06), // 5
        0x23 => plain(0x07), // 6
        0x24 => plain(0x08), // 7
        0x25 => plain(0x09), // 8
        0x26 => plain(0x0A), // 9
        0x27 => plain(0x0B), // 0
        0x28 => plain(0x1C), // Enter
        0x29 => plain(0x01), // Escape
        0x2A => plain(0x0E), // Backspace
        0x2B => plain(0x0F), // Tab
        0x2C => plain(0x39), // Space
        0x2D => plain(0x0C), // -
        0x2E => plain(0x0D), // =
        0x2F => plain(0x1A), // [
        0x30 => plain(0x1B), // ]
        // Backslash and the non-US hash key are one physical position on most keyboards.
        0x31 | 0x32 => plain(0x2B),
        0x33 => plain(0x27),    // ;
        0x34 => plain(0x28),    // '
        0x35 => plain(0x29),    // `
        0x36 => plain(0x33),    // ,
        0x37 => plain(0x34),    // .
        0x38 => plain(0x35),    // /
        0x39 => plain(0x3A),    // Caps Lock
        0x3A => plain(0x3B),    // F1
        0x3B => plain(0x3C),    // F2
        0x3C => plain(0x3D),    // F3
        0x3D => plain(0x3E),    // F4
        0x3E => plain(0x3F),    // F5
        0x3F => plain(0x40),    // F6
        0x40 => plain(0x41),    // F7
        0x41 => plain(0x42),    // F8
        0x42 => plain(0x43),    // F9
        0x43 => plain(0x44),    // F10
        0x44 => plain(0x57),    // F11
        0x45 => plain(0x58),    // F12
        0x46 => extended(0x37), // Print Screen
        0x47 => plain(0x46),    // Scroll Lock
        // Pause is the one key whose scancode is a three-byte sequence rather than a code with a
        // prefix, so it cannot be expressed here and is left unmapped rather than sent wrong.
        0x49 => extended(0x52), // Insert
        0x4A => extended(0x47), // Home
        0x4B => extended(0x49), // Page Up
        0x4C => extended(0x53), // Delete
        0x4D => extended(0x4F), // End
        0x4E => extended(0x51), // Page Down
        0x4F => extended(0x4D), // Right
        0x50 => extended(0x4B), // Left
        0x51 => extended(0x50), // Down
        0x52 => extended(0x48), // Up
        0x53 => plain(0x45),    // Num Lock
        0x54 => extended(0x35), // Keypad /
        0x55 => plain(0x37),    // Keypad *
        0x56 => plain(0x4A),    // Keypad -
        0x57 => plain(0x4E),    // Keypad +
        0x58 => extended(0x1C), // Keypad Enter
        0x59 => plain(0x4F),    // Keypad 1
        0x5A => plain(0x50),    // Keypad 2
        0x5B => plain(0x51),    // Keypad 3
        0x5C => plain(0x4B),    // Keypad 4
        0x5D => plain(0x4C),    // Keypad 5
        0x5E => plain(0x4D),    // Keypad 6
        0x5F => plain(0x47),    // Keypad 7
        0x60 => plain(0x48),    // Keypad 8
        0x61 => plain(0x49),    // Keypad 9
        0x62 => plain(0x52),    // Keypad 0
        0x63 => plain(0x53),    // Keypad .
        0x64 => plain(0x56),    // Non-US backslash
        0x65 => extended(0x5D), // Application
        0xE0 => plain(0x1D),    // Left Control
        0xE1 => plain(0x2A),    // Left Shift
        0xE2 => plain(0x38),    // Left Alt
        0xE3 => extended(0x5B), // Left GUI
        0xE4 => extended(0x1D), // Right Control
        0xE5 => plain(0x36),    // Right Shift
        0xE6 => extended(0x38), // Right Alt
        0xE7 => extended(0x5C), // Right GUI
        _ => None,
    }
}

#[cfg(test)]
mod windows_tests {
    use super::windows_scancode;

    #[test]
    fn the_letters_are_where_a_ps2_keyboard_puts_them() {
        // The top row of a US keyboard, in the order the scancodes run: Q is 0x10 and the rest
        // follow. If these drift, every key on Windows types the wrong character.
        let top = [
            (0x14, 0x10), // Q
            (0x1A, 0x11), // W
            (0x08, 0x12), // E
            (0x15, 0x13), // R
            (0x17, 0x14), // T
        ];
        for (usage, expected) in top {
            assert_eq!(
                windows_scancode(usage),
                Some((expected, false)),
                "{usage:#x}"
            );
        }
    }

    #[test]
    fn the_arrows_are_extended_and_their_keypad_twins_are_not() {
        // These share scancodes; only the extended flag tells them apart. Getting it wrong turns
        // every arrow key into a number.
        for (arrow, keypad, code) in [
            (0x4F, 0x5E, 0x4D), // Right, Keypad 6
            (0x50, 0x5C, 0x4B), // Left, Keypad 4
            (0x51, 0x5A, 0x50), // Down, Keypad 2
            (0x52, 0x60, 0x48), // Up, Keypad 8
        ] {
            assert_eq!(windows_scancode(arrow), Some((code, true)));
            assert_eq!(windows_scancode(keypad), Some((code, false)));
        }
    }

    #[test]
    fn both_control_keys_exist_and_are_told_apart() {
        assert_eq!(windows_scancode(0xE0), Some((0x1D, false)), "left control");
        assert_eq!(windows_scancode(0xE4), Some((0x1D, true)), "right control");
        assert_eq!(windows_scancode(0xE1), Some((0x2A, false)), "left shift");
        assert_eq!(windows_scancode(0xE5), Some((0x36, false)), "right shift");
    }

    #[test]
    fn distinct_keys_share_a_code_only_where_a_keyboard_does() {
        let mut seen = std::collections::HashMap::new();
        for usage in 0..=0xFFu16 {
            let Some(code) = windows_scancode(usage) else {
                continue;
            };
            if let Some(previous) = seen.insert(code, usage) {
                // Backslash and the non-US hash key are one physical position.
                assert_eq!(
                    (previous, usage),
                    (0x31, 0x32),
                    "{previous:#x} and {usage:#x} both map to {code:?}"
                );
            }
        }
    }

    /// The keys one host can take and the other cannot, named rather than discovered.
    ///
    /// A key that works on one host and silently does nothing on another is the kind of gap a
    /// person only finds mid-sentence, so the difference is written down and asserted rather than
    /// left to whichever table was edited last. Everything here is a key a PC keyboard does not
    /// have: `F13` upwards, Apple's `Help`, and the keypad `=` that only Apple keypads carry.
    const MAC_ONLY: [u16; 10] = [
        0x67, // Keypad =
        0x68, // F13
        0x69, // F14
        0x6A, // F15
        0x6B, // F16
        0x6C, // F17
        0x6D, // F18
        0x6E, // F19
        0x6F, // F20
        0x75, // Help
    ];

    #[test]
    fn the_two_hosts_differ_only_where_the_keyboards_do() {
        for usage in 0..=0xFFu16 {
            let mac = super::mac_virtual_keycode(usage).is_some();
            let windows = windows_scancode(usage).is_some();
            if mac && !windows {
                assert!(
                    MAC_ONLY.contains(&usage),
                    "{usage:#x} works on macOS but not on Windows, and is not a key only Apple \
                     keyboards have — either map it or add it to MAC_ONLY with a reason"
                );
            }
            if windows && !mac {
                // The other direction: keys a PC keyboard has and an Apple one does not. Print
                // Screen and Scroll Lock have no macOS event at all, and Menu is the key between
                // the right Alt and Control that Apple never shipped.
                const WINDOWS_ONLY: [u16; 3] = [
                    0x46, // Print Screen
                    0x47, // Scroll Lock
                    0x65, // Application, the Menu key
                ];
                assert!(
                    WINDOWS_ONLY.contains(&usage),
                    "{usage:#x} works on Windows but not on macOS, and is not a key only PC \
                     keyboards have"
                );
            }
        }
    }

    #[test]
    fn every_key_a_phone_can_send_reaches_both_hosts() {
        // The accessory row the mobile clients actually offer. These are the keys a person taps
        // on a phone, so a gap here is one they would hit immediately.
        for usage in [
            0x29, // Escape
            0x2B, // Tab
            0xE0, // Control
            0x50, // Left
            0x52, // Up
            0x51, // Down
            0x4F, // Right
            0x31, // Backslash, which the pipe key uses
            0x2D, // Minus
        ] {
            assert!(windows_scancode(usage).is_some(), "Windows: {usage:#x}");
            assert!(
                super::mac_virtual_keycode(usage).is_some(),
                "macOS: {usage:#x}"
            );
        }
    }
}

/// The Linux input event code (`KEY_*`, `linux/input-event-codes.h`) for a USB HID keyboard-page
/// usage.
///
/// Positional, like the other two: the device this crate creates reports key presses, and the
/// compositor's own layout decides which character each one produces.
///
/// Linux names more keys than either of the others — it has `F13` upwards like macOS, the Menu
/// key like Windows, and `Pause`, which neither of the other two can express. So this table is a
/// superset of both, and the tests check that rather than assume it.
pub const fn linux_keycode(usage: u16) -> Option<u16> {
    Some(match usage {
        0x04 => 30, // A
        0x05 => 48, // B
        0x06 => 46, // C
        0x07 => 32, // D
        0x08 => 18, // E
        0x09 => 33, // F
        0x0A => 34, // G
        0x0B => 35, // H
        0x0C => 23, // I
        0x0D => 36, // J
        0x0E => 37, // K
        0x0F => 38, // L
        0x10 => 50, // M
        0x11 => 49, // N
        0x12 => 24, // O
        0x13 => 25, // P
        0x14 => 16, // Q
        0x15 => 19, // R
        0x16 => 31, // S
        0x17 => 20, // T
        0x18 => 22, // U
        0x19 => 47, // V
        0x1A => 17, // W
        0x1B => 45, // X
        0x1C => 21, // Y
        0x1D => 44, // Z
        0x1E => 2,  // 1
        0x1F => 3,  // 2
        0x20 => 4,  // 3
        0x21 => 5,  // 4
        0x22 => 6,  // 5
        0x23 => 7,  // 6
        0x24 => 8,  // 7
        0x25 => 9,  // 8
        0x26 => 10, // 9
        0x27 => 11, // 0
        0x28 => 28, // Enter
        0x29 => 1,  // Escape
        0x2A => 14, // Backspace
        0x2B => 15, // Tab
        0x2C => 57, // Space
        0x2D => 12, // -
        0x2E => 13, // =
        0x2F => 26, // [
        0x30 => 27, // ]
        // Backslash and the non-US hash key are one position, as they are everywhere else.
        0x31 | 0x32 => 43,
        0x33 => 39,  // ;
        0x34 => 40,  // '
        0x35 => 41,  // `
        0x36 => 51,  // ,
        0x37 => 52,  // .
        0x38 => 53,  // /
        0x39 => 58,  // Caps Lock
        0x3A => 59,  // F1
        0x3B => 60,  // F2
        0x3C => 61,  // F3
        0x3D => 62,  // F4
        0x3E => 63,  // F5
        0x3F => 64,  // F6
        0x40 => 65,  // F7
        0x41 => 66,  // F8
        0x42 => 67,  // F9
        0x43 => 68,  // F10
        0x44 => 87,  // F11
        0x45 => 88,  // F12
        0x46 => 99,  // Print Screen (SysRq)
        0x47 => 70,  // Scroll Lock
        0x48 => 119, // Pause, which only Linux can express
        0x49 => 110, // Insert
        0x4A => 102, // Home
        0x4B => 104, // Page Up
        0x4C => 111, // Delete
        0x4D => 107, // End
        0x4E => 109, // Page Down
        0x4F => 106, // Right
        0x50 => 105, // Left
        0x51 => 108, // Down
        0x52 => 103, // Up
        0x53 => 69,  // Num Lock
        0x54 => 98,  // Keypad /
        0x55 => 55,  // Keypad *
        0x56 => 74,  // Keypad -
        0x57 => 78,  // Keypad +
        0x58 => 96,  // Keypad Enter
        0x59 => 79,  // Keypad 1
        0x5A => 80,  // Keypad 2
        0x5B => 81,  // Keypad 3
        0x5C => 75,  // Keypad 4
        0x5D => 76,  // Keypad 5
        0x5E => 77,  // Keypad 6
        0x5F => 71,  // Keypad 7
        0x60 => 72,  // Keypad 8
        0x61 => 73,  // Keypad 9
        0x62 => 82,  // Keypad 0
        0x63 => 83,  // Keypad .
        0x64 => 86,  // Non-US backslash
        0x65 => 127, // Application, the Menu key
        0x67 => 117, // Keypad =
        0x68 => 183, // F13
        0x69 => 184, // F14
        0x6A => 185, // F15
        0x6B => 186, // F16
        0x6C => 187, // F17
        0x6D => 188, // F18
        0x6E => 189, // F19
        0x6F => 190, // F20
        0x75 => 138, // Help
        0xE0 => 29,  // Left Control
        0xE1 => 42,  // Left Shift
        0xE2 => 56,  // Left Alt
        0xE3 => 125, // Left Meta
        0xE4 => 97,  // Right Control
        0xE5 => 54,  // Right Shift
        0xE6 => 100, // Right Alt
        0xE7 => 126, // Right Meta
        _ => return None,
    })
}

#[cfg(test)]
mod linux_tests {
    use super::{linux_keycode, mac_virtual_keycode, windows_scancode};

    #[test]
    fn the_letters_are_where_the_kernel_puts_them() {
        // KEY_Q through KEY_T, 16..20, which is the same order as the PS/2 codes they came from.
        for (usage, expected) in [
            (0x14, 16), // Q
            (0x1A, 17), // W
            (0x08, 18), // E
            (0x15, 19), // R
            (0x17, 20), // T
        ] {
            assert_eq!(linux_keycode(usage), Some(expected), "{usage:#x}");
        }
    }

    #[test]
    fn linux_can_press_everything_the_other_two_can() {
        // The kernel names more keys than either desktop: F13 upwards like macOS, the Menu key
        // like Windows, and Pause, which neither of the others can express. So anything the other
        // two can send has to work here, and a gap would mean a key that works on two hosts and
        // silently does nothing on the third.
        for usage in 0..=0xFFu16 {
            if mac_virtual_keycode(usage).is_some() {
                assert!(
                    linux_keycode(usage).is_some(),
                    "{usage:#x} works on macOS but not on Linux"
                );
            }
            if windows_scancode(usage).is_some() {
                assert!(
                    linux_keycode(usage).is_some(),
                    "{usage:#x} works on Windows but not on Linux"
                );
            }
        }
    }

    #[test]
    fn the_key_neither_desktop_could_express_is_here() {
        // Pause: a three-byte scancode sequence on Windows and no event at all on macOS, but an
        // ordinary key code to the kernel.
        assert_eq!(linux_keycode(0x48), Some(119));
        assert_eq!(windows_scancode(0x48), None);
        assert_eq!(mac_virtual_keycode(0x48), None);
    }

    #[test]
    fn distinct_keys_share_a_code_only_where_a_keyboard_does() {
        let mut seen = std::collections::HashMap::new();
        for usage in 0..=0xFFu16 {
            let Some(code) = linux_keycode(usage) else {
                continue;
            };
            if let Some(previous) = seen.insert(code, usage) {
                // Backslash and the non-US hash key are one physical position.
                assert_eq!(
                    (previous, usage),
                    (0x31, 0x32),
                    "{previous:#x} and {usage:#x} both map to {code}"
                );
            }
        }
    }

    #[test]
    fn every_key_a_phone_can_send_reaches_linux_too() {
        for usage in [0x29, 0x2B, 0xE0, 0x50, 0x52, 0x51, 0x4F, 0x31, 0x2D] {
            assert!(linux_keycode(usage).is_some(), "{usage:#x}");
        }
    }
}
