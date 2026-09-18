//! Spike 0.2: measure what the operating system really reports as damage.
//!
//! `cargo run -p termirust-screen-capture --release --example capture_stats -- 10 2.0`
//!
//! Captures the main display for the given seconds at the given pixels per point, feeds every
//! frame to the codec with the reported damage, and prints how often frames arrive, how much of
//! the screen the damage covers, whether any tile that really changed was missing from the damage,
//! and how many bytes the codec would send.
//!
//! The same measurement on every platform, deliberately: the numbers are only worth having if a
//! Windows run can be set beside a macOS one. On macOS it needs the Screen Recording permission
//! for the terminal; on Windows it needs no permission, but Desktop Duplication cannot resample,
//! so the scale argument must be left at 1.

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn main() {
    use std::time::{Duration, Instant};

    use termirust_screen_capture::{CaptureConfig, FrameSource, displays};
    use termirust_screen_codec::{
        Encoder, EncoderConfig, Generation, SurfaceId, TileGrid, TileHashes,
    };

    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);

    let displays = match displays() {
        Ok(displays) => displays,
        Err(error) => {
            eprintln!("capture unavailable: {error}");
            if cfg!(target_os = "macos") {
                eprintln!("allow Screen Recording for this terminal and run it again");
            }
            std::process::exit(2);
        }
    };
    let display = displays[0];
    println!(
        "display {} at {} x {} points, scale {scale}",
        display.id,
        display.size_points.width(),
        display.size_points.height()
    );

    let mut config = CaptureConfig::display(display.id);
    config.scale = scale;
    config.max_fps = 30;
    #[cfg(target_os = "macos")]
    let started_source = termirust_screen_capture::ScreenCaptureKitSource::start(config);
    #[cfg(target_os = "windows")]
    let started_source = termirust_screen_capture::DesktopDuplicationSource::start(config);
    let mut source = match started_source {
        Ok(source) => source,
        Err(error) => {
            eprintln!("capture did not start: {error}");
            if cfg!(target_os = "windows") && (scale - 1.0).abs() > f32::EPSILON {
                eprintln!("Desktop Duplication captures at native pixels; try a scale of 1");
            }
            std::process::exit(2);
        }
    };

    let started = Instant::now();
    let mut encoder: Option<Encoder> = None;
    let mut hashes: Option<TileHashes> = None;
    let (mut frames, mut unknown_damage, mut damaged_tiles, mut total_tiles) =
        (0u64, 0u64, 0usize, 0usize);
    let (mut changed_tiles, mut missed_tiles, mut bytes) = (0usize, 0usize, 0usize);
    let mut first_frame_bytes = 0usize;
    while started.elapsed() < Duration::from_secs(seconds) {
        let Ok(Some(captured)) = source.next_frame(Duration::from_millis(200)) else {
            continue;
        };
        let Ok(frame) = captured.frame() else {
            continue;
        };
        frames += 1;
        let grid = TileGrid::new(frame.size());
        let encoder = encoder.get_or_insert_with(|| {
            println!(
                "frames are {} x {} pixels, frame scale {:?}",
                frame.size().width(),
                frame.size().height(),
                captured.scale
            );
            Encoder::new(
                SurfaceId(1),
                Generation(1),
                frame.size(),
                EncoderConfig::default(),
            )
        });
        let changed = match hashes.as_mut() {
            Some(previous) => Some(previous.update(&frame, None).expect("same size")),
            None => {
                hashes = Some(TileHashes::compute(&frame));
                None
            }
        };
        if let Some(changed) = &changed {
            changed_tiles += changed.len();
        }
        match captured.damage_rects() {
            None => unknown_damage += 1,
            Some(rects) => {
                let damage = grid.tiles_for_damage(rects);
                damaged_tiles += damage.len();
                total_tiles += grid.len();
                if let Some(changed) = &changed {
                    missed_tiles += changed
                        .iter()
                        .filter(|tile| !damage.contains(*tile))
                        .count();
                }
            }
        }
        let batch = encoder
            .encode_at(&frame, captured.damage_rects(), captured.timestamp_ms)
            .expect("frame matches the encoder");
        if !batch.ops.is_empty() {
            let size = batch.encode().map(|encoded| encoded.len()).unwrap_or(0);
            if frames == 1 {
                first_frame_bytes = size;
            } else {
                bytes += size;
            }
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "frames delivered: {frames} in {elapsed:.1} s ({:.1} per second; unchanged frames are not delivered)",
        frames as f64 / elapsed
    );
    println!("frames without damage: {unknown_damage}");
    if total_tiles > 0 {
        println!(
            "damage covered {:.2} % of tiles per frame",
            damaged_tiles as f64 * 100.0 / total_tiles as f64
        );
    }
    let tiles_per_frame = encoder
        .as_ref()
        .map(|encoder| TileGrid::new(encoder.size()).len())
        .unwrap_or(1);
    println!(
        "tiles that really changed: {changed_tiles} ({:.2} % of tiles per frame); outside the reported damage: {missed_tiles}",
        changed_tiles as f64 * 100.0 / (tiles_per_frame as f64 * frames.max(1) as f64)
    );
    println!(
        "codec would send the first frame in {:.1} KB, then {:.1} KB ({:.1} KB/s)",
        first_frame_bytes as f64 / 1000.0,
        bytes as f64 / 1000.0,
        bytes as f64 / 1000.0 / elapsed
    );
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn main() {
    eprintln!("capture_stats needs macOS or Windows");
}
