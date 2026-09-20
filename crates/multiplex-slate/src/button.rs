//! Buttons: the neutral [`Button`], the quiet [`ToolbarButton`], and the
//! square [`IconButton`]. All three are styled `gpui_base::Button`s, so focus,
//! keyboard activation, and accessibility come from the base layer.

use gpui::{
    App, ClickEvent, ElementId, FontWeight, InteractiveElement, Interactivity, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
    prelude::FluentBuilder as _,
};
use gpui_base::Selectable;

use crate::{
    icon::{Icon, IconName, IconSize},
    theme::{ActiveTheme, SlateStyled, focus_ring_shadows, inset_ring},
    tooltip::Tooltip,
};

/// The three control heights.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ControlSize {
    /// `control.height.compact`, 22 px.
    Small,
    /// `control.height.default`, 26 px.
    #[default]
    Default,
    /// `control.height.large`, 36 px.
    Large,
}

/// Button emphasis. There should be at most one strong button per surface.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonWeight {
    #[default]
    Default,
    Strong,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A neutral button with a hairline border.
#[derive(IntoElement)]
pub struct Button {
    base: gpui_base::Button,
    label: Option<SharedString>,
    icon: Option<IconName>,
    size: ControlSize,
    weight: ButtonWeight,
    danger: bool,
    disabled: bool,
    selected: bool,
    full_width: bool,
    tooltip: Option<Tooltip>,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: gpui_base::Button::new(id),
            label: None,
            icon: None,
            size: ControlSize::Default,
            weight: ButtonWeight::Default,
            danger: false,
            disabled: false,
            selected: false,
            full_width: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Adds a leading icon.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    pub fn weight(mut self, weight: ButtonWeight) -> Self {
        self.weight = weight;
        self
    }

    /// Shorthand for the strong weight.
    pub fn strong(self) -> Self {
        self.weight(ButtonWeight::Strong)
    }

    /// Colors the label for an action that deletes or closes something.
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    /// Disabled buttons drop to 40% and leave the tab order.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Stretches the button to its container's width.
    pub fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    /// Names the action, or explains why it is disabled.
    pub fn tooltip(mut self, tooltip: impl Into<Tooltip>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Selectable for Button {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl InteractiveElement for Button {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Button {}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let (height, padding) = match self.size {
            ControlSize::Small => (m.control_small, m.space[3]),
            ControlSize::Default => (m.control_default, m.space_compact),
            ControlSize::Large => (m.control_large, m.space[4]),
        };
        let text_style = match self.weight {
            ButtonWeight::Default => theme.typography.label,
            ButtonWeight::Strong => theme.typography.label.weight(FontWeight::SEMIBOLD),
        };
        let text = if self.danger { c.status_error } else { c.text };
        let (fill, fill_hover, border) = match self.weight {
            ButtonWeight::Default => (c.control, c.control_hover, c.border_strong),
            ButtonWeight::Strong => (c.control_hover, c.selected, c.border_focus),
        };
        let pressed = c.selected;
        let disabled = self.disabled;
        let ring = focus_ring_shadows(&theme);

        self.base
            .disabled(disabled)
            .selected(self.selected)
            .flex_none()
            .h(height)
            .px(padding)
            .gap(m.space_dense)
            .rounded(m.radius_control)
            .border(m.hairline)
            .border_color(border)
            .bg(fill)
            .text_color(text)
            .type_style(text_style)
            .font_family(theme.typography.ui_family.clone())
            .whitespace_nowrap()
            .when(self.full_width, |this| this.w_full())
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(fill_hover))
                    .active(move |style| style.bg(pressed))
                    .focus_visible(move |style| style.shadow(ring))
            })
            .styles(move |styles| {
                styles
                    .selected(move |style| style.bg(pressed))
                    .disabled(|style| style.opacity(0.4))
            })
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(IconSize::Small).color(if self.danger {
                    text
                } else {
                    c.text_muted
                }))
            })
            .when_some(self.label, |this, label| this.child(label))
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(move |window, cx| tooltip.clone().build(window, cx))
            })
            .when_some(self.on_click, |this, handler| this.on_click(handler))
    }
}

