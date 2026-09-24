//! The split layout of a workspace: a binary tree of panes, the pure operations
//! on it, and the pixel layout it produces.

use crate::models::{SavedSplitNode, SplitAxis};
use crate::ui::theme::PANE_GAP;

pub(super) const MIN_SPLIT_RATIO: f32 = 0.08;
pub(super) const MAX_SPLIT_RATIO: f32 = 0.92;
/// How close a dragged divider must come to a third or a half to snap to it.
pub(super) const RATIO_SNAP_DISTANCE: f32 = 0.02;
/// How far one keyboard resize moves a divider, as a share of its split.
pub(super) const KEYBOARD_DIVIDER_STEP: f32 = 0.05;

/// Recursive split layout for a workspace — a binary tree of panes.
#[derive(Clone, Debug)]
pub(super) enum SplitNode {
    /// A single terminal pane (by id).
    Leaf(u64),
    /// Two children laid out along `axis`; `ratio` is the fraction given to `a`.
    Split {
        axis: SplitAxis,
        ratio: f32,
        a: Box<SplitNode>,
        b: Box<SplitNode>,
    },
}

impl SplitNode {
    pub(super) fn collect_leaves(&self, out: &mut Vec<u64>) {
        match self {
            SplitNode::Leaf(id) => out.push(*id),
            SplitNode::Split { a, b, .. } => {
                a.collect_leaves(out);
                b.collect_leaves(out);
            }
        }
    }

    pub(super) fn leaf_ids(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    pub(super) fn first_leaf(&self) -> u64 {
        match self {
            SplitNode::Leaf(id) => *id,
            SplitNode::Split { a, .. } => a.first_leaf(),
        }
    }

    /// Replace the leaf for `target` with a split of `target` and `new_node`.
    pub(super) fn split_leaf(
        &mut self,
        target: u64,
        new_node: &SplitNode,
        axis: SplitAxis,
        new_first: bool,
    ) -> bool {
        match self {
            SplitNode::Leaf(id) if *id == target => {
                let existing = SplitNode::Leaf(*id);
                let (a, b) = if new_first {
                    (new_node.clone(), existing)
                } else {
                    (existing, new_node.clone())
                };
                *self = SplitNode::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(a),
                    b: Box::new(b),
                };
                true
            }
            SplitNode::Leaf(_) => false,
            SplitNode::Split { a, b, .. } => {
                a.split_leaf(target, new_node, axis, new_first)
                    || b.split_leaf(target, new_node, axis, new_first)
            }
        }
    }

