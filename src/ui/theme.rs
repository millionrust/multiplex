use std::sync::{OnceLock, RwLock};

use gpui::{BoxShadow, Hsla, point, px};
use termirust_ui_contract::{ColorValue, DesignTokens, StatusKind, StatusVisual, ThemeKind};

use crate::models::ThemePreset;

#[derive(Clone, Copy)]
struct ThemePalette {
    app_bg: u32,
    chrome_bg: u32,
    chrome_tab: u32,
    chrome_tab_active: u32,
    terminal_bg: u32,
    terminal_panel: u32,
    border_dark: u32,
    text_on_dark: u32,
    text_muted_dark: u32,
    slate: u32,
    hover: u32,
}

const OCEAN: ThemePalette = ThemePalette {
    app_bg: 0x0a0e17,
    chrome_bg: 0x0a0e17,
    chrome_tab: 0x171c2a,
    chrome_tab_active: 0x232b3d,
    terminal_bg: 0x07101c,
    terminal_panel: 0x0f1825,
    border_dark: 0x10141d,
    text_on_dark: 0xeaedf3,
    text_muted_dark: 0x6e7689,
    slate: 0x4863a0,
    hover: 0x1c2230,
};

const DAYLIGHT: ThemePalette = ThemePalette {
    app_bg: 0xf6f1e6,
    chrome_bg: 0x2b3a4d,
    chrome_tab: 0x3a4a5e,
    chrome_tab_active: 0x55687f,
    terminal_bg: 0x0d1620,
    terminal_panel: 0x142030,
    border_dark: 0x223040,
    text_on_dark: 0xf4f7fb,
    text_muted_dark: 0xa3b3c4,
    slate: 0x496b8f,
    hover: 0xede4cf,
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

fn palette() -> ThemePalette {
    let preset = *theme_preset_state()
        .read()
        .expect("theme preset lock poisoned");
    let mut base = match preset {
        ThemePreset::Daylight | ThemePreset::FlexokiLight | ThemePreset::KanagawaLotus => DAYLIGHT,
        _ => OCEAN,
    };
    base.terminal_bg = preset.preview_bg();
    base.terminal_panel = mix(base.terminal_bg, base.app_bg, 0.5);
    base
}

fn design_theme_kind(preset: ThemePreset) -> ThemeKind {
    match preset {
        ThemePreset::Daylight | ThemePreset::FlexokiLight | ThemePreset::KanagawaLotus => {
            ThemeKind::Light
        }
        ThemePreset::Ocean
        | ThemePreset::FlexokiDark
        | ThemePreset::KanagawaWave
        | ThemePreset::KanagawaDragon
        | ThemePreset::HackerBlue
        | ThemePreset::HackerGreen
        | ThemePreset::HackerRed => ThemeKind::Dark,
    }
}

pub const fn design_tokens_for(theme: ThemeKind) -> DesignTokens {
    DesignTokens::new(theme)
}

pub fn current_design_tokens() -> DesignTokens {
    let preset = *theme_preset_state()
        .read()
        .expect("theme preset lock poisoned");
    design_tokens_for(design_theme_kind(preset))
}

pub fn semantic_status(kind: StatusKind) -> StatusVisual {
    current_design_tokens().status(kind)
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let blend =
        |sa: u8, sb: u8| -> u8 { ((sa as f32) * (1.0 - t) + (sb as f32) * t).round() as u8 };
    let ar = ((a >> 16) & 0xff) as u8;
    let ag = ((a >> 8) & 0xff) as u8;
    let ab = (a & 0xff) as u8;
    let br = ((b >> 16) & 0xff) as u8;
    let bg = ((b >> 8) & 0xff) as u8;
    let bb = (b & 0xff) as u8;
    ((blend(ar, br) as u32) << 16) | ((blend(ag, bg) as u32) << 8) | (blend(ab, bb) as u32)
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

fn token_color(value: ColorValue) -> Hsla {
    Hsla {
        a: f32::from(value.alpha) / 255.0,
        ..gpui::rgb(
            (u32::from(value.red) << 16) | (u32::from(value.green) << 8) | u32::from(value.blue),
        )
        .into()
    }
}

pub fn app_bg() -> Hsla {
    token_color(current_design_tokens().color_bg_canvas())
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
    token_color(current_design_tokens().color_bg_canvas())
}

pub fn library_sidebar() -> Hsla {
    token_color(current_design_tokens().color_bg_surface())
}

pub fn library_card() -> Hsla {
    token_color(current_design_tokens().color_bg_surface())
}

pub fn terminal_bg() -> Hsla {
    color(palette().terminal_bg)
}

pub fn terminal_panel() -> Hsla {
    color(palette().terminal_panel)
}

pub fn border() -> Hsla {
    token_color(current_design_tokens().color_border_default())
}

pub fn border_dark() -> Hsla {
    color(palette().border_dark)
}

pub fn text_main() -> Hsla {
    token_color(current_design_tokens().color_text_primary())
}

pub fn text_on_dark() -> Hsla {
    color(palette().text_on_dark)
}

pub fn text_muted() -> Hsla {
    token_color(current_design_tokens().color_text_muted())
}

pub fn text_muted_dark() -> Hsla {
    color(palette().text_muted_dark)
}

pub fn accent() -> Hsla {
    token_color(current_design_tokens().color_action_primary())
}

pub fn accent_soft() -> Hsla {
    token_color(current_design_tokens().color_selection())
}

pub fn focus_ring() -> Hsla {
    token_color(current_design_tokens().color_focus())
}

pub fn success() -> Hsla {
    token_color(semantic_status(StatusKind::Done).color)
}

pub fn warning() -> Hsla {
    token_color(semantic_status(StatusKind::Attention).color)
}

pub fn danger() -> Hsla {
    token_color(semantic_status(StatusKind::Error).color)
}

pub fn slate() -> Hsla {
    color(palette().slate)
}

pub fn hover() -> Hsla {
    color(palette().hover)
}

pub fn modal_scrim() -> Hsla {
    token_color(current_design_tokens().color_overlay_scrim())
}

pub fn window_close() -> Hsla {
    token_color(current_design_tokens().color_window_close())
}

pub fn window_minimize() -> Hsla {
    token_color(current_design_tokens().color_window_minimize())
}

pub fn window_zoom() -> Hsla {
    token_color(current_design_tokens().color_window_zoom())
}

pub fn native_control_offset_x() -> f32 {
    -current_design_tokens()
        .layout_window_native_control_offset()
        .0
}

pub fn popover_shadow() -> Vec<BoxShadow> {
    let shadow = current_design_tokens().shadow_popover();
    if !shadow.visible {
        return Vec::new();
    }
    vec![BoxShadow {
        color: token_color(shadow.color),
        offset: point(px(shadow.x), px(shadow.y)),
        blur_radius: px(shadow.blur),
        spread_radius: px(shadow.spread),
    }]
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

pub fn card_shadow_color() -> Hsla {
    with_alpha(color(0x0a1322), 0.05)
}

pub fn soft_border() -> Hsla {
    with_alpha(color(0x0a1322), 0.06)
}

pub fn terminal_default_bg() -> Hsla {
    color(palette().terminal_bg)
}

pub fn terminal_default_fg() -> Hsla {
    let preset = *theme_preset_state()
        .read()
        .expect("theme preset lock poisoned");
    let bg = palette().terminal_bg;
    let r = (bg >> 16) & 0xff;
    let g = (bg >> 8) & 0xff;
    let b = bg & 0xff;
    let luminance = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
    let _ = preset;
    if luminance > 0.55 {
        color(0x1f2933)
    } else {
        color(0xe2e8f0)
    }
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
        ActionTone::Accent => token_color(current_design_tokens().color_action_primary_text()),
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

pub const HOST_SIDEBAR_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_host_sidebar_width()
    .0;
pub const BORDER_HAIRLINE: f32 = DesignTokens::new(ThemeKind::System).border_hairline().0;
pub const CHROME_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_chrome_height()
    .0;
pub const DIALOG_MAX_WIDTH: f32 = HOST_SIDEBAR_WIDTH + HOST_SIDEBAR_WIDTH;
pub const ICON_SIZE_DEFAULT: f32 = DesignTokens::new(ThemeKind::System).icon_size_default().0;
pub const ICON_SIZE_SMALL: f32 = DesignTokens::new(ThemeKind::System).icon_size_small().0;
pub const ICON_SIZE_MEDIUM: f32 = DesignTokens::new(ThemeKind::System).icon_size_medium().0;
pub const ICON_SIZE_LARGE: f32 = DesignTokens::new(ThemeKind::System).icon_size_large().0;
pub const ICON_SIZE_COMPACT: f32 = DesignTokens::new(ThemeKind::System).icon_size_compact().0;
pub const ICON_SIZE_STATUS: f32 = DesignTokens::new(ThemeKind::System).icon_size_status().0;
pub const ICON_SIZE_INDICATOR: f32 = DesignTokens::new(ThemeKind::System).icon_size_indicator().0;
pub const WORKSPACE_HEADER_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_workspace_header_height()
    .0;
pub const CARD_RADIUS: f32 = DesignTokens::new(ThemeKind::System).radius_panel().0;
pub const CONTROL_RADIUS: f32 = DesignTokens::new(ThemeKind::System).radius_control().0;
pub const PILL_RADIUS: f32 = DesignTokens::new(ThemeKind::System).radius_pill().0;
pub const SPACE_2: f32 = DesignTokens::new(ThemeKind::System).space_2().0;
pub const SPACE_3: f32 = DesignTokens::new(ThemeKind::System).space_3().0;
pub const SPACE_4: f32 = DesignTokens::new(ThemeKind::System).space_4().0;
pub const SPACE_5: f32 = DesignTokens::new(ThemeKind::System).space_5().0;
pub const SPACE_6: f32 = DesignTokens::new(ThemeKind::System).space_6().0;
pub const SPACE_7: f32 = DesignTokens::new(ThemeKind::System).space_7().0;
pub const SPACE_8: f32 = DesignTokens::new(ThemeKind::System).space_8().0;
pub const SPACE_9: f32 = DesignTokens::new(ThemeKind::System).space_9().0;
pub const SPACE_0: f32 = DesignTokens::new(ThemeKind::System).space_0().0;
pub const SPACE_1: f32 = DesignTokens::new(ThemeKind::System).space_1().0;
pub const SPACE_COMPACT: f32 = DesignTokens::new(ThemeKind::System).space_compact().0;
pub const SPACE_MICRO: f32 = DesignTokens::new(ThemeKind::System).space_micro().0;
pub const SPACE_FINE: f32 = DesignTokens::new(ThemeKind::System).space_fine().0;
pub const SPACE_DENSE: f32 = DesignTokens::new(ThemeKind::System).space_dense().0;
pub const SHELL_SPACE_DENSE: f32 = DesignTokens::new(ThemeKind::System).space_shell_dense().0;
pub const SHELL_SPACE_TIGHT: f32 = DesignTokens::new(ThemeKind::System).space_shell_tight().0;
pub const SHELL_SPACE_COMPACT: f32 = DesignTokens::new(ThemeKind::System).space_shell_compact().0;
pub const SHELL_BANNER_HORIZONTAL: f32 = DesignTokens::new(ThemeKind::System)
    .space_shell_banner_horizontal()
    .0;
pub const TYPE_CAPTION_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_caption().size;
pub const TYPE_BODY_SMALL_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_body_small().size;
pub const TYPE_BODY_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_body().size;
pub const TYPE_HEADING_SMALL_SIZE: f32 = DesignTokens::new(ThemeKind::System)
    .type_heading_small()
    .size;
pub const TYPE_HEADING_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_heading().size;
pub const TYPE_MICRO_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_micro().size;
pub const TYPE_NANO_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_nano().size;
pub const TYPE_METRIC_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_metric().size;
pub const STATUS_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_status_height()
    .0;
pub const CONNECT_PANEL_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_connect_panel_width()
    .0;
pub const INSPECTOR_DEFAULT_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_inspector_default()
    .0;

// Host and connection layouts preserve their current geometry while sourcing it
// exclusively from the governed design scale.
pub const HOST_CARD_WIDTH: f32 = INSPECTOR_DEFAULT_WIDTH + ICON_SIZE_MEDIUM;
pub const HOST_CARD_HEIGHT: f32 = SPACE_9;
pub const HOST_CARD_RADIUS: f32 = CARD_RADIUS + SPACE_1;
pub const HOST_ICON_SIZE_DENSE: f32 = TYPE_BODY_SMALL_SIZE;
pub const HOST_ICON_SIZE_BODY: f32 = TYPE_BODY_SIZE;
pub const HOST_ICON_SIZE_TINY: f32 = TYPE_MICRO_SIZE;
pub const HOST_CONTROL_HEIGHT: f32 = SPACE_6 + SPACE_5;
pub const HOST_MENU_WIDTH: f32 = HOST_SIDEBAR_WIDTH - HOST_CONTROL_HEIGHT;
pub const HOST_MENU_NARROW_WIDTH: f32 = SHELL_TAB_LABEL_MAXIMUM - ICON_SIZE_MEDIUM;
pub const HOST_MENU_WIDE_WIDTH: f32 = HOST_SIDEBAR_WIDTH + ICON_SIZE_MEDIUM;
pub const HOST_BULK_GROUP_WIDTH: f32 = SHELL_TAB_LABEL_MAXIMUM - SHELL_TOOLBAR_BUTTON_SIZE;
pub const HOST_OVERLAY_TOP: f32 = PALETTE_OFFSET_TOP + SPACE_DENSE;
pub const HOST_OVERLAY_LOW_TOP: f32 = PALETTE_OFFSET_TOP + SPACE_9 - SPACE_2;
pub const HOST_OVERLAY_RIGHT_WIDE: f32 = HOST_SIDEBAR_WIDTH - ICON_SIZE_MEDIUM;
pub const HOST_OVERLAY_RIGHT_NARROW: f32 = SPACE_8 + SPACE_1;
pub const HOST_TOOLBAR_OFFSET_VIEW: f32 = SHELL_TAB_LABEL_MAXIMUM + SPACE_COMPACT;
pub const HOST_TOOLBAR_OFFSET_TAG: f32 = CHROME_HEIGHT + STATUS_HEIGHT + SPACE_8 + BORDER_HAIRLINE;
pub const HOST_TOOLBAR_OFFSET_SORT: f32 = SPACE_9 + SPACE_3;
pub const CONNECT_CONTENT_TOP: f32 = SPACE_8 + SPACE_8;
pub const CONNECT_FAILURE_PANEL_WIDTH: f32 = CONNECT_PANEL_WIDTH + SPACE_8 + SPACE_8 + SPACE_2;
pub const CONNECT_PORT_WIDTH: f32 = SPACE_9 - SPACE_2;
pub const HOST_EDITOR_WIDTH: f32 = HOST_CARD_WIDTH + HOST_CONTROL_HEIGHT;
pub const HOST_EDITOR_TALL_CONTROL: f32 = SPACE_9 + SPACE_DENSE;
pub const HOST_EDITOR_ICON_CONTAINER: f32 = SHELL_COMPACT_CONTROL_HEIGHT + SPACE_1;
pub const HOST_COMPACT_ROW_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_host_compact_row_height()
    .0;

// SFTP library geometry is composed from the governed global scale so the
// dense two-pane browser preserves its current proportions without literals.
pub const SFTP_PATH_ROW_HEIGHT: f32 = SPACE_6 + SPACE_4;
pub const SFTP_COLUMN_HEADER_HEIGHT: f32 = SPACE_7;
pub const SFTP_LOCAL_ROW_HEIGHT: f32 = SFTP_PATH_ROW_HEIGHT;
pub const SFTP_HOST_ROW_HEIGHT: f32 = SPACE_8 - SPACE_1;
pub const SFTP_PICKER_BADGE_HEIGHT: f32 = SPACE_6 + SPACE_1;
pub const SFTP_ICON_CONTAINER: f32 = SPACE_7 + SPACE_1;
pub const SFTP_ICON_CONTAINER_SMALL: f32 = TYPE_HEADING_SIZE + SPACE_1;
pub const SFTP_EMPTY_ICON_CONTAINER: f32 = SPACE_9;
pub const SFTP_COLUMN_NAME_WIDTH: f32 = HOST_SIDEBAR_WIDTH + SPACE_9 - SPACE_2;
pub const SFTP_COLUMN_MODIFIED_WIDTH: f32 = (SPACE_9 + SPACE_5) + (SPACE_9 + SPACE_5);
pub const SFTP_COLUMN_SIZE_WIDTH: f32 = SPACE_9 + SPACE_5;
pub const SFTP_EMPTY_COPY_WIDTH: f32 =
    SFTP_COLUMN_SIZE_WIDTH + SFTP_COLUMN_SIZE_WIDTH + SFTP_COLUMN_SIZE_WIDTH;
pub const SFTP_EMPTY_ICON_SIZE: f32 = SPACE_6 + SPACE_2;
pub const SFTP_REMOTE_ROW_RADIUS: f32 = CARD_RADIUS + SPACE_1;
pub const SFTP_ROW_LABEL_GAP: f32 = BORDER_HAIRLINE;
pub const SHELL_COMPACT_CONTROL_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_compact_control_height()
    .0;
pub const SHELL_TOOLBAR_BUTTON_SIZE: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_toolbar_button_size()
    .0;
pub const SHELL_NAVIGATION_ROW_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_navigation_row_height()
    .0;
pub const SHELL_TAB_DROP_MINIMUM: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_tab_drop_minimum()
    .0;
pub const SHELL_TRAFFIC_LIGHT_SIZE: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_traffic_light_size()
    .0;
pub const SHELL_TAB_LABEL_MAXIMUM: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_tab_label_maximum()
    .0;
pub const SHELL_RENAME_FIELD_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_rename_field_width()
    .0;
pub const SHELL_WORKSPACE_MENU_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_workspace_menu_width()
    .0;
pub const SHELL_PANE_MENU_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_pane_menu_width()
    .0;
pub const SHELL_NAV_BADGE_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_nav_badge_width()
    .0;
pub const SHELL_NAV_BADGE_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_nav_badge_height()
    .0;
pub const PALETTE_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_palette_width()
    .0;
pub const PALETTE_OFFSET_TOP: f32 = DesignTokens::new(ThemeKind::System)
    .layout_palette_offset_top()
    .0;
pub const WINDOW_DEFAULT_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_window_default_width()
    .0;
pub const WINDOW_DEFAULT_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_window_default_height()
    .0;
