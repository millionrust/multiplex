//! Split panes: an app-owned binary tree of up to four panes, drawn with
//! resizable dividers and edge drop zones.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Axis, Context, DragMoveEvent, ElementId,
    InteractiveElement as _, IntoElement, ParentElement, Pixels, Point, Render, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _, relative,
};

use crate::{
    terminal::DragChip,
    theme::{ActiveTheme, SlateTheme},
};

/// A split never gives either side less than this share of its parent.
pub const MIN_SPLIT_RATIO: f32 = 0.15;
/// A split never gives either side more than this share of its parent.
pub const MAX_SPLIT_RATIO: f32 = 0.85;

/// Keeps a ratio inside the allowed range.
pub fn clamp_ratio(ratio: f32) -> f32 {
    ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
}

/// The pane edge a drop lands on, which decides where the new pane goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl DropEdge {
    fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Top | Self::Bottom => Axis::Vertical,
        }
    }

    /// Whether the incoming pane takes the first slot of the new split.
    fn incoming_first(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }

    /// The edge nearest a point, given as fractions of the pane size.
    pub fn nearest(x: f32, y: f32) -> Self {
        let candidates = [
            (x, Self::Left),
            (1. - x, Self::Right),
            (y, Self::Top),
            (1. - y, Self::Bottom),
        ];
        candidates
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, edge)| edge)
            .unwrap_or(Self::Right)
    }
}

/// A path from the root to a split: `false` is the first child, `true` the
/// second.
pub type SplitPath = Vec<bool>;

/// The split layout of one terminal tab.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitNode<P> {
    Leaf(P),
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<SplitNode<P>>,
        second: Box<SplitNode<P>>,
    },
}

impl<P: Clone + PartialEq> SplitNode<P> {
    pub fn leaf(pane: P) -> Self {
        Self::Leaf(pane)
    }

    pub fn pane_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    /// Panes in reading order.
    pub fn panes(&self) -> Vec<&P> {
        match self {
            Self::Leaf(pane) => vec![pane],
            Self::Split { first, second, .. } => {
                let mut panes = first.panes();
                panes.extend(second.panes());
                panes
            }
        }
    }

    pub fn contains(&self, pane: &P) -> bool {
        match self {
            Self::Leaf(leaf) => leaf == pane,
            Self::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// Splits `target`, putting `incoming` on `edge`. Fails when `target` is
    /// missing or the tree is already at `max_panes`.
    pub fn split(&mut self, target: &P, incoming: P, edge: DropEdge, max_panes: usize) -> bool {
        if self.pane_count() >= max_panes || !self.contains(target) {
            return false;
        }
        self.split_unchecked(target, incoming, edge)
    }

    fn split_unchecked(&mut self, target: &P, incoming: P, edge: DropEdge) -> bool {
        match self {
            Self::Leaf(leaf) if leaf == target => {
                let existing = Box::new(Self::Leaf(leaf.clone()));
                let incoming = Box::new(Self::Leaf(incoming));
                let (first, second) = if edge.incoming_first() {
                    (incoming, existing)
                } else {
                    (existing, incoming)
                };
                *self = Self::Split {
                    axis: edge.axis(),
                    ratio: 0.5,
                    first,
                    second,
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.split_unchecked(target, incoming.clone(), edge)
                    || second.split_unchecked(target, incoming, edge)
            }
        }
    }

    /// Removes a pane and lets its sibling take the space. The last pane of a
    /// tab cannot be removed; close the tab instead.
    pub fn remove(&mut self, pane: &P) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                if matches!(first.as_ref(), Self::Leaf(leaf) if leaf == pane) {
                    *self = (**second).clone();
                    true
                } else if matches!(second.as_ref(), Self::Leaf(leaf) if leaf == pane) {
                    *self = (**first).clone();
                    true
                } else {
                    first.remove(pane) || second.remove(pane)
                }
            }
        }
    }

    /// Moves a pane next to another pane in the same tree.
    pub fn move_pane(&mut self, pane: &P, target: &P, edge: DropEdge) -> bool {
        if pane == target || !self.contains(pane) || !self.contains(target) {
            return false;
        }
        let snapshot = self.clone();
        if !self.remove(pane) || !self.split_unchecked(target, pane.clone(), edge) {
            *self = snapshot;
            return false;
        }
        true
    }

