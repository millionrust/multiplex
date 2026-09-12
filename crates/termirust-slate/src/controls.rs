//! Selection controls: [`Segmented`], [`Toggle`], and [`FilterTabs`].

use std::rc::Rc;

use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_base::{Switch, ToggleGroup};

use crate::{
    icon::{Icon, IconName, IconSize},
    theme::{ActiveTheme, SlateStyled, focus_ring_shadows, inset_ring},
};

type IndexHandler = Rc<dyn Fn(&usize, &mut Window, &mut App) + 'static>;

/// One option in a segmented control: text, an icon, or both.
#[derive(Clone)]
pub struct Segment {
    label: Option<SharedString>,
    icon: Option<IconName>,
    accessibility_label: SharedString,
}

impl Segment {
    pub fn text(label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self {
            accessibility_label: label.clone(),
            label: Some(label),
            icon: None,
        }
    }

    /// An icon-only segment. `label` names it for assistive technology.
    pub fn icon(icon: IconName, label: impl Into<SharedString>) -> Self {
        Self {
            label: None,
            icon: Some(icon),
            accessibility_label: label.into(),
        }
    }
}

/// Picks one option from a small set.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    segments: Vec<Segment>,
    selected: usize,
    disabled: bool,
    on_change: Option<IndexHandler>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            segments: Vec::new(),
            selected: 0,
            disabled: false,
            on_change: None,
        }
    }

    pub fn segment(mut self, segment: Segment) -> Self {
        self.segments.push(segment);
        self
    }

    pub fn segments(mut self, segments: impl IntoIterator<Item = Segment>) -> Self {
        self.segments.extend(segments);
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the index of the segment the person picked.
    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        // The pressed segment sits on the control fill with a hairline ring, so it
        // stands off the track in every theme, including Light and High contrast.
        let pressed_bg = c.control;
        let pressed_ring = inset_ring(m.hairline, c.border_strong);
        let pressed_text = c.text;
        let hover_text = c.text_secondary;
        let ring = focus_ring_shadows(&theme);
        let segment_height = m.control_small;

        ToggleGroup::new(self.id.clone())
            .flex()
            .flex_none()
            .items_center()
            .gap(m.space[1])
            .p(m.space[1])
            .rounded(m.radius_control)
            .bg(c.surface)
            .border(m.hairline)
            .border_color(c.border)
            .when(self.disabled, |this| this.opacity(0.4))
            .children(
                self.segments
                    .into_iter()
                    .enumerate()
                    .map(|(index, segment)| {
                        let on_change = self.on_change.clone();
                        let is_pressed = index == self.selected;
                        let ring = ring.clone();
                        let pressed_ring = pressed_ring.clone();
                        gpui_base::Toggle::new(("segment", index))
                            .pressed(is_pressed)
                            .disabled(self.disabled)
                            .accessibility_label(segment.accessibility_label)
                            .h(segment_height)
                            .px(if segment.label.is_some() {
                                m.space_compact
                            } else {
                                m.space_dense
                            })
                            .gap(m.space_dense)
                            .rounded(m.radius_segment)
                            .text_color(c.text_muted)
                            .type_style(theme.typography.caption)
                            .whitespace_nowrap()
                            .when(!self.disabled && !is_pressed, |this| {
                                this.cursor_pointer()
                                    .hover(move |style| style.text_color(hover_text))
                            })
                            .focus_visible(move |style| style.shadow(ring))
                            .styles(move |styles| {
                                styles.pressed(move |style| {
                                    style
                                        .bg(pressed_bg)
                                        .text_color(pressed_text)
                                        .shadow(pressed_ring)
                                })
                            })
                            .when_some(segment.icon, |this, icon| {
                                this.child(Icon::new(icon).size(IconSize::Small))
                            })
                            .when_some(segment.label, |this, label| this.child(label))
                            .when_some(on_change, |this, on_change| {
                                this.on_change(move |pressed, _, window, cx| {
                                    if pressed {
                                        on_change(&index, window, cx);
                                    }
                                })
                            })
                    }),
            )
    }
}

