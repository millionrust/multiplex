use gpui::Hsla;

const APP_BG: u32 = 0xe9edf2;
const CHROME_BG: u32 = 0x1d2234;
const CHROME_TAB: u32 = 0x2a3043;
const CHROME_TAB_ACTIVE: u32 = 0x3a4358;
const LIBRARY_BG: u32 = 0xeef2f5;
const LIBRARY_SIDEBAR: u32 = 0xf7f8fa;
const LIBRARY_CARD: u32 = 0xffffff;
const TERMINAL_BG: u32 = 0x091521;
const TERMINAL_PANEL: u32 = 0x111c2b;
const BORDER: u32 = 0xd2d9e2;
const BORDER_DARK: u32 = 0x2b3547;
const TEXT_MAIN: u32 = 0x293345;
const TEXT_ON_DARK: u32 = 0xe5edf7;
const TEXT_MUTED: u32 = 0x838e9e;
const TEXT_MUTED_DARK: u32 = 0x95a4b8;
const ACCENT: u32 = 0x4f87ff;
const ACCENT_SOFT: u32 = 0xdbe7ff;
const FOCUS_RING: u32 = 0x6ea0ff;
const SUCCESS: u32 = 0x67c778;
const WARNING: u32 = 0xf2b24d;
const DANGER: u32 = 0xed7b63;
const SLATE: u32 = 0x2c538d;
const HOVER: u32 = 0xe2e8ef;
const MODAL_SCRIM: u32 = 0x07111d;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionTone {
    Accent,
    AccentSoft,
    Success,
    Danger,
    Neutral,
}

fn color(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

pub fn app_bg() -> Hsla {
    color(APP_BG)
}

pub fn chrome_bg() -> Hsla {
    color(CHROME_BG)
}

pub fn chrome_tab() -> Hsla {
    color(CHROME_TAB)
}

pub fn chrome_tab_active() -> Hsla {
    color(CHROME_TAB_ACTIVE)
}

pub fn library_bg() -> Hsla {
    color(LIBRARY_BG)
}

pub fn library_sidebar() -> Hsla {
    color(LIBRARY_SIDEBAR)
}

pub fn library_card() -> Hsla {
    color(LIBRARY_CARD)
}

pub fn terminal_bg() -> Hsla {
    color(TERMINAL_BG)
}

pub fn terminal_panel() -> Hsla {
    color(TERMINAL_PANEL)
}

pub fn border() -> Hsla {
    color(BORDER)
}

pub fn border_dark() -> Hsla {
    color(BORDER_DARK)
}

pub fn text_main() -> Hsla {
    color(TEXT_MAIN)
}

pub fn text_on_dark() -> Hsla {
    color(TEXT_ON_DARK)
}

pub fn text_muted() -> Hsla {
    color(TEXT_MUTED)
}

pub fn text_muted_dark() -> Hsla {
    color(TEXT_MUTED_DARK)
}

pub fn accent() -> Hsla {
    color(ACCENT)
}

pub fn accent_soft() -> Hsla {
    color(ACCENT_SOFT)
}

pub fn focus_ring() -> Hsla {
    color(FOCUS_RING)
}

pub fn success() -> Hsla {
    color(SUCCESS)
}

pub fn warning() -> Hsla {
    color(WARNING)
}

pub fn danger() -> Hsla {
    color(DANGER)
}

pub fn slate() -> Hsla {
    color(SLATE)
}

pub fn hover() -> Hsla {
    color(HOVER)
}

pub fn modal_scrim() -> Hsla {
    with_alpha(color(MODAL_SCRIM), 0.56)
}

pub fn card_hover() -> Hsla {
    with_alpha(hover(), 0.82)
}

pub fn card_hover_subtle() -> Hsla {
    with_alpha(hover(), 0.5)
}

pub fn pane_focus_glow() -> Hsla {
    with_alpha(accent(), 0.15)
}

pub fn avatar_glow(accent: Hsla) -> Hsla {
    with_alpha(accent, 0.3)
}

pub fn terminal_selection_bg() -> Hsla {
    accent_soft()
}

pub fn terminal_selection_fg() -> Hsla {
    text_main()
}

pub fn terminal_search_match_bg() -> Hsla {
    with_alpha(warning(), 0.38)
}

pub fn terminal_search_active_match_bg() -> Hsla {
    with_alpha(accent(), 0.52)
}

pub fn action_fill(tone: ActionTone) -> Hsla {
    match tone {
        ActionTone::Accent => accent(),
        ActionTone::AccentSoft => accent_soft(),
        ActionTone::Success => success(),
        ActionTone::Danger => danger(),
        ActionTone::Neutral => hover(),
    }
}

pub fn action_foreground(tone: ActionTone) -> Hsla {
    match tone {
        ActionTone::Accent => library_card(),
        ActionTone::AccentSoft | ActionTone::Success | ActionTone::Danger | ActionTone::Neutral => {
            text_main()
        }
    }
}

pub fn action_border(tone: ActionTone) -> Hsla {
    with_alpha(action_fill(tone), 0.4)
}

pub fn action_hover(tone: ActionTone) -> Hsla {
    with_alpha(action_fill(tone), 0.92)
}

pub fn action_active(tone: ActionTone) -> Hsla {
    with_alpha(action_fill(tone), 0.8)
}

pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

pub fn host_chip_color(label: &str) -> Hsla {
    let hash = label
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let index = (hash as usize) % HOST_CHIP_COLORS.len();
    color(HOST_CHIP_COLORS[index])
}

pub const HOST_SIDEBAR_WIDTH: f32 = 200.0;
pub const CHROME_HEIGHT: f32 = 50.0;
pub const CHROME_INSET_LEFT: f32 = 76.0;
pub const STATUS_HEIGHT: f32 = 30.0;
pub const WORKSPACE_HEADER_HEIGHT: f32 = 50.0;
pub const CARD_RADIUS: f32 = 12.0;
