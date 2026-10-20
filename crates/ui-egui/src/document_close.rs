use egui::{Rect, Sense, Ui, vec2};
use serde_json::json;

use crate::WordApp;
use crate::theme::Tokens;

pub(crate) fn show(app: &mut WordApp, ui: &mut Ui, height: f32) {
    let enabled = app.dialog.is_none() && app.colors.is_none() && !app.file_dialog_open();
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(height, height), sense);
    let t = Tokens::get(ui.ctx());
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.hover);
    }
    crate::icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), "close", t.icon, t.accent);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tl!("Close document")));
    if response.on_hover_text(tl!("Close document")).clicked() && enabled {
        let _ = app.run("file.close", json!({}));
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn phone(dirty: bool) -> egui_kittest::Harness<'static, WordApp> {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        app.ui.language = "vi".into();
        if dirty {
            app.run("text.insert", json!({"text": "Draft"})).unwrap();
        }
        let mut h = egui_kittest::Harness::builder().with_size(vec2(390.0, 844.0)).build_ui_state(
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
    fn close_button_is_reachable_at_390_and_closes_a_clean_document() {
        let mut h = phone(false);
        let button = h.get_by_label("Đóng tài liệu");
        let rect = button.rect();
        assert!(rect.left() >= 0.0 && rect.right() <= 390.0 && rect.top() >= 0.0 && rect.bottom() <= 844.0, "{rect:?}");
        button.click();
        h.run_steps(4);
        assert!(h.state().quit_requested);
        assert!(h.state().dialog.is_none());
    }

    #[test]
    fn close_button_uses_the_save_changes_flow_at_390() {
        let mut h = phone(true);
        h.get_by_label("Đóng tài liệu").click();
        h.run_steps(4);
        assert!(matches!(h.state().dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })));
        assert!(!h.state().quit_requested);
        h.state_mut().run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
        h.run_steps(4);
        assert!(!h.state().quit_requested);
        assert!(h.state().session.dirty);
        h.get_by_label("Đóng tài liệu").click();
        h.run_steps(4);
        h.state_mut().run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert!(h.state().quit_requested);
    }
}
