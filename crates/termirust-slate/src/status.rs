//! The status system: every status is a fixed glyph shape, a label, and the
//! status color, so state never depends on color alone.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled, Transformation, Window, div, percentage, prelude::FluentBuilder as _, svg,
};
use termirust_ui_contract::StatusKind;

use crate::{
    icon::GlyphShape,
    theme::{ActiveTheme, SlateStyled},
};

/// One turn of the busy spinner. It moves in eight steps, not smoothly, so it
/// reads as progress without pulling the eye.
const SPIN_PERIOD: Duration = Duration::from_millis(900);
const SPIN_STEPS: f32 = 8.;

/// Every semantic status, in the order the design system lists them.
pub const ALL_STATUSES: [StatusKind; 8] = [
    StatusKind::Idle,
    StatusKind::Busy,
    StatusKind::Done,
    StatusKind::Attention,
    StatusKind::Error,
    StatusKind::Offline,
    StatusKind::Orphaned,
    StatusKind::PermissionDenied,
];

/// A status glyph, optionally followed by its label.
#[derive(IntoElement)]
pub struct StatusGlyph {
    kind: StatusKind,
    label: Option<SharedString>,
}

impl StatusGlyph {
    pub fn new(kind: StatusKind) -> Self {
        Self { kind, label: None }
    }

    /// The text shown after the glyph, in the status color. Use the product
    /// word for the state, such as "Live" or "Host key changed".
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Uses the generic word for the status. Prefer a product word where
    /// one exists; the contract's status text is a message ID, not copy.
    pub fn default_label(self) -> Self {
        let label = default_label(self.kind);
        self.label(label)
    }
}

/// The generic English word for each status.
pub fn default_label(kind: StatusKind) -> &'static str {
    match kind {
        StatusKind::Idle => "Idle",
        StatusKind::Busy => "Running",
        StatusKind::Done => "Done",
        StatusKind::Attention => "Needs attention",
        StatusKind::Error => "Failed",
        StatusKind::Offline => "Offline",
        StatusKind::Orphaned => "Orphaned",
        StatusKind::PermissionDenied => "Permission denied",
    }
}

impl RenderOnce for StatusGlyph {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let m = &theme.metrics;
        let visual = theme.tokens.status(self.kind);
        let color = theme.status_color(self.kind);
        let shape = GlyphShape::from_contract(visual.shape).unwrap_or(GlyphShape::HollowCircle);
        let glyph = svg()
            .path(shape.asset_path())
            .size(m.icon_status)
            .flex_none()
            .text_color(color);
        let glyph = if shape == GlyphShape::RingSpinner && !theme.reduced_motion {
            glyph
                .with_animation(
                    SharedString::from(format!("status-spin-{:?}", self.kind)),
                    Animation::new(SPIN_PERIOD).repeat(),
                    |glyph, delta| {
                        let step = (delta * SPIN_STEPS).floor() / SPIN_STEPS;
                        glyph.with_transformation(Transformation::rotate(percentage(step)))
                    },
                )
                .into_any_element()
        } else {
            glyph.into_any_element()
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(m.space_dense)
            .child(glyph)
            .when_some(self.label, |this, label| {
                this.text_color(color)
                    .type_style(theme.typography.caption)
                    .whitespace_nowrap()
                    .child(label)
            })
    }
}
