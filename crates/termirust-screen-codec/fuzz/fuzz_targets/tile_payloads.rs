#![no_main]

use libfuzzer_sys::fuzz_target;
use termirust_screen_codec::{decode_lossless, decode_lossy};

// The first two bytes choose a tile size; the rest is an untrusted payload.
fuzz_target!(|data: &[u8]| {
    let [width, height, payload @ ..] = data else {
        return;
    };
    let width = u32::from(*width % 65);
    let height = u32::from(*height % 65);
    if let Ok(pixels) = decode_lossless(payload, width, height) {
        assert_eq!(pixels.len(), (width * height * 4) as usize);
    }
    if let Ok(pixels) = decode_lossy(payload, width, height) {
        assert_eq!(pixels.len(), (width * height * 4) as usize);
    }
});
