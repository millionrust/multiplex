//! Overlays: menus, the command palette, dialogs, and toasts.
//!
//! Every overlay takes the elevated fill and the popover shadow. Modal
//! overlays sit on the scrim, trap focus, and close on Escape.

use std::{rc::Rc, time::Duration};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, DismissEvent, ElementId, Entity, FocusHandle,
    FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement, Pixels, Point,
    Render, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled, Task, Window,
    anchored, deferred, div, prelude::FluentBuilder as _,
};
use gpui_base::{Align, ElementExt as _, POPUP_PRIORITY, Placement, Positioner};

use crate::{
    KeyPlatform,
    button::{Button, ButtonWeight},
    icon::{Icon, IconName, IconSize},
    kbd::format_keystroke,
    theme::{ActiveTheme, SlateStyled, SlateTheme},
};

type DismissHandler = Rc<dyn Fn(&DismissEvent, &mut Window, &mut App) + 'static>;
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type MenuBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Menu + 'static>;

/// Menu widths from the design system.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuWidth {
    /// 212 px, pane menus.
    #[default]
    Pane,
    /// 238 px, workspace tab menus.
    Workspace,
}

/// One menu row.
pub struct MenuItem {
    label: SharedString,
    icon: Option<IconName>,
    keystroke: Option<SharedString>,
    disabled_reason: Option<SharedString>,
    destructive: bool,
    handler: Option<ClickHandler>,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            keystroke: None,
            disabled_reason: None,
            destructive: false,
            handler: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The shortcut, in GPUI keystroke syntax.
    pub fn keystroke(mut self, keystroke: impl Into<SharedString>) -> Self {
        self.keystroke = Some(keystroke.into());
        self
    }

    /// Disables the item. The reason sits where the shortcut would be.
    pub fn disabled(mut self, reason: impl Into<SharedString>) -> Self {
        self.disabled_reason = Some(reason.into());
        self
    }

    /// Colors the item as destructive. Destructive items go last.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handler = Some(Rc::new(handler));
        self
    }
}

enum MenuEntry {
    Item(MenuItem),
    Separator,
}

/// A menu surface. Show it with [`PopoverMenu`] or [`ContextMenu`].
#[derive(IntoElement)]
pub struct Menu {
    width: MenuWidth,
    entries: Vec<MenuEntry>,
    on_dismiss: Option<DismissHandler>,
}

impl Menu {
    pub fn new() -> Self {
        Self {
            width: MenuWidth::Pane,
            entries: Vec::new(),
            on_dismiss: None,
        }
    }

    pub fn width(mut self, width: MenuWidth) -> Self {
        self.width = width;
        self
    }

    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(MenuEntry::Item(item));
        self
    }

    /// Groups actions by consequence.
    pub fn separator(mut self) -> Self {
        self.entries.push(MenuEntry::Separator);
        self
    }

    /// Called after an item runs, so the host can close the menu.
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&DismissEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl Default for Menu {
    fn default() -> Self {
        Self::new()
    }
}

fn overlay_surface(theme: &SlateTheme) -> gpui::Div {
    let c = &theme.colors;
    let m = &theme.metrics;
    div()
        .bg(c.elevated)
        .border(m.hairline)
        .border_color(c.border_strong)
        .rounded(m.radius_dialog)
        .shadow(theme.shadows.popover.clone())
        .font_family(theme.typography.ui_family.clone())
}

