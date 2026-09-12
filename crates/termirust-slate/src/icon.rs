//! The Slate icon set and the asset source that serves it.
//!
//! Icons are single-weight strokes on a 16-unit grid. They are generated as SVG
//! from path data at load time, so the crate carries no binary assets. GPUI
//! renders an SVG as a mask, so the element's text color is the icon color.

use std::borrow::Cow;

use gpui::{
    App, AssetSource, Hsla, IntoElement, Pixels, RenderOnce, Result, SharedString, Styled, Window,
    svg,
};

use crate::theme::ActiveTheme;

const ICON_PREFIX: &str = "slate/icons/";
const STATUS_PREFIX: &str = "slate/status/";

macro_rules! icons {
    ($($variant:ident => $name:literal, $path:literal;)*) => {
        /// Every icon in the Slate set.
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum IconName {
            $($variant,)*
        }

        impl IconName {
            pub const ALL: &'static [IconName] = &[$(IconName::$variant,)*];

            /// The kebab-case name used in asset paths.
            pub fn name(self) -> &'static str {
                match self {
                    $(IconName::$variant => $name,)*
                }
            }

            fn path_data(self) -> &'static str {
                match self {
                    $(IconName::$variant => $path,)*
                }
            }

            fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(IconName::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

icons! {
    Home => "home", "M2.5 7.5L8 3l5.5 4.5M4 6.5V13h8V6.5M6.5 13V9.5h3V13";
    Activity => "activity", "M2 8h3l2-5 2 10 2-5h3";
    Projects => "projects", "M2 4.5A1.5 1.5 0 0 1 3.5 3H6l1.5 1.5h5A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5z";
    Sessions => "sessions", "M3.5 2.5h9a1 1 0 0 1 1 1v2a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-2a1 1 0 0 1 1-1zM3.5 9.5h9a1 1 0 0 1 1 1v2a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-2a1 1 0 0 1 1-1z";
    Sftp => "sftp", "M2 4.5A1.5 1.5 0 0 1 3.5 3H6l1.5 1.5h5A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5zM8 7v4M6.5 9.5L8 11l1.5-1.5";
    File => "file", "M4 2h5l3 3v9H4zM9 2v3h3";
    Devices => "devices", "M14 8A6 6 0 1 1 2 8a6 6 0 0 1 12 0zM2 8h12M8 2c2.2 2 2.2 10 0 12M8 2c-2.2 2-2.2 10 0 12";
    Settings => "settings", "M10 8a2 2 0 1 1-4 0 2 2 0 0 1 4 0zM8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4";
    Presets => "presets", "M3 4h2.2M7.8 4H13M3 8h5.7M11.3 8H13M3 12h1.2M6.8 12H13M7.8 4a1.3 1.3 0 1 1-2.6 0 1.3 1.3 0 0 1 2.6 0zM11.3 8a1.3 1.3 0 1 1-2.6 0 1.3 1.3 0 0 1 2.6 0zM6.8 12a1.3 1.3 0 1 1-2.6 0 1.3 1.3 0 0 1 2.6 0z";
    Vault => "vault", "M3.5 3h9A1.5 1.5 0 0 1 14 4.5v7a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5v-7A1.5 1.5 0 0 1 3.5 3zM10.2 8a2.2 2.2 0 1 1-4.4 0 2.2 2.2 0 0 1 4.4 0z";
    Key => "key", "M7.5 8a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0zM7.5 8H14M11.5 8v2M13.5 8v1.5";
    Snippets => "snippets", "M6 4L3 8l3 4M10 4l3 4-3 4";
    KnownHosts => "known-hosts", "M8 2l5 2v4c0 3-2.2 5-5 6-2.8-1-5-3-5-6V4z";
    Logs => "logs", "M5.5 4H13M5.5 8H13M5.5 12H13M2.8 4h.4M2.8 8h.4M2.8 12h.4";
    Server => "server", "M3.5 2.5h9a1 1 0 0 1 1 1V7h-11V3.5a1 1 0 0 1 1-1zM2.5 7h11v3.5a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1zM5 4.8h.5M5 9.3h.5M6.5 13.5h3M8 11.5v2";
    Local => "local", "M3.5 3.5h9a1 1 0 0 1 1 1V10h-11V4.5a1 1 0 0 1 1-1zM1.5 12.5h13M5.5 6l1.5 1.2L5.5 8.4M8.2 8.4h2";
    Terminal => "terminal", "M3.5 3h9A1.5 1.5 0 0 1 14 4.5v7a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5v-7A1.5 1.5 0 0 1 3.5 3zM5 7l2 1.5L5 10M8.5 10.5H11";
    Search => "search", "M11.5 7a4.5 4.5 0 1 1-9 0 4.5 4.5 0 0 1 9 0zM10.5 10.5L14 14";
    Plus => "plus", "M8 3v10M3 8h10";
    Close => "close", "M4 4l8 8M12 4l-8 8";
    SplitRight => "split-right", "M3.5 3h9A1.5 1.5 0 0 1 14 4.5v7a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5v-7A1.5 1.5 0 0 1 3.5 3zM8 3v10";
    SplitDown => "split-down", "M3.5 3h9A1.5 1.5 0 0 1 14 4.5v7a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5v-7A1.5 1.5 0 0 1 3.5 3zM2 8h12";
    Broadcast => "broadcast", "M9.5 8a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0zM5 5a4.2 4.2 0 0 0 0 6M11 5a4.2 4.2 0 0 1 0 6";
    Detach => "detach", "M9 3h4v4M13 3L8 8M11 9.5V13H3V5h3.5";
    Grid => "grid", "M3 3h4v4H3zM9 3h4v4H9zM3 9h4v4H3zM9 9h4v4H9z";
    List => "list", "M3 4h10M3 8h10M3 12h10";
    Sidebar => "sidebar", "M3.5 3h9A1.5 1.5 0 0 1 14 4.5v7a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5v-7A1.5 1.5 0 0 1 3.5 3zM6 3v10";
    ChevronRight => "chevron-right", "M6 3.5L10.5 8 6 12.5";
    ChevronDown => "chevron-down", "M3.5 6L8 10.5 12.5 6";
    More => "more", "M3.5 8h.5M7.75 8h.5M12 8h.5";
    Check => "check", "M3 8.5l3 3 7-7";
    Refresh => "refresh", "M13 8a5 5 0 1 1-1.5-3.6M13 2.5v3h-3";
    Upload => "upload", "M8 11V3M5 6l3-3 3 3M3 13h10";
    Download => "download", "M8 3v8M5 8l3 3 3-3M3 13h10";
    Lock => "lock", "M4 7.5h8v6H4zM5.5 7.5V5.5a2.5 2.5 0 0 1 5 0v2";
    Warning => "warning", "M8 2.5l6 10.5H2zM8 6.5v3M8 11.3v.2";
}

/// The eight fixed status glyph shapes, drawn on a 12-unit grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GlyphShape {
    FilledCircle,
    RingSpinner,
    HollowCircle,
    Diamond,
    Octagon,
    DashedCircle,
    HollowDiamond,
    HollowSquare,
}

impl GlyphShape {
    pub(crate) const ALL: [Self; 8] = [
        Self::FilledCircle,
        Self::RingSpinner,
        Self::HollowCircle,
        Self::Diamond,
        Self::Octagon,
        Self::DashedCircle,
        Self::HollowDiamond,
        Self::HollowSquare,
    ];

    /// Maps the `shape` field of the token contract's status table.
    pub(crate) fn from_contract(shape: &str) -> Option<Self> {
        Some(match shape {
            "filled_circle" => Self::FilledCircle,
            "ring_spinner" => Self::RingSpinner,
            "hollow_circle" => Self::HollowCircle,
            "diamond" => Self::Diamond,
            "octagon" => Self::Octagon,
            "dashed_circle" => Self::DashedCircle,
            "hollow_diamond" => Self::HollowDiamond,
            "hollow_square" => Self::HollowSquare,
            _ => return None,
        })
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::FilledCircle => "filled-circle",
            Self::RingSpinner => "ring-spinner",
            Self::HollowCircle => "hollow-circle",
            Self::Diamond => "diamond",
            Self::Octagon => "octagon",
            Self::DashedCircle => "dashed-circle",
            Self::HollowDiamond => "hollow-diamond",
            Self::HollowSquare => "hollow-square",
        }
    }

    pub(crate) fn asset_path(self) -> SharedString {
        format!("{STATUS_PREFIX}{}.svg", self.file_name()).into()
    }

    fn body(self) -> &'static str {
        match self {
            Self::FilledCircle => r##"<circle cx="6" cy="6" r="3.5" fill="#000"/>"##,
            Self::RingSpinner => {
                r##"<path d="M6 3.25A2.75 2.75 0 1 1 3.25 6" fill="none" stroke="#000" stroke-width="1.5" stroke-linecap="round"/>"##
            }
            Self::HollowCircle => {
                r##"<circle cx="6" cy="6" r="2.75" fill="none" stroke="#000" stroke-width="1.5"/>"##
            }
            Self::Diamond => r##"<path d="M6 1.75L10.25 6 6 10.25 1.75 6z" fill="#000"/>"##,
            Self::Octagon => {
                r##"<path d="M4.6 2.5h2.8l2.1 2.1v2.8L7.4 9.5H4.6L2.5 7.4V4.6z" fill="#000"/>"##
            }
            Self::DashedCircle => {
                r##"<circle cx="6" cy="6" r="2.75" fill="none" stroke="#000" stroke-width="1.5" stroke-dasharray="1.6 1.28"/>"##
            }
            Self::HollowDiamond => {
                r##"<path d="M6 2.3L9.7 6 6 9.7 2.3 6z" fill="none" stroke="#000" stroke-width="1.5" stroke-linejoin="round"/>"##
            }
            Self::HollowSquare => {
                r##"<rect x="3.25" y="3.25" width="5.5" height="5.5" rx="1" fill="none" stroke="#000" stroke-width="1.5"/>"##
            }
        }
    }
}

