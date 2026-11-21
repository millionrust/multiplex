//! Ctrl-] then d, typed in an attached window, detaches it from its Session; `session attach`
//! and `controller ssh attach` also detach on Ctrl-C.
//!
//! The window's input is forwarded byte for byte, because a terminal sends far more than keys:
//! mouse and focus reports, pastes, and its answers to the program's own questions (cursor
//! position, device attributes, mode reports, colours, its name). Parsing that stream as key
//! events drops whatever the parser does not know, and a parser that waits for the end of a
//! sequence it cannot read swallows every key after it. So this filter looks for the detach keys
//! only, in each encoding a terminal may send them in, and leaves every other byte as it arrived:
//! a Ctrl-] not followed by d reaches the program, and nothing inside a bracketed paste is a key.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CTRL_C: u8 = 0x03;
const CTRL_RIGHT_BRACKET: u8 = 0x1d;
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
/// Kitty's modifier bits are one more than the value sent; Caps Lock and Num Lock are reported
/// too, and neither changes which key was meant.
const LOCK_MODIFIERS: u32 = 64 | 128;
const SHIFT: u32 = 1;
const CONTROL: u32 = 4;
/// Kitty's codes for the modifier keys themselves (Shift, Control, Alt, Super, Hyper, Meta, left
/// and right, then ISO level 3 and 5 shift), which it reports when every key is reported.
const MODIFIER_KEYS: std::ops::RangeInclusive<u32> = 57_441..=57_454;

#[derive(Debug, Default)]
pub(crate) struct DetachKeys {
    /// The leader and any release reports that followed it, held until the next key decides.
    pending: Option<Vec<u8>>,
    paste: PasteTracker,
    /// Ctrl-C detaches too, for an attach that inspects a Session rather than standing in for
    /// its terminal.
    ctrl_c: bool,
}

impl DetachKeys {
    /// Ctrl-] then d detaches, and so does Ctrl-C.
    pub(crate) fn with_ctrl_c() -> Self {
        Self {
            ctrl_c: true,
            ..Self::default()
        }
    }

    /// The bytes to send on, and whether the detach keys were typed. Bytes after the detach keys
    /// in the same read are dropped with the window.
    pub(crate) fn filter(&mut self, input: &[u8]) -> (Vec<u8>, bool) {
        let mut output = Vec::with_capacity(input.len());
        let mut index = 0;
        while index < input.len() {
            let token = next_token(&input[index..]);
            index += token.len();
            let in_paste = self.paste.in_paste();
            self.paste.feed(token);
            if in_paste || self.paste.in_paste() {
                self.flush(&mut output);
                output.extend_from_slice(token);
                continue;
            }
            let key = Key::of(token);
            if key == Key::CtrlC && self.ctrl_c {
                return (output, true);
            }
            if let Some(pending) = self.pending.as_mut() {
                match key {
                    Key::Detach => return (output, true),
                    Key::Release | Key::Modifier | Key::Report => pending.extend_from_slice(token),
                    Key::Leader | Key::CtrlC | Key::Other => {
                        self.flush(&mut output);
                        output.extend_from_slice(token);
                    }
                }
            } else if key == Key::Leader {
                self.pending = Some(token.to_vec());
            } else {
                output.extend_from_slice(token);
            }
        }
        (output, false)
    }

    fn flush(&mut self, output: &mut Vec<u8>) {
        if let Some(pending) = self.pending.take() {
            output.extend_from_slice(&pending);
        }
    }
}

/// One control sequence or string (OSC, DCS, APC, PM, SOS) when a whole one starts here, ESC and
/// the byte after it when Alt is held with a key, otherwise one byte. A sequence split across reads
/// is passed on byte by byte, which only means a detach key or report sent that way is not seen.
fn next_token(input: &[u8]) -> &[u8] {
    if input.len() >= 3 && input[0] == ESC && matches!(input[1], b']' | b'P' | b'_' | b'^' | b'X') {
        let mut index = 2;
        while index < input.len() {
            match input[index] {
                BEL if input[1] == b']' => return &input[..=index],
                ESC if input.get(index + 1) == Some(&b'\\') => return &input[..index + 2],
                ESC => break,
                _ => index += 1,
            }
        }
    }
    if input.len() >= 3 && input[0] == ESC && input[1] == b'[' {
        for (offset, byte) in input[2..].iter().enumerate() {
            match byte {
                0x20..=0x3f => {}
                0x40..=0x7e => return &input[..offset + 3],
                _ => break,
            }
        }
    }
    // Alt-Ctrl-C and Alt-Ctrl-] are not Ctrl-C and Ctrl-].
    if input.len() >= 2 && input[0] == ESC && input[1] != b'[' {
        return &input[..2];
    }
    &input[..1]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Leader,
    Detach,
    CtrlC,
    /// A key-release report, which kitty's protocol sends between a key and the next one.
    Release,
    /// A modifier key pressed on its own, as Shift is on the way to typing D.
    Modifier,
    /// Something the terminal reports rather than a key: focus, a mouse report, or its answer to
    /// a program's question. Programs that ask often and track the mouse would otherwise put one
    /// between Ctrl-] and d and turn the d into typing.
    Report,
    Other,
}