impl RenderOnce for Menu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let width = match self.width {
            MenuWidth::Pane => m.pane_menu_width,
            MenuWidth::Workspace => m.workspace_menu_width,
        };
        let highlight = c.selected;
        let platform = KeyPlatform::current();
        let on_dismiss = self.on_dismiss;

        overlay_surface(&theme)
            .id("menu")
            .flex()
            .flex_col()
            .w(width)
            .p(m.space[2])
            .type_style(theme.typography.body)
            .children(self.entries.into_iter().enumerate().map(|(index, entry)| {
                match entry {
                    MenuEntry::Separator => div()
                        .my(m.space[2])
                        .h(m.hairline)
                        .bg(c.border_subtle)
                        .into_any_element(),
                    MenuEntry::Item(item) => {
                        let disabled = item.disabled_reason.is_some();
                        let text = if disabled {
                            c.text_faint
                        } else if item.destructive {
                            c.status_error
                        } else {
                            c.text
                        };
                        let trailing = item.disabled_reason.clone().or_else(|| {
                            item.keystroke
                                .as_ref()
                                .map(|keys| format_keystroke(keys, platform).into())
                        });
                        let handler = item.handler.clone().filter(|_| !disabled);
                        let on_dismiss = on_dismiss.clone();
                        div()
                            .id(("menu-item", index))
                            .flex()
                            .items_center()
                            .gap(m.space[3])
                            .h(m.control_default)
                            .px(m.space[3])
                            .rounded(m.radius_control)
                            .text_color(text)
                            .when(!disabled, |this| {
                                this.cursor_pointer()
                                    .hover(move |style| style.bg(highlight))
                            })
                            .when_some(item.icon, |this, icon| {
                                this.child(Icon::new(icon).size(IconSize::Default))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(item.label),
                            )
                            .when_some(trailing, |this, trailing| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_color(c.text_faint)
                                        .type_style(theme.typography.micro)
                                        .child(trailing),
                                )
                            })
                            .when_some(handler, |this, handler| {
                                this.on_click(move |event, window, cx| {
                                    handler(event, window, cx);
                                    if let Some(dismiss) = &on_dismiss {
                                        dismiss(&DismissEvent, window, cx);
                                    }
                                })
                            })
                            .into_any_element()
                    }
                }
            }))
    }
}

#[derive(Default)]
struct PopoverMenuState {
    open: bool,
    trigger: Bounds<Pixels>,
}

/// A trigger that opens a menu below it.
#[derive(IntoElement)]
pub struct PopoverMenu {
    id: ElementId,
    trigger: AnyElement,
    menu: MenuBuilder,
    align_end: bool,
}

impl PopoverMenu {
    /// `trigger` is usually an [`crate::IconButton`] or [`crate::ToolbarButton`].
    pub fn new(
        id: impl Into<ElementId>,
        trigger: impl IntoElement,
        menu: impl Fn(&mut Window, &mut App) -> Menu + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            trigger: trigger.into_any_element(),
            menu: Rc::new(menu),
            align_end: false,
        }
    }

    /// Aligns the menu with the trigger's trailing edge.
    pub fn align_end(mut self) -> Self {
        self.align_end = true;
        self
    }
}

impl RenderOnce for PopoverMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| PopoverMenuState::default());
        let theme = cx.slate();
        let open = state.read(cx).open;
        let trigger_bounds = state.read(cx).trigger;
        let toggle = state.clone();
        let measure = state.clone();

        div()
            .id(self.id)
            .flex_none()
            .child(self.trigger)
            .on_click(move |_, _, cx| {
                toggle.update(cx, |state, cx| {
                    state.open = !state.open;
                    cx.notify();
                });
            })
            .on_prepaint(move |bounds, _, cx| {
                measure.update(cx, |state, _| state.trigger = bounds);
            })
            .when(open, |this| {
                let close = state.clone();
                let dismiss = state.clone();
                let menu = (self.menu)(window, cx).on_dismiss(move |_, _, cx| {
                    dismiss.update(cx, |state, cx| {
                        state.open = false;
                        cx.notify();
                    });
                });
                this.child(
                    deferred(
                        Positioner::side(trigger_bounds)
                            .placement(Placement::Bottom)
                            .align(if self.align_end {
                                Align::End
                            } else {
                                Align::Start
                            })
                            .offset(theme.metrics.space[2])
                            .occlude()
                            .child(
                                div()
                                    .on_mouse_down_out(move |_, _, cx| {
                                        close.update(cx, |state, cx| {
                                            state.open = false;
                                            cx.notify();
                                        });
                                    })
                                    .child(menu),
                            ),
                    )
                    .with_priority(POPUP_PRIORITY),
                )
            })
    }
}