    /// Remove `pane`'s leaf, collapsing the parent split into its sibling.
    pub(super) fn without_pane(self, pane: u64) -> Option<SplitNode> {
        match self {
            SplitNode::Leaf(id) => (id != pane).then_some(SplitNode::Leaf(id)),
            SplitNode::Split { axis, ratio, a, b } => {
                match (a.without_pane(pane), b.without_pane(pane)) {
                    (None, None) => None,
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (Some(a), Some(b)) => Some(SplitNode::Split {
                        axis,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                }
            }
        }
    }

    /// Set the ratio of the split whose `b` subtree starts at `divider_id`.
    pub(super) fn set_ratio(&mut self, divider_id: u64, ratio: f32) -> bool {
        if let SplitNode::Split { ratio: r, a, b, .. } = self {
            if b.first_leaf() == divider_id {
                *r = ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
                return true;
            }
            return a.set_ratio(divider_id, ratio) || b.set_ratio(divider_id, ratio);
        }
        false
    }

    pub(super) fn contains(&self, pane: u64) -> bool {
        match self {
            SplitNode::Leaf(id) => *id == pane,
            SplitNode::Split { a, b, .. } => a.contains(pane) || b.contains(pane),
        }
    }

    pub(super) fn leaf_count(&self) -> usize {
        match self {
            SplitNode::Leaf(_) => 1,
            SplitNode::Split { a, b, .. } => a.leaf_count() + b.leaf_count(),
        }
    }

    /// Exchange the places of two panes, keeping every ratio.
    pub(super) fn swap_leaves(&mut self, first: u64, second: u64) -> bool {
        if first == second || !self.contains(first) || !self.contains(second) {
            return false;
        }
        self.swap_leaves_unchecked(first, second);
        true
    }

    fn swap_leaves_unchecked(&mut self, first: u64, second: u64) {
        match self {
            SplitNode::Leaf(id) if *id == first => *id = second,
            SplitNode::Leaf(id) if *id == second => *id = first,
            SplitNode::Leaf(_) => {}
            SplitNode::Split { a, b, .. } => {
                a.swap_leaves_unchecked(first, second);
                b.swap_leaves_unchecked(first, second);
            }
        }
    }

    /// Take `pane` out of the tree and split `target` with it on `edge`. The
    /// tree is unchanged when either pane is missing or they are the same.
    pub(super) fn move_leaf(&mut self, pane: u64, target: u64, edge: SplitEdge) -> bool {
        if pane == target || !self.contains(pane) || !self.contains(target) {
            return false;
        }
        let Some(mut rest) = self.clone().without_pane(pane) else {
            return false;
        };
        let (axis, new_first) = edge.axis_and_order();
        if !rest.split_leaf(target, &SplitNode::Leaf(pane), axis, new_first) {
            return false;
        }
        *self = rest;
        true
    }

    /// Give every pane along a run of same-axis splits an equal share.
    pub(super) fn equalize(&mut self) {
        if let SplitNode::Split { axis, ratio, a, b } = self {
            let wa = a.weight(*axis) as f32;
            let wb = b.weight(*axis) as f32;
            *ratio = wa / (wa + wb);
            a.equalize();
            b.equalize();
        }
    }

    /// How many panes sit side by side along `axis` in this subtree.
    fn weight(&self, axis: SplitAxis) -> usize {
        match self {
            SplitNode::Split {
                axis: own, a, b, ..
            } if *own == axis => a.weight(axis) + b.weight(axis),
            _ => 1,
        }
    }

    /// Even out only the split whose `b` subtree starts at `divider_id`.
    pub(super) fn equalize_divider(&mut self, divider_id: u64) -> bool {
        if let SplitNode::Split { axis, ratio, a, b } = self {
            if b.first_leaf() == divider_id {
                let wa = a.weight(*axis) as f32;
                let wb = b.weight(*axis) as f32;
                *ratio = wa / (wa + wb);
                return true;
            }
            return a.equalize_divider(divider_id) || b.equalize_divider(divider_id);
        }
        false
    }

    /// Move the divider nearest to `pane` along `axis` by `delta` of its span.
    /// Returns false when no split on that axis contains the pane.
    pub(super) fn nudge_ratio(&mut self, pane: u64, axis: SplitAxis, delta: f32) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split {
                axis: own,
                ratio,
                a,
                b,
            } => {
                let child = if a.contains(pane) {
                    a
                } else if b.contains(pane) {
                    b
                } else {
                    return false;
                };
                if child.nudge_ratio(pane, axis, delta) {
                    return true;
                }
                if *own != axis {
                    return false;
                }
                *ratio = (*ratio + delta).clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
                true
            }
        }
    }
}

/// Build a right-leaning flat split tree along one axis (used to reconstruct a
/// layout from saved state that predates nested splits).
pub(super) fn flat_split(pane_ids: &[u64], axis: SplitAxis) -> Option<SplitNode> {
    let (first, rest) = pane_ids.split_first()?;
    let mut node = SplitNode::Leaf(*first);
    for id in rest {
        node = SplitNode::Split {
            axis,
            ratio: 0.5,
            a: Box::new(node),
            b: Box::new(SplitNode::Leaf(*id)),
        };
    }
    Some(node)
}

/// Convert a runtime `SplitNode` (pane ids) to its persistable form, mapping
/// each pane id to its index. Returns `None` if any pane id is missing.
pub(super) fn split_node_to_saved(
    node: &SplitNode,
    indices: &std::collections::HashMap<u64, usize>,
) -> Option<SavedSplitNode> {
    match node {
        SplitNode::Leaf(id) => indices.get(id).copied().map(SavedSplitNode::Leaf),
        SplitNode::Split { axis, ratio, a, b } => Some(SavedSplitNode::Split {
            axis: *axis,
            ratio: *ratio,
            a: Box::new(split_node_to_saved(a, indices)?),
            b: Box::new(split_node_to_saved(b, indices)?),
        }),
    }
}

