//! The desktop edit lock as the app shows it, against a fake account hook: every document
//! read-only with the lock's reason, Save, Export and Print still there, and editing back without
//! reopening anything once the lock lifts.

use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

use crate::WordApp;
use crate::account::{AccountHook, AccountSnapshot, AccountView, EditLock, LicenceView, SignedInView};
use crate::i18n;
use crate::tests_account::{Fake, account_page, harness_in, signed_in};

type H = egui_kittest::Harness<'static, WordApp>;

const SIGN_IN: &str = "Đăng nhập PI ID để sửa";
const NO_PLAN: &str = "Tổ chức này chưa có gói Office để sửa";

fn locked(view: AccountView, lock: EditLock, width: f32) -> (std::sync::Arc<Fake>, H) {
    let fake = Fake::new(view);
    fake.set_lock(Some(lock));
    let h = harness_in(Some(&fake), width, "vi");
    (fake, h)
}

fn body(h: &H) -> String {
    h.state().session.doc.plain_text(wordcraft_doc::StoryRef::Body)
}

#[test]
fn a_locked_app_shows_its_documents_read_only_with_the_reason_and_still_saves() {
    let (fake, mut h) = locked(AccountView::Guest, EditLock::SignIn, 1440.0);
    assert!(h.state().read_only());
    assert!(h.get_by_label("Chỉ xem").accesskit_node().is_disabled(), "the mode button says Viewing, greyed");
    assert!(h.query_by_label(SIGN_IN).is_some(), "the notice above the page says why");
    assert_eq!(h.state_mut().run("text.insert", json!({"text": "x"})), Err(SIGN_IN.to_string()), "the status bar says why");
    assert_eq!(fake.calls().iter().filter(|c| *c == "edit_refused").count(), 1, "the hook may sync");
    assert!(h.state_mut().run("edit.undo", json!({})).is_err());
    for edit in ["format.bold", "para.style", "insert.table", "ui.changePicture"] {
        assert!(!h.state().access_allows(edit), "{edit} is greyed");
    }
    for keep in ["file.save", "file.saveAs", "file.exportPdf", "file.print", "ui.print", "edit.copy", "file.open", "file.new"] {
        assert!(h.state().access_allows(keep), "{keep} stays");
    }
    let dir = std::env::temp_dir().join(format!("wordcraft-ui-edit-lock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("draft.docx");
    h.state_mut().run("file.save", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(path.is_file(), "Save writes the document");
    h.state_mut().run("file.exportPdf", json!({"path": dir.join("draft.pdf").to_string_lossy()})).unwrap();
    // A document opened while locked is locked too.
    h.state_mut().run("file.new", json!({})).unwrap();
    h.run_steps(2);
    assert!(h.state().read_only() && h.state().session.edit_lock);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_lease_unlocks_the_open_document_in_place() {
    let (fake, mut h) = locked(AccountView::SignedIn(signed_in(LicenceView::Free)), EditLock::NoPlan, 1440.0);
    assert_eq!(h.state_mut().run("text.insert", json!({"text": "x"})), Err(NO_PLAN.to_string()));
    let document = h.state().session.document_id();
    assert!(h.query_by_label("Đóng").is_some(), "the notice shows");
    fake.set_lock(None);
    h.run_steps(2);
    assert!(!h.state().read_only());
    // The refused edit's status line keeps the reason; the notice is the bar with Close.
    assert!(h.query_by_label("Đóng").is_none(), "the notice is gone");
    h.state_mut().run("caret.docEnd", json!({})).unwrap();
    h.state_mut().run("text.insert", json!({"text": " đã sửa"})).unwrap();
    assert_eq!(h.state().session.document_id(), document, "nothing was reopened");
    assert!(body(&h).contains("Draft đã sửa"));
    assert!(h.query_by_label("Chỉnh sửa").is_some(), "the mode button says Editing again");
}

/// What the input method was composing goes into the document before the lock, as when a host
/// turns editing off.
#[test]
fn locking_keeps_the_composition_being_typed() {
    let fake = Fake::new(AccountView::Guest);
    let mut h = harness_in(Some(&fake), 1440.0, "vi");
    h.state_mut().run("caret.docEnd", json!({})).unwrap();
    h.state_mut().canvas.ime_preedit = " chữ".into();
    fake.set_lock(Some(EditLock::SignIn));
    h.run_steps(2);
    assert!(h.state().read_only());
    assert!(h.state().canvas.ime_preedit.is_empty());
    assert!(body(&h).contains("Draft chữ"), "{:?}", body(&h));
}

#[test]
fn the_notice_signs_a_guest_in_and_closes_until_the_reason_changes() {
    let (fake, mut h) = locked(AccountView::Guest, EditLock::SignIn, 1440.0);
    // The title bar's account button is called "Đăng nhập" too; the notice's is lower down.
    h.query_all_by_label("Đăng nhập").max_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y)).unwrap().click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["sign_in"]);
    assert!(h.state().ui.backstage, "File › Account shows the browser wait and Cancel");

    let (fake, mut h) = locked(AccountView::SignedIn(signed_in(LicenceView::Free)), EditLock::ChooseOrg, 1440.0);
    assert!(h.query_by_label("Chọn tổ chức để sửa").is_some());
    h.get_by_label("Đóng").click();
    h.run_steps(2);
    assert!(h.query_by_label("Chọn tổ chức để sửa").is_none(), "closed");
    assert!(h.state().read_only(), "closing the notice unlocks nothing");
    fake.set_lock(Some(EditLock::NoPlan));
    h.run_steps(2);
    assert!(h.query_by_label(NO_PLAN).is_some(), "a new reason shows again");
    h.get_by_label("Tài khoản").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.backstage_page, crate::account::ACCOUNT_PAGE);
}

