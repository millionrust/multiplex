//! The window shell: title bar and tabs, sidebar, toolbar, and status bar.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, InteractiveElement, Interactivity,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, RenderOnce, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, WindowControlArea, div,
    prelude::FluentBuilder as _,
};
use gpui_base::Selectable;

use crate::{
    StatusKind,
    button::IconButton,
    icon::{Icon, IconName, IconSize},
    status::StatusGlyph,
    theme::{ActiveTheme, SlateStyled, focus_ring_shadows},
};

type MouseDownHandler = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The single-row title bar: traffic lights, Home and terminal tabs, an empty
/// area that drags the window, and a trailing slot for search.
#[derive(IntoElement)]
pub struct TitleBar {
    id: ElementId,
    traffic_lights: bool,
    tabs: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
    scroll: Option<ScrollHandle>,
    on_double_click: Option<MouseDownHandler>,
}

impl TitleBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            traffic_lights: true,
            tabs: Vec::new(),
            trailing: Vec::new(),
            scroll: None,
            on_double_click: None,
        }
    }

    /// Draws Slate's own close, minimize, and zoom controls. Turn this off
    /// when the platform draws them.
    pub fn traffic_lights(mut self, show: bool) -> Self {
        self.traffic_lights = show;
        self
    }

    pub fn tab(mut self, tab: impl IntoElement) -> Self {
        self.tabs.push(tab.into_any_element());
        self
    }

    pub fn tabs(mut self, tabs: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.tabs
            .extend(tabs.into_iter().map(IntoElement::into_any_element));
        self
    }

    /// Content at the trailing edge, usually a [`crate::SearchTrigger`].
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }

    /// Lets the caller keep the active tab scrolled into view.
    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// Double-clicking the empty area opens a local terminal.
    pub fn on_double_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_double_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TitleBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let height = m.chrome_height;
        // Every region but the active tab draws the bottom hairline, so the
        // active tab reads as the top of the pane below it.
        let segment = move || div().h(height).border_b(m.hairline).border_color(c.border);
        let light = |id: &'static str, color, area, action: fn(&mut Window)| {
            div()
                .id(id)
                .size(m.traffic_light)
                .rounded(m.radius_pill)
                .bg(color)
                .window_control_area(area)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, _| action(window))
        };
        let on_double_click = self.on_double_click;

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .w_full()
            .h(height)
            .bg(c.chrome)
            .text_color(c.text_muted)
            .type_style(theme.typography.body)
            .font_family(theme.typography.ui_family.clone())
            .when(self.traffic_lights, |this| {
                this.child(
                    segment()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(m.space[3])
                        .px(m.space_compact + m.space[2])
                        .child(light(
                            "close",
                            c.window_close,
                            WindowControlArea::Close,
                            |w| w.remove_window(),
                        ))
                        .child(light(
                            "minimize",
                            c.window_minimize,
                            WindowControlArea::Min,
                            |w| w.minimize_window(),
                        ))
                        .child(light("zoom", c.window_zoom, WindowControlArea::Max, |w| {
                            w.zoom_window()
                        })),
                )
            })
            .child(
                div()
                    .id("tabs")
                    .flex()
                    .flex_shrink(1.)
                    .min_w_0()
                    .h(height)
                    .overflow_x_scroll()
                    .when_some(self.scroll, |this, handle| this.track_scroll(&handle))
                    .children(self.tabs),
            )
            .child(
                segment()
                    .id("drag-area")
                    .flex_1()
                    .min_w(m.space[8])
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        if event.click_count >= 2 {
                            if let Some(handler) = &on_double_click {
                                handler(event, window, cx);
                            }
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .when(!self.trailing.is_empty(), |this| {
                this.child(
                    segment()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(m.space[3])
                        .pr(m.space[3])
                        .children(self.trailing),
                )
            })
    }
}

/// One tab in the title bar. It exposes GPUI's interaction hooks so the app
/// can attach drag, drop, and context menu behavior.
#[derive(IntoElement)]
pub struct TitleBarTab {
    base: Stateful<Div>,
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    status: Option<StatusKind>,
    pane_count: usize,
    active: bool,
    drop_target: bool,
    on_click: Option<ClickHandler>,
    on_close: Option<ClickHandler>,
}

