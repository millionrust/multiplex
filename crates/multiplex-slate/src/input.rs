//! Text fields. Every field shares one anatomy: a recessed fill on the
//! surface color, a hairline that changes on focus, an optional leading icon,
//! and an optional trailing key cap or action.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement, RenderOnce, SharedString,
    Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_base::{Input, InputBase, input::InputState};

use crate::{
    icon::{Icon, IconName, IconSize},
    kbd::KeyCap,
    theme::{ActiveTheme, SlateStyled, focus_ring_shadows},
};

/// Which of the field layouts to draw.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InputVariant {
    /// 26 px, for filtering lists and in-pane search.
    #[default]
    Filter,
    /// 30 px with a stronger border, for forms.
    Form,
    /// 36 px, the Home quick connect field.
    QuickConnect,
}

/// A labelled single-line text field over a gpui-base [`InputState`].
///
/// The caller owns the state entity, so it can read the value and subscribe to
/// its change and Enter events.
#[derive(IntoElement)]
pub struct TextField {
    id: ElementId,
    state: Entity<InputState>,
    variant: InputVariant,
    label: Option<SharedString>,
    icon: Option<IconName>,
    trailing: Option<AnyElement>,
    error: Option<SharedString>,
    hint: Option<SharedString>,
    disabled: bool,
}

impl TextField {
    pub fn new(id: impl Into<ElementId>, state: &Entity<InputState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            variant: InputVariant::Filter,
            label: None,
            icon: None,
            trailing: None,
            error: None,
            hint: None,
            disabled: false,
        }
    }

    pub fn variant(mut self, variant: InputVariant) -> Self {
        self.variant = variant;
        self
    }

    /// A caption shown above the field.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A shortcut key cap at the trailing edge.
    pub fn keystroke(mut self, keystroke: impl Into<SharedString>) -> Self {
        self.trailing = Some(KeyCap::new(keystroke).into_any_element());
        self
    }

    /// An action at the trailing edge, such as the quick connect button.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    /// An error message below the field. It should say how to fix the value.
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Helper text below the field, shown when there is no error.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let focus_handle: FocusHandle = self.state.read(cx).focus_handle(cx);
        let focused = focus_handle.is_focused(window);
        let (height, border, radius) = match self.variant {
            InputVariant::Filter => (m.control_default, c.border, m.radius_control),
            InputVariant::Form => (
                m.control_default + m.space[2],
                c.border_strong,
                m.radius_control,
            ),
            InputVariant::QuickConnect => (m.quick_connect, c.border, m.radius_panel),
        };
        let has_error = self.error.is_some();
        let resting_border = if has_error { c.status_error } else { border };
        let focused_border = if has_error {
            c.status_error
        } else {
            c.border_focus
        };
        let text_style = match self.variant {
            InputVariant::QuickConnect => theme.typography.body_large,
            _ => theme.typography.body,
        };
        let state = self.state.clone();
        let disabled = self.disabled;

        let field = InputBase::new(self.id.clone())
            .focused(focused)
            .disabled(disabled)
            .flex()
            .items_center()
            .w_full()
            .h(height)
            .px(m.space[3])
            .gap(m.space[3])
            .rounded(radius)
            .bg(c.surface)
            .border(m.hairline)
            .border_color(resting_border)
            .text_color(c.text)
            .type_style(text_style)
            .font_family(theme.typography.ui_family.clone())
            .when(!disabled, |this| {
                this.cursor_text()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        state.update(cx, |state, cx| state.focus(window, cx));
                    })
            })
            .styles(move |styles| {
                styles
                    .focused(move |style| style.border_color(focused_border))
                    .disabled(|style| style.opacity(0.4))
            })
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(IconSize::Small).color(c.text_faint))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(Input::new(&self.state)),
            )
            .children(self.trailing);

        div()
            .flex()
            .flex_col()
            .gap(m.space_dense)
            .w_full()
            .when_some(self.label, |this, label| {
                this.child(
                    div()
                        .text_color(c.text_muted)
                        .type_style(theme.typography.caption)
                        .child(label),
                )
            })
            .child(field)
            .map(|this| match (self.error, self.hint) {
                (Some(error), _) => this.child(
                    div()
                        .text_color(c.status_error)
                        .type_style(theme.typography.caption)
                        .child(error),
                ),
                (None, Some(hint)) => this.child(
                    div()
                        .text_color(c.text_faint)
                        .type_style(theme.typography.caption)
                        .child(hint),
                ),
                (None, None) => this,
            })
    }
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The title bar search. It looks like a field but opens the command palette,
/// which is where typing happens.
#[derive(IntoElement)]
pub struct SearchTrigger {
    base: gpui_base::Button,
    placeholder: SharedString,
    keystroke: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl SearchTrigger {
    pub fn new(id: impl Into<ElementId>, placeholder: impl Into<SharedString>) -> Self {
        let placeholder = placeholder.into();
        Self {
            base: gpui_base::Button::new(id).accessibility_label(placeholder.clone()),
            placeholder,
            keystroke: None,
            on_click: None,
        }
    }

    pub fn keystroke(mut self, keystroke: impl Into<SharedString>) -> Self {
        self.keystroke = Some(keystroke.into());
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

impl RenderOnce for SearchTrigger {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let hover = c.border_strong;
        let ring = focus_ring_shadows(&theme);
        self.base
            .flex_1()
            .min_w_0()
            .h(m.control_default)
            .px(m.space[3])
            .gap(m.space[3])
            .justify_start()
            .rounded(m.radius_control)
            .bg(c.canvas)
            .border(m.hairline)
            .border_color(c.border)
            .text_color(c.text_faint)
            .type_style(theme.typography.body)
            .font_family(theme.typography.ui_family.clone())
            .cursor_pointer()
            .hover(move |style| style.border_color(hover))
            .focus_visible(move |style| style.shadow(ring))
            .child(Icon::new(IconName::Search).size(IconSize::Small))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.placeholder),
            )
            .when_some(self.keystroke, |this, keys| this.child(KeyCap::new(keys)))
            .when_some(self.on_click, |this, handler| this.on_click(handler))
    }
}