impl IconName {
    /// The asset path GPUI loads through [`SlateAssets`].
    pub fn asset_path(self) -> SharedString {
        format!("{ICON_PREFIX}{}.svg", self.name()).into()
    }
}

fn icon_svg(icon: IconName) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="{}"/></svg>"##,
        icon.path_data()
    )
}

fn glyph_svg(shape: GlyphShape) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 12 12">{}</svg>"#,
        shape.body()
    )
}

/// Serves Slate's icons and status glyphs, and forwards every other path to
/// an optional fallback source so an app can keep its own assets.
pub struct SlateAssets {
    fallback: Option<Box<dyn AssetSource>>,
}

impl SlateAssets {
    pub fn new() -> Self {
        Self { fallback: None }
    }

    /// Serves paths outside the Slate namespace from `fallback`.
    pub fn with_fallback(fallback: impl AssetSource) -> Self {
        Self {
            fallback: Some(Box::new(fallback)),
        }
    }

    fn slate_asset(path: &str) -> Option<String> {
        if let Some(name) = path
            .strip_prefix(ICON_PREFIX)
            .and_then(|rest| rest.strip_suffix(".svg"))
        {
            return IconName::from_name(name).map(icon_svg);
        }
        let name = path
            .strip_prefix(STATUS_PREFIX)
            .and_then(|rest| rest.strip_suffix(".svg"))?;
        GlyphShape::ALL
            .into_iter()
            .find(|shape| shape.file_name() == name)
            .map(glyph_svg)
    }
}

