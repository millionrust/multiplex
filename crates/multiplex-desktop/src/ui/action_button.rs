//! Variation 08: a token-styled action surface around the existing GPUI button behavior.
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, Interactivity, IntoElement,
    ParentElement, RenderOnce, SharedString, StyleRefinement, Styled, Window, div, px, relative,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::{Disableable, Icon, Selectable, Sizable, Size, StyledExt as _};

use super::theme;

#[derive(IntoElement)]
pub(crate) struct ActionButton {
    inner: Button,
    tone: theme::ActionTone,
    label: Option<SharedString>,
    icon: Option<Icon>,
    disabled: bool,
    segment: Option<bool>,
    customized: bool,
}

impl ActionButton {
    pub fn new(id: impl Into<ElementId>, tone: theme::ActionTone, cx: &App) -> Self {
        let inner = Button::new(id)
            .small()
            .h(px(theme::CONTROL_HEIGHT_DEFAULT))
            .px(px(theme::SPACE_3))
            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
            .rounded(px(theme::CONTROL_RADIUS))
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(theme::action_fill(tone))
                    .foreground(theme::action_foreground(tone))
                    .border(theme::action_border(tone))
                    .hover(theme::action_hover(tone))
                    .active(theme::action_active(tone)),
            );
        Self {
            inner,
            tone,
            label: None,
            icon: None,
            disabled: false,
            segment: None,
            customized: false,
        }
    }
    pub fn segmented(mut self, active: bool, cx: &App) -> Self {
        self.segment = Some(active);
        self.inner = self.inner.custom(
            ButtonCustomVariant::new(cx)
                .color(if active {
                    theme::control_bg()
                } else {
                    gpui::transparent_black()
                })
                .foreground(if active {
                    theme::text_main()
                } else {
                    theme::text_muted()
                })
                .border(if active {
                    theme::border_strong()
                } else {
                    gpui::transparent_black()
                })
                .hover(theme::action_hover(self.tone))
                .active(theme::action_active(self.tone)),
        );
        self
    }
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.inner = self.inner.tooltip(tooltip);
        self
    }
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.inner = self.inner.on_click(handler);
        self
    }
}

impl Styled for ActionButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}
impl InteractiveElement for ActionButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.interactivity()
    }
}
impl ParentElement for ActionButton {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.inner.extend(elements);
    }
}
impl Disableable for ActionButton {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self.inner = self.inner.disabled(disabled);
        self
    }
}
impl Selectable for ActionButton {
    fn selected(mut self, selected: bool) -> Self {
        self.inner = self.inner.selected(selected);
        self
    }
    fn is_selected(&self) -> bool {
        self.inner.is_selected()
    }
}
impl Sizable for ActionButton {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.inner = self.inner.with_size(size);
        self
    }
}
impl ButtonVariants for ActionButton {
    fn with_variant(mut self, variant: gpui_component::button::ButtonVariant) -> Self {
        self.customized = true;
        self.inner = self.inner.with_variant(variant);
        self
    }
}
impl RenderOnce for ActionButton {
    fn render(mut self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let foreground = if self.disabled || self.segment == Some(false) {
            theme::text_muted()
        } else if self.segment == Some(true) {
            theme::text_main()
        } else {
            theme::action_foreground(self.tone)
        };
        if let Some(label) = self.label {
            // Keep the label in the app palette across the underlying component's hover state.
            self.inner = if self.customized {
                self.inner.label(label)
            } else {
                self.inner.child(
                    div()
                        .flex_none()
                        .line_height(relative(1.0))
                        .font_medium()
                        .text_color(foreground)
                        .child(label),
                )
            };
        }
        if let Some(icon) = self.icon {
            self.inner = self.inner.icon(icon.text_color(foreground));
        }
        if self.disabled {
            self.inner = self.inner.bg(theme::library_card()).shadow(Vec::new());
        } else {
            if self.segment.is_none() && !self.customized && self.inner.style().background.is_none()
            {
                self.inner = self.inner.bg(theme::action_background(self.tone));
            }
            if !self.customized && self.segment.is_none() {
                self.inner = self.inner.shadow(theme::button_shadow(self.tone));
            }
        }
        self.inner
    }
}
