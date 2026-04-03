use gpui::Hsla;

pub fn app_bg() -> Hsla {
    gpui::rgb(0xe9edf2).into()
}

pub fn chrome_bg() -> Hsla {
    gpui::rgb(0x1d2234).into()
}

pub fn chrome_tab() -> Hsla {
    gpui::rgb(0x2a3043).into()
}

pub fn chrome_tab_active() -> Hsla {
    gpui::rgb(0x3a4358).into()
}

pub fn library_bg() -> Hsla {
    gpui::rgb(0xeef2f5).into()
}

pub fn library_sidebar() -> Hsla {
    gpui::rgb(0xf7f8fa).into()
}

pub fn library_card() -> Hsla {
    gpui::rgb(0xffffff).into()
}

pub fn terminal_bg() -> Hsla {
    gpui::rgb(0x091521).into()
}

pub fn terminal_panel() -> Hsla {
    gpui::rgb(0x111c2b).into()
}

pub fn border() -> Hsla {
    gpui::rgb(0xd2d9e2).into()
}

pub fn border_dark() -> Hsla {
    gpui::rgb(0x2b3547).into()
}

pub fn text_main() -> Hsla {
    gpui::rgb(0x293345).into()
}

pub fn text_on_dark() -> Hsla {
    gpui::rgb(0xe5edf7).into()
}

pub fn text_muted() -> Hsla {
    gpui::rgb(0x838e9e).into()
}

pub fn text_muted_dark() -> Hsla {
    gpui::rgb(0x95a4b8).into()
}

pub fn accent() -> Hsla {
    gpui::rgb(0x4f87ff).into()
}

pub fn accent_soft() -> Hsla {
    gpui::rgb(0xdbe7ff).into()
}

pub fn focus_ring() -> Hsla {
    gpui::rgb(0x6ea0ff).into()
}

pub fn success() -> Hsla {
    gpui::rgb(0x67c778).into()
}

pub fn warning() -> Hsla {
    gpui::rgb(0xf2b24d).into()
}

pub fn danger() -> Hsla {
    gpui::rgb(0xed7b63).into()
}

pub fn slate() -> Hsla {
    gpui::rgb(0x2c538d).into()
}

pub fn hover() -> Hsla {
    gpui::rgb(0xe2e8ef).into()
}

pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

const HOST_CHIP_COLORS: &[u32] = &[
    0xdd6b2d, // orange
    0x2c538d, // slate blue
    0x0d9488, // teal
    0x7c3aed, // indigo
    0xbe185d, // rose
    0xb45309, // amber
    0x059669, // emerald
    0x6366f1, // violet
];

pub fn host_chip_color(label: &str) -> Hsla {
    let hash = label
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let index = (hash as usize) % HOST_CHIP_COLORS.len();
    gpui::rgb(HOST_CHIP_COLORS[index]).into()
}

pub const HOST_SIDEBAR_WIDTH: f32 = 200.0;
pub const CHROME_HEIGHT: f32 = 50.0;
pub const CHROME_INSET_LEFT: f32 = 76.0;
pub const STATUS_HEIGHT: f32 = 30.0;
pub const WORKSPACE_HEADER_HEIGHT: f32 = 50.0;
pub const CARD_RADIUS: f32 = 12.0;