impl Key {
    fn of(token: &[u8]) -> Self {
        match token {
            [CTRL_RIGHT_BRACKET] => return Self::Leader,
            [CTRL_C] => return Self::CtrlC,
            b"d" | b"D" => return Self::Detach,
            _ => {}
        }
        if is_report(token) {
            return Self::Report;
        }
        let Some(key) = EncodedKey::parse(token) else {
            return Self::Other;
        };
        if key.event == 3 {
            return Self::Release;
        }
        if MODIFIER_KEYS.contains(&key.code) {
            return Self::Modifier;
        }
        match (key.code, key.modifiers & !LOCK_MODIFIERS) {
            (93, CONTROL) => Self::Leader,
            (99, CONTROL) => Self::CtrlC,
            (100, 0 | SHIFT) => Self::Detach,
            _ => Self::Other,
        }
    }
}

/// Whether a token is a terminal report no key is ever sent as. The one ambiguity, a cursor
/// position at row 1 against Shift-F3 (`CSI 1 ; 2 R`), only delays that key while a detach is
/// pending. X10 mouse reports (`CSI M` and three raw bytes) are left out: their bytes can be d.
fn is_report(token: &[u8]) -> bool {
    if token.len() > 2 && token[0] == ESC && token[1] != b'[' {
        // Only whole strings are tokens this long; Alt and a key is two bytes.
        return true;
    }
    let Some(body) = token.strip_prefix(b"\x1b[") else {
        return false;
    };
    let Some((&last, parameters)) = body.split_last() else {
        return false;
    };
    if matches!(parameters.first(), Some(b'?' | b'>' | b'=' | b'<'))
        || parameters.iter().any(|byte| (0x20..=0x2f).contains(byte))
    {
        return true;
    }
    match last {
        b'I' | b'O' => parameters.is_empty(),
        b'R' | b'n' | b't' => true,
        b'M' => !parameters.is_empty(),
        _ => false,
    }
}

/// A key in kitty's `CSI code ; modifiers : event u` form or xterm's modifyOtherKeys
/// `CSI 27 ; modifiers ; code ~` form, with modifiers as bits rather than the value sent.
struct EncodedKey {
    code: u32,
    modifiers: u32,
    event: u32,
}

impl EncodedKey {
    fn parse(token: &[u8]) -> Option<Self> {
        let body = token.strip_prefix(b"\x1b[")?;
        let (&last, parameters) = body.split_last()?;
        let parameters = std::str::from_utf8(parameters).ok()?;
        let mut fields = parameters.split(';');
        let number = |text: Option<&str>| -> Option<u32> {
            match text {
                None | Some("") => Some(1),
                Some(text) => text.parse().ok(),
            }
        };
        match last {
            b'u' => {
                // The first field may carry shifted and base-layout alternates after colons.
                let code = fields.next()?.split(':').next()?.parse().ok()?;
                let mut modifier = fields.next().unwrap_or("").split(':');
                let modifiers = number(modifier.next())?.checked_sub(1)?;
                let event = number(modifier.next())?;
                Some(Self {
                    code,
                    modifiers,
                    event,
                })
            }
            b'~' if fields.next()? == "27" => {
                let modifiers = number(fields.next())?.checked_sub(1)?;
                let code = fields.next()?.parse().ok()?;
                fields.next().is_none().then_some(Self {
                    code,
                    modifiers,
                    event: 1,
                })
            }
            _ => None,
        }
    }
}

/// Whether the bytes so far are inside a bracketed paste, followed byte by byte so a marker
/// split across reads still counts.
#[derive(Debug, Default)]
struct PasteTracker {
    in_paste: bool,
    matched: usize,
}

