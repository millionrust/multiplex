//! Pictures arrive as a lossy first pass and become exact once they stop changing.

use termirust_screen_codec::{
    Decoder, Encoder, EncoderConfig, FrameBuffer, Generation, REFINE_IDLE_MS, Rect, Size,
    SurfaceId, TileOp,
};

const CACHE: usize = 64 << 20;

fn photo(size: Size, seed: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size);
    let mut pixels = Vec::with_capacity((size.width() * size.height() * 4) as usize);
    for y in 0..size.height() {
        for x in 0..size.width() {
            let noise = ((x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ seed)
                .wrapping_mul(2_654_435_761)
                >> 27) as u8;
            pixels.extend([
                (x / 2) as u8 + noise,
                (y / 2) as u8 + noise,
                ((x + y) / 4) as u8,
                255,
            ]);
        }
    }
    buffer.write_rect(size.bounds(), &pixels).unwrap();
    buffer
}

fn encoder(size: Size) -> Encoder {
    Encoder::new(
        SurfaceId(1),
        Generation(1),
        size,
        EncoderConfig {
            cache_bytes: CACHE,
            ..EncoderConfig::default()
        },
    )
}

#[test]
fn idle_pictures_become_exact_within_the_budget() {
    let size = Size::new(320, 192).unwrap();
    let frame = photo(size, 1);
    let mut encoder = encoder(size);
    let mut decoder = Decoder::new(CACHE);

    let first = encoder.encode_at(&frame.as_frame(), None, 0).unwrap();
    assert!(
        first
            .ops
            .iter()
            .all(|op| matches!(op, TileOp::Lossy { .. }))
    );
    decoder.apply(&first).unwrap();
    assert_ne!(decoder.framebuffer().unwrap(), &frame);
    let lossy_tiles = encoder.approximate_tiles();
    assert_eq!(lossy_tiles, first.ops.len());

    assert!(
        encoder
            .refine_at(REFINE_IDLE_MS - 1, usize::MAX)
            .unwrap()
            .is_none(),
        "refined too early"
    );

    let mut batches = 0;
    let mut now = REFINE_IDLE_MS;
    while let Some(batch) = encoder.refine_at(now, 2_000).unwrap() {
        assert!(
            batch
                .ops
                .iter()
                .all(|op| matches!(op, TileOp::Lossless { .. } | TileOp::Cached { .. }))
        );
        decoder.apply(&batch).unwrap();
        batches += 1;
        now += 16;
    }
    assert!(
        batches > 1,
        "a small budget should spread refinement over several batches"
    );
    assert_eq!(encoder.approximate_tiles(), 0);
    assert_eq!(decoder.framebuffer().unwrap(), &frame);
}

#[test]
fn recently_changed_tiles_wait() {
    let size = Size::new(256, 128).unwrap();
    let mut encoder = encoder(size);
    let mut decoder = Decoder::new(CACHE);
    let a = photo(size, 1);
    decoder
        .apply(&encoder.encode_at(&a.as_frame(), None, 0).unwrap())
        .unwrap();

    let mut b = a.clone();
    let changed = Rect::new(0, 0, 64, 64);
    let mut pixels = Vec::new();
    photo(size, 99)
        .as_frame()
        .copy_rect_into(changed, &mut pixels);
    b.write_rect(changed, &pixels).unwrap();
    decoder
        .apply(&encoder.encode_at(&b.as_frame(), None, 200).unwrap())
        .unwrap();

    let batch = encoder
        .refine_at(REFINE_IDLE_MS, usize::MAX)
        .unwrap()
        .unwrap();
    assert!(
        batch.ops.iter().all(|op| match op {
            TileOp::Lossless { tile, .. } | TileOp::Cached { tile, .. } => tile.0 != 0,
            _ => false,
        }),
        "the tile changed at 200 ms was refined at 250 ms"
    );
    decoder.apply(&batch).unwrap();
    let late = encoder
        .refine_at(200 + REFINE_IDLE_MS, usize::MAX)
        .unwrap()
        .unwrap();
    decoder.apply(&late).unwrap();
    assert_eq!(decoder.framebuffer().unwrap(), &b);
}
