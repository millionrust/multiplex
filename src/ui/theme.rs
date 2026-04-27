use std::sync::{OnceLock, RwLock};

use gpui::Hsla;

use crate::models::ThemePreset;

#[derive(Clone, Copy)]
struct ThemePalette {
    app_bg: u32,
    chrome_bg: u32,
    chrome_tab: u32,
    chrome_tab_active: u32,
    library_bg: u32,
    library_sidebar: u32,
    library_card: u32,
    terminal_bg: u32,
    terminal_panel: u32,
    border: u32,
    border_dark: u32,
    text_main: u32,
    text_on_dark: u32,
    text_muted: u32,
    text_muted_dark: u32,
    accent: u32,
    accent_soft: u32,
    focus_ring: u32,
    success: u32,
    warning: u32,
    danger: u32,
    slate: u32,
    hover: u32,
    modal_scrim: u32,
}

const OCEAN: ThemePalette = ThemePalette {
    app_bg: 0xeef1f5,
    chrome_bg: 0x161c2b,
    chrome_tab: 0x222a3d,
    chrome_tab_active: 0x3d4d6c,
    library_bg: 0xf2f4f8,
    library_sidebar: 0xffffff,
    library_card: 0xffffff,
    terminal_bg: 0x07101c,
    terminal_panel: 0x0f1825,
    border: 0xdde2ea,
    border_dark: 0x1f2738,
    text_main: 0x1f2937,
    text_on_dark: 0xeef2fb,
    text_muted: 0x6b7585,
    text_muted_dark: 0x8e9ab1,
    accent: 0x4f87ff,
    accent_soft: 0xdbe7ff,
    focus_ring: 0x6ea0ff,
    success: 0x44c178,
    warning: 0xe89e3b,
    danger: 0xe25d5d,
    slate: 0x2c538d,
    hover: 0xe6eaf1,
    modal_scrim: 0x0a1322,
};

const DAYLIGHT: ThemePalette = ThemePalette {
    app_bg: 0xf6f1e6,
    chrome_bg: 0x2b3a4d,
    chrome_tab: 0x3a4a5e,
    chrome_tab_active: 0x55687f,
    library_bg: 0xfaf6ec,
    library_sidebar: 0xfffdf8,
    library_card: 0xfffdf8,
    terminal_bg: 0x0d1620,
    terminal_panel: 0x142030,
    border: 0xdacfba,
    border_dark: 0x223040,
    text_main: 0x2a2924,
    text_on_dark: 0xf4f7fb,
    text_muted: 0x6f6557,
    text_muted_dark: 0xa3b3c4,
    accent: 0x2f9d7e,
    accent_soft: 0xd9f4ea,
    focus_ring: 0x56b89a,
    success: 0x44b069,
    warning: 0xd99332,
    danger: 0xd9594a,
    slate: 0x496b8f,
    hover: 0xede4cf,
    modal_scrim: 0x0d141d,
};

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

fn theme_preset_state() -> &'static RwLock<ThemePreset> {
    static THEME_PRESET: OnceLock<RwLock<ThemePreset>> = OnceLock::new();
    THEME_PRESET.get_or_init(|| RwLock::new(ThemePreset::Ocean))
}

fn palette() -> &'static ThemePalette {
    let preset = *theme_preset_state()
        .read()
        .expect("theme preset lock poisoned");
    match preset {
        ThemePreset::Ocean => &OCEAN,
        ThemePreset::Daylight => &DAYLIGHT,
    }
}

pub fn set_theme_preset(preset: ThemePreset) {
    *theme_preset_state()
        .write()
        .expect("theme preset lock poisoned") = preset;
}

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
    color(palette().app_bg)
}

pub fn chrome_bg() -> Hsla {
    color(palette().chrome_bg)
}

pub fn chrome_tab() -> Hsla {
    color(palette().chrome_tab)
}

pub fn chrome_tab_active() -> Hsla {
    color(palette().chrome_tab_active)
}

pub fn library_bg() -> Hsla {
    color(palette().library_bg)
}

pub fn library_sidebar() -> Hsla {
    color(palette().library_sidebar)
}

pub fn library_card() -> Hsla {
    color(palette().library_card)
}

pub fn terminal_bg() -> Hsla {
    color(palette().terminal_bg)
}

pub fn terminal_panel() -> Hsla {
    color(palette().terminal_panel)
}

pub fn border() -> Hsla {
    color(palette().border)
}

pub fn border_dark() -> Hsla {
    color(palette().border_dark)
}

pub fn text_main() -> Hsla {
    color(palette().text_main)
}

pub fn text_on_dark() -> Hsla {
    color(palette().text_on_dark)
}

pub fn text_muted() -> Hsla {
    color(palette().text_muted)
}

pub fn text_muted_dark() -> Hsla {
    color(palette().text_muted_dark)
}

pub fn accent() -> Hsla {
    color(palette().accent)
}

pub fn accent_soft() -> Hsla {
    color(palette().accent_soft)
}

pub fn focus_ring() -> Hsla {
    color(palette().focus_ring)
}

pub fn success() -> Hsla {
    color(palette().success)
}

pub fn warning() -> Hsla {
    color(palette().warning)
}

pub fn danger() -> Hsla {
    color(palette().danger)
}

pub fn slate() -> Hsla {
    color(palette().slate)
}

pub fn hover() -> Hsla {
    color(palette().hover)
}

pub fn modal_scrim() -> Hsla {
    with_alpha(color(palette().modal_scrim), 0.56)
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
