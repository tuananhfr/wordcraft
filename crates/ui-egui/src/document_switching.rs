use egui::{Key, Modifiers};
use serde_json::{Value, json};

use crate::WordApp;
use crate::keys::ShortcutRow;

pub(crate) fn command_for(app: &WordApp, key: Key, m: Modifiers) -> Option<(&'static str, Value)> {
    if key != Key::Tab || !m.ctrl || m.alt || m.mac_cmd || !app.has_document() {
        return None;
    }
    let reg = &app.session.registry;
    let canonical = crate::keys::combo(m, "Tab", true);
    let explicit = if m.shift { "Ctrl+Shift+Tab" } else { "Ctrl+Tab" };
    if app.session.keymap.resolve(reg, &canonical).is_some() || app.session.keymap.resolve(reg, explicit).is_some() {
        return None;
    }
    let count = app.document_count();
    if count == 0 {
        return None;
    }
    let active = app.active_document_index();
    let index = if m.shift {
        if active == 0 { count.saturating_sub(1) } else { active - 1 }
    } else if active.saturating_add(1) >= count {
        0
    } else {
        active + 1
    };
    Some(("ui.activateDocument", json!({"index": index})))
}

pub(crate) fn rows(app: &WordApp) -> Vec<ShortcutRow> {
    [(false, "Ctrl+Tab", "Next Document"), (true, "Ctrl+Shift+Tab", "Previous Document")]
        .into_iter()
        .filter_map(|(shift, key, label)| {
            let m = Modifiers { ctrl: true, command: !cfg!(target_os = "macos"), shift, ..Modifiers::NONE };
            let (id, params) = command_for(app, Key::Tab, m)?;
            Some(ShortcutRow { key: key.into(), label, id, params })
        })
        .collect()
}

pub(crate) fn handle(app: &mut WordApp, ctx: &egui::Context) {
    if app.dialog.is_some() || app.colors.is_some() || app.file_dialog_open() || app.ui.keytips != crate::keytips::Phase::Off {
        return;
    }
    // Consume before egui gives Tab to focus navigation or the document's editing commands.
    let events = ctx.input(|i| i.events.clone());
    for event in &events {
        if let egui::Event::Key { key, modifiers, pressed: true, .. } = event
            && let Some((id, params)) = command_for(app, *key, *modifiers)
        {
            let _ = app.run(id, params);
            ctx.input_mut(|i| i.events.retain(|e| e != event));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_tab_cycles_three_documents_and_plain_tab_still_types() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        for text in ["A", "B"] {
            app.run("text.insert", json!({"text": text})).unwrap();
            app.run("file.new", json!({})).unwrap();
        }
        app.run("text.insert", json!({"text": "C"})).unwrap();
        app.canvas.want_focus = true;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            app,
        );
        h.run_steps(6);
        for (shift, index, text) in [(false, 0, "A"), (false, 1, "B"), (false, 2, "C"), (true, 1, "B"), (true, 0, "A"), (true, 2, "C")] {
            let modifiers = Modifiers { ctrl: true, command: !cfg!(target_os = "macos"), shift, ..Modifiers::NONE };
            h.event(egui::Event::Key { key: Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers });
            h.run_steps(2);
            assert_eq!(h.state().active_document_index(), index);
            assert_eq!(h.state().session.doc.plain_text(wordcraft_doc::StoryRef::Body), text);
        }
        h.event(egui::Event::Key { key: Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.run_steps(2);
        assert_eq!(h.state().active_document_index(), 2);
        assert_eq!(h.state().session.doc.plain_text(wordcraft_doc::StoryRef::Body), "C\t");
    }

    #[test]
    fn customized_ctrl_tab_is_preserved() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        let key = if cfg!(target_os = "macos") { "Ctrl+Tab" } else { "Mod+Tab" };
        app.run("tools.customizeKeyboard", json!({"assign": {"command": "format.bold", "key": key}})).unwrap();
        let m = Modifiers { ctrl: true, command: !cfg!(target_os = "macos"), ..Modifiers::NONE };
        assert!(command_for(&app, Key::Tab, m).is_none());
        assert!(rows(&app).iter().all(|r| r.key != "Ctrl+Tab"));
    }
}
