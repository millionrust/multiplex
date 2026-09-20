//! Terminal chrome. The terminal grid itself is app-owned; Slate draws the
//! ground, padding, and headers around it so a pane and the app read as one
//! surface.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Div, ElementId, FontWeight,
    InteractiveElement, Interactivity, IntoElement, ParentElement, Render, RenderOnce,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::{
    StatusKind,
    button::IconButton,
    icon::IconName,
    status::StatusGlyph,
    theme::{ActiveTheme, SlateStyled},
};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Unfocused panes dim their terminal text to this opacity.
const UNFOCUSED_TEXT_OPACITY: f32 = 0.6;

/// The 36 px header at the top of a terminal tab: host, address, duration,
/// and the tab's tools on the right.
#[derive(IntoElement)]
pub struct WorkspaceHeader {
    name: SharedString,
    detail: Option<SharedString>,
    tools: Vec<AnyElement>,
}

impl WorkspaceHeader {
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            detail: None,
            tools: Vec::new(),
        }
    }

    /// Address and duration: `deploy@10.0.4.21:22 · 2h 14m`.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A tool on the right, usually a [`crate::ToolbarButton`].
    pub fn tool(mut self, tool: impl IntoElement) -> Self {
        self.tools.push(tool.into_any_element());
        self
    }
}

impl RenderOnce for WorkspaceHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(m.space[3])
            .w_full()
            .h(m.workspace_header_height)
            .pl(m.space[5])
            .pr(m.space[3])
            .bg(c.surface)
            .border_b(m.hairline)
            .border_color(c.border_subtle)
            .font_family(theme.typography.ui_family.clone())
            .type_style(theme.typography.body)
            .child(
                div()
                    .flex_none()
                    .text_color(c.text)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.name),
            )
            .when_some(self.detail, |this, detail| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(c.text_faint)
                        .type_style(theme.typography.caption)
                        .child(detail),
                )
            })
            .child(div().flex_1())
            .children(self.tools)
    }
}

/// The 28 px header shown on each pane once a tab holds two or more. It is
/// the drag handle for rearranging panes; attach [`crate::PaneDrag`] with
/// GPUI's `on_drag`.
#[derive(IntoElement)]
pub struct PaneHeader {
    base: Stateful<Div>,
    name: SharedString,
    status: Option<StatusKind>,
    duration: Option<SharedString>,
    focused: bool,
    on_detach: Option<ClickHandler>,
    on_close: Option<ClickHandler>,
}

impl PaneHeader {
    pub fn new(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Self {
        Self {
            base: div().id(id),
            name: name.into(),
            status: None,
            duration: None,
            focused: false,
            on_detach: None,
            on_close: None,
        }
    }

    pub fn status(mut self, status: StatusKind) -> Self {
        self.status = Some(status);
        self
    }

    pub fn duration(mut self, duration: impl Into<SharedString>) -> Self {
        self.duration = Some(duration.into());
        self
    }

    /// The focused pane has the lit header.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Adds "Move to its own tab".
    pub fn on_detach(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_detach = Some(Rc::new(handler));
        self
    }

    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }
}

impl InteractiveElement for PaneHeader {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for PaneHeader {}

impl RenderOnce for PaneHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let focused = self.focused;
        self.base
            .flex()
            .flex_none()
            .items_center()
            .gap(m.space[3])
            .w_full()
            .h(m.pane_header_height)
            .pl(m.space[4])
            .pr(m.space[2])
            .border_b(m.hairline)
            .border_color(c.border_subtle)
            .bg(if focused { c.surface } else { c.canvas })
            .text_color(if focused { c.text } else { c.text_muted })
            .font_family(theme.typography.ui_family.clone())
            .type_style(theme.typography.caption)
            .cursor_grab()
            .when_some(self.status, |this, status| {
                this.child(StatusGlyph::new(status))
            })
            .child(
                div()
                    .flex_none()
                    .max_w(m.tab_label_maximum)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(focused, |this| this.font_weight(FontWeight::SEMIBOLD))
                    .child(self.name),
            )
            .when_some(self.duration, |this, duration| {
                this.child(div().text_color(c.text_faint).child(duration))
            })
            .child(div().flex_1())
            .when_some(self.on_detach, |this, handler| {
                this.child(
                    IconButton::new("detach", IconName::Detach, "Move to its own tab")
                        .on_click(move |event, window, cx| handler(event, window, cx)),
                )
            })
            .when_some(self.on_close, |this, handler| {
                this.child(
                    IconButton::new("close", IconName::Close, "Close pane")
                        .compact()
                        .on_click(move |event, window, cx| handler(event, window, cx)),
                )
            })
    }
}

/// A terminal pane: optional header and banner, then the padded terminal
/// ground holding the app's terminal renderer.
#[derive(IntoElement)]
pub struct TerminalPane {
    header: Option<AnyElement>,
    banner: Option<AnyElement>,
    focused: bool,
    children: Vec<AnyElement>,
}

impl TerminalPane {
    pub fn new() -> Self {
        Self {
            header: None,
            banner: None,
            focused: true,
            children: Vec::new(),
        }
    }

    /// A [`PaneHeader`], shown when the tab holds two or more panes.
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// A failure or ended-session [`crate::Banner`].
    pub fn banner(mut self, banner: impl IntoElement) -> Self {
        self.banner = Some(banner.into_any_element());
        self
    }

    /// Unfocused panes dim their text so input's target is obvious.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

impl Default for TerminalPane {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for TerminalPane {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for TerminalPane {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(c.terminal)
            .children(self.header)
            .children(self.banner)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .py(m.terminal_padding)
                    .px(m.terminal_inline)
                    .text_color(c.terminal_fg)
                    .font_family(theme.typography.mono_family.clone())
                    .type_style(theme.typography.terminal)
                    .when(!self.focused, |this| this.opacity(UNFOCUSED_TEXT_OPACITY))
                    .children(self.children),
            )
    }
}

/// The chip that follows the pointer while a pane or tab is dragged.
#[derive(Clone)]
pub struct DragChip {
    label: SharedString,
}

impl DragChip {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }

    /// Builds the drag preview entity for GPUI's `on_drag`.
    pub fn build(label: impl Into<SharedString>, cx: &mut App) -> gpui::Entity<Self> {
        cx.new(|_| Self::new(label))
    }
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .py(m.space[2])
            .px(m.space[3])
            .rounded(m.radius_control)
            .bg(c.control)
            .border(m.hairline)
            .border_color(c.accent)
            .shadow(theme.shadows.toast.clone())
            .text_color(c.text)
            .type_style(theme.typography.caption)
            .font_family(theme.typography.ui_family.clone())
            .child(self.label.clone())
    }
}
