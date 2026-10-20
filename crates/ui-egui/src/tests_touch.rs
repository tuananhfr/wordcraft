//! Fingers on the page in a phone-wide window (see `touch`): a drag scrolls without selecting and
//! a quick one flings on (`fling`), a tap places the caret, a long press selects the word with its
//! menu, two fingers zoom, a swipe turns a column in Read Mode. A mouse drag still selects.

use egui::{Event, PointerButton, Pos2, TouchDeviceId, TouchId, TouchPhase, pos2, vec2};

use crate::WordApp;

struct Window {
    ctx: egui::Context,
    app: WordApp,
    time: f64,
}

impl Window {
    fn new() -> Window {
        let text: Vec<String> = (1..=150).map(|n| format!("Paragraph {n}: lorem ipsum dolor sit amet, consectetur adipiscing.")).collect();
        let app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text(&text.join("\n"))), Default::default());
        let mut w = Window { ctx: egui::Context::default(), app, time: 0.0 };
        for _ in 0..5 {
            w.frame(vec![]);
        }
        w
    }

    /// One frame of a 390 × 844 window, through the app's raw input hook as the hosts run it.
    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 30.0;
        let screen = egui::Rect::from_min_size(Pos2::ZERO, vec2(390.0, 844.0));
        let mut input = egui::RawInput { events, time: Some(self.time), screen_rect: Some(screen), ..Default::default() };
        self.app.raw_input_hook(&mut input);
        let app = &mut self.app;
        self.ctx
            .run_ui(input, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            })
            .drop_without_applying_deltas();
    }

    /// A point inside the first word on the first page, `dy` points below its first line.
    fn on_text(&self, dy: f32) -> Pos2 {
        let page = self.app.canvas.page_rects[0];
        let scale = self.app.canvas.scale;
        pos2(page.min.x + 82.0 * scale, page.min.y + 72.0 * scale + 8.0 + dy)
    }
}

fn touch(id: u64, at: Pos2, phase: TouchPhase) -> Event {
    Event::Touch { device_id: TouchDeviceId(0), id: TouchId(id), phase, pos: at, force: None }
}

fn button(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() }
}

/// A finger press as a browser reports it: `pointerdown` in a frame of its own (eframe runs one
/// inside the handler), then `touchstart` with its own copy of the press.
fn web_press(w: &mut Window, at: Pos2) {
    w.frame(vec![button(at, true)]);
    w.frame(vec![button(at, true), touch(1, at, TouchPhase::Start)]);
}

fn web_release(w: &mut Window, at: Pos2) {
    w.frame(vec![button(at, false)]);
    w.frame(vec![button(at, false), Event::PointerGone, touch(1, at, TouchPhase::End)]);
}

#[test]
fn a_finger_drag_scrolls_the_page_and_keeps_the_selection() {
    let mut w = Window::new();
    assert!(w.app.narrow.is_narrow());
    let before = w.app.session.sel.clone();
    let offset = w.app.canvas.scroll_offset;
    let from = w.on_text(300.0);
    web_press(&mut w, from);
    for k in 1..=10 {
        let p = from - vec2(0.0, 25.0 * k as f32);
        w.frame(vec![Event::PointerMoved(p), touch(1, p, TouchPhase::Move)]);
    }
    let moved = w.app.canvas.scroll_offset.y - offset.y;
    assert!((moved - 250.0).abs() < 2.0, "the page followed the finger: {moved}");
    web_release(&mut w, from - vec2(0.0, 250.0));
    w.frame(vec![]);
    assert_eq!(w.app.session.sel, before, "nothing selected, the caret where it was");
    assert!(!w.app.canvas.drag_selecting());
}

#[test]
fn a_mouse_drag_still_selects_and_does_not_scroll() {
    let mut w = Window::new();
    let offset = w.app.canvas.scroll_offset;
    let from = w.on_text(0.0);
    w.frame(vec![Event::PointerMoved(from)]);
    w.frame(vec![button(from, true)]);
    for k in 1..=6 {
        w.frame(vec![Event::PointerMoved(from + vec2(0.0, 20.0 * k as f32))]);
    }
    w.frame(vec![button(from + vec2(0.0, 120.0), false)]);
    w.frame(vec![]);
    assert!(!w.app.session.sel.is_collapsed(), "the drag selected text");
    assert_eq!(w.app.canvas.scroll_offset, offset, "and didn't scroll");
}

