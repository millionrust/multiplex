#![no_main]

use libfuzzer_sys::fuzz_target;
use termirust_screen_codec::{Batch, Decoder};

// A viewer receives bytes from the network: parsing and applying them must never panic or
// allocate beyond the batch bounds, whatever the host sends.
fuzz_target!(|data: &[u8]| {
    let Ok(batch) = Batch::decode(data) else {
        return;
    };
    if batch.size.width() as u64 * batch.size.height() as u64 > 4_000_000 {
        return;
    }
    let mut decoder = Decoder::new(1 << 20);
    let _ = decoder.apply(&batch);
    if let Ok(bytes) = batch.encode() {
        assert_eq!(Batch::decode(&bytes).as_ref(), Ok(&batch));
    }
});
