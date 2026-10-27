use std::cell::RefCell;
use std::rc::Rc;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use wordcraft_doc::{Document, StoryRef};
use wordcraft_engine::Session;

use crate::dialogs::Dialog;
use crate::{Download, Services, WordApp};

type Files = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

/// A web app whose share service, when there is one, takes the copy (`true`) or declines it.
fn app(share: Option<bool>) -> (WordApp, Files, Files) {
    let (shared, downloaded) = (Files::default(), Files::default());
    let (to_share, to_download) = (shared.clone(), downloaded.clone());
    let services = Services {
        download: Some(Box::new(move |name: &str, bytes: &[u8]| {
            to_download.borrow_mut().push((name.to_string(), bytes.to_vec()));
            Ok(Download::Done)
        })),
        share: share.map(|taken| {
            Box::new(move |name: &str, bytes: &[u8]| {
                to_share.borrow_mut().push((name.to_string(), bytes.to_vec()));
                taken
            }) as Box<dyn Fn(&str, &[u8]) -> bool>
        }),
        ..Default::default()
    };
    let mut app = WordApp::new(Session::new(Document::from_text("Biên bản họp")), services);
    app.session.dirty = true;
    (app, shared, downloaded)
}

fn harness(app: WordApp) -> Harness<'static, WordApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn click(h: &mut Harness<'static, WordApp>, role: Role, label: &str) {
    h.get_by_role_and_label(role, label).click();
    h.run_steps(4);
}

fn assert_reopens(file: &(String, Vec<u8>)) {
    let (name, bytes) = file;
    assert!(name.ends_with(".docx"), "{name}");
    let back = wordcraft_engine::io::open_bytes(name, bytes).unwrap();
    assert!(back.plain_text(StoryRef::Body).contains("Biên bản họp"));
}

#[test]
fn without_a_share_service_share_still_opens_export_and_backstage_has_no_entry() {
    let (app, shared, downloaded) = app(None);
    let mut h = harness(app);
    click(&mut h, Role::Button, "Share");
    assert!(h.state().ui.backstage && h.state().ui.backstage_page == "export");
    assert!(h.query_by_label("Share…").is_none());
    assert!(shared.borrow().is_empty() && downloaded.borrow().is_empty());
    assert!(h.state_mut().run(super::SHARE, json!({})).is_err(), "no share command without the service");
}

#[test]
fn share_hands_the_document_to_the_share_sheet_from_the_tab_strip_and_backstage() {
    let (app, shared, downloaded) = app(Some(true));
    let mut h = harness(app);
    click(&mut h, Role::Button, "Share");
    assert!(!h.state().ui.backstage, "shares straight away instead of opening Export");
    assert_eq!(shared.borrow().len(), 1);
    assert_reopens(&shared.borrow()[0]);
    assert!(h.state().session.dirty, "a shared copy doesn't save the document");

    h.state_mut().run("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    h.run_steps(4);
    click(&mut h, Role::Button, "Share…");
    assert_eq!(shared.borrow().len(), 2);
    assert_eq!(shared.borrow()[0], shared.borrow()[1]);
    assert!(downloaded.borrow().is_empty());
}

#[test]
fn a_declined_share_downloads_the_same_copy() {
    let (app, shared, downloaded) = app(Some(false));
    let mut h = harness(app);
    click(&mut h, Role::Button, "Share");
    assert_eq!(shared.borrow().len(), 1);
    assert_eq!(*downloaded.borrow(), *shared.borrow());
    assert_reopens(&downloaded.borrow()[0]);
    assert!(h.state().session.dirty);
}

#[test]
fn a_loss_warning_comes_first_and_save_anyway_shares() {
    let (mut app, shared, _) = app(Some(true));
    app.session.doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
    let mut h = harness(app);
    click(&mut h, Role::Button, "Share");
    assert!(matches!(h.state().dialog, Some(Dialog::LossWarning(_))));
    assert!(shared.borrow().is_empty());
    click(&mut h, Role::Button, "Save anyway");
    assert!(h.state().dialog.is_none());
    assert_eq!(shared.borrow().len(), 1);
    assert_reopens(&shared.borrow()[0]);
    assert!(h.state().session.doc.passthrough.contains_key("wordcraft:package.signed"), "the document itself keeps its signature");
}

#[test]
fn save_a_copy_in_a_loss_warning_from_share_shares_too() {
    let (mut app, shared, downloaded) = app(Some(true));
    app.session.doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
    let mut h = harness(app);
    click(&mut h, Role::Button, "Share");
    click(&mut h, Role::Button, "Save a copy");
    assert!(h.state().dialog.is_none());
    assert_eq!(shared.borrow().len(), 1, "a shared file is already a copy");
    assert_reopens(&shared.borrow()[0]);
    assert!(downloaded.borrow().is_empty());
}

/// Enter or Space "clicks" a frame later, without the user activation a share sheet needs.
#[test]
fn keyboard_activation_downloads_instead_of_sharing() {
    let press = |h: &mut Harness<'static, WordApp>, label: &str| {
        h.get_by_role_and_label(Role::Button, label).focus();
        h.run_steps(2);
        h.key_press(egui::Key::Enter);
        h.run_steps(4);
    };
    let (mut app, shared, downloaded) = app(Some(true));
    app.run("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut h = harness(app);
    press(&mut h, "Share…");
    assert!(shared.borrow().is_empty());
    assert_eq!(downloaded.borrow().len(), 1);
    assert_reopens(&downloaded.borrow()[0]);

    for button in ["Save anyway", "Save a copy"] {
        let app = h.state_mut();
        app.session.doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
        app.share_document(true).unwrap();
        h.run_steps(4);
        assert!(matches!(h.state().dialog, Some(Dialog::LossWarning(_))));
        press(&mut h, button);
        assert!(h.state().dialog.is_none(), "{button}");
        assert!(shared.borrow().is_empty(), "{button}");
    }
    assert_eq!(downloaded.borrow().len(), 3);
}
