//! A playing video becomes a motion region: sent as lossy tiles at a capped rate, and sent again
//! when it stops.

use termirust_screen_codec::{
    Decoder, Encoder, EncoderConfig, FrameBuffer, Generation, MotionEvent, Rect, Size, SurfaceId,
    TileOp,
};

const CACHE: usize = 64 << 20;
const VIDEO: Rect = Rect::new(256, 128, 384, 256);

fn desktop(size: Size, video_frame: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size);
    buffer.fill_rect(size.bounds(), [36, 30, 28, 255]).unwrap();
    buffer
        .fill_rect(Rect::new(40, 40, 160, 12), [230, 232, 235, 255])
        .unwrap();
    let mut pixels = Vec::with_capacity((VIDEO.width * VIDEO.height * 4) as usize);
    for y in 0..VIDEO.height {
        for x in 0..VIDEO.width {
            let t = video_frame * 9;
            let noise = ((x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ t)
                .wrapping_mul(2_654_435_761)
                >> 28) as u8;
            pixels.extend([
                ((x + t) % 256) as u8 / 2 + noise,
                ((y + t * 2) % 256) as u8 / 2 + noise,
                ((x + y) / 3) as u8 + noise,
                255,
            ]);
        }
    }
    buffer.write_rect(VIDEO, &pixels).unwrap();
    buffer
}

#[test]
fn video_is_promoted_throttled_and_resent_when_it_stops() {
    let size = Size::new(960, 540).unwrap();
    let mut encoder = Encoder::new(
        SurfaceId(1),
        Generation(1),
        size,
        EncoderConfig {
            cache_bytes: CACHE,
            ..EncoderConfig::default()
        },
    );
    let mut decoder = Decoder::new(CACHE);

    let mut promoted_at = None;
    let mut batches_with_video_tiles_after_promotion = 0;
    let mut now = 0;
    for frame_index in 0..60 {
        let frame = desktop(size, frame_index);
        let batch = encoder.encode_at(&frame.as_frame(), None, now).unwrap();
        decoder.apply(&batch).unwrap();
        if let Some(MotionEvent::Promoted(rect)) = encoder.motion_event() {
            assert!(rect.contains_rect(VIDEO));
            promoted_at = Some(now);
        }
        if promoted_at.is_some_and(|at| now > at) {
            let video_ops: Vec<_> = batch
                .ops
                .iter()
                .filter(|op| !matches!(op, TileOp::Move { .. }))
                .collect();
            if !video_ops.is_empty() {
                batches_with_video_tiles_after_promotion += 1;
                assert!(
                    video_ops
                        .iter()
                        .all(|op| matches!(op, TileOp::Lossy { .. } | TileOp::Solid { .. })),
                    "motion tiles must be lossy"
                );
            }
        }
        now += 33;
    }
    let promoted_at = promoted_at.expect("video was never promoted");
    let seconds = (now - promoted_at) as f64 / 1_000.0;
    let rate = f64::from(batches_with_video_tiles_after_promotion) / seconds;
    assert!(rate <= 8.5, "video tiles went out {rate:.1} times a second");

    let still = desktop(size, 59);
    let mut demoted = false;
    for _ in 0..40 {
        let batch = encoder.encode_at(&still.as_frame(), None, now).unwrap();
        decoder.apply(&batch).unwrap();
        if matches!(encoder.motion_event(), Some(MotionEvent::Demoted(_))) {
            demoted = true;
            assert!(!batch.ops.is_empty(), "demotion must resend the region");
        }
        now += 33;
    }
    assert!(demoted, "video was never demoted");
    assert_eq!(encoder.motion_region(), None);

    let framebuffer = decoder.framebuffer().unwrap();
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    still
        .as_frame()
        .copy_rect_into(Rect::new(40, 40, 160, 12), &mut expected);
    framebuffer
        .as_frame()
        .copy_rect_into(Rect::new(40, 40, 160, 12), &mut actual);
    assert_eq!(actual, expected, "text outside the region stays exact");
}

/// The saving the motion path exists for.
///
/// While a region is promoted but nothing else is carrying it, the tile path keeps covering it at
/// a throttled rate — that is what a viewer with no decoder sees. Once something above the encoder
/// says the video is arriving, those tiles are pixels paid for twice, and must stop.
#[test]
fn a_carried_region_costs_the_tile_path_nothing() {
    let size = Size::new(960, 640).unwrap();
    let mut encoder = Encoder::new(
        SurfaceId(1),
        Generation(1),
        size,
        EncoderConfig {
            cache_bytes: CACHE,
            ..EncoderConfig::default()
        },
    );

    let mut now = 0;
    let mut promoted = false;
    for step in 0..60u32 {
        encoder
            .encode_at(&desktop(size, step).as_frame(), None, now)
            .unwrap();
        now += 33;
        if encoder.motion_region().is_some() {
            promoted = true;
            break;
        }
    }
    assert!(promoted, "the region was never promoted");
    let region = encoder.motion_region().expect("a region");

    let ops_inside = |encoder: &mut Encoder, from: u32, now: &mut u64| {
        let mut inside = 0;
        for step in from..from + 30 {
            let batch = encoder
                .encode_at(&desktop(size, step).as_frame(), None, *now)
                .unwrap();
            *now += 33;
            for op in &batch.ops {
                let tile = match op {
                    TileOp::Solid { tile, .. }
                    | TileOp::Cached { tile, .. }
                    | TileOp::Lossy { tile, .. }
                    | TileOp::Lossless { tile, .. } => *tile,
                    _ => continue,
                };
                // 64-pixel tiles, laid out row by row across the surface.
                let across = size.width().div_ceil(64);
                let rect = Rect::new((tile.0 % across) * 64, (tile.0 / across) * 64, 64, 64);
                if region.contains_rect(rect) {
                    inside += 1;
                }
            }
        }
        inside
    };

    let throttled = ops_inside(&mut encoder, 100, &mut now);
    assert!(
        throttled > 0,
        "the tile path should cover the region while nothing else is"
    );

    encoder.set_motion_carried(true);
    let carried = ops_inside(&mut encoder, 200, &mut now);
    assert_eq!(
        carried, 0,
        "a carried region still cost {carried} tile operations, so its pixels go twice"
    );
}
