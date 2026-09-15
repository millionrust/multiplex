//! Synthetic desktop workloads for measuring bytes on the wire. Shared by the workload test and
//! the `workload_report` example. The content is generated, not captured, so numbers indicate
//! relative cost and catch regressions; real captures come with the ScreenCaptureKit spike.

// The test and the example each use a different subset of this module.
#![allow(dead_code)]

use termirust_screen_codec::{
    Batch, Decoder, Encoder, EncoderConfig, FrameBuffer, Generation, Rect, Size, SurfaceId,
};

pub const WIDTH: u32 = 1512;
pub const HEIGHT: u32 = 982;
const CACHE: usize = 64 << 20;
const FRAME_MS: u64 = 33;
/// Refinement budget per frame, about 60 KB/s at 30 frames a second.
const REFINE_BUDGET: usize = 2_000;

pub struct Report {
    pub name: &'static str,
    pub target: &'static str,
    pub seconds: f64,
    pub batches: usize,
    pub bytes: usize,
    pub largest_batch: usize,
    pub per_batch: Vec<usize>,
}

impl Report {
    pub fn kb_per_second(&self) -> f64 {
        self.bytes as f64 / 1_000.0 / self.seconds
    }
}

pub fn size() -> Size {
    Size::new(WIDTH, HEIGHT).unwrap()
}

/// A dark editor: text lines every 18 px starting at `first_line`, a line being typed with
/// `typed` characters, a sidebar, and a title bar.
pub fn editor(first_line: u32, typed: u32) -> FrameBuffer {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(editor_pixel(x, y, first_line, typed));
        }
    }
    let mut buffer = FrameBuffer::new(size());
    buffer.write_rect(size().bounds(), &pixels).unwrap();
    buffer
}

fn editor_pixel(x: u32, y: u32, first_line: u32, typed: u32) -> [u8; 4] {
    if y < 30 {
        return if (400..700).contains(&x) && (10..20).contains(&y) && (x / 7 + y).is_multiple_of(3)
        {
            [180, 185, 190, 255]
        } else {
            [50, 41, 37, 255]
        };
    }
    if x < 220 {
        let row = (y - 30) / 22;
        return if (40..40 + (row * 29 % 140)).contains(&x)
            && (y - 30) % 22 > 6
            && (y - 30) % 22 < 16
            && !(x + row).is_multiple_of(4)
        {
            [154, 162, 173, 255]
        } else {
            [34, 29, 26, 255]
        };
    }
    let offset = y - 30 + first_line * 18;
    let line = offset / 18;
    let glyph_row = offset % 18;
    let length = if line == first_line + 2 {
        40 + typed * 8
    } else {
        line * 97 % 900 + 60
    };
    if glyph_row < 13 && (260..260 + length).contains(&x) {
        let column = (x - 260) / 8;
        let seed = column.wrapping_mul(2_654_435_761) ^ line.wrapping_mul(40_503);
        if (seed >> (glyph_row % 8)) & 1 == 1 && (x - 260) % 8 < 6 {
            let color = match (line + column / 6) % 4 {
                0 => [217, 139, 201, 255],
                1 => [242, 167, 116, 255],
                2 => [97, 233, 176, 255],
                _ => [212, 205, 201, 255],
            };
            return color;
        }
    }
    [35, 30, 28, 255]
}

