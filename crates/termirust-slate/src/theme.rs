//! The active Slate theme.
//!
//! Every value here comes from the generated token contract in
//! `termirust-ui-contract`, which is generated from `design/tokens.toml`. Components
//! read colors and sizes from [`SlateTheme`] and never hold literals of their own.

use std::rc::Rc;

use gpui::{
    App, BoxShadow, FontWeight, Global, Hsla, Pixels, Rgba, SharedString, Styled, Window,
    WindowAppearance, point, px,
};
use gpui_base::{
    ColorTokens, RadiusTokens, ShadowTokens, SpacingTokens, TextStyleToken, ThemeAppearance,
    TypographyTokens,
};
use termirust_ui_contract::{
    ColorValue, DesignTokens, DimensionValue, ShadowValue, StatusKind, ThemeKind, TypographyValue,
};

/// The theme a person picks in Settings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemeChoice {
    /// Follows the operating system between Light and Dark.
    #[default]
    System,
    Light,
    Dark,
    HighContrast,
    RecordingFriendly,
}

impl ThemeChoice {
    pub const ALL: [Self; 5] = [
        Self::System,
        Self::Light,
        Self::Dark,
        Self::HighContrast,
        Self::RecordingFriendly,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::HighContrast => "High contrast",
            Self::RecordingFriendly => "Recording",
        }
    }

    /// Resolves the choice to a concrete token theme.
    pub fn resolve(self, appearance: WindowAppearance) -> ThemeKind {
        match self {
            Self::System => match appearance {
                WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeKind::Light,
                WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeKind::Dark,
            },
            Self::Light => ThemeKind::Light,
            Self::Dark => ThemeKind::Dark,
            Self::HighContrast => ThemeKind::HighContrast,
            Self::RecordingFriendly => ThemeKind::RecordingFriendly,
        }
    }
}

/// Resolved color roles for one theme.
#[derive(Clone, Debug)]
pub struct Colors {
    pub chrome: Hsla,
    pub surface: Hsla,
    pub canvas: Hsla,
    pub elevated: Hsla,
    pub terminal: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub control: Hsla,
    pub control_hover: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub border_subtle: Hsla,
    pub border_focus: Hsla,
    pub accent: Hsla,
    pub accent_text: Hsla,
    pub focus: Hsla,
    pub focus_separation: Hsla,
    pub selection: Hsla,
    pub scrim: Hsla,
    pub status_idle: Hsla,
    pub status_busy: Hsla,
    pub status_done: Hsla,
    pub status_attention: Hsla,
    pub status_error: Hsla,
    pub status_offline: Hsla,
    pub group_production: Hsla,
    pub group_staging: Hsla,
    pub group_lab: Hsla,
    pub group_jump: Hsla,
    pub window_close: Hsla,
    pub window_minimize: Hsla,
    pub window_zoom: Hsla,
    pub terminal_fg: Hsla,
    pub terminal_cursor: Hsla,
    /// The sixteen ANSI palette slots, 0 through 15.
    pub ansi: [Hsla; 16],
}