    /// Sets the ratio of the split at `path`, clamped to the allowed range.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) -> bool {
        match (self, path.split_first()) {
            (Self::Split { ratio: slot, .. }, None) => {
                *slot = clamp_ratio(ratio);
                true
            }
            (Self::Split { first, second, .. }, Some((&go_second, rest))) => {
                if go_second {
                    second.set_ratio(rest, ratio)
                } else {
                    first.set_ratio(rest, ratio)
                }
            }
            (Self::Leaf(_), _) => false,
        }
    }
}

/// The drag payload for a pane. Attach it to a [`crate::PaneHeader`] with
/// `.on_drag(PaneDrag::new(id, label), PaneDrag::preview)`.
#[derive(Clone)]
pub struct PaneDrag<P> {
    pub pane: P,
    pub label: SharedString,
}

impl<P: Clone + 'static> PaneDrag<P> {
    pub fn new(pane: P, label: impl Into<SharedString>) -> Self {
        Self {
            pane,
            label: label.into(),
        }
    }

    /// The drag preview constructor for GPUI's `on_drag`.
    pub fn preview(
        drag: &Self,
        _: Point<Pixels>,
        _: &mut Window,
        cx: &mut App,
    ) -> gpui::Entity<DragChip> {
        DragChip::build(drag.label.clone(), cx)
    }
}

/// A completed pane drop.
#[derive(Clone, Debug)]
pub struct PaneDrop<P> {
    /// The dragged pane. It may come from another tab.
    pub pane: P,
    /// The pane it was dropped on.
    pub target: P,
    pub edge: DropEdge,
}

/// A divider move: the split's path and its new, clamped ratio.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitResize {
    pub path: SplitPath,
    pub ratio: f32,
}

#[derive(Clone)]
struct DividerDrag {
    key: SharedString,
}

struct EmptyDrag;

impl Render for EmptyDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct SplitState<P> {
    hint: Option<(P, DropEdge)>,
    active_divider: Option<SharedString>,
}

type PaneRenderer<P> = Rc<dyn Fn(&P, &mut Window, &mut App) -> AnyElement + 'static>;
type ResizeHandler = Rc<dyn Fn(&SplitResize, &mut Window, &mut App) + 'static>;
type DropHandler<P> = Rc<dyn Fn(&PaneDrop<P>, &mut Window, &mut App) + 'static>;

/// Draws a [`SplitNode`] tree. Slate owns the dividers and drop zones; the app
/// owns the tree and draws each pane.
#[derive(IntoElement)]
pub struct SplitPanes<P: Clone + PartialEq + 'static> {
    id: ElementId,
    tree: SplitNode<P>,
    render_pane: PaneRenderer<P>,
    on_resize: Option<ResizeHandler>,
    on_drop: Option<DropHandler<P>>,
    max_panes: Option<usize>,
    header_inset: bool,
}

impl<P: Clone + PartialEq + 'static> SplitPanes<P> {
    pub fn new(
        id: impl Into<ElementId>,
        tree: &SplitNode<P>,
        render_pane: impl Fn(&P, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            tree: tree.clone(),
            render_pane: Rc::new(render_pane),
            on_resize: None,
            on_drop: None,
            max_panes: None,
            header_inset: true,
        }
    }

    /// Called while a divider moves, with the split's path and new ratio.
    pub fn on_resize(
        mut self,
        handler: impl Fn(&SplitResize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_resize = Some(Rc::new(handler));
        self
    }

    /// Called when a pane is dropped on a pane edge.
    pub fn on_drop(
        mut self,
        handler: impl Fn(&PaneDrop<P>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drop = Some(Rc::new(handler));
        self
    }

    /// Overrides the pane cap. The default is `layout.split.max_panes`.
    pub fn max_panes(mut self, max: usize) -> Self {
        self.max_panes = Some(max);
        self
    }
}

struct Ctx<P: Clone + PartialEq + 'static> {
    id: SharedString,
    theme: Rc<SlateTheme>,
    state: gpui::Entity<SplitState<P>>,
    render_pane: PaneRenderer<P>,
    on_resize: Option<ResizeHandler>,
    on_drop: Option<DropHandler<P>>,
    tree: Rc<SplitNode<P>>,
    max_panes: usize,
    dragging: bool,
    /// The divider being dragged, so it keeps its accent line off-hover.
    active_divider: Option<SharedString>,
    header_inset: bool,
}

