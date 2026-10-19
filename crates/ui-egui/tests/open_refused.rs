//! A zip bomb opened from disk says why in the interface language and opens nothing.
use std::io::{Cursor, Write};

use serde_json::json;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

fn bomb() -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(&vec![b' '; 64 << 20]).unwrap();
    z.finish().unwrap().into_inner()
}

#[test]
fn a_zip_bomb_shows_the_translated_reason_and_opens_nothing() {
    let dir = std::env::temp_dir().join(format!("wordcraft-bomb-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Bomb.docx");
    std::fs::write(&path, bomb()).unwrap();
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
    let ctx = egui::Context::default();
    // A frame applies the interface language.
    let frame = |app: &mut WordApp| ctx.run_ui(egui::RawInput::default(), |ui| app.logic(ui.ctx())).drop_without_applying_deltas();
    app.run("ui.language", json!({"value": "vi"})).unwrap();
    frame(&mut app);
    app.ui.backstage = true;
    let e = app.run("file.open", json!({"path": path.to_string_lossy()})).unwrap_err();
    assert!(wordcraft_engine::io::too_large(&e), "{e}");
    let status = app.status_msg.as_ref().map(|(m, _)| m.as_str());
    assert_eq!(status, Some("Tệp quá lớn hoặc bị nén bất thường nên không mở được."));
    assert!(!app.ui.backstage, "the status bar saying why is shown");
    assert!(app.session.path.is_none(), "the blank document stays");
    app.run("ui.language", json!({"value": "en"})).unwrap();
    frame(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The web build starts on the Start screen with no document; a dropped file arrives through the
/// inbox. The refusal must still show in the empty window.
#[test]
fn a_zip_bomb_dropped_on_the_start_screen_says_why() {
    use egui_kittest::kittest::Queryable;
    let inbox: wordcraft_ui_egui::Inbox = Default::default();
    let services = Services { inbox: Some(inbox.clone()), ..Services::default() };
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), services);
    app.ui.language = "vi".into();
    assert!(app.start_screen());
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    inbox.lock().unwrap().push(("ratio.docx".into(), bomb()));
    h.run_steps(3);
    assert!(!h.state().has_document(), "nothing opened");
    assert!(!h.state().ui.backstage, "off the Start screen, onto the window whose status bar says why");
    let vi = "Tệp quá lớn hoặc bị nén bất thường nên không mở được.";
    assert!(h.query_by_label(vi).is_some(), "shown");
    // Status messages normally fade after 6 s; in the empty window it is all there is.
    if let Some((_, at)) = h.state_mut().status_msg.as_mut() {
        *at -= 60_000.0;
    }
    h.run_steps(2);
    assert!(h.query_by_label(vi).is_some(), "still shown a minute later");
}
