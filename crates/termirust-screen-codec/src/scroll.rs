//! Finds vertical scrolls so a moved block of text costs one move instead of every tile it covers.
//!
//! Rows of the region are hashed in both frames. Each changed row votes, through the window of
//! rows starting at it, for the offsets at which the previous frame had an identical and
//! distinctive window. Windows make blank gaps and flat glyph rows distinctive where they meet
//! text. The winning offset is kept when enough rows agree, and the longest run of rows it
//! explains becomes the move.

use std::collections::HashMap;

use xxhash_rust::xxh3::xxh3_64;

use crate::{Frame, Rect, TILE_SIZE};

/// Largest scroll distance searched, in pixels.
pub const MAX_SCROLL_PIXELS: i32 = 384;
/// A window seen more often than this in the previous frame (blank areas, backgrounds) does not
/// vote, because it would match everywhere.
const MAX_WINDOW_REPEATS: usize = 3;
/// Rows combined into one voting window.
const WINDOW_ROWS: usize = 8;
/// Fewest rows a move must explain.
const MIN_MOVE_ROWS: u32 = TILE_SIZE;

/// A detected move: each row `y` of `rect` in the new frame equals row `y + dy` of the old one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerticalMove {
    pub rect: Rect,
    pub dy: i32,
}

/// Looks for a vertical scroll of `region` between `previous` and `next`, which must be the same
/// size. Returns `None` when no offset explains at least one tile's worth of rows.
pub fn detect_vertical_move(
    previous: &Frame<'_>,
    next: &Frame<'_>,
    region: Rect,
) -> Option<VerticalMove> {
    if previous.size() != next.size() {
        return None;
    }
    let region = region.clamp_to(next.size());
    if region.width < TILE_SIZE || region.height < MIN_MOVE_ROWS * 2 {
        return None;
    }
    let old_rows = row_hashes(previous, region);
    let new_rows = row_hashes(next, region);

    let old_windows = window_hashes(&old_rows);
    let new_windows = window_hashes(&new_rows);
    let mut positions: HashMap<u64, Vec<u32>> = HashMap::new();
    for (row, hash) in old_windows.iter().enumerate() {
        positions.entry(*hash).or_default().push(row as u32);
    }

    let mut votes: HashMap<i32, u32> = HashMap::new();
    let mut changed = 0u32;
    for (row, hash) in new_windows.iter().enumerate() {
        if old_rows[row] == new_rows[row] {
            continue;
        }
        changed += 1;
        let Some(candidates) = positions.get(hash) else {
            continue;
        };
        if candidates.len() > MAX_WINDOW_REPEATS {
            continue;
        }
        for &old_row in candidates {
            let dy = old_row as i32 - row as i32;
            if dy != 0 && dy.abs() <= MAX_SCROLL_PIXELS {
                *votes.entry(dy).or_default() += 1;
            }
        }
    }
    let (dy, best) = votes
        .into_iter()
        .max_by_key(|&(dy, count)| (count, -dy.abs()))?;
    if best < 8 || best * 4 < changed {
        return None;
    }

    let mut best_run = (0u32, 0u32);
    let mut run_start = None;
    for row in 0..=region.height {
        let explained = row < region.height && {
            let source = row as i32 + dy;
            source >= 0
                && (source as u32) < region.height
                && new_rows[row as usize] == old_rows[source as usize]
        };
        match (explained, run_start) {
            (true, None) => run_start = Some(row),
            (false, Some(start)) => {
                if row - start > best_run.1 - best_run.0 {
                    best_run = (start, row);
                }
                run_start = None;
            }
            _ => {}
        }
    }
    let rows = best_run.1 - best_run.0;
    if rows < MIN_MOVE_ROWS {
        return None;
    }
    Some(VerticalMove {
        rect: Rect::new(region.x, region.y + best_run.0, region.width, rows),
        dy,
    })
}

fn window_hashes(rows: &[u64]) -> Vec<u64> {
    rows.windows(WINDOW_ROWS)
        .map(|window| {
            let bytes: Vec<u8> = window.iter().flat_map(|hash| hash.to_le_bytes()).collect();
            xxh3_64(&bytes)
        })
        .collect()
}

fn row_hashes(frame: &Frame<'_>, region: Rect) -> Vec<u64> {
    (region.y..region.bottom())
        .map(|y| xxh3_64(frame.row_span(y, region)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameBuffer, Size};

    /// An editor: a dark background with one distinct "line of text" every 18 pixels.
    fn editor(size: Size, first_line: u32) -> FrameBuffer {
        let mut buffer = FrameBuffer::new(size);
        buffer.fill_rect(size.bounds(), [29, 25, 23, 255]).unwrap();
        for y in 0..size.height() {
            let line = (y + first_line * 18) / 18;
            if (y + first_line * 18) % 18 < 12 {
                for x in 0..(line * 37 % 300 + 40).min(size.width()) {
                    if (x * 3 + line * 7 + (y + first_line * 18) % 18) % 5 < 3 {
                        let shade = (line * 13 % 200) as u8 + 40;
                        buffer
                            .fill_rect(Rect::new(x, y, 1, 1), [shade, 232, 235, 255])
                            .unwrap();
                    }
                }
            }
        }
        buffer
    }

    #[test]
    fn scrolling_three_lines_is_one_move() {
        let size = Size::new(640, 480).unwrap();
        let before = editor(size, 0);
        let after = editor(size, 3);
        let found =
            detect_vertical_move(&before.as_frame(), &after.as_frame(), size.bounds()).unwrap();
        assert_eq!(found.dy, 54);
        assert!(
            found.rect.height >= 480 - 54 - 18,
            "move covers {} rows",
            found.rect.height
        );

        let mut moved = before.clone();
        moved.apply_move(found.rect, found.dy).unwrap();
        let mut expected = Vec::new();
        let mut actual = Vec::new();
        after.as_frame().copy_rect_into(found.rect, &mut expected);
        moved.as_frame().copy_rect_into(found.rect, &mut actual);
        assert_eq!(actual, expected);
    }

    #[test]
    fn scrolling_back_up_is_a_negative_move() {
        let size = Size::new(640, 480).unwrap();
        let found = detect_vertical_move(
            &editor(size, 5).as_frame(),
            &editor(size, 2).as_frame(),
            size.bounds(),
        )
        .unwrap();
        assert_eq!(found.dy, -54);
    }

    #[test]
    fn unrelated_changes_and_blank_screens_are_not_moves() {
        let size = Size::new(640, 480).unwrap();
        let before = editor(size, 0);
        let mut after = before.clone();
        after
            .fill_rect(Rect::new(0, 100, 640, 200), [200, 10, 10, 255])
            .unwrap();
        assert_eq!(
            detect_vertical_move(&before.as_frame(), &after.as_frame(), size.bounds()),
            None
        );

        let blank = FrameBuffer::new(size);
        assert_eq!(
            detect_vertical_move(&blank.as_frame(), &blank.as_frame(), size.bounds()),
            None
        );
    }

    #[test]
    fn small_regions_are_skipped() {
        let size = Size::new(640, 480).unwrap();
        let before = editor(size, 0);
        let after = editor(size, 1);
        assert_eq!(
            detect_vertical_move(
                &before.as_frame(),
                &after.as_frame(),
                Rect::new(0, 0, 63, 480)
            ),
            None
        );
        assert_eq!(
            detect_vertical_move(
                &before.as_frame(),
                &after.as_frame(),
                Rect::new(0, 0, 640, 127)
            ),
            None
        );
    }
}
