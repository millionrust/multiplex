//! A viewer that loses batches and reconnects receives only what changed since its last
//! acknowledgement, and still ends up pixel-exact.

use termirust_screen_codec::{
    Batch, Decoder, Encoder, EncoderConfig, FrameBuffer, Generation, Rect, Resume, Size, SurfaceId,
    TileOp,
};

const CACHE: usize = 64 << 20;

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
    let cursor = Rect::new(40 + cursor_column * 8, 36, 2, 14).clamp_to(size);
    buffer.fill_rect(cursor, [242, 167, 116, 255]).unwrap();
    buffer
}

/// Encodes a frame and delivers it unless `lost`. Returns the wire size.
fn step(
    encoder: &mut Encoder,
    decoder: &mut Decoder,
    frame: &FrameBuffer,
    lost: bool,
) -> (Batch, usize) {
    let batch = encoder.encode(&frame.as_frame(), None).unwrap();
    let bytes = batch.encode().unwrap().len();
    if !lost {
        let applied = decoder.apply(&batch).unwrap();
        for miss in applied.misses {
            encoder.cache_miss(miss);
        }
        encoder.acknowledge(applied.sequence);
    }
    (batch, bytes)
}

#[test]
fn lost_typing_is_repaired_without_a_full_refresh() {
    let size = Size::new(1024, 640).unwrap();
    let mut encoder = encoder(size);
    let mut decoder = Decoder::new(CACHE);
    let (_, full) = step(&mut encoder, &mut decoder, &editor(size, 0, 0), false);

    step(&mut encoder, &mut decoder, &editor(size, 0, 1), true);
    step(&mut encoder, &mut decoder, &editor(size, 0, 2), true);

    assert_eq!(encoder.resume(decoder.last_sequence()), Resume::Partial);
    let current = editor(size, 0, 3);
    let (_, repair) = step(&mut encoder, &mut decoder, &current, false);
    assert_eq!(decoder.framebuffer().unwrap(), &current);
    assert!(repair < full / 10, "repair took {repair} of {full} bytes");
}

#[test]
fn a_lost_scroll_is_repaired_exactly() {
    let size = Size::new(1024, 640).unwrap();
    let mut encoder = encoder(size);
    let mut decoder = Decoder::new(CACHE);
    step(&mut encoder, &mut decoder, &editor(size, 0, 0), false);
    let (lost_scroll, _) = step(&mut encoder, &mut decoder, &editor(size, 4, 0), true);
    assert!(
        lost_scroll
            .ops
            .iter()
            .any(|op| matches!(op, TileOp::Move { .. }))
    );
    assert_eq!(encoder.resume(decoder.last_sequence()), Resume::Partial);
    let current = editor(size, 4, 1);
    step(&mut encoder, &mut decoder, &current, false);
    assert_eq!(decoder.framebuffer().unwrap(), &current);
}

#[test]
fn lost_cache_insertions_are_sent_again_instead_of_missing() {
    let size = Size::new(512, 256).unwrap();
    let mut encoder = encoder(size);
    let mut decoder = Decoder::new(CACHE);
    step(&mut encoder, &mut decoder, &editor(size, 0, 0), false);
    step(&mut encoder, &mut decoder, &editor(size, 9, 0), true);

    assert_eq!(encoder.resume(decoder.last_sequence()), Resume::Partial);
    step(&mut encoder, &mut decoder, &editor(size, 9, 0), false);
    let batch = encoder
        .encode(&editor(size, 9, 5).as_frame(), None)
        .unwrap();
    let applied = decoder.apply(&batch).unwrap();
    assert!(
        applied.misses.is_empty(),
        "resume left references to lost tiles"
    );
    assert_eq!(decoder.framebuffer().unwrap(), &editor(size, 9, 5));
}

#[test]
fn unknown_or_regressed_acknowledgements_get_a_full_refresh() {
    let size = Size::new(256, 128).unwrap();
    let mut encoder = encoder(size);
    let mut decoder = Decoder::new(CACHE);
    step(&mut encoder, &mut decoder, &editor(size, 0, 0), false);
    step(&mut encoder, &mut decoder, &editor(size, 2, 0), false);
    assert_eq!(encoder.resume(0), Resume::Full);
    assert_eq!(
        encoder.resume(1),
        Resume::Full,
        "acknowledgement went backwards"
    );

    let mut fresh = Decoder::new(CACHE);
    let frame = editor(size, 2, 1);
    let batch = encoder.encode(&frame.as_frame(), None).unwrap();
    fresh.apply(&batch).unwrap();
    assert_eq!(fresh.framebuffer().unwrap(), &frame);
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(40))]

    /// Sessions with outages: once a batch is lost, every batch is lost until the viewer
    /// reconnects and resumes, as on a reliable stream that drops.
    #[test]
    fn random_outages_followed_by_resume_stay_exact(
        steps in proptest::collection::vec((0u32..10, 0u32..40, 0u8..4), 1..16),
    ) {
        let size = Size::new(640, 400).unwrap();
        let mut encoder = encoder(size);
        let mut decoder = Decoder::new(CACHE);
        let mut frame = editor(size, 0, 0);
        step(&mut encoder, &mut decoder, &frame, false);
        let mut connected = true;
        for (line, column, event) in steps {
            frame = editor(size, line, column);
            match (connected, event) {
                (true, 0) => connected = false,
                (false, 1) => {
                    encoder.resume(decoder.last_sequence());
                    connected = true;
                }
                _ => {}
            }
            step(&mut encoder, &mut decoder, &frame, !connected);
            if connected {
                proptest::prop_assert_eq!(decoder.framebuffer().unwrap(), &frame);
            }
        }
        if !connected {
            encoder.resume(decoder.last_sequence());
        }
        step(&mut encoder, &mut decoder, &frame, false);
        proptest::prop_assert_eq!(decoder.framebuffer().unwrap(), &frame);
    }
}