/// A menu at the pointer, for right-click. The caller owns the open state and
/// stores the click position.
#[derive(IntoElement)]
pub struct ContextMenu {
    position: Point<Pixels>,
    menu: Menu,
    on_dismiss: DismissHandler,
}

impl ContextMenu {
    pub fn new(
        position: Point<Pixels>,
        menu: Menu,
        on_dismiss: impl Fn(&DismissEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            position,
            menu,
            on_dismiss: Rc::new(on_dismiss),
        }
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let dismiss = self.on_dismiss.clone();
        let item_dismiss = self.on_dismiss;
        deferred(
            anchored().position(self.position).snap_to_window().child(
                div()
                    .occlude()
                    .on_mouse_down_out(move |_, window, cx| dismiss(&DismissEvent, window, cx))
                    .child(
                        self.menu.on_dismiss(move |_, window, cx| {
                            item_dismiss(&DismissEvent, window, cx)
                        }),
                    ),
            ),
        )
        .with_priority(POPUP_PRIORITY)
    }
}

/// One command palette result.
#[derive(Clone)]
pub struct PaletteItem {
    kind: SharedString,
    title: SharedString,
    hint: Option<SharedString>,
}

impl PaletteItem {
    /// `kind` is the short word in the first column: Host, Snippet, Go to.
    pub fn new(kind: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            hint: None,
        }
    }

    /// Right-aligned detail, such as the address.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Case-insensitive match on the title, hint, and kind. An empty query
    /// matches everything.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        [Some(&self.title), self.hint.as_ref(), Some(&self.kind)]
            .into_iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(&query))
    }
}

type IndexHandler = Rc<dyn Fn(&usize, &mut Window, &mut App) + 'static>;

/// The ⌘K palette: one field, then up to nine results, most relevant first.
///
/// The caller owns the query field and the highlighted index. Arrow keys call
/// `on_highlight`, Enter calls `on_select`, and Escape or a scrim click calls
/// `on_dismiss`.
#[derive(IntoElement)]
pub struct CommandPalette {
    query: AnyElement,
    items: Vec<PaletteItem>,
    highlighted: usize,
    on_highlight: Option<IndexHandler>,
    on_select: Option<IndexHandler>,
    on_dismiss: Option<DismissHandler>,
}

/// Kind column width in palette rows.
const PALETTE_KIND_WIDTH: f32 = 56.;
/// Hints stop at this width and then ellipsize.
const PALETTE_HINT_MAX_WIDTH: f32 = 240.;
/// The palette shows at most this many results.
pub const PALETTE_MAX_RESULTS: usize = 9;

impl CommandPalette {
    /// `query` is the field element, usually a [`crate::TextField`] or a bare
    /// gpui-base `Input`.
    pub fn new(query: impl IntoElement) -> Self {
        Self {
            query: query.into_any_element(),
            items: Vec::new(),
            highlighted: 0,
            on_highlight: None,
            on_select: None,
            on_dismiss: None,
        }
    }

    pub fn items(mut self, items: impl IntoIterator<Item = PaletteItem>) -> Self {
        self.items
            .extend(items.into_iter().take(PALETTE_MAX_RESULTS));
        self
    }

    pub fn highlighted(mut self, index: usize) -> Self {
        self.highlighted = index;
        self
    }