/// Resolved sizes for one theme.
#[derive(Clone, Debug)]
pub struct Metrics {
    pub hairline: Pixels,
    pub focus_ring: Pixels,
    pub focus_offset: Pixels,
    pub radius_control: Pixels,
    pub radius_panel: Pixels,
    pub radius_dialog: Pixels,
    pub radius_pill: Pixels,
    pub radius_segment: Pixels,
    pub radius_progress: Pixels,
    pub control_small: Pixels,
    pub control_default: Pixels,
    pub control_large: Pixels,
    pub icon_button: Pixels,
    pub toolbar_button: Pixels,
    pub icon_indicator: Pixels,
    pub icon_compact: Pixels,
    pub icon_small: Pixels,
    pub icon_status: Pixels,
    pub icon_default: Pixels,
    pub icon_medium: Pixels,
    pub icon_large: Pixels,
    pub chrome_height: Pixels,
    pub status_height: Pixels,
    pub toolbar_height: Pixels,
    pub workspace_header_height: Pixels,
    pub pane_header_height: Pixels,
    pub navigation_row: Pixels,
    pub sidebar_width: Pixels,
    pub inspector_width: Pixels,
    pub inspector_minimum: Pixels,
    pub inspector_maximum: Pixels,
    pub palette_width: Pixels,
    pub palette_offset: Pixels,
    pub dialog_width: Pixels,
    pub dialog_wide_width: Pixels,
    pub pane_menu_width: Pixels,
    pub workspace_menu_width: Pixels,
    pub table_row: Pixels,
    pub list_row: Pixels,
    pub card_height: Pixels,
    pub card_tile: Pixels,
    pub card_minimum_width: Pixels,
    pub quick_connect: Pixels,
    pub divider_hit: Pixels,
    pub traffic_light: Pixels,
    pub tab_label_maximum: Pixels,
    pub terminal_padding: Pixels,
    pub terminal_inline: Pixels,
    /// The spacing scale `space.0` through `space.9`.
    pub space: [Pixels; 10],
    pub space_micro: Pixels,
    pub space_fine: Pixels,
    pub space_dense: Pixels,
    pub space_compact: Pixels,
    pub max_panes: usize,
}

/// A resolved type style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyle {
    pub size: Pixels,
    pub line_height: Pixels,
    pub weight: FontWeight,
}

impl TypeStyle {
    fn from_token(value: TypographyValue) -> Self {
        Self {
            size: px(value.size),
            line_height: px(value.line_height),
            weight: FontWeight(f32::from(value.weight)),
        }
    }

    /// The same style at another weight.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = weight;
        self
    }
}

/// Resolved type roles for one theme.
#[derive(Clone, Debug)]
pub struct Typography {
    pub ui_family: SharedString,
    pub mono_family: SharedString,
    pub nano: TypeStyle,
    pub micro: TypeStyle,
    pub caption: TypeStyle,
    pub body_small: TypeStyle,
    pub body: TypeStyle,
    pub body_relaxed: TypeStyle,
    pub body_large: TypeStyle,
    pub label: TypeStyle,
    pub heading_small: TypeStyle,
    pub heading: TypeStyle,
    pub title: TypeStyle,
    pub metric: TypeStyle,
    pub terminal: TypeStyle,
}

/// Resolved shadows for one theme. An empty list means the theme draws none.
#[derive(Clone, Debug)]
pub struct Shadows {
    pub popover: Vec<BoxShadow>,
    pub modal: Vec<BoxShadow>,
    pub toast: Vec<BoxShadow>,
}

/// The theme every Slate component reads while it renders.
#[derive(Clone, Debug)]
pub struct SlateTheme {
    pub choice: ThemeChoice,
    pub kind: ThemeKind,
    pub tokens: DesignTokens,
    pub colors: Colors,
    pub metrics: Metrics,
    pub typography: Typography,
    pub shadows: Shadows,
    /// Stops spinners and other looping motion.
    pub reduced_motion: bool,
}

#[derive(Clone)]
struct GlobalSlateTheme(Rc<SlateTheme>);

impl Global for GlobalSlateTheme {}

impl SlateTheme {
    /// Resolves every token for one theme.
    pub fn new(choice: ThemeChoice, kind: ThemeKind) -> Self {
        let tokens = DesignTokens::new(kind);
        Self {
            choice,
            kind,
            colors: colors(tokens),
            metrics: metrics(tokens),
            typography: typography(tokens),
            shadows: Shadows {
                popover: shadow(tokens.shadow_popover()),
                modal: shadow(tokens.shadow_modal()),
                toast: shadow(tokens.shadow_toast()),
            },
            tokens,
            reduced_motion: false,
        }
    }

    /// Returns the active theme, or Dark before [`crate::init`] has run.
    pub fn global(cx: &App) -> Rc<SlateTheme> {
        cx.try_global::<GlobalSlateTheme>()
            .map(|theme| theme.0.clone())
            .unwrap_or_else(|| Rc::new(Self::new(ThemeChoice::Dark, ThemeKind::Dark)))
    }

