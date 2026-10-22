#![cfg(not(target_arch = "wasm32"))]

//! Native update notices use a fake service and never contact a server or run installers.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use egui_kittest::{Harness, kittest::Queryable};
use wordcraft_ui_egui::{
    WordApp,
    updates::{CheckOutcome, Offer, PreviousResult, RELEASES_PAGE, UpdateHook},
};

struct Fake {
    page_only: bool,
    msi: bool,
    auto: AtomicBool,
    calls: AtomicUsize,
    forced: AtomicBool,
    skipped: Mutex<Option<String>>,
    downloads: AtomicUsize,
    installed: AtomicBool,
    fail_download: AtomicBool,
    wait_download: AtomicBool,
    check_error: AtomicBool,
    up_to_date: AtomicBool,
    previous: Mutex<Option<PreviousResult>>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            page_only: false,
            msi: false,
            auto: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            forced: AtomicBool::new(false),
            skipped: Mutex::new(None),
            downloads: AtomicUsize::new(0),
            installed: AtomicBool::new(false),
            fail_download: AtomicBool::new(false),
            wait_download: AtomicBool::new(false),
            check_error: AtomicBool::new(false),
            up_to_date: AtomicBool::new(false),
            previous: Mutex::new(None),
        }
    }
}

impl UpdateHook for Fake {
    fn check(&self, forced: bool) -> Result<CheckOutcome, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.forced.store(forced, Ordering::SeqCst);
        if self.check_error.load(Ordering::SeqCst) {
            return Err("test check failure".into());
        }
        if self.up_to_date.load(Ordering::SeqCst) {
            return Ok(CheckOutcome::UpToDate);
        }
        Ok(CheckOutcome::Available(Offer {
            version: "99.0.0".into(),
            notes_url: format!("{RELEASES_PAGE}/tag/erpcons-v99.0.0"),
            download_page_only: self.page_only,
            windows_msi: self.msi,
        }))
    }
    fn download(&self, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(), String> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        progress(5, 10);
        while self.wait_download.load(Ordering::SeqCst) && !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if self.fail_download.load(Ordering::SeqCst) {
            return Err("test download failure".into());
        }
        if cancel.load(Ordering::Relaxed) { Err("cancelled".into()) } else { Ok(()) }
    }
    fn install_and_restart(&self) -> Result<(), String> {
        self.installed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn skip(&self, version: &str) -> Result<(), String> {
        *self.skipped.lock().unwrap() = Some(version.into());
        Ok(())
    }
    fn auto_check(&self) -> bool {
        self.auto.load(Ordering::SeqCst)
    }
    fn set_auto_check(&self, on: bool) -> Result<(), String> {
        self.auto.store(on, Ordering::SeqCst);
        Ok(())
    }
    fn take_previous_result(&self) -> Option<PreviousResult> {
        self.previous.lock().unwrap().take()
    }
}

fn harness(fake: Arc<Fake>) -> Harness<'static, WordApp> {
    let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
    app.ui.language = "en".into();
    app.services.updates = Some(fake);
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    )
}

