use std::sync::{OnceLock, RwLock};

use gpui::{BoxShadow, Hsla, point, px};
use termirust_ui_contract::{ColorValue, DesignTokens, StatusKind, StatusVisual, ThemeKind};

use crate::models::ThemePreset;

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

#[derive(Clone, Copy)]
struct ThemeState {
    preset: ThemePreset,
    system_is_dark: bool,
}

fn theme_state() -> &'static RwLock<ThemeState> {
    static THEME: OnceLock<RwLock<ThemeState>> = OnceLock::new();
    THEME.get_or_init(|| {
        RwLock::new(ThemeState {
            preset: ThemePreset::System,
            system_is_dark: true,
        })
    })
}

fn read_theme_state() -> ThemeState {
    *theme_state()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The token theme a choice resolves to. System follows the operating system appearance.
pub fn design_theme_kind(preset: ThemePreset, system_is_dark: bool) -> ThemeKind {
    match preset {
        ThemePreset::System if system_is_dark => ThemeKind::Dark,
        ThemePreset::System => ThemeKind::Light,
        ThemePreset::Dark => ThemeKind::Dark,
        ThemePreset::Light => ThemeKind::Light,
        ThemePreset::HighContrast => ThemeKind::HighContrast,
        ThemePreset::Recording => ThemeKind::RecordingFriendly,
    }
}

pub const fn design_tokens_for(theme: ThemeKind) -> DesignTokens {
    DesignTokens::new(theme)
}

pub fn current_theme_kind() -> ThemeKind {
    let state = read_theme_state();
    design_theme_kind(state.preset, state.system_is_dark)
}

pub fn current_design_tokens() -> DesignTokens {
    design_tokens_for(current_theme_kind())
}

pub fn semantic_status(kind: StatusKind) -> StatusVisual {
    current_design_tokens().status(kind)
}

pub fn motion_duration(value: termirust_ui_contract::DurationValue) -> std::time::Duration {
    std::time::Duration::from_millis(u64::from(value.0))
}

pub fn token_status_color(kind: StatusKind) -> Hsla {
    token_color(semantic_status(kind).color)
}

pub fn set_theme_preset(preset: ThemePreset) {
    theme_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .preset = preset;
}

/// Records the operating system appearance, which the System theme follows.
pub fn set_system_appearance(appearance: gpui::WindowAppearance) {
    theme_state()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .system_is_dark = matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    );
}