/// Rebuild a runtime `SplitNode` from its persisted form, mapping pane indices
/// back to live pane ids.
pub(super) fn saved_to_split_node(node: &SavedSplitNode, pane_ids: &[u64]) -> Option<SplitNode> {
    match node {
        SavedSplitNode::Leaf(index) => pane_ids.get(*index).copied().map(SplitNode::Leaf),
        SavedSplitNode::Split { axis, ratio, a, b } => Some(SplitNode::Split {
            axis: *axis,
            ratio: *ratio,
            a: Box::new(saved_to_split_node(a, pane_ids)?),
            b: Box::new(saved_to_split_node(b, pane_ids)?),
        }),
    }
}

#[derive(Clone, Copy)]
pub(super) struct PaneRect {
    pub(super) pane_id: u64,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

#[derive(Clone, Copy)]
pub(super) struct DividerRect {
    pub(super) divider_id: u64,
    pub(super) axis: SplitAxis,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) span: f32,
    pub(super) ratio: f32,
}

/// Walk a `SplitNode` tree, emitting a flat pixel rect for every pane leaf and
/// every divider, within the box `(x, y, width, height)`.
pub(super) fn compute_split_layout(
    node: &SplitNode,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    panes: &mut Vec<PaneRect>,
    dividers: &mut Vec<DividerRect>,
) {
    match node {
        SplitNode::Leaf(id) => panes.push(PaneRect {
            pane_id: *id,
            x,
            y,
            width: width.max(1.0),
            height: height.max(1.0),
        }),
        SplitNode::Split { axis, ratio, a, b } => {
            let ratio = ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
            match axis {
                SplitAxis::Horizontal => {
                    // Ratios are shares of the width plus one gap, so equal shares give
                    // equal panes however deeply the splits nest.
                    let span = (width + PANE_GAP).max(2.0);
                    let aw = (span * ratio - PANE_GAP).max(1.0);
                    let bw = (width - aw - PANE_GAP).max(1.0);
                    compute_split_layout(a, x, y, aw, height, panes, dividers);
                    dividers.push(DividerRect {
                        divider_id: b.first_leaf(),
                        axis: *axis,
                        x: x + aw,
                        y,
                        width: PANE_GAP,
                        height,
                        span,
                        ratio,
                    });
                    compute_split_layout(b, x + aw + PANE_GAP, y, bw, height, panes, dividers);
                }
                SplitAxis::Vertical => {
                    let span = (height + PANE_GAP).max(2.0);
                    let ah = (span * ratio - PANE_GAP).max(1.0);
                    let bh = (height - ah - PANE_GAP).max(1.0);
                    compute_split_layout(a, x, y, width, ah, panes, dividers);
                    dividers.push(DividerRect {
                        divider_id: b.first_leaf(),
                        axis: *axis,
                        x,
                        y: y + ah,
                        width,
                        height: PANE_GAP,
                        span,
                        ratio,
                    });
                    compute_split_layout(b, x, y + ah + PANE_GAP, width, bh, panes, dividers);
                }
            }
        }
    }
}

/// The edge of a pane that another pane is placed against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SplitEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl SplitEdge {
    /// The split axis this edge creates, and whether the incoming pane comes first.
    pub(super) fn axis_and_order(self) -> (SplitAxis, bool) {
        match self {
            SplitEdge::Left => (SplitAxis::Horizontal, true),
            SplitEdge::Right => (SplitAxis::Horizontal, false),
            SplitEdge::Top => (SplitAxis::Vertical, true),
            SplitEdge::Bottom => (SplitAxis::Vertical, false),
        }
    }
}

/// A divider ratio that clicked into a third or a half while dragging.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RatioMark {
    OneThird,
    Half,
    TwoThirds,
}

impl RatioMark {
    pub(super) fn label(self) -> &'static str {
        match self {
            RatioMark::OneThird => "\u{2153}",
            RatioMark::Half => "\u{00bd}",
            RatioMark::TwoThirds => "\u{2154}",
        }
    }
}

/// Clamp a dragged ratio and pull it onto a third or a half when it is close.
pub(super) fn snap_ratio(ratio: f32, snapping: bool) -> (f32, Option<RatioMark>) {
    let ratio = ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
    if !snapping {
        return (ratio, None);
    }
    [
        (1.0 / 3.0, RatioMark::OneThird),
        (0.5, RatioMark::Half),
        (2.0 / 3.0, RatioMark::TwoThirds),
    ]
    .into_iter()
    .find(|(mark, _)| (ratio - mark).abs() < RATIO_SNAP_DISTANCE)
    .map_or((ratio, None), |(mark, label)| (mark, Some(label)))
}

/// A direction to move focus between panes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

