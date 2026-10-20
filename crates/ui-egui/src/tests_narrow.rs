//! The page in a phone-wide window (see `narrow`): crossing below the breakpoint fits the page
//! width and hides the rulers once; what the user then picks stands while narrow; widening gives
//! every document its wide zoom and rulers back. Tablets and desktops are left alone.

use egui::{Pos2, vec2};
use serde_json::json;

use crate::WordApp;
use crate::canvas::PX_PER_PT;

const PHONE: f32 = 390.0;
const TABLET: f32 = 768.0;
const DESKTOP: f32 = 1366.0;

struct Window {
    ctx: egui::Context,
    app: WordApp,
    time: f64,
}

impl Window {
    fn new(width: f32) -> Window {
        let text: Vec<String> = (1..=40).map(|n| format!("Paragraph {n}: lorem ipsum dolor sit amet, consectetur adipiscing.")).collect();
        let app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text(&text.join("\n"))), Default::default());
        let mut w = Window { ctx: egui::Context::default(), app, time: 0.0 };
        w.resize(width);
        w
    }

    /// A few frames at `width` (844 or 768 tall), so the fit and the canvas settle.
    fn resize(&mut self, width: f32) {
        let height = if width < 1000.0 { 844.0 } else { 768.0 };
        for _ in 0..5 {
            self.time += 1.0 / 30.0;
            let screen = egui::Rect::from_min_size(Pos2::ZERO, vec2(width, height));
            let mut input = egui::RawInput { time: Some(self.time), screen_rect: Some(screen), ..Default::default() };
            self.app.raw_input_hook(&mut input);
            let app = &mut self.app;
            self.ctx
                .run_ui(input, |ui| {
                    app.logic(ui.ctx());
                    app.ui(ui);
                })
                .drop_without_applying_deltas();
        }
    }

    fn shown_zoom(&self) -> f32 {
        self.app.canvas.scale / PX_PER_PT
    }

    fn view(&self) -> (f32, String, bool) {
        let v = &self.app.session.view;
        ((v.zoom * 100.0).round(), v.fit.clone(), v.ruler)
    }

    fn page_fits_across(&self) -> bool {
        let (page, area) = (self.app.canvas.page_rects[0], self.app.canvas.canvas_rect.unwrap());
        page.left() >= area.left() && page.right() <= area.right()
    }
}

#[test]
fn a_phone_window_fits_the_page_width_without_rulers_and_widening_gives_the_view_back() {
    let mut w = Window::new(DESKTOP);
    w.app.run("view.zoom", json!({"value": 125})).unwrap();
    w.resize(DESKTOP);
    assert_eq!(w.view(), (125.0, String::new(), true));
    let ui_before = serde_json::to_value(&w.app.ui).unwrap();

    w.resize(PHONE);
    assert_eq!(w.app.session.view.fit, "pageWidth", "View › Page Width");
    assert!(!w.app.session.view.ruler, "both rulers hidden");
    assert!(w.page_fits_across(), "no line is cut at the right edge");
    assert!(w.shown_zoom() < 0.6, "a letter page fits a phone well below 100%: {}", w.shown_zoom());
    assert_eq!(serde_json::to_value(&w.app.ui).unwrap(), ui_before, "nothing persisted changes");

    w.resize(DESKTOP);
    assert_eq!(w.view(), (125.0, String::new(), true), "the wide zoom and ruler are back");
    assert!((w.shown_zoom() - 1.25).abs() < 1e-3);
}

#[test]
fn a_zoom_or_ruler_picked_while_narrow_stands_until_the_window_widens() {
    let mut w = Window::new(DESKTOP);
    w.app.run("view.ruler", json!({"value": false})).unwrap();
    w.resize(PHONE);
    let fitted = w.shown_zoom();
    w.app.run("view.zoomIn", json!({})).unwrap();
    w.app.run("view.ruler", json!({"value": true})).unwrap();
    w.resize(PHONE);
    let zoomed = w.shown_zoom();
    assert!(zoomed > fitted, "Zoom In zoomed: {fitted} -> {zoomed}");
    for width in [420.0, 599.0, PHONE] {
        w.resize(width);
        assert!((w.shown_zoom() - zoomed).abs() < 1e-3, "the user's zoom holds at {width}");
        assert!(w.app.session.view.ruler, "the ruler the user turned on stays at {width}");
    }
    w.resize(DESKTOP);
    assert_eq!(w.view(), (100.0, String::new(), false), "the wide 100% and the wide ruler (off)");
    // The next crossing fits again, whatever was picked last time.
    w.resize(PHONE);
    assert_eq!(w.app.session.view.fit, "pageWidth");
    assert!(!w.app.session.view.ruler);
}

#[test]
fn a_page_width_chosen_while_wide_comes_back_as_page_width() {
    let mut w = Window::new(DESKTOP);
    w.app.run("view.onePage", json!({})).unwrap();
    w.resize(DESKTOP);
    let one_page = w.shown_zoom();
    w.resize(PHONE);
    assert_eq!(w.app.session.view.fit, "pageWidth");
    w.resize(DESKTOP);
    assert_eq!(w.app.session.view.fit, "onePage", "the wide fit mode is back, not a frozen percent");
    assert!((w.shown_zoom() - one_page).abs() < 1e-3);
}

#[test]
fn every_open_document_gets_its_own_wide_view_back() {
    let mut w = Window::new(DESKTOP);
    let a = w.app.session.document_id();
    w.app.run("view.zoom", json!({"value": 150})).unwrap();
    w.app.run("file.new", json!({})).unwrap();
    let b = w.app.session.document_id();
    w.app.run("view.zoom", json!({"value": 80})).unwrap();
    w.app.run("ui.activateDocument", json!({"document": a})).unwrap();
    w.resize(PHONE);
    assert_eq!(w.app.session.view.fit, "pageWidth");
    // Created while narrow: a copy of the shown view, fitted already; its wide view is A's.
    w.app.run("file.new", json!({})).unwrap();
    let c = w.app.session.document_id();
    w.resize(PHONE);
    assert_eq!(w.app.session.view.fit, "pageWidth");
    w.app.run("ui.activateDocument", json!({"document": b})).unwrap();
    w.resize(PHONE);
    assert_eq!(w.app.session.view.fit, "pageWidth", "a parked document is fitted when it shows");
    assert!(w.page_fits_across());

    w.resize(DESKTOP);
    assert_eq!(w.view(), (80.0, String::new(), true), "B's own 80%");
    for (document, zoom) in [(a, 150.0), (c, 150.0)] {
        w.app.run("ui.activateDocument", json!({"document": document})).unwrap();
        w.resize(DESKTOP);
        assert_eq!(w.view(), (zoom, String::new(), true), "the parked document {document} got its wide view back");
    }
}

#[test]
fn a_tablet_or_desktop_window_keeps_the_zoom_and_rulers() {
    for width in [TABLET, DESKTOP] {
        let w = Window::new(width);
        assert_eq!(w.view(), (100.0, String::new(), true), "{width}");
        assert!((w.shown_zoom() - 1.0).abs() < 1e-3, "{width}");
    }
}
