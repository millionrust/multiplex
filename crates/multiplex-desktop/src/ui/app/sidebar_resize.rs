//! Dividers that resize the main navigation sidebar and the Settings section list. A drag
//! follows the pointer from the window's own mouse handlers, so it keeps going when the
//! pointer leaves the thin strip, and the width is saved once the button comes up.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, CursorStyle, Div, InteractiveElement as _, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, SharedString, Stateful, Styled as _, div, px,
};

use crate::storage::save_saved_state;
use crate::ui::app::MultiplexApp;
use crate::ui::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResizableSidebar {
    /// Connections, Sessions, Files and the rest, down the left of the library.
    Library,
    /// The section list inside Settings.
    SettingsNav,
}

impl ResizableSidebar {
    fn default_width(self) -> f32 {
        match self {
            Self::Library => theme::HOST_SIDEBAR_WIDTH,
            Self::SettingsNav => theme::SETTINGS_NAV_WIDTH,
        }
    }

    fn min_width(self) -> f32 {
        match self {
            Self::Library => theme::LIBRARY_SIDEBAR_MIN_WIDTH,
            Self::SettingsNav => theme::SETTINGS_NAV_MIN_WIDTH,
        }
    }

    fn max_width(self) -> f32 {
        match self {
            Self::Library => theme::LIBRARY_SIDEBAR_MAX_WIDTH,
            Self::SettingsNav => theme::SETTINGS_NAV_MAX_WIDTH,
        }
    }

    fn clamp(self, width: f32) -> f32 {
        if width.is_finite() {
            width.clamp(self.min_width(), self.max_width())
        } else {
            self.default_width()
        }
    }

    fn selector(self) -> &'static str {
        match self {
            Self::Library => "library-sidebar-divider",
            Self::SettingsNav => "settings-nav-divider",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SidebarResizeDrag {
    sidebar: ResizableSidebar,
    origin_x: f32,
    start_width: f32,
}

impl MultiplexApp {
    /// How wide `sidebar` is drawn: what it was dragged to, kept inside its bounds.
    pub(super) fn sidebar_width(&self, sidebar: ResizableSidebar) -> f32 {
        let saved = match sidebar {
            ResizableSidebar::Library => self.saved.settings.library_sidebar_width,
            ResizableSidebar::SettingsNav => self.saved.settings.settings_nav_width,
        };
        saved.map_or(sidebar.default_width(), |width| sidebar.clamp(width))
    }

    fn set_sidebar_width(&mut self, sidebar: ResizableSidebar, width: Option<f32>) {
        let width = width.map(|width| sidebar.clamp(width));
        match sidebar {
            ResizableSidebar::Library => self.saved.settings.library_sidebar_width = width,
            ResizableSidebar::SettingsNav => self.saved.settings.settings_nav_width = width,
        }
    }

    pub(super) fn sidebar_resizing(&self) -> bool {
        self.sidebar_resize.is_some()
    }

    pub(super) fn start_sidebar_resize(
        &mut self,
        sidebar: ResizableSidebar,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_resize = Some(SidebarResizeDrag {
            sidebar,
            origin_x: f32::from(position.x),
            start_width: self.sidebar_width(sidebar),
        });
        cx.notify();
    }

    pub(super) fn handle_sidebar_resize_move(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.sidebar_resize else {
            return;
        };
        let width = drag.start_width + f32::from(position.x) - drag.origin_x;
        self.set_sidebar_width(drag.sidebar, Some(width));
        cx.notify();
    }

    pub(super) fn finish_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_resize.take().is_none() {
            return;
        }
        let _ = save_saved_state(&self.saved);
        cx.notify();
    }

    /// Double-clicking a divider puts the sidebar back to its design width.
    fn reset_sidebar_width(&mut self, sidebar: ResizableSidebar, cx: &mut Context<Self>) {
        self.sidebar_resize = None;
        self.set_sidebar_width(sidebar, None);
        let _ = save_saved_state(&self.saved);
        cx.notify();
    }

    /// The strip between `sidebar` and what is beside it: a hairline that lights up under the
    /// pointer and while dragged, inside a wider target that is easy to catch.
    pub(super) fn render_sidebar_divider(
        &self,
        sidebar: ResizableSidebar,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let dragging = self
            .sidebar_resize
            .is_some_and(|drag| drag.sidebar == sidebar);
        let selector = sidebar.selector();
        let group = SharedString::from(selector);
        div()
            .id(selector)
            .debug_selector(move || selector.to_string())
            .group(group.clone())
            .flex_none()
            .h_full()
            .w(px(theme::SIDEBAR_DIVIDER_HIT_WIDTH))
            .flex()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        this.reset_sidebar_width(sidebar, cx);
                    } else {
                        this.start_sidebar_resize(sidebar, event.position, cx);
                    }
                }),
            )
            .child(
                div()
                    .h_full()
                    .w(px(theme::BORDER_HAIRLINE))
                    .bg(if dragging {
                        theme::accent()
                    } else {
                        theme::soft_border()
                    })
                    .when(!dragging, |line| {
                        line.group_hover(group, |style| style.bg(theme::accent()))
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::ResizableSidebar;
    use crate::ui::theme;

    #[test]
    fn widths_stay_inside_their_bounds() {
        for sidebar in [ResizableSidebar::Library, ResizableSidebar::SettingsNav] {
            assert!(sidebar.min_width() < sidebar.default_width());
            assert!(sidebar.default_width() < sidebar.max_width());
            assert_eq!(sidebar.clamp(0.0), sidebar.min_width());
            assert_eq!(sidebar.clamp(f32::MAX), sidebar.max_width());
            assert_eq!(sidebar.clamp(f32::NAN), sidebar.default_width());
        }
        assert_eq!(
            ResizableSidebar::Library.clamp(theme::HOST_SIDEBAR_WIDTH),
            theme::HOST_SIDEBAR_WIDTH
        );
    }
}
