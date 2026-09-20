//! Tooltips name icon-only controls and explain why something is disabled.
//!
//! Slate uses GPUI's own tooltip lifecycle (500 ms delay, instant once one is
//! showing) and only supplies the view.

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{
    kbd::KeyCap,
    theme::{ActiveTheme, SlateStyled},
};

/// Text wraps past this width.
const TOOLTIP_MAX_WIDTH: f32 = 260.;

/// Tooltip content: one line of text and an optional shortcut.
#[derive(Clone, Debug)]
pub struct Tooltip {
    text: SharedString,
    keystroke: Option<SharedString>,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            keystroke: None,
        }
    }

    /// Adds a shortcut in GPUI keystroke syntax, such as `cmd-shift-b`.
    pub fn keystroke(mut self, keystroke: impl Into<SharedString>) -> Self {
        self.keystroke = Some(keystroke.into());
        self
    }

    /// Builds the tooltip view for GPUI's `.tooltip(...)` hook.
    pub fn build(self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|_| TooltipView(self)).into()
    }
}

impl From<&'static str> for Tooltip {
    fn from(text: &'static str) -> Self {
        Self::new(text)
    }
}

impl From<SharedString> for Tooltip {
    fn from(text: SharedString) -> Self {
        Self::new(text)
    }
}

impl From<String> for Tooltip {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

struct TooltipView(Tooltip);

impl Render for TooltipView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        // GPUI places the tooltip relative to the cursor; the small top
        // margin keeps it off the pointer.
        div().pt(m.space[3]).child(
            div()
                .flex()
                .items_center()
                .gap(m.space_dense)
                .min_h(m.toolbar_button)
                .max_w(gpui::px(TOOLTIP_MAX_WIDTH))
                .px(m.space[3])
                .py(m.space[1])
                .rounded(m.radius_control)
                .bg(c.control)
                .border(m.hairline)
                .border_color(c.border_strong)
                .shadow(theme.shadows.toast.clone())
                .text_color(c.text)
                .type_style(theme.typography.caption)
                .font_family(theme.typography.ui_family.clone())
                .child(self.0.text.clone())
                .when_some(self.0.keystroke.clone(), |this, keys| {
                    this.child(KeyCap::new(keys))
                }),
        )
    }
}
