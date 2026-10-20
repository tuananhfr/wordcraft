//! Fingers on the page in a phone-wide window (see `touch`): a drag scrolls without selecting,
//! a tap places the caret, a long press selects the word with its menu, two fingers zoom. A mouse
//! drag still selects.

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
    let to = from - vec2(0.0, 250.0);
    web_release(&mut w, to);
    w.frame(vec![]);
    let moved = w.app.canvas.scroll_offset.y - offset.y;
    assert!((moved - 250.0).abs() < 2.0, "the page followed the finger: {moved}");
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