impl TitleBarTab {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let id = id.into();
        Self {
            base: div().id(id.clone()),
            id,
            label: label.into(),
            icon: None,
            status: None,
            pane_count: 1,
            active: false,
            drop_target: false,
            on_click: None,
            on_close: None,
        }
    }

    /// The fixed first tab.
    pub fn home(id: impl Into<ElementId>) -> Self {
        Self::new(id, "Home").icon(IconName::Home)
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The status of the tab's focused pane.
    pub fn status(mut self, status: StatusKind) -> Self {
        self.status = Some(status);
        self
    }

    /// Shown when the tab holds two or more panes.
    pub fn pane_count(mut self, count: usize) -> Self {
        self.pane_count = count;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Shows the 2 px insertion marker while a pane is dragged over the tab.
    pub fn drop_target(mut self, drop_target: bool) -> Self {
        self.drop_target = drop_target;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Adds the close button. Home has none.
    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }
}

impl Selectable for TitleBarTab {
    fn selected(mut self, selected: bool) -> Self {
        self.active = self.active || selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.active
    }
}

impl InteractiveElement for TitleBarTab {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for TitleBarTab {}

impl RenderOnce for TitleBarTab {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let group = SharedString::from(format!("title-tab-{:?}", self.id));
        let hover_text = c.text;
        let hover_bg = c.hover;
        let active = self.active;
        let ring = focus_ring_shadows(&theme);

        self.base
            .group(group.clone())
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(m.chrome_height)
            .pl(m.space_compact + m.space[2])
            .pr(m.space_compact)
            .gap(m.space[3])
            .border_r(m.hairline)
            .border_color(c.border)
            .cursor_pointer()
            .map(|this| {
                if active {
                    this.bg(c.canvas).text_color(c.text)
                } else {
                    this.border_b(m.hairline)
                        .text_color(c.text_muted)
                        .hover(move |style| style.text_color(hover_text).bg(hover_bg))
                }
            })
            .focus_visible(move |style| style.shadow(ring))
            .when(self.drop_target, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w(m.space[1])
                        .bg(c.accent),
                )
            })
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(IconSize::Small))
            })
            .when_some(self.status, |this, status| {
                this.child(StatusGlyph::new(status))
            })
            .child(
                div()
                    .max_w(m.tab_label_maximum)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.label),
            )
            .when(self.pane_count >= 2, |this| {
                this.child(
                    div()
                        .text_color(c.text_faint)
                        .type_style(theme.typography.micro)
                        .child(self.pane_count.to_string()),
                )
            })
            .when_some(self.on_close, |this, on_close| {
                this.child(
                    div()
                        .flex_none()
                        .when(!active, |slot| {
                            slot.invisible().group_hover(group, |style| style.visible())
                        })
                        .child(
                            IconButton::new("close", IconName::Close, "Close tab")
                                .compact()
                                .on_click(move |event, window, cx| {
                                    cx.stop_propagation();
                                    on_close(event, window, cx)
                                }),
                        ),
                )
            })
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
    }
}

/// The left navigation list.
#[derive(IntoElement)]
pub struct Sidebar {
    header: Option<SharedString>,
    children: Vec<AnyElement>,
    footer: Vec<AnyElement>,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            header: None,
            children: Vec::new(),
            footer: Vec::new(),
        }
    }

    /// A 40 px header row with a caption, such as the vault name.
    pub fn header(mut self, header: impl Into<SharedString>) -> Self {
        self.header = Some(header.into());
        self
    }

    /// Items pinned to the bottom of the sidebar.
    pub fn footer(mut self, element: impl IntoElement) -> Self {
        self.footer.push(element.into_any_element());
        self
    }
}

impl Default for Sidebar {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Sidebar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Sidebar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(m.sidebar_width)
            .h_full()
            .bg(c.surface)
            .border_r(m.hairline)
            .border_color(c.border)
            .font_family(theme.typography.ui_family.clone())
            .when_some(self.header, |this, header| {
                this.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(m.toolbar_height)
                        .px(m.space[5])
                        .text_color(c.text_muted)
                        .type_style(theme.typography.caption.weight(FontWeight::SEMIBOLD))
                        .child(header),
                )
            })
            .child(
                div()
                    .id("sidebar-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(m.space[3])
                    .gap(m.hairline)
                    .children(self.children),
            )
            .when(!self.footer.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .p(m.space[3])
                        .gap(m.hairline)
                        .border_t(m.hairline)
                        .border_color(c.border_subtle)
                        .children(self.footer),
                )
            })
    }
}

/// A section caption inside the sidebar.
#[derive(IntoElement)]
pub struct SidebarHeading {
    label: SharedString,
}

impl SidebarHeading {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl RenderOnce for SidebarHeading {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        div()
            .pt(m.space_compact + m.space[2])
            .pb(m.space_dense)
            .px(m.space[3])
            .text_color(theme.colors.text_muted)
            .type_style(theme.typography.caption.weight(FontWeight::SEMIBOLD))
            .child(self.label)
    }
}

/// One destination in the sidebar. The active item is the only place the
/// accent colors an icon.
#[derive(IntoElement)]
pub struct NavItem {
    base: gpui_base::Button,
    icon: IconName,
    label: SharedString,
    badge: Option<SharedString>,
    active: bool,
    tool: bool,
    on_click: Option<ClickHandler>,
}

impl NavItem {
    pub fn new(id: impl Into<ElementId>, icon: IconName, label: impl Into<SharedString>) -> Self {
        Self {
            base: gpui_base::Button::new(id),
            icon,
            label: label.into(),
            badge: None,
            active: false,
            tool: false,
            on_click: None,
        }
    }