fn wait_for(h: &mut Harness<'static, WordApp>, label: &str) {
    for _ in 0..200 {
        h.run_steps(2);
        if h.query_by_label_contains(label).is_some() {
            h.run_steps(3);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label_contains(label);
}

#[test]
fn available_at_start_later_hides_and_forced_check_restores_notice() {
    let fake = Arc::new(Fake::default());
    let mut h = harness(fake.clone());
    wait_for(&mut h, "Văn bản – ERPCons 99.0.0 is available");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert!(!fake.forced.load(Ordering::SeqCst));
    h.get_by_label("Remind me later").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("is available").is_none());
    h.state_mut().check_for_updates();
    wait_for(&mut h, "Văn bản – ERPCons 99.0.0 is available");
    assert!(fake.forced.load(Ordering::SeqCst));
}

#[test]
fn skip_passes_version_and_hides_notice() {
    let fake = Arc::new(Fake::default());
    let mut h = harness(fake.clone());
    wait_for(&mut h, "Skip this version");
    h.get_by_label("Skip this version").click();
    h.run_steps(3);
    assert_eq!(fake.skipped.lock().unwrap().as_deref(), Some("99.0.0"));
    assert!(h.query_by_label_contains("is available").is_none());
}

#[test]
fn download_failure_offers_retry_and_success_launches_installer() {
    let fake = Arc::new(Fake::default());
    fake.fail_download.store(true, Ordering::SeqCst);
    let mut h = harness(fake.clone());
    wait_for(&mut h, "Update now");
    h.get_by_label("Update now").click();
    wait_for(&mut h, "test download failure");
    h.get_by_label("Retry");
    assert!(!fake.installed.load(Ordering::SeqCst));
    fake.fail_download.store(false, Ordering::SeqCst);
    h.get_by_label("Retry").click();
    for _ in 0..400 {
        h.run_steps(2);
        if fake.installed.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(fake.installed.load(Ordering::SeqCst));
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 2);
}

#[test]
fn previous_results_are_shown_and_failure_retry_forces_check() {
    for (previous, label, failed) in [
        (PreviousResult::Installed { version: "1.2.3".into() }, "Updated to version 1.2.3", false),
        (PreviousResult::Failed { version: "1.2.3".into(), reason: "test reason".into() }, "Update to 1.2.3 failed: test reason", true),
    ] {
        let fake = Arc::new(Fake::default());
        fake.auto.store(false, Ordering::SeqCst);
        *fake.previous.lock().unwrap() = Some(previous);
        let mut h = harness(fake.clone());
        wait_for(&mut h, label);
        assert!(fake.previous.lock().unwrap().is_none());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        if failed {
            h.get_by_label("Retry").click();
            wait_for(&mut h, "is available");
            assert!(fake.forced.load(Ordering::SeqCst));
        }
    }
}

#[test]
fn unsaved_document_cancel_aborts_update_without_discarding_work() {
    let fake = Arc::new(Fake::default());
    let mut h = harness(fake.clone());
    h.state_mut().run("text.insert", serde_json::json!({"text":"Draft"})).unwrap();
    wait_for(&mut h, "Update now");
    h.get_by_label("Update now").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_some());
    h.state_mut().run("ui.saveChanges", serde_json::json!({"answer":"cancel"})).unwrap();
    h.run_steps(3);
    assert!(h.state().session.dirty);
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 0);
    assert!(!fake.installed.load(Ordering::SeqCst));
}

#[test]
fn forced_checks_report_up_to_date_and_errors_when_auto_is_off() {
    let fake = Arc::new(Fake::default());
    fake.auto.store(false, Ordering::SeqCst);
    fake.up_to_date.store(true, Ordering::SeqCst);
    let mut h = harness(fake.clone());
    h.run_steps(3);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    h.state_mut().check_for_updates();
    wait_for(&mut h, "Văn bản – ERPCons is up to date.");
    fake.check_error.store(true, Ordering::SeqCst);
    h.state_mut().check_for_updates();
    wait_for(&mut h, "test check failure");
    h.get_by_label("Retry");
}

#[test]
fn cancel_download_keeps_app_open_and_never_installs() {
    let fake = Arc::new(Fake::default());
    fake.wait_download.store(true, Ordering::SeqCst);
    let mut h = harness(fake.clone());
    wait_for(&mut h, "Update now");
    h.get_by_label("Update now").click();
    wait_for(&mut h, "Cancel");
    h.get_by_label("Cancel").click();
    wait_for(&mut h, "Update now");
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 1);
    assert!(!fake.installed.load(Ordering::SeqCst));
}

#[test]
fn auto_check_checkbox_updates_the_service_preference() {
    let fake = Arc::new(Fake::default());
    fake.auto.store(false, Ordering::SeqCst);
    let mut h = harness(fake.clone());
    h.state_mut().ui.backstage = true;
    h.state_mut().ui.backstage_page = "options".into();
    h.run_steps(3);
    h.get_by_label("Automatically check for updates").click();
    h.run_steps(3);
    assert!(fake.auto.load(Ordering::SeqCst));
}

#[test]
fn edits_made_during_download_are_guarded_before_install() {
    let fake = Arc::new(Fake::default());
    fake.wait_download.store(true, Ordering::SeqCst);
    let mut h = harness(fake.clone());
    wait_for(&mut h, "Update now");
    h.get_by_label("Update now").click();
    wait_for(&mut h, "Cancel");
    h.state_mut().run("text.insert", serde_json::json!({"text":"New edits"})).unwrap();
    fake.wait_download.store(false, Ordering::SeqCst);
    for _ in 0..400 {
        h.run_steps(2);
        if h.state().dialog.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.state().dialog.is_some());
    assert!(!fake.installed.load(Ordering::SeqCst));
    h.state_mut().run("ui.saveChanges", serde_json::json!({"answer":"cancel"})).unwrap();
    h.run_steps(5);
    assert!(h.state().session.dirty);
    assert!(!fake.installed.load(Ordering::SeqCst));
}