#[test]
fn the_account_page_says_why_and_offers_retry_where_it_helps() {
    let (_, mut h) = locked(AccountView::Guest, EditLock::SignIn, 1440.0);
    account_page(&mut h);
    assert!(h.query_all_by_label(SIGN_IN).count() >= 1);
    assert!(h.query_by_label_contains("khi không có tài khoản").is_none(), "no longer says every feature works without one");

    let (fake, mut h) = locked(AccountView::SignedIn(signed_in(LicenceView::Free)), EditLock::NoPlan, 1440.0);
    account_page(&mut h);
    assert!(h.query_all_by_label(NO_PLAN).count() >= 1);
    h.get_by_label("Thử lại").click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["retry_activation"]);

    let (_, mut h) = locked(AccountView::SignedIn(signed_in(LicenceView::DeviceRevoked)), EditLock::Revoked, 1440.0);
    account_page(&mut h);
    assert!(h.query_by_label("Thử lại").is_none(), "a revoked device needs a new sign-in, not a retry");
}

/// At start-up, while the adapter reads the account, editing is held but nothing is announced.
#[test]
fn the_start_up_check_locks_quietly() {
    let (_, mut h) = locked(AccountView::Guest, EditLock::Checking, 1440.0);
    assert!(h.state().read_only());
    assert!(h.query_by_label(i18n::tr(i18n::Lang::from_pref("vi"), EditLock::Checking.reason())).is_none());
    account_page(&mut h);
    assert!(h.query_by_label_contains("khi không có tài khoản").is_some());
}

/// The web adapter has no edit lock (the trait's default), and its `allowed` may say no to
/// `erpconsoffice.use`: the web free tier still edits.
#[test]
fn a_hook_without_an_edit_lock_never_locks() {
    struct Web;
    impl AccountHook for Web {
        fn snapshot(&self) -> AccountSnapshot {
            AccountSnapshot { view: AccountView::SignedIn(signed_in(LicenceView::Free)), busy: false, error: None }
        }
        fn sign_in(&self) {}
        fn cancel_sign_in(&self) {}
        fn sign_out(&self) {}
        fn select_org(&self, _org_id: Option<String>) {}
        fn retry_activation(&self) {}
        fn allowed(&self, feature: &str) -> bool {
            !crate::account::is_pro_feature(feature)
        }
        fn warning_shown(&self, _days: u32) {}
        fn attach(&self, _ctx: &egui::Context) {}
    }
    assert_eq!(Web.edit_lock(), None);
    let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Draft")), Default::default());
    app.services.account = Some(Box::new(Web));
    let mut h: H = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    assert!(!h.state().allowed("erpconsoffice.use"));
    assert!(!h.state().read_only());
    h.state_mut().run("text.insert", json!({"text": "x"})).unwrap();
}

#[test]
fn every_lock_reason_is_translated_in_vietnamese_and_polish() {
    let all = [
        EditLock::Checking,
        EditLock::Unavailable,
        EditLock::SignIn,
        EditLock::ChooseOrg,
        EditLock::Revoked,
        EditLock::DeviceCap,
        EditLock::NotActivated,
        EditLock::SessionOnly,
        EditLock::RenewOffline,
        EditLock::RenewFailed,
        EditLock::NoPlan,
    ];
    for code in ["vi", "pl"] {
        let lang = i18n::Lang::from_pref(code);
        for lock in all {
            assert!(i18n::has(lang, lock.reason()), "{code}: {lock:?}");
        }
    }
    let vi = i18n::Lang::from_pref("vi");
    assert_eq!(i18n::tr(vi, EditLock::SignIn.reason()), SIGN_IN);
    assert_eq!(i18n::tr(vi, EditLock::ChooseOrg.reason()), "Chọn tổ chức để sửa");
    assert!(all.iter().filter(|l| l.retry_offered()).all(|l| l.sync_may_lift() || *l == EditLock::DeviceCap));
    assert!(!EditLock::Revoked.retry_offered() && !EditLock::SignIn.retry_offered() && !EditLock::SessionOnly.retry_offered());
}

/// Renders the locked app and File › Account to PNGs when `WORDCRAFT_ACCOUNT_SCREENSHOTS` names a
/// folder.
#[test]
fn edit_lock_screenshots() {
    let Some(dir) = std::env::var_os("WORDCRAFT_ACCOUNT_SCREENSHOTS") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let offline =
        AccountView::SignedIn(SignedInView { online: false, ..signed_in(LicenceView::Inactive { expires_at: crate::tests_account::EXPIRES }) });
    let shots = [
        ("lock-guest", AccountView::Guest, EditLock::SignIn, false),
        ("lock-renew-offline", offline, EditLock::RenewOffline, false),
        ("lock-account-no-plan", AccountView::SignedIn(signed_in(LicenceView::Free)), EditLock::NoPlan, true),
    ];
    for (name, view, lock, page) in shots {
        for width in [1280.0, 390.0] {
            let (_, mut h) = locked(view.clone(), lock, width);
            if page {
                account_page(&mut h);
            }
            h.run_steps(4);
            h.render().unwrap().save(dir.join(format!("{name}-{width}.png"))).unwrap();
        }
    }
}
