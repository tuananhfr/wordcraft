//! Fingers on the page: a one-finger drag scrolls instead of selecting, a tap places the caret
//! (it is handled as the click it already is), a long press selects the word and opens the
//! context menu. Mouse and pen input never pans: a gesture is a touch only while egui reports
//! fingers down ([`egui::InputState::any_touches`]).
//!
//! The press itself is handled like a mouse press, because on the web it arrives before the
//! browser's touch events do (eframe runs a frame inside `pointerdown`, `touchstart` follows).
//! Once a touch drag is under way the selection from before the press comes back.

use egui::{Event, PointerButton, Pos2, RawInput, Vec2};

/// A repeated press (or release) this soon and this close to the previous one is the same touch
/// reported twice: browsers send `pointerdown` and `touchstart` (`pointerup` and `touchend`).
const DUPLICATE_SECS: f64 = 0.5;
const DUPLICATE_DIST: f32 = 16.0;

/// Points a finger must swipe sideways in Read Mode to turn a column.
const SWIPE_TURN: f32 = 48.0;

/// Drops the second copy of a primary press or release, so one tap is one click rather than
/// a press-press-release-release egui would count as a double-click (a word selected on a tap).
#[derive(Clone, Debug, Default)]
pub struct PressDedup {
    down: bool,
    last: Option<(f64, Pos2)>,
}

impl PressDedup {
    pub fn filter(&mut self, raw: &mut RawInput) {
        let now = raw.time.or(self.last.map(|(t, _)| t)).unwrap_or(0.0);
        raw.events.retain(|e| {
            let Event::PointerButton { pos, button: PointerButton::Primary, pressed, .. } = e else { return true };
            let repeat = *pressed == self.down
                && self.last.is_some_and(|(t, at)| (0.0..DUPLICATE_SECS).contains(&(now - t)) && at.distance(*pos) <= DUPLICATE_DIST);
            if repeat {
                return false;
            }
            self.down = *pressed;
            self.last = Some((now, *pos));
            true
        });
    }
}

#[derive(Clone, Debug)]
enum State<S> {
    Idle,
    /// A primary press on the page, with the selection from before it.
    Pressed {
        origin: Pos2,
        before: Option<S>,
    },
    /// A finger drag scrolling the page; `travel` is its whole movement, `pinched` whether a
    /// second finger joined in.
    Panning {
        last: Pos2,
        travel: Vec2,
        pinched: bool,
    },
    /// A long press opened the context menu: the rest of the touch does nothing.
    Held,
}

/// This frame's touch scroll, from [`PageTouch::pan`].
#[derive(Debug, PartialEq)]
pub struct Pan<S> {
    /// How far the content follows the finger (screen points, positive = down/right).
    pub delta: Vec2,
    /// The drag became a pan this frame: put back this selection and drop the drag-select.
    pub started: Option<S>,
}

/// A finger pan let go, from [`PageTouch::end`].
#[derive(Debug, PartialEq)]
pub struct PanEnd {
    /// A sideways swipe: `Some(-1.0)` back, `Some(1.0)` forward (Read Mode turns a column).
    pub swipe: Option<f32>,
    /// How fast the finger moved as it lifted (points per second), for the fling.
    pub velocity: Vec2,
}

/// The page's touch gesture; `S` is the selection type kept from before a press.
#[derive(Clone, Debug)]
pub struct PageTouch<S> {
    state: State<S>,
}

impl<S> Default for PageTouch<S> {
    fn default() -> Self {
        PageTouch { state: State::Idle }
    }
}

impl<S> PageTouch<S> {
    /// Before the page scrolls. `may_pan` is false while a tool (pen, Draw Table) owns the drag.
    pub fn pan(&mut self, i: &egui::InputState, may_pan: bool, max_click_dist: f32) -> Pan<S> {
        let none = Pan { delta: Vec2::ZERO, started: None };
        let Some(p) = i.pointer.latest_pos() else { return none };
        if !i.pointer.primary_down() {
            return none;
        }
        match &mut self.state {
            State::Pressed { origin, before } if may_pan && i.any_touches() && p.distance(*origin) > max_click_dist => {
                let (from, before) = (*origin, before.take());
                self.state = State::Panning { last: p, travel: p - from, pinched: false };
                Pan { delta: p - from, started: before }
            }
            State::Panning { last, travel, pinched } => {
                // Two fingers: follow their middle, so a pinch doesn't jump with the first finger.
                let multi = i.multi_touch();
                *pinched |= multi.is_some();
                let d = multi.map_or(p - *last, |m| m.translation_delta);
                *last = p;
                *travel += d;
                Pan { delta: d, started: None }
            }
            _ => none,
        }
    }

