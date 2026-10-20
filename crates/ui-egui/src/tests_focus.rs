use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::WordApp;

fn harness(width: f32) -> egui_kittest::Harness<'static, WordApp> {
    let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Draft")), Default::default());
    app.ui.language = "en".into();
    app.canvas.want_focus = true;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(width, 844.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

#[test]
fn focus_hides_chrome_and_escape_restores_it_without_interrupting_typing() {
    for width in [390.0, 1366.0] {
        let mut h = harness(width);
        let before = h.state().canvas.canvas_rect.unwrap();
        let ribbon_collapsed = h.state().ribbon_collapsed();
        let ruler = h.state().session.view.ruler;
        assert!(h.query_by_label("Home").is_some());
        assert!(h.query_by_label("Print Layout").is_some(), "status bar view control");
        h.state_mut().run("view.focus", json!({})).unwrap();
        h.run_steps(4);
        assert!(h.state().session.view.focus_mode);
        assert!(h.query_by_label("Home").is_none(), "ribbon hidden at {width}");
        assert!(h.query_by_label("Print Layout").is_none(), "status bar hidden at {width}");
        let focused = h.state().canvas.canvas_rect.unwrap();
        assert!(focused.top() < before.top() && focused.bottom() > before.bottom(), "{before:?} -> {focused:?}");
        assert!(h.state().canvas.focused);
        h.event(egui::Event::Text("x".into()));
        h.run_steps(2);
        assert!(h.state().session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains('x'));
        h.key_press(egui::Key::Escape);
        h.run_steps(4);
        assert!(!h.state().session.view.focus_mode);
        assert!(h.query_by_label("Home").is_some());
        assert!(h.query_by_label("Print Layout").is_some());
        assert_eq!(h.state().ribbon_collapsed(), ribbon_collapsed);
        assert_eq!(h.state().session.view.ruler, ruler);
    }
}

#[test]
fn escape_collapses_selection_before_leaving_focus() {
    let mut h = harness(1366.0);
    h.state_mut().run("view.focus", json!({})).unwrap();
    h.state_mut().run("select.text", json!({"text": "Draft"})).unwrap();
    h.run_steps(4);
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    assert!(h.state().session.sel.is_collapsed());
    assert!(h.state().session.view.focus_mode);
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    assert!(!h.state().session.view.focus_mode);
}

#[test]
fn escape_cancels_the_save_dialog_before_leaving_focus() {
    let mut h = harness(1366.0);
    h.state_mut().run("view.focus", json!({})).unwrap();
    h.state_mut().run("text.insert", json!({"text": "unsaved"})).unwrap();
    h.run_steps(4);
    h.state_mut().run("file.close", json!({})).unwrap();
    h.run_steps(4);
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    assert!(!h.state().quit_requested);
    assert!(h.state().session.view.focus_mode);
    h.state_mut().canvas.want_focus = true;
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    assert!(!h.state().session.view.focus_mode);
}

#[test]
fn focus_preserves_a_custom_escape_binding() {
    let mut h = harness(1366.0);
    h.state_mut().run("tools.customizeKeyboard", json!({"assign": {"command": "format.bold", "key": "Escape"}})).unwrap();
    h.state_mut().run("view.focus", json!({})).unwrap();
    h.run_steps(4);
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    assert!(h.state().session.view.focus_mode);
    assert_eq!(h.state().session.keymap.assigned.get("Escape").map(String::as_str), Some("format.bold"));
    h.event(egui::Event::Text("x".into()));
    h.run_steps(2);
    let paragraph = h.state().session.doc.para_at(&h.state().session.sel.focus).unwrap();
    let offset = paragraph.text.find('x').unwrap();
    assert_eq!(paragraph.props_of_char(offset).bold, Some(true));
}