    /// Installs a theme for the whole app and projects it onto gpui-base,
    /// so unstyled base controls such as text inputs pick up the same palette.
    pub fn install(choice: ThemeChoice, window: Option<&Window>, cx: &mut App) {
        let appearance = window
            .map(Window::appearance)
            .unwrap_or_else(|| cx.window_appearance());
        let reduced_motion = cx
            .try_global::<GlobalSlateTheme>()
            .is_some_and(|theme| theme.0.reduced_motion);
        let mut theme = Self::new(choice, choice.resolve(appearance));
        theme.reduced_motion = reduced_motion;
        *gpui_base::Theme::global_mut(cx) = theme.base_theme();
        cx.set_global(GlobalSlateTheme(Rc::new(theme)));
        cx.refresh_windows();
    }

    /// Switches looping motion on or off without changing the theme.
    pub fn set_reduced_motion(reduced_motion: bool, cx: &mut App) {
        let mut theme = (*Self::global(cx)).clone();
        theme.reduced_motion = reduced_motion;
        cx.set_global(GlobalSlateTheme(Rc::new(theme)));
        cx.refresh_windows();
    }

    pub fn is_light(&self) -> bool {
        self.kind == ThemeKind::Light
    }

    /// The token color for a semantic status.
    pub fn status_color(&self, status: StatusKind) -> Hsla {
        color(self.tokens.status(status).color)
    }

    /// The gpui-base projection of this theme.
    pub fn base_theme(&self) -> gpui_base::Theme {
        let c = &self.colors;
        let m = &self.metrics;
        let t = &self.typography;
        let base_text = |style: TypeStyle| TextStyleToken {
            size: style.size,
            line_height: style.line_height,
            weight: style.weight,
        };
        let mut theme = gpui_base::Theme {
            appearance: if self.is_light() {
                ThemeAppearance::Light
            } else {
                ThemeAppearance::Dark
            },
            ..Default::default()
        };
        theme.tokens.colors = ColorTokens {
            background: c.canvas,
            foreground: c.text,
            surface: c.surface,
            surface_foreground: c.text,
            primary: c.control,
            primary_foreground: c.text,
            secondary: c.control_hover,
            secondary_foreground: c.text,
            muted: c.hover,
            muted_foreground: c.text_faint,
            // Base inputs paint their selection from the accent at 40% alpha.
            accent: c.accent,
            accent_foreground: c.accent_text,
            destructive: c.status_error,
            destructive_foreground: c.text,
            border: c.border,
            input: c.border_strong,
            ring: c.focus,
            selection: c.selection,
        };
        theme.tokens.radius = RadiusTokens {
            none: m.space[0],
            sm: m.radius_segment,
            md: m.radius_control,
            lg: m.radius_panel,
            xl: m.radius_dialog,
            full: m.radius_pill,
        };
        theme.tokens.spacing = SpacingTokens {
            xxs: m.space[1],
            xs: m.space[2],
            sm: m.space[3],
            md: m.space[4],
            lg: m.space[5],
            xl: m.space[6],
            xxl: m.space[7],
        };
        theme.tokens.typography = TypographyTokens {
            sans: t.ui_family.clone(),
            mono: t.mono_family.clone(),
            xs: base_text(t.caption),
            sm: base_text(t.body),
            md: base_text(t.heading),
            lg: base_text(t.body_large),
            xl: base_text(t.title),
            mono_md: base_text(t.terminal),
        };
        theme.tokens.shadow = ShadowTokens {
            sm: self.shadows.toast.clone(),
            md: self.shadows.popover.clone(),
            lg: self.shadows.modal.clone(),
        };
        theme.resizable.handle = Some(c.border);
        theme.resizable.active_handle = Some(c.accent);
        theme
    }
}

/// Access to the active Slate theme through an app context.
pub trait ActiveTheme {
    fn slate(&self) -> Rc<SlateTheme>;
}

