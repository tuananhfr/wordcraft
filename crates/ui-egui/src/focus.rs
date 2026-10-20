use egui::{Key, Modifiers};
use serde_json::json;

use crate::WordApp;

#[derive(Default)]
pub(crate) struct State {
    hinted: bool,
    hint_until: Option<f64>,
    escape_blocked: bool,
}

pub(crate) fn begin_frame(app: &mut WordApp, ctx: &egui::Context) {
    // A dialog or popup can close during layout without consuming its Escape event.
    app.focus.escape_blocked = app.ui.backstage
        || app.dialog.is_some()
        || app.colors.is_some()
        || app.file_dialog_open()
        || app.controls.popup.is_some()
        || app.ui.keytips != crate::keytips::Phase::Off
        || egui::Popup::is_any_open(ctx)
        || app.canvas.auto_scroll.active()
        || app.session.view.read_mode
        || app.session.math.is_some()
        || app.session.painter.is_some()
        || app.session.view.draw.mode != wordcraft_engine::cmd::draw::DrawMode::Select
        || wordcraft_engine::cmd::draw::lasso_selection(&app.session).is_some()
        || app.session.view.draw.replay
        || app.canvas.table_tool.is_some()
        || !app.session.sel.is_collapsed();
}

pub(crate) fn changed(app: &mut WordApp, was_focus: bool) {
    if !app.session.view.focus_mode {
        app.focus.hint_until = None;
        return;
    }
    if was_focus {
        return;
    }
    app.ribbon_peek = false;
    app.canvas.want_focus = true;
    if !app.focus.hinted {
        app.focus.hinted = true;
        app.focus.hint_until = Some(crate::now_ms() + 4000.0);
    }
}

pub(crate) fn escape(app: &mut WordApp, ctx: &egui::Context) -> bool {
    if !app.session.view.focus_mode
        || app.focus.escape_blocked
        || app.ui.backstage
        || app.dialog.is_some()
        || app.colors.is_some()
        || app.file_dialog_open()
        || app.controls.popup.is_some()
        || app.canvas.link_from.is_some()
        || egui::Popup::is_any_open(ctx)
        || !app.canvas.ime_preedit.is_empty()
    {
        return false;
    }
    if !ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        return false;
    }
    let _ = app.run("view.focus", json!({"value": false}));
    app.focus.hint_until = None;
    true
}

pub(crate) fn hint(app: &mut WordApp, ctx: &egui::Context) {
    let Some(until) = app.focus.hint_until else { return };
    if !app.has_document() || !app.session.view.focus_mode || crate::now_ms() >= until {
        app.focus.hint_until = None;
        return;
    }
    egui::Area::new(egui::Id::new("focus_hint"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 12.0))
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_max_width((ctx.content_rect().width() - 32.0).max(0.0));
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(tl!("Press Esc to exit Focus mode"));
            });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::WordApp;

    #[test]
    fn focus_hint_is_only_offered_once_per_editor_session() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        app.run("view.focus", json!({"value": false})).unwrap();
        assert!(!app.focus.hinted);
        app.run("view.focus", json!({"value": true})).unwrap();
        assert!(app.focus.hinted && app.focus.hint_until.is_some());
        app.run("view.focus", json!({"value": false})).unwrap();
        app.run("view.focus", json!({"value": true})).unwrap();
        assert!(app.focus.hint_until.is_none());
    }
}