    /// After the page's response, before the mouse handling. `before` is the selection now,
    /// read only on a press. Returns whether the touch owns the pointer (no mouse handling).
    pub fn after(&mut self, i: &egui::InputState, pressed_on_page: bool, long_touched: bool, before: impl FnOnce() -> S) -> bool {
        if long_touched && matches!(self.state, State::Pressed { .. }) {
            self.state = State::Held;
        }
        if pressed_on_page && matches!(self.state, State::Idle) {
            let origin = i.pointer.press_origin().or(i.pointer.latest_pos()).unwrap_or_default();
            self.state = State::Pressed { origin, before: Some(before()) };
        }
        matches!(self.state, State::Panning { .. } | State::Held)
    }

    /// The press is over (call once per frame, last). Returns how a pan that ended this frame
    /// was let go.
    pub fn end(&mut self, i: &egui::InputState) -> Option<PanEnd> {
        if i.pointer.primary_down() || i.pointer.primary_pressed() {
            return None;
        }
        let State::Panning { travel, pinched, .. } = std::mem::replace(&mut self.state, State::Idle) else { return None };
        let swipe = (travel.x.abs() >= SWIPE_TURN && travel.x.abs() > travel.y.abs()).then(|| -travel.x.signum());
        // After a pinch the pointer is one of the two fingers: its speed is the zoom's, not a throw.
        let velocity = if pinched { Vec2::ZERO } else { i.pointer.velocity() };
        Some(PanEnd { swipe, velocity })
    }

    /// A long press or a pan is in progress.
    pub fn is_active(&self) -> bool {
        matches!(self.state, State::Panning { .. } | State::Held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{TouchDeviceId, TouchId, TouchPhase, pos2, vec2};

    fn press(at: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    fn touch(at: Pos2, phase: TouchPhase) -> Event {
        Event::Touch { device_id: TouchDeviceId(0), id: TouchId(1), phase, pos: at, force: None }
    }

    fn filtered(d: &mut PressDedup, time: f64, events: Vec<Event>) -> Vec<Event> {
        let mut raw = RawInput { time: Some(time), events, ..Default::default() };
        d.filter(&mut raw);
        raw.events
    }

    #[test]
    fn a_touch_reported_twice_is_one_press_and_one_release() {
        let at = pos2(100.0, 200.0);
        let mut d = PressDedup::default();
        // Web order: pointerdown (its own frame), then touchstart's copy with the touch.
        assert_eq!(filtered(&mut d, 1.0, vec![press(at, true)]), vec![press(at, true)]);
        assert_eq!(filtered(&mut d, 1.016, vec![press(at, true), touch(at, TouchPhase::Start)]), vec![touch(at, TouchPhase::Start)]);
        assert_eq!(filtered(&mut d, 1.1, vec![press(at, false)]), vec![press(at, false)]);
        let end = vec![press(at, false), Event::PointerGone, touch(at, TouchPhase::End)];
        assert_eq!(filtered(&mut d, 1.11, end), vec![Event::PointerGone, touch(at, TouchPhase::End)]);
    }

    #[test]
    fn mouse_clicks_and_double_clicks_pass_untouched() {
        let at = pos2(10.0, 10.0);
        let mut d = PressDedup::default();
        let clicks = vec![press(at, true), press(at, false), press(at, true), press(at, false)];
        assert_eq!(filtered(&mut d, 2.0, clicks.clone()), clicks, "a double-click in one frame");
        for (k, e) in clicks.iter().enumerate() {
            assert_eq!(filtered(&mut d, 3.0 + k as f64 * 0.05, vec![e.clone()]), vec![e.clone()], "a double-click over frames");
        }
        // A press after a release that never arrived (outside the window) isn't lost for long.
        assert_eq!(filtered(&mut d, 4.0, vec![press(at, true)]), vec![press(at, true)]);
        assert_eq!(filtered(&mut d, 5.0, vec![press(at, true)]), vec![press(at, true)], "half a second later it counts");
        assert_eq!(filtered(&mut d, 5.1, vec![press(pos2(300.0, 10.0), true)]), vec![press(pos2(300.0, 10.0), true)], "elsewhere it counts");
        let secondary = Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Default::default() };
        assert_eq!(filtered(&mut d, 5.2, vec![secondary.clone(), secondary.clone()]), vec![secondary.clone(), secondary], "only the primary button");
    }

    /// One frame through a real egui context, returning what `f` makes of its input.
    fn frame<R>(ctx: &egui::Context, t: f64, events: Vec<Event>, f: impl FnOnce(&egui::InputState) -> R) -> R {
        let (mut f, mut out) = (Some(f), None);
        let screen = egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 800.0));
        ctx.run_ui(RawInput { time: Some(t), screen_rect: Some(screen), events, ..Default::default() }, |ui| {
            if let Some(f) = f.take() {
                out = Some(ui.input(f));
            }
        })
        .drop_without_applying_deltas();
        out.unwrap()
    }

