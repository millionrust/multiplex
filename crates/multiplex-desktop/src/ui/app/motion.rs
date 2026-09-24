//! Layout motion: panes and canvas nodes glide to their new places instead of
//! jumping, the canvas view flies instead of cutting, and a pane's terminal is
//! resized once the motion has settled rather than on every frame.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::ui::theme;

/// How long a layout change takes to settle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum MotionSpeed {
    /// Switching Split and Canvas, applying a preset, zooming a pane.
    Morph,
    /// Adding, closing, moving, or swapping one pane.
    Quick,
    /// Moving a divider from the keyboard.
    Nudge,
    /// The canvas view flying to fit content or to a node.
    Camera,
    /// The canvas view zooming one step.
    CameraStep,
    /// A drop preview sliding between the zones of a pane.
    DropPreview,
    /// One cycle of a still-connecting status dot.
    StatusPulse,
    /// One cycle of the dot on something that needs the user.
    AttentionPulse,
}

impl MotionSpeed {
    pub(super) fn duration(self) -> Duration {
        // App-level tests check where things land, not how they get there.
        if cfg!(test) {
            return Duration::ZERO;
        }
        let tokens = theme::current_design_tokens();
        theme::motion_duration(match self {
            MotionSpeed::Morph => tokens.motion_layout_morph(false),
            MotionSpeed::Quick => tokens.motion_layout_quick(false),
            MotionSpeed::Nudge => tokens.motion_layout_nudge(false),
            MotionSpeed::Camera => tokens.motion_camera(false),
            MotionSpeed::CameraStep => tokens.motion_camera_step(false),
            MotionSpeed::DropPreview => tokens.motion_drop_preview(false),
            MotionSpeed::StatusPulse => tokens.motion_status_pulse(false),
            MotionSpeed::AttentionPulse => tokens.motion_attention_pulse(false),
        })
    }
}

/// Fade `element` in over `speed`, or show it at once when motion is off.
pub(super) fn fade_in<E>(
    element: E,
    id: impl Into<gpui::ElementId>,
    speed: MotionSpeed,
) -> gpui::AnyElement
where
    E: gpui::IntoElement + gpui::Styled + 'static,
{
    use gpui::{AnimationExt as _, IntoElement as _};
    let duration = speed.duration();
    if duration.is_zero() {
        return element.into_any_element();
    }
    element
        .with_animation(id, gpui::Animation::new(duration), |element, delta| {
            element.opacity(delta)
        })
        .into_any_element()
}

/// Pulse `element` for as long as it is shown, or hold it still when motion is off.
pub(super) fn pulse<E>(
    element: E,
    id: impl Into<gpui::ElementId>,
    speed: MotionSpeed,
) -> gpui::AnyElement
where
    E: gpui::IntoElement + gpui::Styled + 'static,
{
    use gpui::{AnimationExt as _, IntoElement as _};
    let duration = speed.duration();
    if duration.is_zero() {
        return element.into_any_element();
    }
    element
        .with_animation(
            id,
            gpui::Animation::new(duration)
                .repeat()
                .with_easing(gpui::pulsating_between(0.35, 1.0)),
            |element, delta| element.opacity(delta),
        )
        .into_any_element()
}

/// `cubic-bezier(.2, .8, .2, 1)`: a quick start that settles gently, with no
/// overshoot.
pub(super) fn ease_standard(t: f32) -> f32 {
    cubic_bezier(0.2, 0.8, 0.2, 1.0, t)
}

fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let sample = |a: f32, b: f32, s: f32| {
        let inv = 1.0 - s;
        3.0 * inv * inv * s * a + 3.0 * inv * s * s * b + s * s * s
    };
    let slope = |a: f32, b: f32, s: f32| {
        let inv = 1.0 - s;
        3.0 * inv * inv * a + 6.0 * inv * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    // Solve x(s) = t with Newton's method, falling back to bisection where the
    // curve is too flat for Newton to be reliable.
    let mut s = t;
    for _ in 0..8 {
        let error = sample(x1, x2, s) - t;
        if error.abs() < 1e-5 {
            return sample(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - error / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    s = t;
    for _ in 0..32 {
        let x = sample(x1, x2, s);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    sample(y1, y2, s)
}

pub(super) fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// A rectangle in the workspace body's coordinates.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct MotionRect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

impl MotionRect {
    pub(super) fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(super) fn lerp(self, to: MotionRect, t: f32) -> MotionRect {
        MotionRect {
            x: lerp(self.x, to.x, t),
            y: lerp(self.y, to.y, t),
            width: lerp(self.width, to.width, t),
            height: lerp(self.height, to.height, t),
        }
    }
}

/// Where one pane is drawn on this frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct PaneFrame {
    pub(super) pane_id: u64,
    pub(super) rect: MotionRect,
    pub(super) opacity: f32,
    /// The pane is fading out of a layout it no longer belongs to.
    pub(super) leaving: bool,
}

/// One workspace's panes moving from where they were drawn to a new layout.
#[derive(Clone, Debug)]
pub(super) struct LayoutTransition {
    pub(super) workspace_id: u64,
    from: HashMap<u64, MotionRect>,
    started: Instant,
    duration: Duration,
}

impl LayoutTransition {
    /// `from` is where each pane was drawn when the change began, including
    /// any motion that was still under way.
    pub(super) fn new(
        workspace_id: u64,
        from: HashMap<u64, MotionRect>,
        speed: MotionSpeed,
        now: Instant,
    ) -> Self {
        Self {
            workspace_id,
            from,
            started: now,
            duration: speed.duration(),
        }
    }

    pub(super) fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        ease_standard(elapsed / self.duration.as_secs_f32())
    }

    pub(super) fn is_finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.duration
    }

    /// Where one pane or node is drawn on this frame: moving from where it was
    /// if it was drawn before, otherwise fading in at its new place.
    pub(super) fn frame_for(
        &self,
        pane_id: Option<u64>,
        to: MotionRect,
        now: Instant,
    ) -> (MotionRect, f32) {
        let t = self.progress(now);
        match pane_id.and_then(|pane_id| self.from.get(&pane_id)) {
            Some(from) => (from.lerp(to, t), 1.0),
            None => (to, t),
        }
    }

    /// The frame of every pane in `targets`, plus the panes that were drawn
    /// before but have no place now and should fade out where they were.
    /// `still_shown` says whether a departed pane still exists to be drawn.
    pub(super) fn frames(
        &self,
        now: Instant,
        targets: &[(u64, MotionRect)],
        still_shown: impl Fn(u64) -> bool,
    ) -> Vec<PaneFrame> {
        let t = self.progress(now);
        let mut frames: Vec<PaneFrame> = targets
            .iter()
            .map(|(pane_id, to)| match self.from.get(pane_id) {
                Some(from) => PaneFrame {
                    pane_id: *pane_id,
                    rect: from.lerp(*to, t),
                    opacity: 1.0,
                    leaving: false,
                },
                None => PaneFrame {
                    pane_id: *pane_id,
                    rect: *to,
                    opacity: t,
                    leaving: false,
                },
            })
            .collect();
        let mut leaving: Vec<(u64, MotionRect)> = self
            .from
            .iter()
            .filter(|(pane_id, _)| !targets.iter().any(|(target, _)| target == *pane_id))
            .filter(|(pane_id, _)| still_shown(**pane_id))
            .map(|(pane_id, rect)| (*pane_id, *rect))
            .collect();
        leaving.sort_by_key(|(pane_id, _)| *pane_id);
        // Departing panes are drawn underneath the ones that stay.
        frames.splice(
            0..0,
            leaving.into_iter().map(|(pane_id, rect)| PaneFrame {
                pane_id,
                rect,
                opacity: 1.0 - t,
                leaving: true,
            }),
        );
        frames
    }
}

/// The frames to draw when no transition is running.
pub(super) fn settled_frames(targets: &[(u64, MotionRect)]) -> Vec<PaneFrame> {
    targets
        .iter()
        .map(|(pane_id, rect)| PaneFrame {
            pane_id: *pane_id,
            rect: *rect,
            opacity: 1.0,
            leaving: false,
        })
        .collect()
}

/// A value moving from `from` to `to` over one motion.
#[derive(Clone, Copy, Debug)]
pub(super) struct Tween<T: Copy> {
    pub(super) from: T,
    pub(super) to: T,
    started: Instant,
    duration: Duration,
}

impl<T: Copy> Tween<T> {
    pub(super) fn new(from: T, to: T, speed: MotionSpeed, now: Instant) -> Self {
        Self {
            from,
            to,
            started: now,
            duration: speed.duration(),
        }
    }

    pub(super) fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        ease_standard(elapsed / self.duration.as_secs_f32())
    }

    pub(super) fn is_finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_curve_starts_fast_settles_gently_and_never_overshoots() {
        assert_eq!(ease_standard(0.0), 0.0);
        assert_eq!(ease_standard(1.0), 1.0);
        assert!(ease_standard(0.25) > 0.5, "{}", ease_standard(0.25));
        let mut previous = 0.0;
        for step in 1..=100 {
            let value = ease_standard(step as f32 / 100.0);
            assert!(value >= previous - 1e-4 && value <= 1.0 + 1e-4);
            previous = value;
        }
    }

    /// A transition timed by the real layout token, which tests otherwise zero.
    fn duration() -> Duration {
        theme::motion_duration(theme::current_design_tokens().motion_layout_morph(false))
    }

    fn transition(from: &[(u64, MotionRect)]) -> (LayoutTransition, Instant) {
        let now = Instant::now();
        let transition = LayoutTransition {
            workspace_id: 1,
            from: from.iter().copied().collect(),
            started: now,
            duration: duration(),
        };
        (transition, now)
    }

    #[test]
    fn panes_move_from_where_they_were_drawn_to_their_new_place() {
        let a = MotionRect::new(0.0, 0.0, 100.0, 100.0);
        let b = MotionRect::new(100.0, 0.0, 100.0, 100.0);
        let (transition, start) = transition(&[(1, a)]);

        let first = transition.frames(start, &[(1, b)], |_| true);
        assert_eq!(first[0].rect, a);

        let halfway = transition.frames(start + duration() / 2, &[(1, b)], |_| true);
        assert!(halfway[0].rect.x > 50.0 && halfway[0].rect.x < 100.0);

        let done = start + duration();
        assert!(transition.is_finished(done));
        assert_eq!(transition.frames(done, &[(1, b)], |_| true)[0].rect, b);
    }

    #[test]
    fn new_panes_fade_in_and_departed_panes_fade_out_underneath() {
        let a = MotionRect::new(0.0, 0.0, 100.0, 100.0);
        let (transition, start) = transition(&[(1, a), (2, a)]);
        let frames = transition.frames(start, &[(1, a), (3, a)], |_| true);
        assert_eq!(frames.len(), 3);
        assert!(frames[0].leaving && frames[0].pane_id == 2 && frames[0].opacity == 1.0);
        assert_eq!(frames[2].pane_id, 3);
        assert_eq!(frames[2].opacity, 0.0);

        let end = transition.frames(start + duration(), &[(1, a), (3, a)], |_| true);
        assert_eq!(end[0].opacity, 0.0);
        assert_eq!(end[2].opacity, 1.0);
    }

    #[test]
    fn closed_panes_are_not_drawn_while_the_rest_move() {
        let a = MotionRect::new(0.0, 0.0, 100.0, 100.0);
        let (transition, start) = transition(&[(1, a), (2, a)]);
        let frames = transition.frames(start, &[(1, a)], |pane_id| pane_id != 2);
        assert_eq!(frames.len(), 1);
    }
}
