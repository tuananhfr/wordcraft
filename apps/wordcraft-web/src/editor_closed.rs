pub(crate) fn should_notify(closed: bool, remaining_documents: usize) -> bool {
    closed && remaining_documents == 0
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn after_frame(app: &mut wordcraft_ui_egui::WordApp) {
    use wasm_bindgen::{JsCast as _, JsValue};

    let closed = app.finish_document_close();
    let remaining = if app.has_document() { app.document_count() } else { 0 };
    if !should_notify(closed, remaining) {
        return;
    }
    let Some(window) = web_sys::window() else { return };
    let Ok(value) = js_sys::Reflect::get(&window, &JsValue::from_str("erpEditorClosed")) else { return };
    let Ok(callback) = value.dyn_into::<js_sys::Function>() else { return };
    if let Err(error) = callback.call0(&window) {
        log::warn!("erpEditorClosed failed: {error:?}");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wordcraft_ui_egui::WordApp;

    use super::should_notify;

    fn poll(app: &mut WordApp) -> usize {
        let closed = app.finish_document_close();
        let remaining = if app.has_document() { app.document_count() } else { 0 };
        usize::from(should_notify(closed, remaining))
    }

    #[test]
    fn closed_hook_only_notifies_on_a_completed_last_document_close() {
        let mut calls = 0;
        for (closed, remaining) in [(false, 0), (false, 2), (true, 1), (true, 0), (false, 0), (false, 0), (false, 1), (true, 0)] {
            if should_notify(closed, remaining) {
                calls += 1;
            }
        }
        assert_eq!(calls, 2);
    }

    #[test]
    fn last_document_close_notifies_once_and_can_notify_again_after_reopening() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        app.start_screen();
        assert_eq!(poll(&mut app), 0, "the initial empty editor is not a close");
        app.run("file.new", json!({})).unwrap();
        app.run("text.insert", json!({"text": "Draft"})).unwrap();
        app.run("file.new", json!({})).unwrap();
        assert_eq!(app.document_count(), 2);
        app.run("file.close", json!({})).unwrap();
        assert_eq!(poll(&mut app), 0, "another document remains");
        assert!(app.has_document());
        app.run("file.close", json!({})).unwrap();
        assert_eq!(poll(&mut app), 0, "save question is still pending");
        app.run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
        assert_eq!(poll(&mut app), 0, "cancel keeps the document");
        app.run("file.close", json!({})).unwrap();
        app.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(poll(&mut app), 1);
        assert!(!app.has_document());
        assert!(!app.any_document_dirty(), "the unload guard no longer sees the discarded draft");
        assert_eq!(poll(&mut app), 0);
        assert_eq!(poll(&mut app), 0);
        app.run("file.new", json!({})).unwrap();
        assert!(app.has_document());
        app.run("file.close", json!({})).unwrap();
        assert_eq!(poll(&mut app), 1);
        assert_eq!(poll(&mut app), 0);
    }

    #[test]
    fn close_all_notifies_once_after_the_last_document_is_closed() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        for _ in 0..3 {
            app.run("text.insert", json!({"text": "Draft"})).unwrap();
            app.run("file.new", json!({})).unwrap();
        }
        app.run("ui.closeAll", json!({})).unwrap();
        assert_eq!(poll(&mut app), 0);
        app.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(poll(&mut app), 1);
        assert!(!app.has_document());
        assert_eq!(poll(&mut app), 0);
    }

    #[test]
    fn close_all_clean_documents_reaches_the_same_start_state_as_last_close() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        let mut documents = Vec::new();
        for text in ["A", "B", "C"] {
            app.run("text.insert", json!({"text": text})).unwrap();
            app.session.dirty = false;
            documents.push(app.session.document_id());
            if text != "C" {
                app.run("file.new", json!({})).unwrap();
            }
        }
        assert_eq!(app.document_count(), 3);
        app.run("ui.closeAll", json!({})).unwrap();
        assert_eq!(poll(&mut app), 1);
        assert!(!app.has_document());
        assert!(!app.any_document_dirty());
        assert!(app.ui.backstage);
        assert_eq!(app.ui.backstage_page, "home");
        assert!(documents.iter().all(|id| *id != app.session.document_id()));
        assert_eq!(app.session.doc.plain_text(wordcraft_doc::StoryRef::Body), "");
        assert_eq!(poll(&mut app), 0);
    }

    #[test]
    fn cancelling_close_all_keeps_every_document_on_the_web_path() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        for text in ["A", "B"] {
            app.run("text.insert", json!({"text": text})).unwrap();
            app.run("file.new", json!({})).unwrap();
        }
        let tabs = app.document_tabs();
        app.run("ui.closeAll", json!({})).unwrap();
        app.run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
        assert_eq!(poll(&mut app), 0);
        assert_eq!(app.document_tabs(), tabs);
    }
}