    #[test]
    fn a_finger_drag_pans_and_hands_back_the_selection_from_before_the_press() {
        let ctx = egui::Context::default();
        let mut g = PageTouch::<&str>::default();
        let at = pos2(200.0, 400.0);
        frame(&ctx, 0.0, vec![], |_| ());
        let owns = frame(&ctx, 0.1, vec![press(at, true), touch(at, TouchPhase::Start)], |i| g.after(i, true, false, || "before"));
        assert!(!owns, "the press is the mouse handling's");
        // Under the click distance: nothing yet.
        let p = pos2(200.0, 397.0);
        let pan = frame(&ctx, 0.12, vec![Event::PointerMoved(p), touch(p, TouchPhase::Move)], |i| g.pan(i, true, 6.0));
        assert_eq!(pan, Pan { delta: Vec2::ZERO, started: None });
        let p = pos2(200.0, 380.0);
        let pan = frame(&ctx, 0.14, vec![Event::PointerMoved(p), touch(p, TouchPhase::Move)], |i| g.pan(i, true, 6.0));
        assert_eq!(pan, Pan { delta: vec2(0.0, -20.0), started: Some("before") }, "the content follows the finger from the press");
        assert!(g.is_active());
        let p2 = pos2(195.0, 350.0);
        let pan = frame(&ctx, 0.16, vec![Event::PointerMoved(p2), touch(p2, TouchPhase::Move)], |i| g.pan(i, true, 6.0));
        assert_eq!(pan, Pan { delta: vec2(-5.0, -30.0), started: None });
        let end = frame(&ctx, 0.2, vec![press(p2, false), Event::PointerGone, touch(p2, TouchPhase::End)], |i| {
            assert!(g.after(i, false, false, || "unused"), "the release is the pan's too");
            g.end(i)
        });
        let end = end.expect("the pan ended");
        assert_eq!(end.swipe, None, "a vertical pan is not a swipe");
        assert!(end.velocity.y < -250.0, "thrown upwards: {:?}", end.velocity);
        assert!(!g.is_active());
    }

    #[test]
    fn a_mouse_drag_never_pans_and_a_tool_keeps_a_finger_drag() {
        for (touching, may_pan) in [(false, true), (true, false)] {
            let ctx = egui::Context::default();
            let mut g = PageTouch::<u8>::default();
            let at = pos2(100.0, 100.0);
            let mut start = vec![press(at, true)];
            if touching {
                start.push(touch(at, TouchPhase::Start));
            }
            frame(&ctx, 0.0, vec![Event::PointerMoved(at)], |_| ());
            frame(&ctx, 0.1, start, |i| g.after(i, true, false, || 1));
            for k in 1..10 {
                let p = pos2(100.0 + 10.0 * k as f32, 100.0);
                let mut ev = vec![Event::PointerMoved(p)];
                if touching {
                    ev.push(touch(p, TouchPhase::Move));
                }
                let pan = frame(&ctx, 0.1 + 0.02 * k as f64, ev, |i| g.pan(i, may_pan, 6.0));
                assert_eq!(pan, Pan { delta: Vec2::ZERO, started: None }, "touch {touching}, tool {}", !may_pan);
                assert!(!g.is_active());
            }
        }
    }

    #[test]
    fn a_sideways_swipe_reports_its_direction_and_a_long_press_holds() {
        let ctx = egui::Context::default();
        let mut g = PageTouch::<u8>::default();
        let at = pos2(300.0, 300.0);
        frame(&ctx, 0.0, vec![], |_| ());
        frame(&ctx, 0.1, vec![press(at, true), touch(at, TouchPhase::Start)], |i| g.after(i, true, false, || 0));
        let p = pos2(200.0, 310.0);
        frame(&ctx, 0.15, vec![Event::PointerMoved(p), touch(p, TouchPhase::Move)], |i| g.pan(i, true, 6.0));
        let end = frame(&ctx, 0.2, vec![press(p, false), touch(p, TouchPhase::End)], |i| g.end(i));
        assert_eq!(end.and_then(|e| e.swipe), Some(1.0), "a swipe to the left goes forward");

        let held = frame(&ctx, 1.0, vec![press(at, true), touch(at, TouchPhase::Start)], |i| g.after(i, true, false, || 0));
        assert!(!held);
        assert!(frame(&ctx, 2.0, vec![], |i| g.after(i, false, true, || 0)), "a long press owns the rest of the touch");
        let p = pos2(300.0, 200.0);
        let pan = frame(&ctx, 2.1, vec![Event::PointerMoved(p), touch(p, TouchPhase::Move)], |i| g.pan(i, true, 6.0));
        assert_eq!(pan.delta, Vec2::ZERO, "moving after a long press doesn't pan");
        assert_eq!(frame(&ctx, 2.2, vec![press(p, false), touch(p, TouchPhase::End)], |i| g.end(i)), None);
        assert!(!g.is_active());
    }
}