/// Recolors gpui-component controls (buttons, inputs, lists, scrollbars, popovers) from the
/// current tokens, so they match the rest of the app in every theme.
pub fn apply_to_components(window: Option<&mut gpui::Window>, cx: &mut gpui::App) {
    let kind = current_theme_kind();
    let tokens = design_tokens_for(kind);
    let mode = if kind == ThemeKind::Light {
        gpui_component::ThemeMode::Light
    } else {
        gpui_component::ThemeMode::Dark
    };
    gpui_component::Theme::change(mode, window, cx);
    let c = |value: ColorValue| token_color(value);
    let colors = &mut gpui_component::Theme::global_mut(cx).colors;
    let canvas = c(tokens.color_bg_canvas());
    let surface = c(tokens.color_bg_surface());
    let elevated = c(tokens.color_bg_elevated());
    let control = c(tokens.color_bg_control());
    let control_hover = c(tokens.color_bg_control_hover());
    let hover = c(tokens.color_bg_hover());
    let selected = c(tokens.color_bg_selected());
    let text = c(tokens.color_text_primary());
    let muted_text = c(tokens.color_text_muted());
    let border = c(tokens.color_border_default());
    let strong_border = c(tokens.color_border_strong());
    let accent = c(tokens.color_action_primary());
    let accent_text = c(tokens.color_action_primary_text());
    let status = |kind: StatusKind| c(tokens.status(kind).color);

    colors.background = canvas;
    colors.foreground = text;
    colors.border = border;
    colors.input = strong_border;
    colors.ring = c(tokens.color_focus());
    colors.selection = c(tokens.color_selection());
    colors.caret = c(tokens.color_terminal_cursor());
    colors.muted = control;
    colors.muted_foreground = muted_text;
    colors.accent = hover;
    colors.accent_foreground = text;
    colors.primary = accent;
    colors.primary_hover = accent.opacity(0.9);
    colors.primary_active = accent.opacity(0.8);
    colors.primary_foreground = accent_text;
    colors.secondary = control;
    colors.secondary_hover = control_hover;
    colors.secondary_active = selected;
    colors.secondary_foreground = text;
    colors.danger = status(StatusKind::Error);
    colors.danger_hover = colors.danger.opacity(0.9);
    colors.danger_active = colors.danger.opacity(0.8);
    colors.danger_foreground = accent_text;
    colors.success = status(StatusKind::Done);
    colors.success_hover = colors.success.opacity(0.9);
    colors.success_active = colors.success.opacity(0.8);
    colors.success_foreground = accent_text;
    colors.info = accent;
    colors.info_hover = accent.opacity(0.9);
    colors.info_active = accent.opacity(0.8);
    colors.info_foreground = accent_text;
    colors.link = accent;
    colors.link_hover = accent.opacity(0.9);
    colors.link_active = accent.opacity(0.8);
    colors.popover = elevated;
    colors.popover_foreground = text;
    colors.list = canvas;
    colors.list_even = canvas;
    colors.list_head = surface;
    colors.list_hover = hover;
    colors.list_active = selected;
    colors.list_active_border = accent;
    colors.scrollbar = gpui::transparent_black();
    colors.scrollbar_thumb = strong_border;
    colors.scrollbar_thumb_hover = c(tokens.color_border_focus());
    colors.sidebar = c(tokens.color_bg_chrome());
    colors.sidebar_foreground = text;
    colors.sidebar_border = border;
    colors.sidebar_accent = selected;
    colors.sidebar_accent_foreground = text;
    colors.sidebar_primary = accent;
    colors.sidebar_primary_foreground = accent_text;
    colors.tab_bar = c(tokens.color_bg_chrome());
    colors.tab = gpui::transparent_black();
    colors.tab_active = selected;
    colors.tab_foreground = muted_text;
    colors.tab_active_foreground = text;
    colors.switch = strong_border;
    colors.switch_thumb = text;
    colors.skeleton = control;
    colors.drop_target = accent.opacity(0.18);
    colors.drag_border = accent;
    cx.refresh_windows();
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

/// The title bar and tab strip.
pub fn chrome_bg() -> Hsla {
    token_color(current_design_tokens().color_bg_chrome())
}

/// A hovered, inactive tab.
pub fn chrome_tab() -> Hsla {
    token_color(current_design_tokens().color_bg_hover())
}

/// The active tab.
pub fn chrome_tab_active() -> Hsla {
    token_color(current_design_tokens().color_bg_selected())
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
    token_color(current_design_tokens().color_bg_terminal())
}

/// Pane headers, bars, and panels drawn over or beside a terminal.
pub fn terminal_panel() -> Hsla {
    token_color(current_design_tokens().color_bg_surface())
}

pub fn border() -> Hsla {
    token_color(current_design_tokens().color_border_default())
}

/// Pane edges and separators on chrome and terminal surfaces.
pub fn border_dark() -> Hsla {
    token_color(current_design_tokens().color_border_default())
}

pub fn text_main() -> Hsla {
    token_color(current_design_tokens().color_text_primary())
}

/// Primary text on chrome and terminal surfaces. These follow the theme, so this is the
/// same color as [`text_main`] and is kept for the call sites that name the surface.
pub fn text_on_dark() -> Hsla {
    token_color(current_design_tokens().color_text_primary())
}

pub fn text_muted() -> Hsla {
    token_color(current_design_tokens().color_text_muted())
}

/// Captions and inactive labels on chrome and terminal surfaces.
pub fn text_muted_dark() -> Hsla {
    token_color(current_design_tokens().color_text_muted())
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

/// The neutral tone for badges and chips that carry no status.
pub fn slate() -> Hsla {
    token_color(current_design_tokens().color_text_secondary())
}

pub fn hover() -> Hsla {
    token_color(current_design_tokens().color_bg_hover())
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

pub fn soft_border() -> Hsla {
    token_color(current_design_tokens().color_border_subtle())
}

pub fn border_strong() -> Hsla {
    token_color(current_design_tokens().color_border_strong())
}

/// Resting fill of an input-like control, such as the selected segment of a segmented control.
pub fn control_bg() -> Hsla {
    token_color(current_design_tokens().color_bg_control())
}

/// Secondary text, between primary text and muted captions.
pub fn text_secondary() -> Hsla {
    token_color(current_design_tokens().color_text_secondary())
}

pub fn terminal_default_bg() -> Hsla {
    terminal_bg()
}

pub fn terminal_default_fg() -> Hsla {
    token_color(current_design_tokens().color_terminal_fg())
}

/// The block cursor. Text under it is drawn in the terminal background color.
pub fn terminal_cursor() -> Hsla {
    token_color(current_design_tokens().color_terminal_cursor())
}

/// One of the sixteen named terminal colors, SGR 0 through 15. Higher indexes are the
/// fixed xterm cube and gray ramp, which are not themed.
pub fn terminal_ansi(index: u8) -> Option<Hsla> {
    let tokens = current_design_tokens();
    let value = match index {
        0 => tokens.color_terminal_ansi_black(),
        1 => tokens.color_terminal_ansi_red(),
        2 => tokens.color_terminal_ansi_green(),
        3 => tokens.color_terminal_ansi_yellow(),
        4 => tokens.color_terminal_ansi_blue(),
        5 => tokens.color_terminal_ansi_magenta(),
        6 => tokens.color_terminal_ansi_cyan(),
        7 => tokens.color_terminal_ansi_white(),
        8 => tokens.color_terminal_ansi_bright_black(),
        9 => tokens.color_terminal_ansi_bright_red(),
        10 => tokens.color_terminal_ansi_bright_green(),
        11 => tokens.color_terminal_ansi_bright_yellow(),
        12 => tokens.color_terminal_ansi_bright_blue(),
        13 => tokens.color_terminal_ansi_bright_magenta(),
        14 => tokens.color_terminal_ansi_bright_cyan(),
        15 => tokens.color_terminal_ansi_bright_white(),
        _ => return None,
    };
    Some(token_color(value))
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
pub const TYPE_TITLE_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_title().size;
pub const TYPE_BODY_LARGE_SIZE: f32 = DesignTokens::new(ThemeKind::System).type_body_large().size;
pub const TYPE_ACTIVITY_TITLE_SIZE: f32 = DesignTokens::new(ThemeKind::System)
    .type_activity_title()
    .size;
pub const ICON_BUTTON_SIZE: f32 = DesignTokens::new(ThemeKind::System)
    .layout_shell_icon_button_size()
    .0;
pub const LIST_ROW_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_list_row_height()
    .0;
pub const DIALOG_WIDTH: f32 = DesignTokens::new(ThemeKind::System).layout_dialog_width().0;
pub const DIALOG_WIDE_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_dialog_wide_width()
    .0;
pub const DIALOG_RADIUS: f32 = DesignTokens::new(ThemeKind::System).radius_dialog().0;
pub const SETTINGS_NAV_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_settings_nav_width()
    .0;
pub const SETTINGS_CONTENT_MAX_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_settings_content_max_width()
    .0;
pub const SEGMENT_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_control_segment_height()
    .0;
pub const SECURITY_DIALOG_MAXIMUM: f32 = DesignTokens::new(ThemeKind::System)
    .layout_security_dialog_maximum()
    .0;
pub const WINDOW_MINIMUM_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_window_minimum_height()
    .0;
/// The tile behind an empty-state icon.
pub const EMPTY_STATE_ICON_TILE: f32 = SPACE_8 + SPACE_3;
/// A pairing QR code, large enough for a phone camera at arm's length.
pub const PAIRING_QR_SIZE: f32 = SPACE_9 * 3.0 + SPACE_5;
/// A short scrolling list: four and a half rows, so a partial row shows it scrolls.
pub const COMPACT_LIST_MAX_HEIGHT: f32 = LIST_ROW_HEIGHT * 4.5;
/// Space around the workspace pane area.
pub const WORKSPACE_PADDING: f32 = SPACE_5;
/// Space between split panes.
pub const PANE_GAP: f32 = SPACE_4;
/// The workspace search bar row.
pub const WORKSPACE_SEARCH_ROW_HEIGHT: f32 = SPACE_8 + SPACE_2;
pub const TERMINAL_PADDING_X: f32 = DesignTokens::new(ThemeKind::System)
    .space_terminal_inline()
    .0;
pub const TERMINAL_PADDING_Y: f32 = DesignTokens::new(ThemeKind::System)
    .space_terminal_padding()
    .0;
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
pub const SETTINGS_THEME_PREVIEW_WIDTH: f32 = SPACE_9 + SPACE_9 + SPACE_8 + SPACE_2;
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
pub const HOST_OVERLAY_TOP: f32 = TOOLBAR_MENU_OFFSET_TOP + SPACE_DENSE;
pub const HOST_OVERLAY_LOW_TOP: f32 = TOOLBAR_MENU_OFFSET_TOP + SPACE_9 - SPACE_2;
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

// Remote Screens geometry, composed from the governed global scale: a display's
// preview keeps a screen's 16:10 proportions, and the panel beside a watched
// screen is narrower than an inspector because it lists displays, not fields.
pub const SCREEN_PREVIEW_WIDTH: f32 = SPACE_8 + SPACE_8;
pub const SCREEN_PREVIEW_HEIGHT: f32 = SPACE_8 + SPACE_4;
pub const SCREEN_PANEL_WIDTH: f32 = SPACE_9 + SPACE_9 + SPACE_9 + SPACE_8;

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

// Vault, key, and Snippet surfaces use named geometry so their compact controls
// remain stable across desktop scale factors without reintroducing raw literals.
pub const SENSITIVE_TAB_HEIGHT: f32 = SPACE_6 + SPACE_1;
pub const SENSITIVE_ICON_TILE_SIZE: f32 = SPACE_8 - SPACE_1;
pub const SENSITIVE_MEMBER_PADDING_Y: f32 = SPACE_DENSE + SPACE_FINE;
pub const SENSITIVE_FORM_MAX_WIDTH: f32 = DIALOG_MAX_WIDTH + HOST_SIDEBAR_WIDTH + SPACE_6;
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
pub const TOOLBAR_MENU_OFFSET_TOP: f32 = DesignTokens::new(ThemeKind::System)
    .layout_toolbar_menu_offset_top()
    .0;
pub const WINDOW_DEFAULT_WIDTH: f32 = DesignTokens::new(ThemeKind::System)
    .layout_window_default_width()
    .0;
pub const WINDOW_DEFAULT_HEIGHT: f32 = DesignTokens::new(ThemeKind::System)
    .layout_window_default_height()
    .0;
// Agent Canvas geometry is composed from the governed global scale. These
// aliases preserve the established dense canvas layout while keeping feature
// code free of visual literals.
pub const CANVAS_KEYBOARD_MOVE_STEP: f32 = SPACE_6;
pub const CANVAS_TOOLBAR_HEIGHT: f32 = SPACE_7 + SPACE_4;
pub const CANVAS_KEYBOARD_REVEAL_PADDING: f32 = SPACE_6;
pub const CANVAS_NODE_HEADER_HEIGHT: f32 = SPACE_7 + SPACE_1;
pub const CANVAS_FIT_PADDING: f32 = SPACE_8;
pub const CANVAS_TRANSCRIPT_FONT_SIZE: f32 = TYPE_CAPTION_SIZE;
pub const CANVAS_TRANSCRIPT_LINE_HEIGHT: f32 = TYPE_HEADING_SIZE;
pub const CANVAS_TRANSCRIPT_PADDING: f32 = SPACE_4;
pub const CANVAS_MINIMAP_WIDTH: f32 = HOST_SIDEBAR_WIDTH - SPACE_7 - SPACE_2;
pub const CANVAS_MINIMAP_HEIGHT: f32 = SPACE_9 + SPACE_8 + SPACE_2;
pub const CANVAS_MINIMAP_PADDING: f32 = SPACE_3;
pub const CANVAS_MINIMAP_MARGIN: f32 = SPACE_4;
pub const CANVAS_PROJECT_PANEL_WIDTH: f32 = CONNECT_PANEL_WIDTH + SPACE_9 + SPACE_7 + SPACE_2;
pub const CANVAS_NOTE_WIDTH: f32 = CONNECT_PANEL_WIDTH;
pub const CANVAS_NOTE_HEIGHT: f32 = HOST_SIDEBAR_WIDTH + SPACE_9 + SPACE_5;
pub const CANVAS_GROUP_WIDTH: f32 =
    CANVAS_NOTE_WIDTH + CANVAS_PROJECT_PANEL_WIDTH - SPACE_7 - SPACE_3;
pub const CANVAS_GROUP_HEIGHT: f32 = CANVAS_NOTE_HEIGHT + CANVAS_NOTE_HEIGHT;
pub const CANVAS_PROGRESS_RADIUS: f32 = DesignTokens::new(ThemeKind::System).radius_progress().0;
pub const CANVAS_EDGE_LABEL_RADIUS: f32 = SPACE_2 + BORDER_HAIRLINE;
pub const CANVAS_POPOVER_RADIUS: f32 = CONTROL_RADIUS + BORDER_HAIRLINE;
pub const CANVAS_NANO_SIZE: f32 = SPACE_3 + BORDER_HAIRLINE;
pub const CANVAS_COMPACT_ICON_SIZE: f32 = TYPE_BODY_SMALL_SIZE + SPACE_1;
pub const CANVAS_METADATA_LINE_HEIGHT: f32 = SPACE_5 + SPACE_1;
pub const CANVAS_EMPTY_ICON_SIZE: f32 = SPACE_6 + SPACE_1;
pub const CANVAS_DENSE_ROW_HEIGHT: f32 = SPACE_7 - SPACE_1;
pub const CANVAS_ACTION_ROW_HEIGHT: f32 = SPACE_7 + CONTROL_RADIUS;
pub const CANVAS_CONTROL_HEIGHT: f32 = SPACE_7 + SPACE_3;
pub const CANVAS_PANEL_HEADER_COMPACT: f32 = SPACE_7 + SPACE_3 + SPACE_1;
pub const CANVAS_LIST_ROW_MIN_HEIGHT: f32 = SPACE_8 - SPACE_1;
pub const CANVAS_SUMMARY_ROW_HEIGHT: f32 = SPACE_8 + CONTROL_RADIUS;
pub const CANVAS_INSPECTOR_MIN_HEIGHT: f32 = HOST_SIDEBAR_WIDTH - SPACE_9 + SPACE_2;
pub const CANVAS_INSPECTOR_HEIGHT: f32 = HOST_SIDEBAR_WIDTH - SPACE_8 + SPACE_3;
pub const CANVAS_INSPECTOR_WIDTH: f32 = HOST_SIDEBAR_WIDTH - SPACE_7;
pub const CANVAS_MIN_PANEL_WIDTH: f32 = HOST_SIDEBAR_WIDTH + SPACE_5 + SPACE_2;
pub const CANVAS_COMPACT_PANEL_WIDTH: f32 = CANVAS_NOTE_HEIGHT;
pub const CANVAS_DIALOG_NARROW_WIDTH: f32 = CANVAS_NOTE_WIDTH + SPACE_6 - SPACE_2;
pub const CANVAS_DIALOG_WIDTH: f32 = CANVAS_PROJECT_PANEL_WIDTH - SPACE_6 + SPACE_2;
pub const CANVAS_PANEL_WIDE_WIDTH: f32 = CANVAS_PROJECT_PANEL_WIDTH + SPACE_8 - SPACE_3;
pub const CANVAS_PANEL_MEDIUM_WIDTH: f32 = CANVAS_PROJECT_PANEL_WIDTH - SPACE_8 - SPACE_4;
pub const CANVAS_PANEL_LARGE_WIDTH: f32 = CANVAS_PROJECT_PANEL_WIDTH + SPACE_6 - SPACE_2;
pub const CANVAS_PROJECT_FILES_WIDTH: f32 =
    CANVAS_PROJECT_PANEL_WIDTH + SPACE_9 + SPACE_7 + SPACE_2;
pub const CANVAS_DROP_ANIMATION_MILLIS: u64 = 140;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_choice_resolves_to_its_token_theme() {
        assert_eq!(
            design_theme_kind(ThemePreset::System, true),
            ThemeKind::Dark
        );
        assert_eq!(
            design_theme_kind(ThemePreset::System, false),
            ThemeKind::Light
        );
        for system_is_dark in [true, false] {
            assert_eq!(
                design_theme_kind(ThemePreset::Dark, system_is_dark),
                ThemeKind::Dark
            );
            assert_eq!(
                design_theme_kind(ThemePreset::Light, system_is_dark),
                ThemeKind::Light
            );
            assert_eq!(
                design_theme_kind(ThemePreset::HighContrast, system_is_dark),
                ThemeKind::HighContrast
            );
            assert_eq!(
                design_theme_kind(ThemePreset::Recording, system_is_dark),
                ThemeKind::RecordingFriendly
            );
        }
    }

    #[gpui::test]
    fn components_are_recolored_from_the_tokens_of_each_theme(cx: &mut gpui::TestAppContext) {
        let _isolation = crate::test_support::TestIsolation::acquire();
        cx.update(|cx| {
            gpui_component::init(cx);
            for preset in ThemePreset::ALL {
                set_theme_preset(preset);
                apply_to_components(None, cx);
                let tokens = current_design_tokens();
                let colors = &gpui_component::Theme::global(cx).colors;
                assert_eq!(colors.background, token_color(tokens.color_bg_canvas()));
                assert_eq!(colors.foreground, token_color(tokens.color_text_primary()));
                assert_eq!(colors.border, token_color(tokens.color_border_default()));
                assert_eq!(colors.primary, token_color(tokens.color_action_primary()));
                assert_eq!(colors.ring, token_color(tokens.color_focus()));
                assert_eq!(
                    gpui_component::Theme::global(cx).mode.is_dark(),
                    current_theme_kind() != ThemeKind::Light,
                    "{preset:?}"
                );
            }
            set_theme_preset(ThemePreset::System);
        });
    }
}