impl PasteTracker {
    fn in_paste(&self) -> bool {
        self.in_paste
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let marker = if self.in_paste {
                PASTE_END
            } else {
                PASTE_START
            };
            self.matched = if byte == marker[self.matched] {
                self.matched + 1
            } else {
                usize::from(byte == marker[0])
            };
            if self.matched == marker.len() {
                self.in_paste = !self.in_paste;
                self.matched = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> (Vec<u8>, bool) {
        run_with(DetachKeys::default(), chunks)
    }

    fn run_with(mut keys: DetachKeys, chunks: &[&[u8]]) -> (Vec<u8>, bool) {
        let mut sent = Vec::new();
        for chunk in chunks {
            let (bytes, detached) = keys.filter(chunk);
            sent.extend(bytes);
            if detached {
                return (sent, true);
            }
        }
        (sent, false)
    }

    /// Everything a terminal sends that is not the detach keys, including the answers that used
    /// to stop every key after them: a mode report, a DCS name, and an OSC colour ended by BEL.
    const PASSED_THROUGH: &[&[u8]] = &[
        b"hello world 123 !@#$%^&*()",
        "é₹👩‍💻漢字e\u{301}".as_bytes(),
        b"\r",
        b"\t",
        b"\x1b[Z",
        b"\x7f",
        b"\x08",
        b"\x1b",
        b"\x1bx",
        b"\x00",
        b"\x03\x04\x1a\x1c\x11\x13\x0c\x12\x15\x17",
        b"\x1b[A\x1b[B\x1b[C\x1b[D",
        b"\x1bOA\x1bOB\x1bOC\x1bOD",
        b"\x1b[H\x1b[F\x1bOH\x1bOF\x1b[1~\x1b[4~",
        b"\x1b[5~\x1b[6~\x1b[2~\x1b[3~",
        b"\x1bOP\x1bOQ\x1bOR\x1bOS\x1b[15~\x1b[24~",
        b"\x1b[1;5A\x1b[1;3D\x1b[15;2~",
        b"\x1b[97;5u\x1b[13u\x1b[97;1:3u\x1b[1;1:3A\x1b[100;5u",
        b"\x1b[27;5;13~",
        b"\x1b[I\x1b[O",
        b"\x1b[<0;10;5M\x1b[<0;10;5m\x1b[<32;11;5M\x1b[<64;10;5M\x1b[<65;10;5M",
        b"\x1b[M !!",
        b"\x1b[0;10;5M",
        b"\x1b[12;40R",
        b"\x1b[?62;22c\x1b[>1;10;0c",
        b"\x1b[?2026;2$y\x1b[2;1$y",
        b"\x1bP>|Terminal 2.14\x1b\\",
        b"\x1bP1$r0m\x1b\\\x1bP1+r544e=787465726d\x1b\\",
        b"\x1b]10;rgb:0000/0000/0000\x1b\\\x1b]11;rgb:ffff/ffff/ffff\x07",
        b"\x1b]4;1;rgb:cdcd/0000/0000\x07\x1b]52;c;aGk=\x07",
        b"\x1b[?0u\x1b[?7u",
        b"\x1b_Gi=1;OK\x1b\\",
        b"\x1b[8;24;80t\x1b[4;480;800t\x1b[?997;1n\x1b[0n",
        b"\x1b[200~pasted\r\nlines\x1b[201~",
    ];

    #[test]
    fn every_other_byte_reaches_the_program_unchanged() {
        let all = PASSED_THROUGH.concat();
        assert_eq!(run(&[&all]), (all.clone(), false));
        let bytes: Vec<&[u8]> = all.chunks(1).collect();
        assert_eq!(run(&bytes), (all, false));
        for chunk in PASSED_THROUGH {
            assert_eq!(run(&[chunk]), (chunk.to_vec(), false), "{chunk:?}");
        }
    }

    #[test]
    fn ctrl_right_bracket_then_d_detaches_in_every_encoding() {
        for leader in [
            b"\x1d".as_slice(),
            b"\x1b[93;5u",
            b"\x1b[93;5:1u",
            b"\x1b[93;69u",
            b"\x1b[93:125;5u",
            b"\x1b[27;5;93~",
        ] {
            for detach in [
                b"d".as_slice(),
                b"D",
                b"\x1b[100u",
                b"\x1b[100;1u",
                b"\x1b[100;2u",
                b"\x1b[100;1:1u",
                b"\x1b[100;65u",
            ] {
                let together = [b"before".as_slice(), leader, detach, b"after"].concat();
                assert_eq!(
                    run(&[&together]),
                    (b"before".to_vec(), true),
                    "{leader:?} {detach:?}"
                );
                assert_eq!(
                    run(&[b"before", leader, detach, b"after"]),
                    (b"before".to_vec(), true),
                    "{leader:?} then {detach:?} in separate reads"
                );
            }
        }
    }

    #[test]
    fn ctrl_c_detaches_only_when_asked_to_and_never_inside_a_paste() {
        for ctrl_c in [
            b"\x03".as_slice(),
            b"\x1b[99;5u",
            b"\x1b[99;69u",
            b"\x1b[27;5;99~",
        ] {
            let typed = [b"ls".as_slice(), ctrl_c, b"after"].concat();
            assert_eq!(
                run_with(DetachKeys::with_ctrl_c(), &[&typed]),
                (b"ls".to_vec(), true),
                "{ctrl_c:?}"
            );
            assert_eq!(run(&[&typed]), (typed.clone(), false), "{ctrl_c:?}");
            let pasted = [b"\x1b[200~".as_slice(), ctrl_c, b"\x1b[201~"].concat();
            assert_eq!(
                run_with(DetachKeys::with_ctrl_c(), &[&pasted]),
                (pasted.clone(), false),
                "{ctrl_c:?}"
            );
        }
        assert_eq!(
            run_with(DetachKeys::with_ctrl_c(), &[b"\x1d", b"\x03"]),
            (Vec::new(), true)
        );
        // Released, or with Shift as Ctrl-Shift-C copies in many terminals, it is not Ctrl-C.
        for other in [b"\x1b[99;5:3u".as_slice(), b"\x1b[99;6u"] {
            assert_eq!(
                run_with(DetachKeys::with_ctrl_c(), &[other]),
                (other.to_vec(), false)
            );
        }
    }

    #[test]
    fn release_reports_between_the_keys_do_not_stop_a_detach() {
        assert_eq!(
            run(&[b"\x1b[93;5u", b"\x1b[93;5:3u", b"\x1b[57442;5:3u", b"d"]),
            (Vec::new(), true)
        );
    }

    /// With every key reported, Shift arrives on its own before the D it shifts.
    #[test]
    fn a_modifier_pressed_between_the_keys_does_not_stop_a_detach() {
        assert_eq!(
            run(&[
                b"\x1b[93;5u",
                b"\x1b[93;5:3u",
                b"\x1b[57442;5:3u",
                b"\x1b[57441;2u",
                b"\x1b[100;2u",
            ]),
            (Vec::new(), true)
        );
        assert_eq!(
            run(&[b"\x1b[93;5u\x1b[57441;2u\x1b[57441;1:3ux"]),
            (b"\x1b[93;5u\x1b[57441;2u\x1b[57441;1:3ux".to_vec(), false)
        );
        // Outside a pending detach a modifier report is like any other key.
        assert_eq!(run(&[b"\x1b[57441;2u"]), (b"\x1b[57441;2u".to_vec(), false));
    }

    /// Codex and Claude Code ask the terminal questions and track the mouse, so answers and
    /// reports can arrive between the two keys.
    #[test]
    fn terminal_reports_between_the_keys_do_not_stop_a_detach() {
        let reports: &[&[u8]] = &[
            b"\x1b[I",
            b"\x1b[O",
            b"\x1b[<35;20;9M",
            b"\x1b[<0;10;5m",
            b"\x1b[0;10;5M",
            b"\x1b[12;40R",
            b"\x1b[?62;22c",
            b"\x1b[>1;10;0c",
            b"\x1b[?2026;2$y",
            b"\x1b[2;1$y",
            b"\x1b[?0u",
            b"\x1b[?997;1n",
            b"\x1b[0n",
            b"\x1b[8;24;80t",
            b"\x1bP>|Terminal 2.14\x1b\\",
            b"\x1b]11;rgb:ffff/ffff/ffff\x07",
            b"\x1b]10;rgb:0000/0000/0000\x1b\\",
            b"\x1b_Gi=1;OK\x1b\\",
        ];
        for report in reports {
            assert_eq!(
                run(&[b"\x1d", report, b"d"]),
                (Vec::new(), true),
                "{report:?}"
            );
            assert_eq!(
                run(&[&[b"\x1d".as_slice(), report, b"d"].concat()]),
                (Vec::new(), true),
                "{report:?} in one read"
            );
            // Held reports reach the program, in order, when the leader turns out to be typing.
            assert_eq!(
                run(&[b"\x1d", report, b"x"]),
                ([b"\x1d".as_slice(), report, b"x"].concat(), false),
                "{report:?}"
            );
        }
        let all = reports.concat();
        assert_eq!(
            run(&[&[b"\x1d".as_slice(), &all, b"d"].concat()]),
            (Vec::new(), true)
        );
    }

    /// An X10 mouse report's raw bytes may be a d; it is not held, so it cannot become one.
    #[test]
    fn an_x10_mouse_report_after_the_leader_is_typing() {
        assert_eq!(run(&[b"\x1d\x1b[Md!!"]), (b"\x1d\x1b[Md!!".to_vec(), false));
    }

    /// An OSC or DCS that never ends in the read is Alt and a key, then text.
    #[test]
    fn an_unfinished_string_is_not_held() {
        assert_eq!(
            run(&[b"\x1d\x1b]11;rgb", b"d"]),
            (b"\x1d\x1b]11;rgbd".to_vec(), false)
        );
    }

    #[test]
    fn alt_with_the_detach_keys_is_another_key() {
        for alt in [b"\x1b\x03".as_slice(), b"\x1b\x1dd", b"\x1b\x1d\x1b\x03"] {
            assert_eq!(
                run_with(DetachKeys::with_ctrl_c(), &[alt]),
                (alt.to_vec(), false),
                "{alt:?}"
            );
        }
        // Escape on its own in one read and Ctrl-C in the next are two keys.
        assert_eq!(
            run_with(DetachKeys::with_ctrl_c(), &[b"\x1b", b"\x03"]),
            (b"\x1b".to_vec(), true)
        );
    }

    #[test]
    fn a_leader_followed_by_anything_else_reaches_the_program_with_it() {
        assert_eq!(run(&[b"\x1dx"]), (b"\x1dx".to_vec(), false));
        assert_eq!(run(&[b"\x1d", b"\x1b[A"]), (b"\x1d\x1b[A".to_vec(), false));
        assert_eq!(run(&[b"\x1d\x1d"]), (b"\x1d\x1d".to_vec(), false));
        assert_eq!(run(&[b"\x1d\x1dd"]), (b"\x1d\x1dd".to_vec(), false));
        assert_eq!(
            run(&[b"\x1b[93;5u", b"\x1b[93;5:3u", b"q"]),
            (b"\x1b[93;5u\x1b[93;5:3uq".to_vec(), false)
        );
        assert_eq!(run(&[b"\x1dxd"]), (b"\x1dxd".to_vec(), false));
    }

    #[test]
    fn a_leader_waits_for_the_next_key() {
        let mut keys = DetachKeys::default();
        assert_eq!(keys.filter(b"ab\x1d"), (b"ab".to_vec(), false));
        assert_eq!(keys.filter(b"c"), (b"\x1dc".to_vec(), false));
    }

    #[test]
    fn the_detach_keys_inside_a_paste_are_text() {
        let paste = b"\x1b[200~notes \x1dd and \x1b[93;5ud\x1b[201~".to_vec();
        assert_eq!(run(&[&paste]), (paste.clone(), false));
        let bytes: Vec<&[u8]> = paste.chunks(1).collect();
        assert_eq!(run(&bytes), (paste, false));
        assert_eq!(
            run(&[b"\x1b[200~x\x1b[201~", b"\x1dd"]),
            (b"\x1b[200~x\x1b[201~".to_vec(), true)
        );
    }

    #[test]
    fn a_leader_before_a_paste_reaches_the_program_before_it() {
        assert_eq!(
            run(&[b"\x1d\x1b[200~d\x1b[201~"]),
            (b"\x1d\x1b[200~d\x1b[201~".to_vec(), false)
        );
    }

    #[test]
    fn unfinished_and_malformed_sequences_pass_through() {
        for bytes in [
            b"\x1b[".as_slice(),
            b"\x1b[93;5",
            b"\x1b[93;xu",
            b"\x1b[;u",
            b"\x1b[93;0u",
            b"\x1b[27;5;93;1~",
            b"\x1b[\x01",
            b"\x1b[99999999999999999999;5u",
        ] {
            assert_eq!(run(&[bytes]), (bytes.to_vec(), false), "{bytes:?}");
        }
    }
}
