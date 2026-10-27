//! Notice for opaque DOCX content retained on save.

use crate::WordApp;

pub(crate) fn opened(app: &mut WordApp) {
    let tabs = app.document_tabs();
    app.alt_chunk_notices.retain(|id| tabs.iter().any(|tab| tab.document == *id));
    if app.session.doc.preserved_alt_chunk_count() > 0 {
        app.alt_chunk_notices.insert(app.session.document_id());
    }
}

pub(crate) fn show(app: &mut WordApp, ui: &mut egui::Ui) {
    let document = app.session.document_id();
    if !app.alt_chunk_notices.contains(&document) {
        return;
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut close = false;
    egui::Frame::new().fill(t.panel).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                close = ui.small_button(tl!("Close")).clicked();
                ui.add(
                    egui::Label::new(tl!("This file contains embedded content that cannot be displayed or edited. It is kept when saving as DOCX."))
                        .wrap(),
                );
            });
        });
    });
    if close {
        app.alt_chunk_notices.remove(&document);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use serde_json::json;
    use wordcraft_doc::{Document, Path, StoryRef};

    #[test]
    fn alt_chunk_notice_is_non_blocking_dismissible_and_survives_tab_switches() {
        let mut doc = Document::from_text("Editable text");
        doc.styles.default_chr.font = Some(wordcraft_fonts::FALLBACK_FAMILY.into());
        doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap().preserved_before.push(wordcraft_doc::para::PreservedAltChunk {
            relationship_id: "old".into(),
            relationship_type: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/aFChunk".into(),
            target: "word/chunk.html".into(),
            content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml".into(),
            bytes: std::sync::Arc::new(b"B-CHUNK".to_vec()),
        });
        let mut app = WordApp::new(wordcraft_engine::Session::new(Document::new()), Default::default());
        app.ui.language = "vi".into();
        let bytes = wordcraft_engine::io::save_bytes("chunk.docx", &doc).unwrap();
        app.run("file.open", json!({"path": "chunk.docx", "data": wordcraft_engine::cmd::insert::base64_encode(&bytes)})).unwrap();
        assert!(app.dialog.is_none());
        let document = app.session.document_id();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1366.0, 844.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            app,
        );
        h.run_steps(6);
        h.get_by_label("Tệp này có nội dung nhúng không thể hiển thị hoặc chỉnh sửa. Nội dung đó được giữ nguyên khi lưu dưới dạng DOCX.");
        h.state_mut().run("text.insert", json!({"text": "Edit"})).unwrap();
        assert!(h.state().alt_chunk_notices.contains(&document));
        h.get_by_label("Đóng").click();
        h.run_steps(4);
        h.state_mut().run("file.new", json!({})).unwrap();
        h.state_mut().activate_document(0).unwrap();
        assert!(!h.state().alt_chunk_notices.contains(&document));
    }
}