impl ActiveTheme for App {
    fn slate(&self) -> Rc<SlateTheme> {
        SlateTheme::global(self)
    }
}

/// Converts a token color to a GPUI color.
pub fn color(value: ColorValue) -> Hsla {
    Rgba {
        r: f32::from(value.red) / 255.0,
        g: f32::from(value.green) / 255.0,
        b: f32::from(value.blue) / 255.0,
        a: f32::from(value.alpha) / 255.0,
    }
    .into()
}

fn dimension(value: DimensionValue) -> Pixels {
    px(value.0)
}

fn shadow(value: ShadowValue) -> Vec<BoxShadow> {
    if !value.visible {
        return Vec::new();
    }
    vec![BoxShadow {
        color: color(value.color),
        offset: point(px(value.x), px(value.y)),
        blur_radius: px(value.blur),
        spread_radius: px(value.spread),
        inset: false,
    }]
}

fn font_family(token: &str) -> SharedString {
    match token {
        "system-ui" => ".SystemUIFont".into(),
        "system-monospace" => {
            if cfg!(target_os = "macos") {
                "Menlo".into()
            } else if cfg!(target_os = "windows") {
                "Cascadia Mono".into()
            } else {
                "DejaVu Sans Mono".into()
            }
        }
        other => SharedString::from(other.to_string()),
    }
}

fn colors(t: DesignTokens) -> Colors {
    Colors {
        chrome: color(t.color_bg_chrome()),
        surface: color(t.color_bg_surface()),
        canvas: color(t.color_bg_canvas()),
        elevated: color(t.color_bg_elevated()),
        terminal: color(t.color_bg_terminal()),
        hover: color(t.color_bg_hover()),
        selected: color(t.color_bg_selected()),
        control: color(t.color_bg_control()),
        control_hover: color(t.color_bg_control_hover()),
        text: color(t.color_text_primary()),
        text_secondary: color(t.color_text_secondary()),
        text_muted: color(t.color_text_muted()),
        text_faint: color(t.color_text_faint()),
        border: color(t.color_border_default()),
        border_strong: color(t.color_border_strong()),
        border_subtle: color(t.color_border_subtle()),
        border_focus: color(t.color_border_focus()),
        accent: color(t.color_action_primary()),
        accent_text: color(t.color_action_primary_text()),
        focus: color(t.color_focus()),
        focus_separation: color(t.color_focus_separation()),
        selection: color(t.color_selection()),
        scrim: color(t.color_overlay_scrim()),
        status_idle: color(t.color_status_idle()),
        status_busy: color(t.color_status_busy()),
        status_done: color(t.color_status_done()),
        status_attention: color(t.color_status_attention()),
        status_error: color(t.color_status_error()),
        status_offline: color(t.color_status_offline()),
        group_production: color(t.color_group_production()),
        group_staging: color(t.color_group_staging()),
        group_lab: color(t.color_group_lab()),
        group_jump: color(t.color_group_jump()),
        window_close: color(t.color_window_close()),
        window_minimize: color(t.color_window_minimize()),
        window_zoom: color(t.color_window_zoom()),
        terminal_fg: color(t.color_terminal_fg()),
        terminal_cursor: color(t.color_terminal_cursor()),
        ansi: [
            color(t.color_terminal_ansi_black()),
            color(t.color_terminal_ansi_red()),
            color(t.color_terminal_ansi_green()),
            color(t.color_terminal_ansi_yellow()),
            color(t.color_terminal_ansi_blue()),
            color(t.color_terminal_ansi_magenta()),
            color(t.color_terminal_ansi_cyan()),
            color(t.color_terminal_ansi_white()),
            color(t.color_terminal_ansi_bright_black()),
            color(t.color_terminal_ansi_bright_red()),
            color(t.color_terminal_ansi_bright_green()),
            color(t.color_terminal_ansi_bright_yellow()),
            color(t.color_terminal_ansi_bright_blue()),
            color(t.color_terminal_ansi_bright_magenta()),
            color(t.color_terminal_ansi_bright_cyan()),
            color(t.color_terminal_ansi_bright_white()),
        ],
    }
}