impl Default for SlateAssets {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetSource for SlateAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(svg) = Self::slate_asset(path) {
            return Ok(Some(Cow::Owned(svg.into_bytes())));
        }
        match &self.fallback {
            Some(fallback) => fallback.load(path),
            None => Ok(None),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries = Vec::new();
        if ICON_PREFIX.starts_with(path) || path.starts_with(ICON_PREFIX) {
            entries.extend(IconName::ALL.iter().map(|icon| icon.asset_path()));
        }
        if let Some(fallback) = &self.fallback {
            entries.extend(fallback.list(path)?);
        }
        Ok(entries)
    }
}

/// Which icon size token to draw at.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IconSize {
    /// `icon.size.compact`, 11 px, the close glyph on tabs and panes.
    Compact,
    /// `icon.size.small`, 12 px, button and toolbar icons.
    Small,
    /// `icon.size.default`, 14 px, sidebar, menus, and icon buttons.
    #[default]
    Default,
    /// `icon.size.medium`, 16 px, the host tile glyph.
    Medium,
    /// `icon.size.large`, 20 px, the empty-state glyph.
    Large,
}

/// A monochrome icon. It inherits the surrounding text color unless given one.
#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: IconSize,
    color: Option<Hsla>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: IconSize::Default,
            color: None,
        }
    }

    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

pub(crate) fn icon_pixels(size: IconSize, cx: &App) -> Pixels {
    let m = &cx.slate().metrics;
    match size {
        IconSize::Compact => m.icon_compact,
        IconSize::Small => m.icon_small,
        IconSize::Default => m.icon_default,
        IconSize::Medium => m.icon_medium,
        IconSize::Large => m.icon_large,
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = icon_pixels(self.size, cx);
        let color = self.color.unwrap_or_else(|| window.text_style().color);
        svg()
            .path(self.name.asset_path())
            .flex_none()
            .size(size)
            .text_color(color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_and_glyph_is_served_as_svg() {
        let assets = SlateAssets::new();
        for icon in IconName::ALL {
            let bytes = assets
                .load(&icon.asset_path())
                .expect("load succeeds")
                .unwrap_or_else(|| panic!("{} is served", icon.name()));
            let text = std::str::from_utf8(&bytes).expect("svg is UTF-8");
            assert!(text.starts_with("<svg") && text.ends_with("</svg>"));
        }
        for shape in GlyphShape::ALL {
            assert!(assets.load(&shape.asset_path()).unwrap().is_some());
        }
    }

    #[test]
    fn unknown_paths_fall_through() {
        assert!(
            SlateAssets::new()
                .load("slate/icons/nope.svg")
                .unwrap()
                .is_none()
        );
        assert!(SlateAssets::new().load("icons/app.svg").unwrap().is_none());
    }

    #[test]
    fn every_contract_status_shape_has_a_glyph() {
        use termirust_ui_contract::{DesignTokens, StatusKind, ThemeKind};
        let tokens = DesignTokens::new(ThemeKind::Dark);
        for kind in [
            StatusKind::Attention,
            StatusKind::Busy,
            StatusKind::Done,
            StatusKind::Error,
            StatusKind::Idle,
            StatusKind::Offline,
            StatusKind::Orphaned,
            StatusKind::PermissionDenied,
        ] {
            let shape = tokens.status(kind).shape;
            assert!(GlyphShape::from_contract(shape).is_some(), "{shape}");
        }
    }
}
