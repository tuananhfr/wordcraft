//! The fling that keeps a one-finger page scroll going after the finger lifts, as on GridCraft's
//! sheet: it starts from the release velocity, slows down, and stops at the content's edge.

use egui::Vec2;

/// Fling speeds, in points per second.
const FLING_MIN_SPEED: f32 = 250.0;
const FLING_STOP_SPEED: f32 = 20.0;
/// Fraction of the fling speed lost per second.
const FLING_DECAY: f32 = 4.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fling {
    /// How fast the content still moves (screen points per second, positive = down/right).
    velocity: Option<Vec2>,
}

impl Fling {
    /// Starts a fling from a pan's release velocity, if it was quick enough.
    pub fn release(&mut self, velocity: Vec2) {
        self.velocity = (velocity.is_finite() && velocity.length() >= FLING_MIN_SPEED).then_some(velocity);
    }

    pub fn stop(&mut self) {
        self.velocity = None;
    }

    pub fn is_active(&self) -> bool {
        self.velocity.is_some()
    }

    /// The distance the content moves in `dt` seconds from scroll `offset` (of `max`), in the sense
    /// of [`crate::touch::Pan::delta`], slowing the fling down; zero once stopped.
    pub fn step(&mut self, dt: f32, offset: Vec2, max: Vec2) -> Vec2 {
        let Some(v) = self.velocity else { return Vec2::ZERO };
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
        let max = if max.is_finite() { max.max(Vec2::ZERO) } else { Vec2::ZERO };
        let to = (offset - v * dt).clamp(Vec2::ZERO, max);
        let mut next = v * (1.0 - FLING_DECAY * dt).max(0.0);
        // An axis that reached its edge stops there rather than pushing into it for the rest.
        if at_edge(v.x, to.x, max.x) {
            next.x = 0.0;
        }
        if at_edge(v.y, to.y, max.y) {
            next.y = 0.0;
        }
        self.velocity = (next.length() >= FLING_STOP_SPEED).then_some(next);
        offset - to
    }
}

/// Moving content down/right lowers the scroll offset toward zero; up/left raises it to `max`.
fn at_edge(v: f32, offset: f32, max: f32) -> bool {
    (v > 0.0 && offset <= 0.0) || (v < 0.0 && offset >= max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    const FRAME: f32 = 1.0 / 60.0;
    const FAR: Vec2 = vec2(0.0, 100_000.0);

    #[test]
    fn a_quick_release_keeps_scrolling_and_slows_to_a_stop() {
        let mut f = Fling::default();
        // A flick up: the content moves up, the offset grows.
        f.release(vec2(0.0, -1200.0));
        assert!(f.is_active());
        let mut offset = vec2(0.0, 500.0);
        let first = f.step(FRAME, offset, FAR);
        assert!(first.y < 0.0, "the content keeps moving up: {first:?}");
        offset -= first;
        let (mut last, mut steps, mut travelled) = (first.y.abs(), 0, first.y.abs());
        while f.is_active() && steps < 1000 {
            let d = f.step(FRAME, offset, FAR);
            assert!(d.y.abs() <= last + 1e-3, "it only slows down");
            (last, travelled) = (d.y.abs(), travelled + d.y.abs());
            offset -= d;
            steps += 1;
        }
        assert!(!f.is_active() && steps < 1000, "it stops on its own");
        assert!((200.0..400.0).contains(&travelled), "about speed / decay: {travelled}");
        assert_eq!(f.step(FRAME, offset, FAR), Vec2::ZERO);
    }

    #[test]
    fn a_slow_release_does_not_fling() {
        let mut f = Fling::default();
        f.release(vec2(0.0, 200.0));
        assert!(!f.is_active());
        f.release(vec2(f32::NAN, 900.0));
        assert!(!f.is_active(), "a broken velocity doesn't fling");
        f.release(vec2(0.0, 900.0));
        f.stop();
        assert!(!f.is_active());
        assert_eq!(f.step(FRAME, vec2(0.0, 300.0), FAR), Vec2::ZERO);
    }

    #[test]
    fn it_stops_at_the_content_edge() {
        let mut f = Fling::default();
        // A flick down near the top: the offset reaches zero and the fling ends there.
        f.release(vec2(0.0, 3000.0));
        let mut offset = vec2(0.0, 30.0);
        let mut steps = 0;
        while f.is_active() && steps < 100 {
            offset -= f.step(FRAME, offset, FAR);
            assert!(offset.y >= 0.0);
            steps += 1;
        }
        assert_eq!(offset.y, 0.0);
        assert!(steps < 5, "stopped at the top, not after the whole fling: {steps}");
        // At the bottom, flicking further up does nothing; a diagonal flick keeps its free axis.
        f.release(vec2(-800.0, -800.0));
        let d = f.step(FRAME, vec2(100.0, 400.0), vec2(1000.0, 400.0));
        assert_eq!(d.y, 0.0);
        assert!(d.x < 0.0);
        assert!(f.is_active(), "still moving sideways");
        assert_eq!(f.step(FRAME, vec2(100.0, 400.0), vec2(1000.0, 400.0)).y, 0.0);
    }
}
