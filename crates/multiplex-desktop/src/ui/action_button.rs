//! Variation 08: a token-styled action surface around the existing GPUI button behavior.
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, Interactivity, IntoElement,
    ParentElement, RenderOnce, SharedString, StyleRefinement, Styled, Window, div, px, relative,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants};
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
    quiet: bool,
    has_children: bool,
    size: Size,
}

impl ActionButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            inner: Button::new(id)
                .small()
                .h(px(theme::CONTROL_HEIGHT_DEFAULT))
                .px(px(theme::SPACE_3))
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .rounded(px(theme::CONTROL_RADIUS)),
            tone: theme::ActionTone::Neutral,
            label: None,
            icon: None,
            disabled: false,
            segment: None,
            customized: false,
            quiet: false,
            has_children: false,
            size: Size::Small,
        }
    }
    pub fn with_tone(id: impl Into<ElementId>, tone: theme::ActionTone, _cx: &App) -> Self {
        let mut button = Self::new(id);
        button.tone = tone;
        button
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
        self.has_children = true;
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
        let size = size.into();
        self.size = size;
        self.inner = self.inner.with_size(size).h(match size {
            Size::XSmall => px(theme::CONTROL_HEIGHT_COMPACT),
            Size::Size(height) => height,
            _ => px(theme::CONTROL_HEIGHT_DEFAULT),
        });
        self
    }
}
impl ButtonVariants for ActionButton {
    fn with_variant(mut self, variant: gpui_component::button::ButtonVariant) -> Self {
        self.customized = matches!(variant, ButtonVariant::Custom(_));
        self.quiet = matches!(
            variant,
            ButtonVariant::Ghost | ButtonVariant::Link | ButtonVariant::Text
        );
        self.tone = match variant {
            ButtonVariant::Primary => theme::ActionTone::Accent,
            ButtonVariant::Danger => theme::ActionTone::Danger,
            ButtonVariant::Info | ButtonVariant::Success | ButtonVariant::Warning => {
                theme::ActionTone::AccentSoft
            }
            _ => theme::ActionTone::Neutral,
        };
        self.inner = self.inner.with_variant(variant);
        self
    }
}
impl RenderOnce for ActionButton {
    fn render(mut self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let foreground = if self.disabled || self.segment == Some(false) {
            theme::text_muted()
        } else if self.segment == Some(true) || self.inner.is_selected() {
            theme::text_main()
        } else {
            theme::action_foreground(self.tone)
        };
        let selected = self.inner.is_selected();
        if self.segment.is_none() && !self.customized {
            self.inner = self.inner.custom(
                ButtonCustomVariant::new(cx)
                    .color(if self.quiet && !selected {
                        gpui::transparent_black()
                    } else {
                        theme::action_fill(self.tone)
                    })
                    .foreground(foreground)
                    .border(if selected {
                        theme::border_strong()
                    } else if self.quiet {
                        gpui::transparent_black()
                    } else {
                        theme::action_border(self.tone)
                    })
                    .hover(theme::action_hover(self.tone))
                    .active(theme::action_active(self.tone)),
            );
        }
        if self.label.is_none() && !self.has_children {
            self.inner = self.inner.px(px(0.));
            if self.inner.style().size.width.is_none() {
                self.inner = self.inner.w(match self.size {
                    Size::XSmall => px(theme::CONTROL_HEIGHT_COMPACT),
                    Size::Size(width) => width,
                    _ => px(theme::CONTROL_HEIGHT_DEFAULT),
                });
            }
        }
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
            if self.segment.is_none()
                && !self.customized
                && !self.quiet
                && self.inner.style().background.is_none()
            {
                self.inner = self.inner.bg(theme::action_background(self.tone));
            }
            if !self.customized && !self.quiet && self.segment.is_none() {
                self.inner = self.inner.shadow(theme::button_shadow(self.tone));
            }
        }
        self.inner
    }
}