    pub fn on_highlight(
        mut self,
        handler: impl Fn(&usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(handler));
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_dismiss(
        mut self,
        handler: impl Fn(&DismissEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

fn modal_focus(id: &'static str, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state(id, cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone()
}

impl RenderOnce for CommandPalette {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let focus = modal_focus("slate-palette-focus", window, cx);
        let count = self.items.len();
        let highlighted = self.highlighted.min(count.saturating_sub(1));
        let highlight_bg = c.selected;
        let on_dismiss = self.on_dismiss.clone();
        let keys_highlight = self.on_highlight.clone();
        let keys_select = self.on_select.clone();

        let rows = self.items.into_iter().enumerate().map(|(index, item)| {
            let select = self.on_select.clone();
            let hover = self.on_highlight.clone();
            div()
                .id(("palette-row", index))
                .flex()
                .items_center()
                .gap(m.space[4])
                .h(m.list_row)
                .px(m.space_compact + m.space[2])
                .text_color(c.text)
                .cursor_pointer()
                .when(index == highlighted, |this| this.bg(highlight_bg))
                .child(
                    div()
                        .flex_none()
                        .w(gpui::px(PALETTE_KIND_WIDTH))
                        .text_color(c.text_faint)
                        .type_style(theme.typography.micro)
                        .child(item.kind),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(item.title),
                )
                .when_some(item.hint, |this, hint| {
                    this.child(
                        div()
                            .flex_none()
                            .max_w(gpui::px(PALETTE_HINT_MAX_WIDTH))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(c.text_faint)
                            .type_style(theme.typography.caption)
                            .child(hint),
                    )
                })
                .when_some(hover, |this, hover| {
                    this.on_hover(move |hovered, window, cx| {
                        if *hovered {
                            hover(&index, window, cx);
                        }
                    })
                })
                .when_some(select, |this, select| {
                    this.on_click(move |_, window, cx| select(&index, window, cx))
                })
        });

        let surface = overlay_surface(&theme)
            .id("palette")
            .flex()
            .flex_col()
            .w(m.palette_width)
            .max_w_full()
            .overflow_hidden()
            .type_style(theme.typography.body)
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                match key {
                    "down" | "up" if count > 0 => {
                        if let Some(handler) = &keys_highlight {
                            let next = if key == "down" {
                                (highlighted + 1) % count
                            } else {
                                (highlighted + count - 1) % count
                            };
                            handler(&next, window, cx);
                            cx.stop_propagation();
                        }
                    }
                    "enter" if count > 0 => {
                        if let Some(handler) = &keys_select {
                            handler(&highlighted, window, cx);
                            cx.stop_propagation();
                        }
                    }
                    _ => {}
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(m.space[3])
                    .h(m.control_large + m.space_dense)
                    .px(m.space_compact + m.space[2])
                    .border_b(m.hairline)
                    .border_color(c.border_subtle)
                    .text_color(c.text)
                    .type_style(theme.typography.body_large)
                    .child(
                        Icon::new(IconName::Search)
                            .size(IconSize::Default)
                            .color(c.text_faint),
                    )
                    .child(div().flex_1().min_w_0().child(self.query)),
            )
            .child(div().flex().flex_col().py(m.space[2]).children(rows));

        modal_host(
            focus,
            theme.clone(),
            m.palette_offset,
            surface,
            on_dismiss,
            cx,
        )
    }
}

fn modal_host(
    focus: FocusHandle,
    theme: Rc<SlateTheme>,
    top: Pixels,
    surface: impl IntoElement,
    on_dismiss: Option<DismissHandler>,
    cx: &mut App,
) -> impl IntoElement {
    let dismiss = on_dismiss.clone();
    gpui_base::Dialog::new(cx)
        .focus_handle(focus)
        .open(true)
        .on_open_change(move |open, _, window, cx| {
            if !open && let Some(dismiss) = &dismiss {
                dismiss(&DismissEvent, window, cx);
            }
        })
        .close_on_escape(true)
        .close_on_backdrop_press(on_dismiss.is_some())
        .flex()
        .flex_col()
        .items_center()
        .pt(top)
        .px(theme.metrics.space[5])
        .backdrop(div().size_full().bg(theme.colors.scrim))
        .popup(surface)
}

/// Dialog emphasis.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DialogKind {
    #[default]
    Standard,
    /// A security prompt: wider, with the title in the attention color.
    Security,
}

/// A modal dialog: title, body, and right-aligned actions.
#[derive(IntoElement)]
pub struct Dialog {
    kind: DialogKind,
    title: SharedString,
    body: Vec<AnyElement>,
    cancel_label: SharedString,
    /// Label, whether the action is destructive, and its handler.
    confirm: Option<(SharedString, bool, ClickHandler)>,
    on_cancel: Option<DismissHandler>,
}

/// Distance from the top of the window to a dialog.
const DIALOG_OFFSET_TOP: f32 = 110.;

impl Dialog {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            kind: DialogKind::Standard,
            title: title.into(),
            body: Vec::new(),
            cancel_label: "Cancel".into(),
            confirm: None,
            on_cancel: None,
        }
    }