type ToggleHandler = Rc<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

/// Flips a setting right away. Pair it with a visible label.
#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    checked: bool,
    disabled: bool,
    label: SharedString,
    on_change: Option<ToggleHandler>,
}

impl Toggle {
    /// `label` names the setting for assistive technology.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            checked: false,
            disabled: false,
            label: label.into(),
            on_change: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the new value.
    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

/// Track and knob geometry from the design system: a 30 × 18 pill with a
/// 14 px knob inset 2 px.
const TRACK_WIDTH: f32 = 30.;
const TRACK_HEIGHT: f32 = 18.;
const KNOB: f32 = 14.;

impl RenderOnce for Toggle {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let on = self.checked;
        let track = c.accent.opacity(0.6);
        let ring = focus_ring_shadows(&theme);
        // The track keeps its hairline in both states so the knob does not
        // shift by a pixel when the border appears.
        let inset = (px(TRACK_HEIGHT) - px(KNOB)) / 2. - m.hairline;
        let travel = px(TRACK_WIDTH) - px(KNOB) - (inset + m.hairline) * 2.;

        Switch::new(self.id)
            .checked(on)
            .disabled(self.disabled)
            .accessibility_label(self.label)
            .relative()
            .flex_none()
            .w(px(TRACK_WIDTH))
            .h(px(TRACK_HEIGHT))
            .rounded(m.radius_pill)
            .border(m.hairline)
            .border_color(c.border)
            .when(!self.disabled, |this| this.cursor_pointer())
            .focus_visible(move |style| style.shadow(ring))
            .styles(move |styles| {
                styles
                    .checked(move |style| style.bg(track).border_color(track))
                    .disabled(|style| style.opacity(0.4))
            })
            .child(
                div()
                    .absolute()
                    .top(inset)
                    .left(if on { inset + travel } else { inset })
                    .size(px(KNOB))
                    .rounded(m.radius_pill)
                    .bg(if on { c.text } else { c.text_muted }),
            )
            .when_some(self.on_change, |this, on_change| {
                this.on_change(move |checked, _, window, cx| on_change(&checked, window, cx))
            })
    }
}

/// Narrows a list without a container: plain words, the current one lit.
#[derive(IntoElement)]
pub struct FilterTabs {
    id: ElementId,
    tabs: Vec<SharedString>,
    selected: usize,
    on_change: Option<IndexHandler>,
}

impl FilterTabs {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            tabs: Vec::new(),
            selected: 0,
            on_change: None,
        }
    }

    pub fn tab(mut self, label: impl Into<SharedString>) -> Self {
        self.tabs.push(label.into());
        self
    }

    pub fn tabs<S: Into<SharedString>>(mut self, labels: impl IntoIterator<Item = S>) -> Self {
        self.tabs.extend(labels.into_iter().map(Into::into));
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for FilterTabs {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.slate();
        let c = &theme.colors;
        let m = &theme.metrics;
        let hover = c.text_secondary;
        let ring = focus_ring_shadows(&theme);

        gpui_base::Tabs::new(self.id.clone())
            .flex()
            .flex_none()
            .items_center()
            .gap(m.space_compact + m.space[2])
            .type_style(theme.typography.caption)
            .children(self.tabs.into_iter().enumerate().map(|(index, label)| {
                let on_change = self.on_change.clone();
                let selected = index == self.selected;
                let ring = ring.clone();
                gpui_base::Tab::new(("filter-tab", index))
                    .selected(selected)
                    .h(m.control_small)
                    .flex()
                    .items_center()
                    .rounded(m.radius_segment)
                    .text_color(if selected { c.text } else { c.text_faint })
                    .when(!selected, |this| {
                        this.cursor_pointer()
                            .hover(move |style| style.text_color(hover))
                    })
                    .focus_visible(move |style| style.shadow(ring))
                    .child(label)
                    .when_some(on_change, |this, on_change| {
                        this.on_click(move |_, window, cx| on_change(&index, window, cx))
                    })
            }))
    }
}
