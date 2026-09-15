use termirust_screen_codec::Size;

/// A position in the global display arrangement, in points. The main display's top-left corner
/// is the origin; displays to its left or above it have negative coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Where one captured surface sits in the global display arrangement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayPlacement {
    surface: u32,
    pixels: Size,
    origin: Point,
    points_per_pixel_x: f64,
    points_per_pixel_y: f64,
}

impl DisplayPlacement {
    /// `pixels` is the captured surface size; `origin_points` and `size_points` are the display's
    /// frame in the arrangement.
    pub fn new(surface: u32, pixels: Size, origin_points: (i32, i32), size_points: Size) -> Self {
        Self {
            surface,
            pixels,
            origin: Point {
                x: f64::from(origin_points.0),
                y: f64::from(origin_points.1),
            },
            points_per_pixel_x: f64::from(size_points.width()) / f64::from(pixels.width()),
            points_per_pixel_y: f64::from(size_points.height()) / f64::from(pixels.height()),
        }
    }

    pub const fn surface(&self) -> u32 {
        self.surface
    }

    /// The global point under surface pixel (`x`, `y`). Coordinates past the edge are clamped to
    /// the last pixel, so a pointer dragged off the viewer's picture stays on this display.
    pub fn to_global(&self, x: u32, y: u32) -> Point {
        let x = x.min(self.pixels.width() - 1);
        let y = y.min(self.pixels.height() - 1);
        Point {
            x: self.origin.x + f64::from(x) * self.points_per_pixel_x,
            y: self.origin.y + f64::from(y) * self.points_per_pixel_y,
        }
    }

    /// A distance of `dx`, `dy` surface pixels, in points.
    pub fn to_points(&self, dx: f64, dy: f64) -> (f64, f64) {
        (dx * self.points_per_pixel_x, dy * self.points_per_pixel_y)
    }
}

/// The placements of every surface a viewer can send input to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayLayout {
    placements: Vec<DisplayPlacement>,
}

impl DisplayLayout {
    pub fn new(placements: Vec<DisplayPlacement>) -> Self {
        Self { placements }
    }

    pub fn placement(&self, surface: u32) -> Option<&DisplayPlacement> {
        self.placements
            .iter()
            .find(|placement| placement.surface == surface)
    }
}