fn metrics(t: DesignTokens) -> Metrics {
    Metrics {
        hairline: px(t.border_hairline().0),
        focus_ring: px(t.focus_ring_width().0),
        focus_offset: px(t.focus_ring_offset().0),
        radius_control: px(t.radius_control().0),
        radius_panel: px(t.radius_panel().0),
        radius_dialog: px(t.radius_dialog().0),
        radius_pill: px(t.radius_pill().0),
        radius_segment: px(t.radius_segment().0),
        radius_progress: px(t.radius_progress().0),
        control_small: dimension(t.control_height_compact()),
        control_default: dimension(t.control_height_default()),
        control_large: dimension(t.control_height_large()),
        icon_button: dimension(t.layout_shell_icon_button_size()),
        toolbar_button: dimension(t.layout_shell_toolbar_button_size()),
        icon_indicator: dimension(t.icon_size_indicator()),
        icon_compact: dimension(t.icon_size_compact()),
        icon_small: dimension(t.icon_size_small()),
        icon_status: dimension(t.icon_size_status()),
        icon_default: dimension(t.icon_size_default()),
        icon_medium: dimension(t.icon_size_medium()),
        icon_large: dimension(t.icon_size_large()),
        chrome_height: dimension(t.layout_chrome_height()),
        status_height: dimension(t.layout_status_height()),
        toolbar_height: dimension(t.layout_toolbar_height()),
        workspace_header_height: dimension(t.layout_workspace_header_height()),
        pane_header_height: dimension(t.layout_pane_header_height()),
        navigation_row: dimension(t.layout_shell_navigation_row_height()),
        sidebar_width: dimension(t.layout_sidebar_default()),
        inspector_width: dimension(t.layout_inspector_default()),
        inspector_minimum: dimension(t.layout_inspector_minimum()),
        inspector_maximum: dimension(t.layout_inspector_maximum()),
        palette_width: dimension(t.layout_palette_width()),
        palette_offset: dimension(t.layout_palette_offset_top()),
        dialog_width: dimension(t.layout_dialog_width()),
        dialog_wide_width: dimension(t.layout_dialog_wide_width()),
        pane_menu_width: dimension(t.layout_shell_pane_menu_width()),
        workspace_menu_width: dimension(t.layout_shell_workspace_menu_width()),
        table_row: dimension(t.layout_table_row_height()),
        list_row: dimension(t.layout_list_row_height()),
        card_height: dimension(t.layout_card_height()),
        card_tile: dimension(t.layout_card_tile()),
        card_minimum_width: dimension(t.layout_card_minimum_width()),
        quick_connect: dimension(t.layout_quick_connect_height()),
        divider_hit: dimension(t.layout_split_divider_hit()),
        traffic_light: dimension(t.layout_shell_traffic_light_size()),
        tab_label_maximum: dimension(t.layout_shell_tab_label_maximum()),
        terminal_padding: px(t.space_terminal_padding().0),
        terminal_inline: px(t.space_terminal_inline().0),
        space: [
            px(t.space_0().0),
            px(t.space_1().0),
            px(t.space_2().0),
            px(t.space_3().0),
            px(t.space_4().0),
            px(t.space_5().0),
            px(t.space_6().0),
            px(t.space_7().0),
            px(t.space_8().0),
            px(t.space_9().0),
        ],
        space_micro: px(t.space_micro().0),
        space_fine: px(t.space_fine().0),
        space_dense: px(t.space_dense().0),
        space_compact: px(t.space_compact().0),
        max_panes: t.layout_split_max_panes().0 as usize,
    }
}

