//! Encoder → wire → decoder, checked against the source pixels.

use termirust_screen_codec::{
    Batch, Decoder, Encoder, EncoderConfig, FrameBuffer, Generation, Rect, Size, SurfaceId, TileOp,
};

const CACHE: usize = 64 << 20;

fn config() -> EncoderConfig {
    EncoderConfig {
        cache_bytes: CACHE,
        ..EncoderConfig::default()
    }
}

/// A dark editor with distinct text lines, scrolled so `first_line` is at the top.
fn editor(size: Size, first_line: u32, cursor_column: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size);
    buffer.fill_rect(size.bounds(), [29, 25, 23, 255]).unwrap();
    for y in 0..size.height() {
        let offset = y + first_line * 18;
        let line = offset / 18;
        if offset % 18 >= 12 {
            continue;
        }
        let length = (line * 37 % 500 + 60).min(size.width());
        for x in (0..length).filter(|x| (x * 3 + line * 7 + offset % 18) % 5 < 3) {
            let shade = (line * 13 % 200) as u8 + 40;
            buffer
                .fill_rect(Rect::new(x, y, 1, 1), [shade, 232, 235, 255])
                .unwrap();
        }
    }
    buffer
        .fill_rect(
            Rect::new(40 + cursor_column * 8, 36, 2, 14),
            [242, 167, 116, 255],
        )
        .unwrap();
    buffer
}

fn send(
    encoder: &mut Encoder,
    decoder: &mut Decoder,
    frame: &FrameBuffer,
    damage: Option<&[Rect]>,
) -> (Batch, usize) {
    let batch = encoder.encode(&frame.as_frame(), damage).unwrap();
    let bytes = batch.encode().unwrap();
    let received = Batch::decode(&bytes).unwrap();
    let applied = decoder.apply(&received).unwrap();
    for miss in applied.misses {
        encoder.cache_miss(miss);
    }
    (batch, bytes.len())
}

#[test]
fn text_screens_arrive_pixel_exact_through_typing_scrolling_and_switching() {
    let size = Size::new(1024, 640).unwrap();
    let mut encoder = Encoder::new(SurfaceId(1), Generation(1), size, config());
    let mut decoder = Decoder::new(CACHE);

    let first = editor(size, 0, 0);
    let (_, first_bytes) = send(&mut encoder, &mut decoder, &first, None);
    assert_eq!(decoder.framebuffer().unwrap(), &first);

    let typed = editor(size, 0, 1);
    let (batch, typed_bytes) = send(
        &mut encoder,
        &mut decoder,
        &typed,
        Some(&[Rect::new(40, 36, 20, 14)]),
    );
    assert_eq!(decoder.framebuffer().unwrap(), &typed);
    assert!(batch.ops.len() <= 2, "typing sent {} ops", batch.ops.len());
    assert!(typed_bytes < 400, "typing took {typed_bytes} bytes");

    let scrolled = editor(size, 3, 1);
    let (batch, scrolled_bytes) = send(&mut encoder, &mut decoder, &scrolled, None);
    assert_eq!(decoder.framebuffer().unwrap(), &scrolled);
    assert!(
        batch
            .ops
            .iter()
            .any(|op| matches!(op, TileOp::Move { dy: 54, .. })),
        "scroll was not a move"
    );
    assert!(
        scrolled_bytes < first_bytes / 3,
        "scroll took {scrolled_bytes} of {first_bytes} bytes"
    );

    let back = editor(size, 0, 1);
    let (batch, back_bytes) = send(&mut encoder, &mut decoder, &back, None);
    assert_eq!(decoder.framebuffer().unwrap(), &back);
    let cached = batch
        .ops
        .iter()
        .filter(|op| matches!(op, TileOp::Cached { .. }))
        .count();
    assert!(
        cached > 0,
        "returning to earlier content used no cached tiles"
    );
    assert!(
        back_bytes < first_bytes / 3,
        "switching back took {back_bytes} of {first_bytes} bytes"
    );
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

    /// Every frame of a random session of scrolls, typing, and flat edits arrives exactly.
    #[test]
    fn random_text_sessions_stay_pixel_exact(
        steps in proptest::collection::vec(
            (0u8..4, 0u32..12, 0u32..700, 0u32..460, 1u32..300, 1u32..200, proptest::prelude::any::<u8>()),
            1..14,
        ),
        damage_hints in proptest::prelude::any::<bool>(),
    ) {
        let size = Size::new(700, 460).unwrap();
        let mut encoder = Encoder::new(SurfaceId(1), Generation(1), size, config());
        let mut decoder = Decoder::new(CACHE);
        let (mut line, mut column) = (0u32, 0u32);
        let mut frame = editor(size, line, column);
        send(&mut encoder, &mut decoder, &frame, None);
        for (kind, amount, x, y, w, h, shade) in steps {
            let mut damage = None;
            match kind {
                0 => {
                    line = amount;
                    frame = editor(size, line, column);
                }
                1 => {
                    column = amount % 50;
                    frame = editor(size, line, column);
                }
                2 => {
                    let rect = Rect::new(x, y, w, h).clamp_to(size);
                    if !rect.is_empty() {
                        frame.fill_rect(rect, [shade, shade / 2, 255 - shade, 255]).unwrap();
                        damage = Some(vec![rect]);
                    }
                }
                _ => {
                    let rect = Rect::new(x, y, w, h).clamp_to(size);
                    for py in rect.y..rect.bottom() {
                        for px in (rect.x..rect.right()).filter(|px| (px + py) % 2 == 0) {
                            frame.fill_rect(Rect::new(px, py, 1, 1), [shade, 0, 0, 255]).unwrap();
                        }
                    }
                    if !rect.is_empty() {
                        damage = Some(vec![rect]);
                    }
                }
            }
            let hint = if damage_hints { damage.as_deref() } else { None };
            send(&mut encoder, &mut decoder, &frame, hint);
            proptest::prop_assert_eq!(decoder.framebuffer().unwrap(), &frame);
        }
    }
}