    /// A count at the trailing edge.
    pub fn badge(mut self, badge: impl Into<SharedString>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Advanced tools use muted text so the main destinations lead.
    pub fn tool(mut self, tool: bool) -> Self {
        self.tool = tool;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl InteractiveElement for NavItem {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for NavItem {}

impl RenderOnce for NavItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let active = self.active;
        let rest_text = if self.tool {
            c.text_muted
        } else {
            c.text_secondary
        };
        let hover_bg = c.hover;
        let ring = focus_ring_shadows(&theme);
        self.base
            .selected(active)
            .w_full()
            .justify_start()
            .h(m.navigation_row)
            .px(m.space[3])
            .gap(m.space[3])
            .rounded(m.radius_control)
            .type_style(theme.typography.body)
            .text_color(if active { c.text } else { rest_text })
            .when(active, |this| this.bg(c.selected))
            .when(!active, |this| {
                this.cursor_pointer().hover(move |style| style.bg(hover_bg))
            })
            .focus_visible(move |style| style.shadow(ring))
            .child(
                Icon::new(self.icon)
                    .size(IconSize::Default)
                    .color(if active { c.accent } else { c.text_muted }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.label),
            )
            .when_some(self.badge, |this, badge| {
                this.child(
                    div()
                        .text_color(c.text_muted)
                        .type_style(theme.typography.caption)
                        .child(badge),
                )
            })
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
    }
}

/// The 40 px row under a view's title bar tab.
#[derive(IntoElement)]
pub struct Toolbar {
    children: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            trailing: Vec::new(),
        }
    }

    /// Content pushed to the trailing edge.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Toolbar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Toolbar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(m.toolbar_height)
            .pl(m.space[5])
            .pr(m.space[3])
            .gap(m.space[4])
            .border_b(m.hairline)
            .border_color(c.border_subtle)
            .font_family(theme.typography.ui_family.clone())
            .children(self.children)
            .child(div().flex_1())
            .children(self.trailing)
    }
}

/// Where the current view sits: `Home / All hosts`.
#[derive(IntoElement)]
pub struct Breadcrumb {
    id: ElementId,
    items: Vec<(SharedString, Option<ClickHandler>)>,
}

impl Breadcrumb {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
        }
    }

    /// Adds a level. The last level is the current view and is not a link.
    pub fn item(mut self, label: impl Into<SharedString>) -> Self {
        self.items.push((label.into(), None));
        self
    }

    /// Adds a level that navigates back to it.
    pub fn link(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push((label.into(), Some(Rc::new(handler))));
        self
    }
}

impl RenderOnce for Breadcrumb {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let last = self.items.len().saturating_sub(1);
        let hover = c.text;
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(m.space[3])
            .type_style(theme.typography.body)
            .whitespace_nowrap()
            .children(
                self.items
                    .into_iter()
                    .enumerate()
                    .flat_map(|(index, (label, handler))| {
                        let separator = (index > 0).then(|| {
                            div()
                                .text_color(c.border_strong)
                                .child("/")
                                .into_any_element()
                        });
                        let current = index == last;
                        let item = div()
                            .id(("crumb", index))
                            .text_color(if current { c.text } else { c.text_muted })
                            .child(label)
                            .when_some(handler.filter(|_| !current), |this, handler| {
                                this.cursor_pointer()
                                    .hover(move |style| style.text_color(hover))
                                    .on_click(move |event, window, cx| handler(event, window, cx))
                            })
                            .into_any_element();
                        separator.into_iter().chain(std::iter::once(item))
                    }),
            )
    }
}

/// The 26 px bar at the bottom of the window.
#[derive(IntoElement)]
pub struct StatusBar {
    start: Vec<AnyElement>,
    end: Vec<AnyElement>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            start: Vec::new(),
            end: Vec::new(),
        }
    }

    /// Adds an item on the leading side: sidebar toggle, live count, attention.
    pub fn start(mut self, element: impl IntoElement) -> Self {
        self.start.push(element.into_any_element());
        self
    }

    /// Adds an item on the trailing side: vault and keep-alive.
    pub fn end(mut self, element: impl IntoElement) -> Self {
        self.end.push(element.into_any_element());
        self
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(m.status_height)
            .px(m.space[3])
            .gap(m.space_compact + m.space[2])
            .bg(c.chrome)
            .border_t(m.hairline)
            .border_color(c.border)
            .text_color(c.text_muted)
            .type_style(theme.typography.caption)
            .font_family(theme.typography.ui_family.clone())
            .whitespace_nowrap()
            .children(self.start)
            .child(div().flex_1())
            .children(self.end)
    }
}

/// A status bar entry: an optional glyph and a short phrase.
#[derive(IntoElement)]
pub struct StatusBarItem {
    status: Option<StatusKind>,
    label: SharedString,
}

impl StatusBarItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            status: None,
            label: label.into(),
        }
    }

    pub fn status(mut self, status: StatusKind) -> Self {
        self.status = Some(status);
        self
    }
}

impl RenderOnce for StatusBarItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.metrics.space_dense)
            .when_some(self.status, |this, status| {
                this.child(StatusGlyph::new(status))
            })
            .child(self.label)
    }
}