fn typography(t: DesignTokens) -> Typography {
    Typography {
        ui_family: font_family(t.font_ui_family().0),
        mono_family: font_family(t.font_mono_family().0),
        nano: TypeStyle::from_token(t.type_nano()),
        micro: TypeStyle::from_token(t.type_micro()),
        caption: TypeStyle::from_token(t.type_caption()),
        body_small: TypeStyle::from_token(t.type_body_small()),
        body: TypeStyle::from_token(t.type_body()),
        body_relaxed: TypeStyle::from_token(t.type_body_relaxed()),
        body_large: TypeStyle::from_token(t.type_body_large()),
        label: TypeStyle::from_token(t.type_label()),
        heading_small: TypeStyle::from_token(t.type_heading_small()),
        heading: TypeStyle::from_token(t.type_heading()),
        title: TypeStyle::from_token(t.type_title()),
        metric: TypeStyle::from_token(t.type_metric()),
        terminal: TypeStyle::from_token(t.type_terminal()),
    }
}

/// Styling helpers shared by every Slate component.
pub trait SlateStyled: Styled + Sized {
    /// Applies a type role's size, line height, and weight.
    fn type_style(self, style: TypeStyle) -> Self {
        self.text_size(style.size)
            .line_height(style.line_height)
            .font_weight(style.weight)
    }

    /// Draws the 2 px keyboard focus ring with its 1 px separation.
    ///
    /// The ring is two spread shadows, so it sits outside the control and
    /// never changes the control's size.
    fn focus_ring(self, theme: &SlateTheme) -> Self {
        self.shadow(focus_ring_shadows(theme))
    }
}

impl<T: Styled> SlateStyled for T {}

/// The shadows that draw the focus ring.
pub fn focus_ring_shadows(theme: &SlateTheme) -> Vec<BoxShadow> {
    let m = &theme.metrics;
    let ring = |spread: Pixels, color: Hsla| BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: spread,
        inset: false,
    };
    vec![
        ring(m.focus_offset + m.focus_ring, theme.colors.focus),
        ring(m.focus_offset, theme.colors.focus_separation),
    ]
}

/// A one-sided inset ring, used by armed toolbar buttons and on icon buttons.
pub fn inset_ring(width: Pixels, color: Hsla) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: width,
        inset: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_resolves_opaque_text_and_grounds() {
        for kind in ThemeKind::ALL {
            let theme = SlateTheme::new(ThemeChoice::Dark, kind);
            assert_eq!(theme.colors.text.a, 1.0, "{kind:?} text");
            assert_eq!(theme.colors.canvas.a, 1.0, "{kind:?} canvas");
            assert!(theme.colors.scrim.a < 1.0 || kind == ThemeKind::HighContrast);
        }
    }

    #[test]
    fn system_choice_follows_the_window_appearance() {
        assert_eq!(
            ThemeChoice::System.resolve(WindowAppearance::Light),
            ThemeKind::Light
        );
        assert_eq!(
            ThemeChoice::System.resolve(WindowAppearance::VibrantDark),
            ThemeKind::Dark
        );
        assert_eq!(
            ThemeChoice::HighContrast.resolve(WindowAppearance::Light),
            ThemeKind::HighContrast
        );
    }

    #[test]
    fn metrics_match_the_slate_scale() {
        let theme = SlateTheme::new(ThemeChoice::Dark, ThemeKind::Dark);
        let m = &theme.metrics;
        assert_eq!(m.control_small, px(22.));
        assert_eq!(m.control_default, px(26.));
        assert_eq!(m.control_large, px(36.));
        assert_eq!(m.radius_control, px(4.));
        assert_eq!(m.chrome_height, px(38.));
        assert_eq!(m.max_panes, 4);
    }

    #[test]
    fn high_contrast_draws_no_shadows() {
        let theme = SlateTheme::new(ThemeChoice::HighContrast, ThemeKind::HighContrast);
        assert!(theme.shadows.popover.is_empty());
        assert!(theme.shadows.toast.is_empty());
    }

    #[test]
    fn token_colors_convert_exactly() {
        let hsla = color(ColorValue::from_rgba(0x74A7F2FF));
        let rgba = Rgba::from(hsla);
        assert!((rgba.r - 0x74 as f32 / 255.0).abs() < 0.002);
        assert!((rgba.b - 0xF2 as f32 / 255.0).abs() < 0.002);
    }
}