#[test]
fn unchanged_frames_send_empty_batches() {
    let size = Size::new(300, 200).unwrap();
    let mut encoder = Encoder::new(SurfaceId(1), Generation(1), size, config());
    let mut decoder = Decoder::new(CACHE);
    let frame = editor(size, 0, 0);
    send(&mut encoder, &mut decoder, &frame, None);
    let (batch, bytes) = send(&mut encoder, &mut decoder, &frame, None);
    assert!(batch.ops.is_empty());
    assert_eq!(bytes, termirust_screen_codec::BATCH_HEADER_BYTES);
}

#[test]
fn cache_misses_are_repaired_on_the_next_batch() {
    let size = Size::new(512, 256).unwrap();
    let grid = termirust_screen_codec::TileGrid::new(size);
    let mut encoder = Encoder::new(SurfaceId(1), Generation(1), size, config());
    let mut decoder = Decoder::new(CACHE);
    let a = editor(size, 0, 0);
    let b = editor(size, 5, 0);
    send(&mut encoder, &mut decoder, &a, None);
    send(&mut encoder, &mut decoder, &b, None);

    // The viewer restarts and loses its cache; the host still believes the tiles of `a` are held.
    let mut restarted = Decoder::new(CACHE);
    let batch = encoder.encode(&a.as_frame(), None).unwrap();
    let applied = restarted.apply(&batch).unwrap();
    assert!(
        !applied.misses.is_empty(),
        "expected cached references to miss"
    );
    let missed: Vec<_> = applied.misses.iter().map(|miss| miss.tile).collect();
    for miss in applied.misses {
        encoder.cache_miss(miss);
    }

    let repair = encoder.encode(&a.as_frame(), None).unwrap();
    assert!(restarted.apply(&repair).unwrap().misses.is_empty());
    let framebuffer = restarted.framebuffer().unwrap();
    for tile in missed {
        let rect = grid.tile_rect(tile).unwrap();
        let mut expected = Vec::new();
        let mut actual = Vec::new();
        a.as_frame().copy_rect_into(rect, &mut expected);
        framebuffer.as_frame().copy_rect_into(rect, &mut actual);
        assert_eq!(actual, expected, "tile {tile:?} was not repaired");
    }
}

#[test]
fn stale_and_repeated_batches_are_refused() {
    let size = Size::new(128, 128).unwrap();
    let mut encoder = Encoder::new(SurfaceId(1), Generation(2), size, config());
    let mut decoder = Decoder::new(CACHE);
    let batch = encoder
        .encode(&editor(size, 0, 0).as_frame(), None)
        .unwrap();
    decoder.apply(&batch).unwrap();
    assert_eq!(
        decoder.apply(&batch),
        Err(termirust_screen_codec::CodecError::StaleBatch)
    );
    let mut old = batch.clone();
    old.generation = Generation(1);
    old.sequence = 99;
    assert_eq!(
        decoder.apply(&old),
        Err(termirust_screen_codec::CodecError::StaleBatch)
    );
}

#[test]
fn tampered_lossless_pixels_are_rejected() {
    let size = Size::new(64, 64).unwrap();
    let mut encoder = Encoder::new(SurfaceId(1), Generation(1), size, config());
    let mut batch = encoder
        .encode(&editor(size, 0, 0).as_frame(), None)
        .unwrap();
    let other = Encoder::new(SurfaceId(1), Generation(1), size, config())
        .encode(&editor(size, 9, 0).as_frame(), None)
        .unwrap();
    if let (
        Some(TileOp::Lossless { payload, .. }),
        Some(TileOp::Lossless {
            payload: foreign, ..
        }),
    ) = (batch.ops.get_mut(0), other.ops.first())
    {
        *payload = foreign.clone();
    } else {
        panic!("expected lossless tiles");
    }
    assert_eq!(
        Decoder::new(CACHE).apply(&batch),
        Err(termirust_screen_codec::CodecError::CorruptPayload)
    );
}