#[test]
fn a_tap_places_the_caret_without_selecting_a_word() {
    let mut w = Window::new();
    let at = w.on_text(0.0);
    web_press(&mut w, at);
    web_release(&mut w, at);
    w.frame(vec![]);
    let sel = &w.app.session.sel;
    assert!(sel.is_collapsed(), "a tap is one click, not a double-click: {sel:?}");
    assert!(sel.focus.off > 0, "the caret moved to the tap: {sel:?}");
    // Two taps in a row still select the word, as a double-click does.
    web_press(&mut w, at);
    web_release(&mut w, at);
    w.frame(vec![]);
    assert!(!w.app.session.sel.is_collapsed(), "a double tap selects the word");
}

#[test]
fn a_long_press_selects_the_word_and_opens_the_context_menu() {
    let mut w = Window::new();
    let at = w.on_text(0.0);
    web_press(&mut w, at);
    for _ in 0..36 {
        w.frame(vec![]);
    }
    assert!(w.app.canvas.context_menu_open, "the menu is open after a long press");
    let word = w.app.session.selected_text();
    assert_eq!(word.trim_end(), "Paragraph", "the word (and its space, as Word selects it) is selected");
    // Moving the finger afterwards neither pans nor changes the selection.
    let offset = w.app.canvas.scroll_offset;
    let p = at - vec2(0.0, 80.0);
    w.frame(vec![Event::PointerMoved(p), touch(1, p, TouchPhase::Move)]);
    web_release(&mut w, p);
    w.frame(vec![]);
    assert_eq!(w.app.session.selected_text(), word);
    assert_eq!(w.app.canvas.scroll_offset, offset);
    assert!(w.app.canvas.context_menu_open, "lifting the finger keeps the menu");
}

#[test]
fn two_fingers_still_zoom() {
    let mut w = Window::new();
    let scale = w.app.canvas.scale;
    let (a, b) = (pos2(150.0, 400.0), pos2(250.0, 400.0));
    web_press(&mut w, a);
    w.frame(vec![touch(2, b, TouchPhase::Start)]);
    for k in 1..=8 {
        let d = vec2(10.0 * k as f32, 0.0);
        w.frame(vec![Event::PointerMoved(a - d), touch(1, a - d, TouchPhase::Move), touch(2, b + d, TouchPhase::Move)]);
    }
    w.frame(vec![touch(2, b + vec2(80.0, 0.0), TouchPhase::End)]);
    web_release(&mut w, a - vec2(80.0, 0.0));
    w.frame(vec![]);
    let pinched = w.app.canvas.scale;
    assert!(pinched > scale * 1.3, "spreading two fingers zoomed in: {scale} -> {pinched}");
    for _ in 0..5 {
        w.frame(vec![]);
    }
    assert_eq!(w.app.canvas.scale, pinched, "the pinched zoom holds; the phone's Page Width doesn't come back");
}

/// A one-finger drag up by `step` points a frame for `frames` frames, then lifted.
fn drag_up(w: &mut Window, from: Pos2, step: f32, frames: usize) {
    web_press(w, from);
    for k in 1..=frames {
        let p = from - vec2(0.0, step * k as f32);
        w.frame(vec![Event::PointerMoved(p), touch(1, p, TouchPhase::Move)]);
    }
    w.frame(vec![button(from - vec2(0.0, step * frames as f32), false)]);
}