impl From<crate::ui::keys::ShortcutDirection> for PaneDirection {
    fn from(direction: crate::ui::keys::ShortcutDirection) -> Self {
        use crate::ui::keys::ShortcutDirection;
        match direction {
            ShortcutDirection::Left => PaneDirection::Left,
            ShortcutDirection::Right => PaneDirection::Right,
            ShortcutDirection::Up => PaneDirection::Up,
            ShortcutDirection::Down => PaneDirection::Down,
        }
    }
}

impl PaneDirection {
    pub(super) fn axis(self) -> SplitAxis {
        match self {
            PaneDirection::Left | PaneDirection::Right => SplitAxis::Horizontal,
            PaneDirection::Up | PaneDirection::Down => SplitAxis::Vertical,
        }
    }

    /// +1 for right and down, -1 for left and up.
    pub(super) fn sign(self) -> f32 {
        match self {
            PaneDirection::Right | PaneDirection::Down => 1.0,
            PaneDirection::Left | PaneDirection::Up => -1.0,
        }
    }
}

/// The pane nearest to `from` on screen in `direction`. Panes that overlap
/// `from` across the direction of travel win over ones that only sit diagonally.
pub(super) fn neighbor_in_direction(
    rects: &[PaneRect],
    from: u64,
    direction: PaneDirection,
) -> Option<u64> {
    const EDGE_SLOP: f32 = 4.0;
    let current = rects.iter().find(|rect| rect.pane_id == from)?;
    let (cx, cy) = (
        current.x + current.width / 2.0,
        current.y + current.height / 2.0,
    );
    let horizontal = direction.axis() == SplitAxis::Horizontal;
    rects
        .iter()
        .filter(|rect| rect.pane_id != from)
        .filter(|rect| match direction {
            PaneDirection::Left => rect.x + rect.width <= current.x + EDGE_SLOP,
            PaneDirection::Right => rect.x >= current.x + current.width - EDGE_SLOP,
            PaneDirection::Up => rect.y + rect.height <= current.y + EDGE_SLOP,
            PaneDirection::Down => rect.y >= current.y + current.height - EDGE_SLOP,
        })
        .map(|rect| {
            let (ox, oy) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            let overlap = if horizontal {
                (rect.y + rect.height).min(current.y + current.height) - rect.y.max(current.y)
            } else {
                (rect.x + rect.width).min(current.x + current.width) - rect.x.max(current.x)
            };
            let (along, across) = if horizontal {
                ((ox - cx).abs(), (oy - cy).abs())
            } else {
                ((oy - cy).abs(), (ox - cx).abs())
            };
            let score = along + across * if overlap > 0.0 { 0.3 } else { 2.0 };
            (rect.pane_id, score)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(pane_id, _)| pane_id)
}

/// One-click split arrangements.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SplitPreset {
    Single,
    Columns,
    Rows,
    MainAndStack,
    ThreeColumns,
    Grid,
    GridOfSix,
}

impl SplitPreset {
    pub(super) const ALL: [SplitPreset; 7] = [
        SplitPreset::Single,
        SplitPreset::Columns,
        SplitPreset::Rows,
        SplitPreset::MainAndStack,
        SplitPreset::ThreeColumns,
        SplitPreset::Grid,
        SplitPreset::GridOfSix,
    ];

    pub(super) fn pane_count(self) -> usize {
        match self {
            SplitPreset::Single => 1,
            SplitPreset::Columns | SplitPreset::Rows => 2,
            SplitPreset::MainAndStack | SplitPreset::ThreeColumns => 3,
            SplitPreset::Grid => 4,
            SplitPreset::GridOfSix => 6,
        }
    }