impl<P: Clone + PartialEq + 'static> RenderOnce for SplitPanes<P> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state =
            window.use_keyed_state((self.id.clone(), "split"), cx, |_, _| SplitState::<P> {
                hint: None,
                active_divider: None,
            });
        let theme = cx.slate();
        let max_panes = self.max_panes.unwrap_or(theme.metrics.max_panes);
        let dragging = cx.has_active_drag();
        let active_divider = dragging
            .then(|| state.read(cx).active_divider.clone())
            .flatten();
        let ctx = Rc::new(Ctx {
            id: format!("{:?}", self.id).into(),
            theme,
            state,
            render_pane: self.render_pane,
            on_resize: self.on_resize,
            on_drop: self.on_drop,
            tree: Rc::new(self.tree.clone()),
            max_panes,
            dragging,
            active_divider,
            header_inset: self.header_inset && self.tree.pane_count() > 1,
        });
        div()
            .id(self.id)
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(render_node(&self.tree, Vec::new(), &ctx, window, cx))
    }
}

fn render_node<P: Clone + PartialEq + 'static>(
    node: &SplitNode<P>,
    path: SplitPath,
    ctx: &Rc<Ctx<P>>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match node {
        SplitNode::Leaf(pane) => render_leaf(pane, ctx, window, cx),
        SplitNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let axis = *axis;
            let ratio = clamp_ratio(*ratio);
            let mut first_path = path.clone();
            first_path.push(false);
            let mut second_path = path.clone();
            second_path.push(true);
            let key: SharedString = format!("{}:{:?}", ctx.id, path).into();
            let grow = |element: AnyElement, share: f32| {
                div()
                    .relative()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .flex_grow(share)
                    .flex_shrink(1.)
                    .flex_basis(relative(0.))
                    .child(element)
            };
            let first = render_node(first, first_path, ctx, window, cx);
            let second = render_node(second, second_path, ctx, window, cx);
            let resize_key = key.clone();
            let resize_ctx = ctx.clone();

            div()
                .id(SharedString::from(format!("split-{key}")))
                .flex()
                .when(axis == Axis::Vertical, |this| this.flex_col())
                .size_full()
                .min_w_0()
                .min_h_0()
                .on_drag_move::<DividerDrag>(
                    move |event: &DragMoveEvent<DividerDrag>, window, cx| {
                        if event.drag(cx).key != resize_key {
                            return;
                        }
                        let Some(handler) = &resize_ctx.on_resize else {
                            return;
                        };
                        let bounds = event.bounds;
                        let position = event.event.position;
                        let next = match axis {
                            Axis::Horizontal => {
                                (position.x - bounds.origin.x) / bounds.size.width.max(gpui::px(1.))
                            }
                            Axis::Vertical => {
                                (position.y - bounds.origin.y)
                                    / bounds.size.height.max(gpui::px(1.))
                            }
                        };
                        handler(
                            &SplitResize {
                                path: path.clone(),
                                ratio: clamp_ratio(next),
                            },
                            window,
                            cx,
                        );
                    },
                )
                .child(grow(first, ratio))
                .child(render_divider(axis, key, ctx))
                .child(grow(second, 1. - ratio))
                .into_any_element()
        }
    }
}