    pub fn kind(mut self, kind: DialogKind) -> Self {
        self.kind = kind;
        self
    }

    /// A paragraph of body copy.
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        let text: SharedString = text.into();
        self.body.push(DialogText(text).into_any_element());
        self
    }

    /// Any other body content, such as a [`FingerprintBlock`].
    pub fn body(mut self, element: impl IntoElement) -> Self {
        self.body.push(element.into_any_element());
        self
    }

    /// The main action, named for what it does. `destructive` colors it.
    pub fn confirm(
        mut self,
        label: impl Into<SharedString>,
        destructive: bool,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.confirm = Some((label.into(), destructive, Rc::new(handler)));
        self
    }

    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// Escape, a scrim click, and Cancel all call this.
    pub fn on_cancel(
        mut self,
        handler: impl Fn(&DismissEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }
}

#[derive(IntoElement)]
struct DialogText(SharedString);

impl RenderOnce for DialogText {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        div()
            .text_color(theme.colors.text_secondary)
            .type_style(theme.typography.body_relaxed)
            .child(self.0)
    }
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let focus = modal_focus("slate-dialog-focus", window, cx);
        let width = match self.kind {
            DialogKind::Standard => m.dialog_width,
            DialogKind::Security => m.dialog_wide_width,
        };
        let title_color = match self.kind {
            DialogKind::Standard => c.text,
            DialogKind::Security => c.status_attention,
        };
        let cancel = self.on_cancel.clone();

        let surface = overlay_surface(&theme)
            .id("dialog")
            .flex()
            .flex_col()
            .w(width)
            .max_w_full()
            .child(
                div()
                    .pt(m.space[5])
                    .px(m.space[5])
                    .pb(m.space[2])
                    .text_color(title_color)
                    .type_style(theme.typography.heading)
                    .child(self.title),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(m.space[4])
                    .pt(m.space[3])
                    .px(m.space[5])
                    .pb(m.space[2])
                    .children(self.body),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(m.space[3])
                    .pt(m.space_compact + m.space[2])
                    .px(m.space[5])
                    .pb(m.space[5])
                    .child(
                        Button::new("dialog-cancel")
                            .label(self.cancel_label)
                            .on_click(move |_, window, cx| {
                                if let Some(cancel) = &cancel {
                                    cancel(&DismissEvent, window, cx);
                                }
                            }),
                    )
                    .when_some(self.confirm, |this, (label, destructive, handler)| {
                        this.child(
                            Button::new("dialog-confirm")
                                .label(label)
                                .weight(ButtonWeight::Strong)
                                .danger(destructive)
                                .on_click(move |event, window, cx| handler(event, window, cx)),
                        )
                    }),
            );

        modal_host(
            focus,
            theme.clone(),
            gpui::px(DIALOG_OFFSET_TOP),
            surface,
            self.on_cancel,
            cx,
        )
    }
}

/// A recessed block for key fingerprints and other values to compare.
#[derive(IntoElement)]
pub struct FingerprintBlock {
    rows: Vec<(SharedString, SharedString)>,
}

impl FingerprintBlock {
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    pub fn row(mut self, label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        self.rows.push((label.into(), value.into()));
        self
    }
}

impl Default for FingerprintBlock {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for FingerprintBlock {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .flex()
            .flex_col()
            .gap(m.space[2])
            .py(m.space_compact)
            .px(m.space[4])
            .rounded(m.radius_control)
            .bg(c.surface)
            .border(m.hairline)
            .border_color(c.border_subtle)
            .children(self.rows.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .gap(m.space[4])
                    .child(
                        div()
                            .flex_none()
                            .w(m.space[9])
                            .text_color(c.text_faint)
                            .type_style(theme.typography.caption)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(c.text)
                            .font_family(theme.typography.mono_family.clone())
                            .type_style(theme.typography.body_small)
                            .child(value),
                    )
            }))
    }
}