/// Encodes every frame from `frames` at 30 frames a second, applies it to a viewer, and spends a
/// small refinement budget on frames that changed nothing.
pub fn run(
    name: &'static str,
    target: &'static str,
    frame_count: u32,
    mut frames: impl FnMut(u32) -> (FrameBuffer, Option<Vec<Rect>>),
) -> Report {
    let mut encoder = Encoder::new(
        SurfaceId(1),
        Generation(1),
        size(),
        EncoderConfig {
            cache_bytes: CACHE,
            ..EncoderConfig::default()
        },
    );
    let mut decoder = Decoder::new(CACHE);
    let (first, _) = frames(0);
    let warmup = encoder.encode_at(&first.as_frame(), None, 0).unwrap();
    decoder.apply(&warmup).unwrap();
    encoder.acknowledge(warmup.sequence);

    let mut report = Report {
        name,
        target,
        seconds: f64::from(frame_count) * FRAME_MS as f64 / 1_000.0,
        batches: 0,
        bytes: 0,
        largest_batch: 0,
        per_batch: Vec::new(),
    };
    for index in 1..=frame_count {
        let now = u64::from(index) * FRAME_MS;
        let (frame, damage) = frames(index);
        let mut batch = encoder
            .encode_at(&frame.as_frame(), damage.as_deref(), now)
            .unwrap();
        if batch.ops.is_empty()
            && let Some(refine) = encoder.refine_at(now, REFINE_BUDGET).unwrap()
        {
            batch = refine;
        }
        let bytes = deliver(&mut encoder, &mut decoder, &batch);
        report.per_batch.push(bytes);
        if bytes > 0 {
            report.batches += 1;
            report.bytes += bytes;
            report.largest_batch = report.largest_batch.max(bytes);
        }
    }
    report
}

/// Sends a batch unless it is empty and returns its size on the wire.
fn deliver(encoder: &mut Encoder, decoder: &mut Decoder, batch: &Batch) -> usize {
    if batch.ops.is_empty() {
        return 0;
    }
    let bytes = batch.encode().unwrap();
    let applied = decoder.apply(&Batch::decode(&bytes).unwrap()).unwrap();
    for miss in applied.misses {
        encoder.cache_miss(miss);
    }
    encoder.acknowledge(applied.sequence);
    bytes.len()
}

pub fn idle() -> Report {
    let frame = editor(0, 10);
    run("Idle desktop", "< 1 kbps", 300, |_| {
        (frame.clone(), Some(Vec::new()))
    })
}

pub fn typing() -> Report {
    let base = editor(0, 0);
    run("Typing in an editor", "10–40 KB/s", 300, |index| {
        let typed = index / 4;
        let mut frame = base.clone();
        let line_y = 30 + 2 * 18;
        let rect = Rect::new(260, line_y, 40 + (typed + 1) * 8, 13);
        let mut pixels = Vec::with_capacity((rect.width * rect.height * 4) as usize);
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                pixels.extend(editor_pixel(x, y, 0, typed));
            }
        }
        frame.write_rect(rect, &pixels).unwrap();
        (frame, Some(vec![rect]))
    })
}

pub fn scrolling() -> Report {
    run("Scrolling a code file", "30–120 KB/s", 90, |index| {
        (editor(index / 3, 10), None)
    })
}

pub fn window_switching() -> Report {
    let a = editor(0, 10);
    let b = editor(137, 3);
    run(
        "Switching windows",
        "≤ 300 KB burst, then ~0",
        300,
        |index| {
            let frame = if (index / 60) % 2 == 0 {
                a.clone()
            } else {
                b.clone()
            };
            (frame, None)
        },
    )
}

pub fn video() -> Report {
    let base = editor(0, 10);
    let area = Rect::new(900, 520, 384, 256);
    run(
        "Video region (tile path)",
        "≤ 600 kbps, choppy",
        150,
        |index| {
            let mut frame = base.clone();
            let t = index * 11;
            let mut pixels = Vec::with_capacity((area.width * area.height * 4) as usize);
            for y in 0..area.height {
                for x in 0..area.width {
                    let noise = ((x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ t)
                        .wrapping_mul(2_654_435_761)
                        >> 28) as u8;
                    pixels.extend([
                        ((x + t) / 2) as u8 + noise,
                        ((y + t) / 2) as u8 + noise,
                        ((x + y) / 4) as u8,
                        255,
                    ]);
                }
            }
            frame.write_rect(area, &pixels).unwrap();
            (frame, Some(vec![area]))
        },
    )
}