    /// Arrange exactly `pane_count()` panes, first pane in the main position.
    pub(super) fn build(self, panes: &[u64]) -> Option<SplitNode> {
        if panes.len() != self.pane_count() {
            return None;
        }
        let leaf = |index: usize| SplitNode::Leaf(panes[index]);
        let split = |axis, ratio, a, b| SplitNode::Split {
            axis,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        };
        use SplitAxis::{Horizontal as Across, Vertical as Down};
        let third = 1.0 / 3.0;
        Some(match self {
            SplitPreset::Single => leaf(0),
            SplitPreset::Columns => split(Across, 0.5, leaf(0), leaf(1)),
            SplitPreset::Rows => split(Down, 0.5, leaf(0), leaf(1)),
            SplitPreset::MainAndStack => {
                split(Across, 0.6, leaf(0), split(Down, 0.5, leaf(1), leaf(2)))
            }
            SplitPreset::ThreeColumns => {
                split(Across, third, leaf(0), split(Across, 0.5, leaf(1), leaf(2)))
            }
            SplitPreset::Grid => split(
                Down,
                0.5,
                split(Across, 0.5, leaf(0), leaf(1)),
                split(Across, 0.5, leaf(2), leaf(3)),
            ),
            SplitPreset::GridOfSix => split(
                Down,
                0.5,
                split(Across, third, leaf(0), split(Across, 0.5, leaf(1), leaf(2))),
                split(Across, third, leaf(3), split(Across, 0.5, leaf(4), leaf(5))),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: u64) -> SplitNode {
        SplitNode::Leaf(id)
    }

    fn split(axis: SplitAxis, ratio: f32, a: SplitNode, b: SplitNode) -> SplitNode {
        SplitNode::Split {
            axis,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// 1 on the left; 2 over 3 on the right.
    fn three_panes() -> SplitNode {
        split(
            SplitAxis::Horizontal,
            0.56,
            leaf(1),
            split(SplitAxis::Vertical, 0.5, leaf(2), leaf(3)),
        )
    }

    fn layout(node: &SplitNode) -> Vec<PaneRect> {
        let mut panes = Vec::new();
        compute_split_layout(node, 0.0, 0.0, 1000.0, 600.0, &mut panes, &mut Vec::new());
        panes
    }

    fn ratio_of(node: &SplitNode) -> f32 {
        match node {
            SplitNode::Split { ratio, .. } => *ratio,
            SplitNode::Leaf(_) => panic!("expected a split"),
        }
    }

    #[test]
    fn swapping_panes_keeps_the_shape_and_ratios() {
        let mut tree = three_panes();
        assert!(tree.swap_leaves(1, 3));
        assert_eq!(tree.leaf_ids(), vec![3, 2, 1]);
        assert_eq!(ratio_of(&tree), 0.56);
        assert!(!tree.swap_leaves(1, 1));
        assert!(!tree.swap_leaves(1, 9));
        assert_eq!(tree.leaf_ids(), vec![3, 2, 1]);
    }

    #[test]
    fn moving_a_pane_places_it_on_the_chosen_edge() {
        let mut tree = three_panes();
        assert!(tree.move_leaf(3, 1, SplitEdge::Top));
        assert_eq!(tree.leaf_ids(), vec![3, 1, 2]);
        let rects = layout(&tree);
        let above = rects.iter().find(|r| r.pane_id == 3).unwrap();
        let below = rects.iter().find(|r| r.pane_id == 1).unwrap();
        assert!(above.y < below.y);
        assert_eq!(above.x, below.x);
    }

    #[test]
    fn moving_a_pane_onto_itself_or_a_stranger_changes_nothing() {
        let mut tree = three_panes();
        assert!(!tree.move_leaf(2, 2, SplitEdge::Left));
        assert!(!tree.move_leaf(2, 9, SplitEdge::Left));
        assert!(!tree.move_leaf(9, 2, SplitEdge::Left));
        assert_eq!(tree.leaf_ids(), vec![1, 2, 3]);
    }

    #[test]
    fn equalize_shares_space_by_panes_along_each_axis() {
        let mut tree = split(
            SplitAxis::Horizontal,
            0.9,
            leaf(1),
            split(SplitAxis::Horizontal, 0.2, leaf(2), leaf(3)),
        );
        tree.equalize();
        let widths: Vec<f32> = layout(&tree).iter().map(|r| r.width.round()).collect();
        assert!(
            widths.windows(2).all(|w| (w[0] - w[1]).abs() <= 1.0),
            "{widths:?}"
        );

        let mut mixed = three_panes();
        mixed.equalize();
        assert_eq!(ratio_of(&mixed), 0.5);
    }

    #[test]
    fn equalizing_one_divider_leaves_the_others() {
        let mut tree = split(
            SplitAxis::Horizontal,
            0.8,
            leaf(1),
            split(SplitAxis::Vertical, 0.2, leaf(2), leaf(3)),
        );
        assert!(tree.equalize_divider(3));
        assert_eq!(ratio_of(&tree), 0.8);
        let SplitNode::Split { b, .. } = &tree else {
            unreachable!()
        };
        assert_eq!(ratio_of(b), 0.5);
    }

    #[test]
    fn nudging_moves_the_nearest_divider_on_that_axis() {
        let mut tree = three_panes();
        assert!(tree.nudge_ratio(3, SplitAxis::Vertical, 0.05));
        let SplitNode::Split { b, .. } = &tree else {
            unreachable!()
        };
        assert!((ratio_of(b) - 0.55).abs() < 1e-6);
        assert_eq!(ratio_of(&tree), 0.56);

        assert!(tree.nudge_ratio(3, SplitAxis::Horizontal, -0.05));
        assert!((ratio_of(&tree) - 0.51).abs() < 1e-6);

        let mut single = leaf(1);
        assert!(!single.nudge_ratio(1, SplitAxis::Horizontal, 0.05));
    }

    #[test]
    fn nudging_stops_at_the_ratio_limits() {
        let mut tree = split(SplitAxis::Horizontal, 0.9, leaf(1), leaf(2));
        for _ in 0..5 {
            tree.nudge_ratio(1, SplitAxis::Horizontal, 0.05);
        }
        assert_eq!(ratio_of(&tree), MAX_SPLIT_RATIO);
    }

    #[test]
    fn ratios_snap_to_thirds_and_halves_unless_bypassed() {
        assert_eq!(snap_ratio(0.49, true), (0.5, Some(RatioMark::Half)));
        assert_eq!(
            snap_ratio(0.34, true),
            (1.0 / 3.0, Some(RatioMark::OneThird))
        );
        assert_eq!(
            snap_ratio(0.655, true),
            (2.0 / 3.0, Some(RatioMark::TwoThirds))
        );
        assert_eq!(snap_ratio(0.45, true), (0.45, None));
        assert_eq!(snap_ratio(0.49, false), (0.49, None));
        assert_eq!(snap_ratio(0.99, true), (MAX_SPLIT_RATIO, None));
    }

    #[test]
    fn focus_moves_to_the_nearest_pane_on_screen() {
        let rects = layout(&three_panes());
        assert_eq!(
            neighbor_in_direction(&rects, 1, PaneDirection::Right),
            Some(2)
        );
        assert_eq!(
            neighbor_in_direction(&rects, 2, PaneDirection::Down),
            Some(3)
        );
        assert_eq!(
            neighbor_in_direction(&rects, 3, PaneDirection::Left),
            Some(1)
        );
        assert_eq!(neighbor_in_direction(&rects, 3, PaneDirection::Up), Some(2));
        assert_eq!(neighbor_in_direction(&rects, 1, PaneDirection::Left), None);
        assert_eq!(neighbor_in_direction(&rects, 9, PaneDirection::Left), None);
    }

    #[test]
    fn focus_prefers_a_pane_that_lines_up_over_a_diagonal_one() {
        let grid = SplitPreset::Grid.build(&[1, 2, 3, 4]).unwrap();
        let rects = layout(&grid);
        assert_eq!(
            neighbor_in_direction(&rects, 3, PaneDirection::Right),
            Some(4)
        );
        assert_eq!(
            neighbor_in_direction(&rects, 2, PaneDirection::Down),
            Some(4)
        );
    }

    #[test]
    fn every_preset_places_its_panes_without_overlap() {
        for preset in SplitPreset::ALL {
            let ids: Vec<u64> = (1..=preset.pane_count() as u64).collect();
            let tree = preset.build(&ids).unwrap();
            assert_eq!(tree.leaf_ids(), ids, "{preset:?}");
            let rects = layout(&tree);
            for (i, a) in rects.iter().enumerate() {
                for b in &rects[i + 1..] {
                    let overlaps = a.x < b.x + b.width
                        && b.x < a.x + a.width
                        && a.y < b.y + b.height
                        && b.y < a.y + a.height;
                    assert!(!overlaps, "{preset:?}: {} and {}", a.pane_id, b.pane_id);
                }
            }
            assert!(preset.build(&ids[1..]).is_none());
        }
    }

    #[test]
    fn three_columns_are_equal() {
        let tree = SplitPreset::ThreeColumns.build(&[1, 2, 3]).unwrap();
        let widths: Vec<f32> = layout(&tree).iter().map(|r| r.width).collect();
        assert!(
            widths.windows(2).all(|w| (w[0] - w[1]).abs() <= 1.0),
            "{widths:?}"
        );
    }
}