/// A quiet button that lives in toolbars and headers.
///
/// It is transparent at rest. "Armed" marks a mode that stays on until turned
/// off, such as Broadcast input.
#[derive(IntoElement)]
pub struct ToolbarButton {
    base: gpui_base::Button,
    label: Option<SharedString>,
    icon: Option<IconName>,
    armed: bool,
    disabled: bool,
    selected: bool,
    tooltip: Option<Tooltip>,
    on_click: Option<ClickHandler>,
}

impl ToolbarButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: gpui_base::Button::new(id),
            label: None,
            icon: None,
            armed: false,
            disabled: false,
            selected: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Marks a mode that stays on, with an inset accent ring.
    pub fn armed(mut self, armed: bool) -> Self {
        self.armed = armed;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<Tooltip>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Selectable for ToolbarButton {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl InteractiveElement for ToolbarButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for ToolbarButton {}

impl RenderOnce for ToolbarButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let lit = self.armed || self.selected;
        let rest_text = if self.armed { c.accent } else { c.text_muted };
        let hover_text = if self.armed { c.accent } else { c.text };
        let control = c.control;
        let disabled = self.disabled;
        let ring = focus_ring_shadows(&theme);

        self.base
            .disabled(disabled)
            .selected(self.selected)
            .flex_none()
            .h(m.toolbar_button)
            .px(m.space[3])
            .gap(m.space_dense)
            .rounded(m.radius_control)
            .type_style(theme.typography.caption)
            .font_family(theme.typography.ui_family.clone())
            .whitespace_nowrap()
            .text_color(rest_text)
            .when(lit, |this| this.bg(control))
            .when(self.armed, |this| {
                this.shadow(inset_ring(m.hairline, c.accent.opacity(0.6)))
            })
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(control).text_color(hover_text))
                    .focus_visible(move |style| style.shadow(ring))
            })
            .styles(|styles| styles.disabled(|style| style.opacity(0.4)))
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(IconSize::Small))
            })
            .when_some(self.label, |this, label| this.child(label))
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(move |window, cx| tooltip.clone().build(window, cx))
            })
            .when_some(self.on_click, |this, handler| this.on_click(handler))
    }
}

/// A square button that holds only an icon. It always carries a tooltip,
/// because the icon alone does not name the action.
#[derive(IntoElement)]
pub struct IconButton {
    base: gpui_base::Button,
    icon: IconName,
    size: IconSize,
    on: bool,
    selected: bool,
    disabled: bool,
    tooltip: Tooltip,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    /// `label` names the action for the tooltip and for assistive technology.
    pub fn new(id: impl Into<ElementId>, icon: IconName, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self {
            base: gpui_base::Button::new(id).accessibility_label(label.clone()),
            icon,
            size: IconSize::Default,
            on: false,
            selected: false,
            disabled: false,
            tooltip: Tooltip::new(label),
            on_click: None,
        }
    }

    /// Uses the compact close glyph, for tab and pane close buttons.
    pub fn compact(mut self) -> Self {
        self.size = IconSize::Compact;
        self
    }

    /// Shows the button as toggled on.
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Replaces the tooltip, for example to add a shortcut or a reason.
    pub fn tooltip(mut self, tooltip: impl Into<Tooltip>) -> Self {
        self.tooltip = tooltip.into();
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Selectable for IconButton {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl InteractiveElement for IconButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for IconButton {}

impl RenderOnce for IconButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let compact = self.size == IconSize::Compact;
        let side = if compact {
            m.icon_compact + m.space_micro * 2.
        } else {
            m.icon_button
        };
        let lit = self.on || self.selected;
        let control = if compact { c.control_hover } else { c.control };
        let text_hover = c.text;
        let disabled = self.disabled;
        let ring = focus_ring_shadows(&theme);
        let tooltip = self.tooltip;

        self.base
            .disabled(disabled)
            .selected(self.selected)
            .flex_none()
            .size(side)
            .rounded(m.radius_control)
            .text_color(if lit { c.text } else { c.text_muted })
            .when(lit, |this| this.bg(control))
            .when(self.on, |this| {
                this.shadow(inset_ring(m.hairline, c.border_strong))
            })
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(control).text_color(text_hover))
                    .focus_visible(move |style| style.shadow(ring))
            })
            .styles(|styles| styles.disabled(|style| style.opacity(0.4)))
            .child(Icon::new(self.icon).size(self.size))
            .tooltip(move |window, cx| tooltip.clone().build(window, cx))
            .when_some(self.on_click, |this, handler| this.on_click(handler))
    }
}