/// A confirmation of something that already happened.
#[derive(IntoElement)]
pub struct Toast {
    message: SharedString,
}

impl Toast {
    /// Past tense and specific: "Added web-3."
    pub fn new(message: impl Into<SharedString>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Toasts wrap past this width.
const TOAST_MAX_WIDTH: f32 = 420.;

impl RenderOnce for Toast {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        div()
            .max_w(gpui::px(TOAST_MAX_WIDTH))
            .py(m.space[3] + m.hairline)
            .px(m.space[4])
            .rounded(m.radius_control)
            .bg(c.control)
            .border(m.hairline)
            .border_color(c.border_strong)
            .shadow(theme.shadows.toast.clone())
            .text_color(c.text)
            .type_style(theme.typography.caption)
            .font_family(theme.typography.ui_family.clone())
            .child(self.message)
    }
}

/// How long a toast stays up.
pub const TOAST_DURATION: Duration = Duration::from_millis(2800);

/// The single toast slot. The newest toast replaces the last one.
///
/// Render it as the last child of a `relative()` root that fills the window.
pub struct Toaster {
    current: Option<SharedString>,
    generation: usize,
    _timer: Option<Task<()>>,
}

impl Toaster {
    pub fn new() -> Self {
        Self {
            current: None,
            generation: 0,
            _timer: None,
        }
    }

    pub fn show(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.current = Some(message.into());
        self.generation += 1;
        let generation = self.generation;
        self._timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.current = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    pub fn current(&self) -> Option<&SharedString> {
        self.current.as_ref()
    }
}

impl Default for Toaster {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for Toaster {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        let Some(message) = self.current.clone() else {
            return div().into_any_element();
        };
        deferred(
            div()
                .absolute()
                .right(m.space_compact + m.space[2])
                .bottom(m.status_height + m.space[4])
                .child(Toast::new(message)),
        )
        .with_priority(POPUP_PRIORITY)
        .into_any_element()
    }
}

/// Opens a toast on a [`Toaster`] entity.
pub fn show_toast(toaster: &Entity<Toaster>, message: impl Into<SharedString>, cx: &mut App) {
    let message = message.into();
    toaster.update(cx, |toaster, cx| toaster.show(message, cx));
}

/// A banner inside a terminal pane. Failures use the error color; an ended
/// session is muted, because ending is normal.
#[derive(IntoElement)]
pub struct Banner {
    ended: bool,
    message: SharedString,
    detail: Option<SharedString>,
    action: Option<AnyElement>,
}

impl Banner {
    pub fn failure(message: impl Into<SharedString>) -> Self {
        Self {
            ended: false,
            message: message.into(),
            detail: None,
            action: None,
        }
    }

    pub fn ended(message: impl Into<SharedString>) -> Self {
        Self {
            ended: true,
            message: message.into(),
            detail: None,
            action: None,
        }
    }

    /// The raw error line, in mono.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Reconnect for SSH, Restart for local shells.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for Banner {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let (fill, text) = if self.ended {
            (c.hover, c.text_muted)
        } else {
            (c.status_error.opacity(0.07), c.status_error)
        };
        div()
            .flex()
            .flex_col()
            .gap(m.space[2])
            .w_full()
            .py(m.space[3])
            .px(m.space[5])
            .bg(fill)
            .border_b(m.hairline)
            .border_color(c.border_subtle)
            .font_family(theme.typography.ui_family.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(m.space[4])
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(text)
                            .type_style(theme.typography.caption.weight(if self.ended {
                                FontWeight::NORMAL
                            } else {
                                FontWeight::MEDIUM
                            }))
                            .child(self.message),
                    )
                    .children(self.action),
            )
            .when_some(self.detail, |this, detail| {
                this.child(
                    div()
                        .text_color(c.text_muted)
                        .font_family(theme.typography.mono_family.clone())
                        .type_style(theme.typography.body_small)
                        .child(detail),
                )
            })
    }
}