#[test]
fn a_quick_flick_keeps_scrolling_after_the_finger_lifts_and_slows_to_a_stop() {
    let mut w = Window::new();
    let from = w.on_text(400.0);
    drag_up(&mut w, from, 30.0, 6);
    assert!(w.app.canvas.fling.is_active(), "a quick release flings");
    let mut offset = w.app.canvas.scroll_offset.y;
    w.frame(vec![Event::PointerGone, touch(1, pos2(200.0, 300.0), TouchPhase::End)]);
    let (mut last, mut frames) = (f32::INFINITY, 0);
    let start = offset;
    while w.app.canvas.fling.is_active() && frames < 200 {
        w.frame(vec![]);
        let moved = w.app.canvas.scroll_offset.y - offset;
        assert!(moved >= 0.0 && moved <= last + 0.5, "it keeps going the same way, slower: {moved} after {last}");
        (last, offset, frames) = (moved, w.app.canvas.scroll_offset.y, frames + 1);
    }
    assert!(frames < 200, "the fling stops on its own");
    assert!(offset - start > 100.0, "the page went on past the finger: {}", offset - start);
    w.frame(vec![]);
    assert_eq!(w.app.canvas.scroll_offset.y, offset, "and stays where it stopped");
}

#[test]
fn a_slow_drag_does_not_fling() {
    let mut w = Window::new();
    let from = w.on_text(400.0);
    drag_up(&mut w, from, 2.0, 30);
    assert!(!w.app.canvas.fling.is_active());
    let offset = w.app.canvas.scroll_offset;
    w.frame(vec![Event::PointerGone, touch(1, pos2(200.0, 300.0), TouchPhase::End)]);
    for _ in 0..5 {
        w.frame(vec![]);
    }
    assert_eq!(w.app.canvas.scroll_offset, offset);
}

#[test]
fn a_new_touch_stops_the_fling() {
    let mut w = Window::new();
    let from = w.on_text(400.0);
    drag_up(&mut w, from, 30.0, 6);
    w.frame(vec![Event::PointerGone, touch(1, pos2(200.0, 300.0), TouchPhase::End)]);
    w.frame(vec![]);
    assert!(w.app.canvas.fling.is_active());
    web_press(&mut w, from);
    assert!(!w.app.canvas.fling.is_active(), "the finger caught the page");
    let offset = w.app.canvas.scroll_offset;
    for _ in 0..5 {
        w.frame(vec![]);
    }
    assert_eq!(w.app.canvas.scroll_offset, offset);
}

#[test]
fn a_fling_stops_at_the_top_of_the_document() {
    let mut w = Window::new();
    let from = w.on_text(100.0);
    drag_up(&mut w, from, 30.0, 6);
    w.frame(vec![Event::PointerGone, touch(1, pos2(200.0, 300.0), TouchPhase::End)]);
    // Back down hard: the page lands on its top edge and the fling ends there.
    web_press(&mut w, from);
    for k in 1..=4 {
        let p = from + vec2(0.0, 60.0 * k as f32);
        w.frame(vec![Event::PointerMoved(p), touch(1, p, TouchPhase::Move)]);
    }
    w.frame(vec![button(from + vec2(0.0, 240.0), false)]);
    assert!(w.app.canvas.fling.is_active());
    for _ in 0..30 {
        w.frame(vec![]);
    }
    assert_eq!(w.app.canvas.scroll_offset.y, 0.0, "at the top");
    assert!(!w.app.canvas.fling.is_active(), "stopped there");
}

#[test]
fn read_mode_swipes_turn_a_column_without_flinging() {
    let mut w = Window::new();
    w.app.session.run("view.readMode", &serde_json::json!({})).unwrap();
    for _ in 0..10 {
        w.frame(vec![]);
    }
    assert!(w.app.canvas.cols > 1, "the document reads over several columns");
    let at = pos2(300.0, 400.0);
    web_press(&mut w, at);
    for k in 1..=4 {
        let p = at - vec2(40.0 * k as f32, 0.0);
        w.frame(vec![Event::PointerMoved(p), touch(1, p, TouchPhase::Move)]);
    }
    web_release(&mut w, at - vec2(160.0, 0.0));
    assert!(!w.app.canvas.fling.is_active(), "Read Mode turns columns, it doesn't fling");
    for _ in 0..30 {
        w.frame(vec![]);
    }
    let step = w.app.session.view.read_width * w.app.canvas.scale + crate::canvas::GAP;
    assert!((w.app.canvas.scroll_offset.x - step).abs() < 1.0, "one column on: {} of {step}", w.app.canvas.scroll_offset.x);
}
