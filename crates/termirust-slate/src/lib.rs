//! Slate, TermiRust's styled component library.
//!
//! Slate sits on `gpui-base`, which owns behavior (focus, keyboard, overlays,
//! accessibility), and on the generated token contract in
//! `termirust-ui-contract`, which owns every color and size. Components are
//! builders in the GPUI style and take no styling escape hatches: a screen
//! that needs a new look needs a new variant here first.
//!
//! Call [`init`] once at startup and serve [`SlateAssets`] (or wrap the app's
//! own asset source with [`SlateAssets::with_fallback`]).

mod button;
mod controls;
mod data;
mod icon;
mod input;
mod kbd;
mod overlay;
mod shell;
mod split;
mod status;
mod terminal;
mod theme;
mod tooltip;

pub use button::{Button, ButtonWeight, ControlSize, IconButton, ToolbarButton};
pub use controls::{FilterTabs, Segment, Segmented, Toggle};
pub use data::{
    Callout, ColumnWidth, EmptyState, HostCard, HostGrid, HostGroup, HostTile, Inspector,
    SectionHeader, Table, TableCell, TableColumn, TableRow, Tone,
};
pub use icon::{Icon, IconName, IconSize, SlateAssets};
pub use input::{InputVariant, SearchTrigger, TextField};
pub use kbd::{KeyCap, KeyPlatform, format_keystroke};
pub use overlay::{
    Banner, CommandPalette, ContextMenu, Dialog, DialogKind, FingerprintBlock, Menu, MenuItem,
    MenuWidth, PALETTE_MAX_RESULTS, PaletteItem, PopoverMenu, TOAST_DURATION, Toast, Toaster,
    show_toast,
};
pub use shell::{
    Breadcrumb, NavItem, Sidebar, SidebarHeading, StatusBar, StatusBarItem, TitleBar, TitleBarTab,
    Toolbar,
};
pub use split::{
    DropEdge, MAX_SPLIT_RATIO, MIN_SPLIT_RATIO, PaneDrag, PaneDrop, SplitNode, SplitPanes,
    SplitPath, SplitResize, clamp_ratio,
};
pub use status::{ALL_STATUSES, StatusGlyph, default_label as status_label};
pub use terminal::{DragChip, PaneHeader, TerminalPane, WorkspaceHeader};
pub use termirust_ui_contract::{StatusKind, ThemeKind};
pub use theme::{
    ActiveTheme, Colors, Metrics, Shadows, SlateStyled, SlateTheme, ThemeChoice, TypeStyle,
    Typography,
};
pub use tooltip::Tooltip;

/// Installs gpui-base and the Slate theme.
pub fn init(choice: ThemeChoice, cx: &mut gpui::App) {
    gpui_base::init(cx);
    SlateTheme::install(choice, None, cx);
}