#[test]
fn download_page_only_never_downloads_or_installs() {
    let fake = Arc::new(Fake { page_only: true, ..Fake::default() });
    let mut h = harness(fake.clone());
    wait_for(&mut h, "Download new version");
    assert!(h.query_by_label("Update now").is_none());
    h.get_by_label("Download new version").click();
    h.run_steps(3);
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 0);
    assert!(!fake.installed.load(Ordering::SeqCst));
}

#[test]
fn windows_msi_ready_notice_explains_permission_prompt() {
    let fake = Arc::new(Fake { msi: true, ..Fake::default() });
    let mut h = harness(fake.clone());
    wait_for(&mut h, "Update now");
    h.get_by_label("Update now").click();
    wait_for(&mut h, "Windows will ask for installation permission");
    assert!(!fake.installed.load(Ordering::SeqCst));
}

#[test]
fn update_notice_screenshots_in_both_themes() {
    let Some(dir) = std::env::var_os("WORDCRAFT_UPDATE_SCREENSHOTS") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for dark in [false, true] {
        let mut h = harness(Arc::new(Fake::default()));
        h.state_mut().ui.language = "vi".into();
        h.state_mut().run("ui.theme", serde_json::json!({"value": if dark { "dark" } else { "light" }})).unwrap();
        wait_for(&mut h, "Đã có Văn bản – ERPCons");
        h.render().unwrap().save(dir.join(if dark { "update-dark.png" } else { "update-light.png" })).unwrap();
    }
}

#[test]
fn every_native_update_string_has_a_translation_in_every_catalog() {
    use wordcraft_ui_egui::i18n::{self, Lang};
    let keys = [
        "Automatically check for updates",
        "Cancel",
        "Check for updates",
        "Checking for a newer version…",
        "Close",
        "Could not start updater: {reason}",
        "Could not write the update: {detail}",
        "Download new version",
        "Permission denied while updating; check folder permissions",
        "Remind me later",
        "Retry",
        "Skip this version",
        "The app will close and reopen after updating",
        "The app will close, Windows will ask for installation permission — choose Yes — then the app will reopen automatically",
        "The downloaded update failed verification",
        "The update check stopped unexpectedly",
        "The update manifest is invalid: {detail}",
        "The update signature is invalid",
        "The updater failed; restart the app and try again",
        "The updater is busy; try again shortly",
        "This installation cannot update automatically",
        "Update cancelled",
        "Update network unavailable: {detail}",
        "Update now",
        "Update server returned HTTP {detail}",
        "Update to {version} failed: {reason}",
        "Updated to version {version}",
        "Văn bản – ERPCons is up to date.",
        "Văn bản – ERPCons {version} is available",
        "What's new",
    ];
    for language in Lang::all().filter(|language| language.code() != "en") {
        for key in keys {
            assert!(i18n::has(language, key), "{}: {key}", language.code());
        }
    }
}

#[test]
fn discarding_unchanged_drafts_does_not_prompt_again_after_download() {
    let fake = Arc::new(Fake::default());
    let mut h = harness(fake.clone());
    h.state_mut().run("text.insert", serde_json::json!({"text":"Draft"})).unwrap();
    wait_for(&mut h, "Update now");
    h.get_by_label("Update now").click();
    h.run_steps(3);
    h.state_mut().run("ui.saveChanges", serde_json::json!({"answer":"dontSave"})).unwrap();
    for _ in 0..400 {
        h.run_steps(2);
        if fake.installed.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(fake.installed.load(Ordering::SeqCst));
    assert!(h.state().dialog.is_none());
}

#[test]
fn every_open_dirty_document_is_guarded() {
    let fake = Arc::new(Fake::default());
    let mut h = harness(fake.clone());
    h.state_mut().run("text.insert", serde_json::json!({"text":"First"})).unwrap();
    h.state_mut().run("file.new", serde_json::json!({})).unwrap();
    h.state_mut().run("text.insert", serde_json::json!({"text":"Second"})).unwrap();
    assert_eq!(h.state().document_count(), 2);
    wait_for(&mut h, "Update now");
    h.get_by_label("Update now").click();
    h.run_steps(3);
    h.state_mut().run("ui.saveChanges", serde_json::json!({"answer":"dontSave"})).unwrap();
    assert!(h.state().dialog.is_some());
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 0);
    h.state_mut().run("ui.saveChanges", serde_json::json!({"answer":"cancel"})).unwrap();
    assert_eq!(h.state().document_count(), 2);
    assert!(!fake.installed.load(Ordering::SeqCst));
}