fn render_divider<P: Clone + PartialEq + 'static>(
    axis: Axis,
    key: SharedString,
    ctx: &Rc<Ctx<P>>,
) -> AnyElement {
    let theme = &ctx.theme;
    let c = &theme.colors;
    let m = &theme.metrics;
    let active = ctx.active_divider.as_ref() == Some(&key);
    let hit = m.divider_hit;
    let offset = (hit - m.hairline) / 2.;
    let group: SharedString = format!("divider-{key}").into();
    let state = ctx.state.clone();
    let drag_key = key.clone();
    let line = div()
        .absolute()
        .bg(c.accent)
        .map(|this| match axis {
            Axis::Horizontal => this
                .top_0()
                .bottom_0()
                .left(offset - m.hairline / 2.)
                .w(m.space[1]),
            Axis::Vertical => this
                .left_0()
                .right_0()
                .top(offset - m.hairline / 2.)
                .h(m.space[1]),
        })
        .when(!active, |this| {
            this.invisible().group_hover(group.clone(), |s| s.visible())
        });

    div()
        .relative()
        .flex_none()
        .bg(c.border)
        .map(|this| match axis {
            Axis::Horizontal => this.w(m.hairline).h_full(),
            Axis::Vertical => this.h(m.hairline).w_full(),
        })
        .child(
            div()
                .id(SharedString::from(format!("divider-hit-{key}")))
                .group(group)
                .absolute()
                .map(|this| match axis {
                    Axis::Horizontal => this
                        .top_0()
                        .bottom_0()
                        .left(-offset)
                        .w(hit)
                        .cursor_col_resize(),
                    Axis::Vertical => this
                        .left_0()
                        .right_0()
                        .top(-offset)
                        .h(hit)
                        .cursor_row_resize(),
                })
                .on_drag(DividerDrag { key: drag_key }, move |drag, _, _, cx| {
                    let key = drag.key.clone();
                    state.update(cx, |state, cx| {
                        state.active_divider = Some(key);
                        cx.notify();
                    });
                    cx.new(|_| EmptyDrag)
                })
                .child(line),
        )
        .into_any_element()
}

fn render_leaf<P: Clone + PartialEq + 'static>(
    pane: &P,
    ctx: &Rc<Ctx<P>>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = &ctx.theme;
    let c = &theme.colors;
    let m = &theme.metrics;
    let hint = ctx
        .dragging
        .then(|| ctx.state.read(cx).hint.clone())
        .flatten()
        .filter(|(target, _)| target == pane)
        .map(|(_, edge)| edge);
    let content = (ctx.render_pane)(pane, window, cx);
    let move_ctx = ctx.clone();
    let drop_ctx = ctx.clone();
    let move_pane = pane.clone();
    let drop_pane = pane.clone();
    let top_inset = if ctx.header_inset {
        m.pane_header_height
    } else {
        m.space[0]
    };

    div()
        .id(SharedString::from(format!(
            "pane-{}-{}",
            ctx.id,
            pane_index(pane, &ctx.tree)
        )))
        .relative()
        .size_full()
        .min_w_0()
        .min_h_0()
        .child(content)
        .on_drag_move::<PaneDrag<P>>(move |event: &DragMoveEvent<PaneDrag<P>>, _, cx| {
            let bounds = event.bounds;
            let position = event.event.position;
            let dragged = event.drag(cx).pane.clone();
            let accepts = dragged != move_pane
                && (move_ctx.tree.contains(&dragged)
                    || move_ctx.tree.pane_count() < move_ctx.max_panes);
            let next = (accepts && bounds.contains(&position)).then(|| {
                let x = (position.x - bounds.origin.x) / bounds.size.width.max(gpui::px(1.));
                let y = (position.y - bounds.origin.y) / bounds.size.height.max(gpui::px(1.));
                (move_pane.clone(), DropEdge::nearest(x, y))
            });
            move_ctx.state.update(cx, |state, cx| {
                let mine = state
                    .hint
                    .as_ref()
                    .is_some_and(|(target, _)| *target == move_pane);
                if next.is_some() {
                    if state.hint != next {
                        state.hint = next;
                        cx.notify();
                    }
                } else if mine {
                    state.hint = None;
                    cx.notify();
                }
            });
        })
        .on_drop::<PaneDrag<P>>(move |drag, window, cx| {
            let hint = drop_ctx.state.update(cx, |state, cx| {
                cx.notify();
                state.hint.take()
            });
            let Some((target, edge)) = hint.filter(|(target, _)| *target == drop_pane) else {
                return;
            };
            if let Some(handler) = &drop_ctx.on_drop {
                handler(
                    &PaneDrop {
                        pane: drag.pane.clone(),
                        target,
                        edge,
                    },
                    window,
                    cx,
                );
            }
        })
        .when_some(hint, |this, edge| {
            let zone = div()
                .absolute()
                .rounded(m.radius_control)
                .bg(c.accent.opacity(0.14))
                .border(m.hairline)
                .border_color(c.accent.opacity(0.7));
            let inset = m.space[2];
            let zone = match edge {
                DropEdge::Left => zone
                    .top(top_inset + inset)
                    .bottom(inset)
                    .left(inset)
                    .w(relative(0.5)),
                DropEdge::Right => zone
                    .top(top_inset + inset)
                    .bottom(inset)
                    .right(inset)
                    .w(relative(0.5)),
                DropEdge::Top => zone
                    .top(top_inset + inset)
                    .left(inset)
                    .right(inset)
                    .h(relative(0.45)),
                DropEdge::Bottom => zone
                    .bottom(inset)
                    .left(inset)
                    .right(inset)
                    .h(relative(0.45)),
            };
            this.child(zone)
        })
        .into_any_element()
}

