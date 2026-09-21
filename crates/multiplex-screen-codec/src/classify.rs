use crate::{BYTES_PER_PIXEL, Frame, Rect};

/// Most distinct colours a tile may have and still be sent as a palette.
pub const MAX_PALETTE_COLORS: usize = 64;

/// A neighbouring pixel pair whose largest channel difference reaches this is an edge.
pub const EDGE_CHANNEL_DELTA: u8 = 48;

/// Share of neighbouring pairs, in thousandths, above which a many-coloured tile is treated as
/// text or UI rather than a picture. Antialiased text on a photo lands above it; photos below.
pub const TEXT_EDGE_DENSITY_MILLI: u32 = 180;

/// How a changed tile is best sent. Alpha is ignored: captured screens are opaque.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TileClass {
    /// Every pixel has this BGR colour.
    Solid([u8; 3]),
    /// Few colours or many hard edges: send losslessly.
    Text,
    /// Many colours and smooth: a lossy first pass is acceptable.
    Picture,
}

/// Classifies the pixels of `rect`. Reads each pixel once and stops counting colours at 65.
pub fn classify(frame: &Frame<'_>, rect: Rect) -> TileClass {
    let mut palette: Vec<[u8; 3]> = Vec::with_capacity(MAX_PALETTE_COLORS + 1);
    let mut last = None;
    let mut edges: u32 = 0;
    let mut pairs: u32 = 0;
    for y in rect.y..rect.bottom() {
        let row = frame.row_span(y, rect);
        let mut previous: Option<[u8; 3]> = None;
        for pixel in row.as_chunks::<BYTES_PER_PIXEL>().0 {
            let color = [pixel[0], pixel[1], pixel[2]];
            if palette.len() <= MAX_PALETTE_COLORS
                && last != Some(color)
                && !palette.contains(&color)
            {
                palette.push(color);
            }
            last = Some(color);
            if let Some(previous) = previous {
                pairs += 1;
                if max_channel_delta(previous, color) >= EDGE_CHANNEL_DELTA {
                    edges += 1;
                }
            }
            previous = Some(color);
        }
    }
    match palette.len() {
        1 => TileClass::Solid(palette[0]),
        count if count <= MAX_PALETTE_COLORS => TileClass::Text,
        _ if pairs > 0 && edges * 1000 / pairs >= TEXT_EDGE_DENSITY_MILLI => TileClass::Text,
        _ => TileClass::Picture,
    }
}

fn max_channel_delta(a: [u8; 3], b: [u8; 3]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(y))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameBuffer, Size};

    fn tile(fill: impl Fn(u32, u32) -> [u8; 3]) -> FrameBuffer {
        let size = Size::new(64, 64).unwrap();
        let mut buffer = FrameBuffer::new(size);
        let pixels: Vec<u8> = (0..64)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let [b, g, r] = fill(x, y);
                [b, g, r, 255]
            })
            .collect();
        buffer.write_rect(size.bounds(), &pixels).unwrap();
        buffer
    }

    fn class(buffer: &FrameBuffer) -> TileClass {
        classify(&buffer.as_frame(), Rect::new(0, 0, 64, 64))
    }

    #[test]
    fn one_colour_is_solid() {
        assert_eq!(
            class(&tile(|_, _| [30, 29, 23])),
            TileClass::Solid([30, 29, 23])
        );
    }

    #[test]
    fn glyphs_on_a_background_are_text() {
        let glyphs = tile(|x, y| {
            if (x / 3 + y / 5) % 4 == 0 {
                [230, 232, 235]
            } else {
                [29, 25, 23]
            }
        });
        assert_eq!(class(&glyphs), TileClass::Text);
    }

    #[test]
    fn antialiased_text_with_many_greys_is_still_text() {
        let antialiased = tile(|x, y| {
            let stroke = 128 + ((y * 7 + x * 13) % 97) as u8;
            if x % 8 < 2 {
                [stroke, stroke, stroke]
            } else {
                [23, 25, 29]
            }
        });
        assert_eq!(class(&antialiased), TileClass::Text);
    }

    #[test]
    fn smooth_gradients_are_pictures() {
        let gradient = tile(|x, y| [(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8]);
        assert_eq!(class(&gradient), TileClass::Picture);
    }

    #[test]
    fn edge_tiles_use_only_their_own_pixels() {
        let size = Size::new(70, 10).unwrap();
        let mut buffer = FrameBuffer::new(size);
        buffer
            .fill_rect(Rect::new(0, 0, 64, 10), [9, 9, 9, 255])
            .unwrap();
        assert_eq!(
            classify(&buffer.as_frame(), Rect::new(64, 0, 6, 10)),
            TileClass::Solid([0, 0, 0])
        );
    }
}