fn pane_index<P: Clone + PartialEq>(pane: &P, tree: &SplitNode<P>) -> usize {
    tree.panes()
        .iter()
        .position(|candidate| *candidate == pane)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> SplitNode<u32> {
        SplitNode::leaf(1)
    }

    #[test]
    fn splitting_places_the_incoming_pane_on_the_drop_edge() {
        let mut layout = tree();
        assert!(layout.split(&1, 2, DropEdge::Right, 4));
        assert_eq!(layout.panes(), vec![&1, &2]);
        assert!(layout.split(&1, 3, DropEdge::Top, 4));
        assert_eq!(layout.panes(), vec![&3, &1, &2]);
        let SplitNode::Split { axis, .. } = &layout else {
            panic!("root is a split");
        };
        assert_eq!(*axis, Axis::Horizontal);
    }

    #[test]
    fn the_pane_cap_is_enforced() {
        let mut layout = tree();
        for pane in 2..=4 {
            assert!(layout.split(&1, pane, DropEdge::Right, 4));
        }
        assert_eq!(layout.pane_count(), 4);
        assert!(!layout.split(&1, 5, DropEdge::Right, 4));
    }

    #[test]
    fn removing_a_pane_collapses_its_split() {
        let mut layout = tree();
        layout.split(&1, 2, DropEdge::Right, 4);
        layout.split(&2, 3, DropEdge::Bottom, 4);
        assert!(layout.remove(&2));
        assert_eq!(layout.panes(), vec![&1, &3]);
        assert!(layout.remove(&3));
        assert_eq!(layout, SplitNode::leaf(1));
        assert!(!layout.remove(&1), "the last pane closes the tab instead");
    }

    #[test]
    fn moving_a_pane_rearranges_without_changing_the_count() {
        let mut layout = tree();
        layout.split(&1, 2, DropEdge::Right, 4);
        layout.split(&2, 3, DropEdge::Right, 4);
        assert!(layout.move_pane(&3, &1, DropEdge::Left));
        assert_eq!(layout.panes(), vec![&3, &1, &2]);
        assert_eq!(layout.pane_count(), 3);
        assert!(!layout.move_pane(&1, &1, DropEdge::Left));
    }

    #[test]
    fn ratios_are_clamped() {
        let mut layout = tree();
        layout.split(&1, 2, DropEdge::Right, 4);
        assert!(layout.set_ratio(&[], 0.02));
        let SplitNode::Split { ratio, .. } = &layout else {
            panic!("root is a split");
        };
        assert_eq!(*ratio, MIN_SPLIT_RATIO);
        assert!(!layout.set_ratio(&[false], 0.5), "a leaf has no ratio");
    }

    #[test]
    fn the_nearest_edge_wins() {
        assert_eq!(DropEdge::nearest(0.9, 0.5), DropEdge::Right);
        assert_eq!(DropEdge::nearest(0.1, 0.5), DropEdge::Left);
        assert_eq!(DropEdge::nearest(0.5, 0.05), DropEdge::Top);
        assert_eq!(DropEdge::nearest(0.5, 0.97), DropEdge::Bottom);
    }
}
