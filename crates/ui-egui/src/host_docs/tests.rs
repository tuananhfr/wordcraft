//! The app with a fake `erpHost` (`erp-host.md` rules 1–9). Times are passed in, so AutoSave's
//! pauses are exact.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::json;
use wordcraft_doc::StoryRef;
use wordcraft_engine::Session;

use super::{HostLink, HostOpen, HostTransport, SaveDone, SaveOutcome, SaveReason};
use crate::draft_bridge::{DraftBridge, HiddenFlush};
use crate::{Download, Services, WordApp, i18n};

struct Sent {
    doc_id: String,
    bytes: Vec<u8>,
    revision: u64,
    reason: SaveReason,
    done: Option<SaveDone>,
}

#[derive(Default)]
struct FakeHost {
    sent: RefCell<Vec<Sent>>,
    calls: RefCell<Vec<String>>,
}

impl HostTransport for FakeHost {
    fn save(&self, doc_id: &str, bytes: &[u8], revision: u64, reason: SaveReason, done: SaveDone) {
        self.sent.borrow_mut().push(Sent { doc_id: doc_id.into(), bytes: bytes.to_vec(), revision, reason, done: Some(done) });
    }
    fn dirty(&self, doc_id: &str, dirty: bool) {
        self.calls.borrow_mut().push(format!("dirty {doc_id} {dirty}"));
    }
    fn opened(&self, doc_id: &str, ok: bool, reason: Option<&str>) {
        self.calls.borrow_mut().push(format!("opened {doc_id} {ok} {}", reason.unwrap_or("")));
    }
    fn closed(&self, doc_id: &str) {
        self.calls.borrow_mut().push(format!("closed {doc_id}"));
    }
}

impl FakeHost {
    fn sends(&self) -> Vec<(String, u64, SaveReason)> {
        self.sent.borrow().iter().map(|s| (s.doc_id.clone(), s.revision, s.reason)).collect()
    }

    /// Answer save `i`, as the shell's promise would, between frames.
    fn answer(&self, i: usize, outcome: SaveOutcome) {
        let done = self.sent.borrow_mut().get_mut(i).and_then(|s| s.done.take());
        (done.expect("an unanswered save"))(outcome);
    }

    fn take_calls(&self) -> Vec<String> {
        std::mem::take(&mut *self.calls.borrow_mut())
    }
}

struct Page {
    host: Rc<FakeHost>,
    link: HostLink,
    downloads: Rc<RefCell<Vec<String>>>,
    drafts: Rc<RefCell<Vec<String>>>,
    ctx: egui::Context,
}

/// The web app with the office shell's `erpHost` and `erpDrafts`, on the Start screen.
fn web_app() -> (WordApp, Page) {
    i18n::set_current(i18n::Lang::from_pref("vi"));
    let host = Rc::new(FakeHost::default());
    let link = HostLink::new(host.clone());
    let downloads: Rc<RefCell<Vec<String>>> = Rc::default();
    let drafts: Rc<RefCell<Vec<String>>> = Rc::default();
    let (d, p) = (downloads.clone(), drafts.clone());
    let bridge = DraftBridge {
        desktop_recovery: false,
        available: Box::new(|| true),
        put: Box::new(move |key, _, _, _| {
            p.borrow_mut().push(key.to_string());
            Ok(())
        }),
        remove: Box::new(|_| Ok(())),
        hidden: Rc::new(RefCell::new(HiddenFlush::default())),
        restores: Rc::default(),
    };
    let services = Services {
        download: Some(Box::new(move |name, _| {
            d.borrow_mut().push(name.to_string());
            Ok(Download::Done)
        })),
        drafts: Some(bridge),
        host: Some(link.clone()),
        ..Default::default()
    };
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), services);
    app.autosave = false;
    app.ui.language = "vi".into();
    app.start_screen();
    (app, Page { host, link, downloads, drafts, ctx: egui::Context::default() })
}

fn docx(text: &str) -> Vec<u8> {
    wordcraft_engine::io::save_bytes_with("x.docx", &wordcraft_doc::Document::from_text(text), None, &Default::default()).unwrap()
}

fn hand_in(page: &Page, doc_id: &str, title: &str, text: &str) {
    page.link.open(HostOpen { doc_id: doc_id.into(), version: 1, title: title.into(), format: "docx".into(), bytes: docx(text) });
}

fn body(app: &WordApp) -> String {
    app.session.doc.plain_text(StoryRef::Body)
}

fn type_text(app: &mut WordApp, text: &str) {
    app.run("text.insert", json!({"text": text})).unwrap();
}

/// The text of a sent save's bytes.
fn sent_text(page: &Page, i: usize) -> String {
    let bytes = page.host.sent.borrow()[i].bytes.clone();
    wordcraft_engine::io::open_bytes("x.docx", &bytes).unwrap().plain_text(StoryRef::Body)
}

#[test]
fn an_open_from_the_host_becomes_a_document_named_by_its_title() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "Báo cáo quý 3", "Xin chào");
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(page.host.take_calls(), vec!["opened d1 true "]);
    assert!(app.has_document());
    assert_eq!(app.title_stem(), "Báo cáo quý 3");
    assert!(body(&app).contains("Xin chào"));
    assert!(!app.session.dirty);
    assert!(!app.ui.backstage);
    hand_in(&page, "d2", "Hợp đồng.docx", "B");
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(page.host.take_calls(), vec!["opened d2 true "]);
    assert_eq!(app.document_count(), 2, "a second document opens in its own tab");
    assert_eq!(app.title_stem(), "Hợp đồng");
}

#[test]
fn refused_opens_report_the_shells_intake_key_and_never_ask_for_a_password() {
    let (mut app, page) = web_app();
    page.link.open(HostOpen { doc_id: "x".into(), version: 1, title: "Bảng".into(), format: "xlsx".into(), bytes: docx("A") });
    page.link.open(HostOpen { doc_id: "g".into(), version: 1, title: "Hỏng".into(), format: "docx".into(), bytes: b"not a zip".to_vec() });
    let locked =
        wordcraft_engine::io::save_bytes_with("x.docx", &wordcraft_doc::Document::from_text("Mật"), Some("sesame"), &Default::default()).unwrap();
    page.link.open(HostOpen { doc_id: "p".into(), version: 1, title: "Khoá".into(), format: "docx".into(), bytes: locked });
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(page.host.take_calls(), vec!["opened x false unsupported", "opened g false parserFailed", "opened p false unsupported",]);
    assert!(app.dialog.is_none(), "no password prompt");
    assert!(!app.has_document());
    for i in 0..10 {
        hand_in(&page, &format!("n{i}"), "T", "A");
    }
    hand_in(&page, "over", "T", "A");
    app.host_tick(&page.ctx, 0.0);
    let calls = page.host.take_calls();
    assert_eq!(calls.iter().filter(|c| c.ends_with(" true ")).count(), 10);
    assert_eq!(calls.last().map(String::as_str), Some("opened over false parserFailed"), "the document limit");
}

#[test]
fn ctrl_s_saves_through_the_host_and_cleans_only_as_of_the_revision_sent() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "Báo cáo", "Xin chào");
    app.host_tick(&page.ctx, 0.0);
    page.host.take_calls();
    type_text(&mut app, "A");
    app.host_observe(100.0);
    assert_eq!(page.host.take_calls(), vec!["dirty d1 true"]);
    let sent_rev = app.session.rev();
    app.run("file.save", json!({})).unwrap();
    assert_eq!(page.host.sends(), vec![("d1".into(), sent_rev, SaveReason::Manual)]);
    assert!(page.downloads.borrow().is_empty(), "never the file picker or a download");
    assert!(sent_text(&page, 0).contains("AXin chào"));
    type_text(&mut app, "B");
    page.host.answer(0, SaveOutcome::Saved { version: 2 });
    app.host_tick(&page.ctx, 200.0);
    assert!(app.session.dirty, "B was typed while the save was in flight");
    assert!(page.host.take_calls().is_empty(), "still dirty: nothing to report");
    assert_eq!(app.save_status(), None);
    app.host_tick(&page.ctx, 3_200.0);
    assert_eq!(page.host.sends().len(), 2, "and AutoSave sends it");
    assert_eq!(page.host.sends()[1].2, SaveReason::Auto);
    page.host.answer(1, SaveOutcome::Saved { version: 3 });
    app.host_tick(&page.ctx, 3_300.0);
    assert!(!app.session.dirty);
    assert_eq!(page.host.take_calls(), vec!["dirty d1 false"]);
}

#[test]
fn autosave_waits_three_seconds_after_the_last_edit() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "T", "");
    app.host_tick(&page.ctx, 0.0);
    type_text(&mut app, "a");
    app.host_observe(1_000.0);
    type_text(&mut app, "b");
    app.host_observe(2_000.0);
    app.host_tick(&page.ctx, 4_999.0);
    assert!(page.host.sends().is_empty());
    app.host_tick(&page.ctx, 5_000.0);
    assert_eq!(page.host.sends(), vec![("d1".into(), app.session.rev(), SaveReason::Auto)]);
    app.host_tick(&page.ctx, 60_000.0);
    assert_eq!(page.host.sends().len(), 1, "no resend of an unchanged document");
}

#[test]
fn saves_in_flight_coalesce_into_one_follow_up_of_the_newest_revision() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "T", "");
    app.host_tick(&page.ctx, 0.0);
    type_text(&mut app, "a");
    app.run("file.save", json!({})).unwrap();
    for (i, t) in ["b", "c", "d"].into_iter().enumerate() {
        type_text(&mut app, t);
        app.run("file.save", json!({})).unwrap();
        app.host_tick(&page.ctx, 40_000.0 * (i + 1) as f64);
    }
    assert_eq!(page.host.sends().len(), 1, "one save in flight per document");
    page.host.answer(0, SaveOutcome::Saved { version: 2 });
    assert_eq!(page.host.sends().len(), 2, "the follow-up goes as soon as the first settles");
    assert_eq!(page.host.sends()[1], ("d1".into(), app.session.rev(), SaveReason::Manual));
    assert!(sent_text(&page, 1).starts_with("abcd"));
    page.host.answer(1, SaveOutcome::Saved { version: 3 });
    app.host_tick(&page.ctx, 200_000.0);
    assert!(!app.session.dirty);
    assert_eq!(page.host.sends().len(), 2);
}

#[test]
fn a_conflict_stops_autosave_keeps_the_document_dirty_and_shows_nothing() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "T", "");
    app.host_tick(&page.ctx, 0.0);
    type_text(&mut app, "a");
    app.host_observe(0.0);
    app.host_tick(&page.ctx, 3_000.0);
    page.host.answer(0, SaveOutcome::Conflict { latest_version: 7 });
    app.host_tick(&page.ctx, 3_100.0);
    assert!(app.session.dirty);
    assert_eq!(app.save_status(), None, "the shell shows the conflict");
    type_text(&mut app, "b");
    app.host_observe(3_200.0);
    app.host_tick(&page.ctx, 100_000.0);
    let flushed = Rc::new(RefCell::new(false));
    let f = flushed.clone();
    page.link.flush(Box::new(move || *f.borrow_mut() = true));
    assert!(*flushed.borrow(), "nothing to flush: the conflict isn't retried");
    assert_eq!(page.host.sends().len(), 1, "never retried by itself");
}

#[test]
fn offline_and_retryable_failures_stay_dirty_and_keep_autosave() {
    for outcome in [SaveOutcome::Offline, SaveOutcome::Failed { retryable: true, message: "Máy chủ bận".into() }] {
        let (mut app, page) = web_app();
        hand_in(&page, "d1", "T", "");
        app.host_tick(&page.ctx, 0.0);
        type_text(&mut app, "a");
        app.host_observe(0.0);
        app.host_tick(&page.ctx, 3_000.0);
        page.host.answer(0, outcome);
        app.host_tick(&page.ctx, 3_100.0);
        assert!(app.session.dirty);
        assert_eq!(app.save_status(), None, "the shell shows the state");
        type_text(&mut app, "b");
        app.host_observe(4_000.0);
        app.host_tick(&page.ctx, 7_000.0);
        assert_eq!(page.host.sends().len(), 2, "AutoSave carries on");
    }
}

#[test]
fn a_non_retryable_failure_stops_autosave_and_shows_the_hosts_message() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "T", "");
    app.host_tick(&page.ctx, 0.0);
    type_text(&mut app, "a");
    app.host_observe(0.0);
    app.host_tick(&page.ctx, 3_000.0);
    page.host.answer(0, SaveOutcome::Failed { retryable: false, message: "Tài liệu vượt quá dung lượng cho phép.".into() });
    app.host_tick(&page.ctx, 3_100.0);
    assert!(app.session.dirty);
    assert_eq!(app.save_status(), Some(crate::save_status::SaveStatus::Failed("Tài liệu vượt quá dung lượng cho phép.")));
    assert_eq!(app.status_msg.as_ref().map(|(m, _)| m.as_str()), Some("Tài liệu vượt quá dung lượng cho phép."));
    type_text(&mut app, "b");
    app.host_observe(3_200.0);
    app.host_tick(&page.ctx, 100_000.0);
    assert_eq!(page.host.sends().len(), 1, "AutoSave stopped");
    app.run("file.save", json!({})).unwrap();
    assert_eq!(page.host.sends().len(), 2, "Ctrl+S still asks the host");
    page.host.answer(1, SaveOutcome::Saved { version: 2 });
    app.host_tick(&page.ctx, 100_100.0);
    assert!(!app.session.dirty);
    assert_eq!(app.save_status(), None);
}

#[test]
fn flush_resolves_once_every_unacknowledged_document_is_handed_to_the_host() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "A", "");
    hand_in(&page, "d2", "B", "");
    hand_in(&page, "d3", "C", "");
    app.host_tick(&page.ctx, 0.0);
    type_text(&mut app, "c");
    app.run("ui.activateDocument", json!({"index": 0})).unwrap();
    type_text(&mut app, "a");
    app.run("file.save", json!({})).unwrap();
    type_text(&mut app, "a");
    app.host_observe(10.0);
    let flushed = Rc::new(RefCell::new(0));
    let f = flushed.clone();
    page.link.flush(Box::new(move || *f.borrow_mut() += 1));
    assert_eq!(
        page.host.sends().iter().map(|(d, _, r)| (d.as_str(), *r)).collect::<Vec<_>>(),
        vec![("d1", SaveReason::Manual), ("d3", SaveReason::Flush)],
        "d1 waits behind its save in flight; d2 is clean"
    );
    assert_eq!(*flushed.borrow(), 0, "d1's newest revision isn't handed over yet");
    page.host.answer(0, SaveOutcome::Offline);
    assert_eq!(page.host.sends().get(2).map(|s| (s.0.as_str(), s.2)), Some(("d1", SaveReason::Flush)));
    assert_eq!(*flushed.borrow(), 1, "resolved without waiting for any answer to the flush's saves");
    page.host.answer(2, SaveOutcome::Saved { version: 3 });
    app.host_tick(&page.ctx, 20.0);
    assert!(!app.session.dirty);
}

#[test]
fn a_signed_host_document_is_neither_autosaved_nor_flushed() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "A", "");
    app.host_tick(&page.ctx, 0.0);
    app.session.doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
    type_text(&mut app, "x");
    app.host_observe(0.0);
    app.host_tick(&page.ctx, 100_000.0);
    let flushed = Rc::new(RefCell::new(false));
    let f = flushed.clone();
    page.link.flush(Box::new(move || *f.borrow_mut() = true));
    assert!(*flushed.borrow(), "skipped: saving drops the signature, which only the user may confirm");
    assert!(page.host.sends().is_empty());
    assert!(app.session.dirty);
}

#[test]
fn acknowledged_after_an_offline_save_cleans_as_of_that_revision() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "T", "");
    app.host_tick(&page.ctx, 0.0);
    page.host.take_calls();
    type_text(&mut app, "a");
    app.host_observe(0.0);
    app.host_tick(&page.ctx, 3_000.0);
    let first = app.session.rev();
    page.host.answer(0, SaveOutcome::Offline);
    type_text(&mut app, "b");
    app.host_observe(3_100.0);
    page.link.acknowledged("d1", first, 2);
    app.host_tick(&page.ctx, 3_200.0);
    assert!(app.session.dirty, "b was typed after the save was sent");
    assert_eq!(page.host.take_calls(), vec!["dirty d1 true"]);

    app.host_tick(&page.ctx, 6_100.0);
    assert_eq!(page.host.sends().len(), 2);
    let second = app.session.rev();
    page.host.answer(1, SaveOutcome::Failed { retryable: true, message: "Máy chủ bận".into() });
    page.link.acknowledged("d1", second, 3);
    app.host_tick(&page.ctx, 6_200.0);
    assert!(!app.session.dirty);
    assert_eq!(page.host.take_calls(), vec!["dirty d1 false"]);

    page.link.acknowledged("d1", first, 2);
    page.link.acknowledged("other", second, 9);
    app.host_tick(&page.ctx, 6_300.0);
    assert_eq!(page.link.with_saves(|s| s.by_doc_id("d1").map(|d| (d.acked, d.version))).flatten(), Some((second, 3)), "an older one is ignored");
    assert!(page.host.take_calls().is_empty());
}

#[test]
fn the_leave_page_guard_counts_local_documents_only() {
    let (mut app, page) = web_app();
    let seen = Rc::new(RefCell::new(None));
    let s = seen.clone();
    app.services.on_dirty = Some(Box::new(move |d| *s.borrow_mut() = Some(d)));
    hand_in(&page, "d1", "A", "");
    app.host_tick(&page.ctx, 0.0);
    type_text(&mut app, "host");
    app.report_dirty();
    assert_eq!(*seen.borrow(), Some(false), "the shell guards its own documents");
    app.run("file.new", json!({})).unwrap();
    type_text(&mut app, "local");
    app.report_dirty();
    assert_eq!(*seen.borrow(), Some(true));
}

#[test]
fn a_closed_host_document_is_not_offered_for_reopening() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "A", "");
    app.host_tick(&page.ctx, 0.0);
    app.run("file.new", json!({})).unwrap();
    type_text(&mut app, "local");
    app.run("ui.activateDocument", json!({"index": 0})).unwrap();
    app.run("file.close", json!({})).unwrap();
    app.host_observe(0.0);
    assert_eq!(app.document_count(), 1);
    assert!(!app.can_reopen_document(), "the host holds it");
    app.run("file.close", json!({})).unwrap();
    app.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
    assert!(app.finish_document_close());
    assert!(app.can_reopen_document(), "a local document still is");
}

#[test]
fn host_documents_are_never_drafted_and_local_ones_still_are() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "T", "");
    app.host_tick(&page.ctx, 0.0);
    type_text(&mut app, "host");
    app.run("file.new", json!({})).unwrap();
    type_text(&mut app, "local");
    let local = app.session.document_id();
    for t in [0.0, 5_000.0, 40_000.0, 80_000.0] {
        app.drafts_tick(&page.ctx, t);
    }
    assert_eq!(*page.drafts.borrow(), vec![format!("doc-{local}")]);
    app.run("file.save", json!({})).unwrap();
    assert_eq!(page.downloads.borrow().len(), 1, "a local document still downloads");
    assert_eq!(page.host.sends().len(), 0);
}

#[test]
fn closing_a_host_tab_tells_the_host() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "A", "");
    hand_in(&page, "d2", "B", "");
    app.host_tick(&page.ctx, 0.0);
    page.host.take_calls();
    app.run("file.close", json!({})).unwrap();
    app.host_observe(0.0);
    assert_eq!(page.host.take_calls(), vec!["closed d2"]);
    app.run("file.close", json!({})).unwrap();
    assert!(app.finish_document_close());
    app.host_observe(0.0);
    assert_eq!(page.host.take_calls(), vec!["closed d1"]);
    assert!(!app.host_owns(app.session.document_id()));
}

#[test]
fn save_changes_on_close_waits_for_the_hosts_save() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "A", "");
    hand_in(&page, "d2", "B", "");
    app.host_tick(&page.ctx, 0.0);
    page.host.take_calls();
    type_text(&mut app, "x");
    app.host_observe(0.0);
    assert_eq!(page.host.take_calls(), vec!["dirty d2 true"]);
    app.run("file.close", json!({})).unwrap();
    assert!(matches!(app.dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })));
    app.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert_eq!(page.host.sends().len(), 1);
    assert_eq!(app.document_count(), 2, "open until the host has the save");
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(app.document_count(), 2, "still waiting");
    page.host.answer(0, SaveOutcome::Saved { version: 2 });
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(app.document_count(), 1);
    assert_eq!(page.host.take_calls(), vec!["closed d2"]);

    type_text(&mut app, "y");
    app.run("file.close", json!({})).unwrap();
    app.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    page.host.answer(1, SaveOutcome::Offline);
    app.host_tick(&page.ctx, 0.0);
    assert!(app.has_document() && !app.quit_requested, "an unsaved document stays open");
    assert!(app.session.dirty);
}

#[test]
fn save_as_of_a_host_document_downloads_a_copy_and_leaves_it_unsaved() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "Báo cáo", "");
    app.host_tick(&page.ctx, 0.0);
    type_text(&mut app, "x");
    app.run("file.saveAs", json!({})).unwrap();
    assert_eq!(*page.downloads.borrow(), vec!["Báo cáo.docx".to_string()]);
    assert!(app.session.dirty, "only the host's save makes it saved");
    assert!(page.host.sends().is_empty());
    app.run("file.exportPdf", json!({})).unwrap();
    assert_eq!(page.downloads.borrow().len(), 2);
    assert!(page.host.sends().is_empty());
}

#[test]
fn reopening_the_same_document_replaces_it_in_its_tab() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "A", "mine");
    hand_in(&page, "d2", "B", "other");
    app.host_tick(&page.ctx, 0.0);
    app.run("ui.activateDocument", json!({"index": 0})).unwrap();
    type_text(&mut app, "edit ");
    app.host_observe(0.0);
    app.run("ui.activateDocument", json!({"index": 1})).unwrap();
    page.host.take_calls();
    page.link.open(HostOpen { doc_id: "d1".into(), version: 5, title: "A".into(), format: "docx".into(), bytes: docx("theirs") });
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(page.host.take_calls(), vec!["opened d1 true ", "dirty d1 false"]);
    assert_eq!(app.document_count(), 2);
    assert!(body(&app).contains("other"), "the shown document stays");
    app.run("ui.activateDocument", json!({"index": 0})).unwrap();
    assert!(body(&app).contains("theirs") && !body(&app).contains("mine"));
    assert!(!app.session.dirty);
    type_text(&mut app, "x");
    app.run("file.save", json!({})).unwrap();
    assert_eq!(page.host.sends().len(), 1);
    assert_eq!(page.host.sends()[0].0, "d1");
}

#[test]
fn without_a_host_save_and_drafts_behave_as_before() {
    let (mut app, page) = web_app();
    app.services.host = None;
    app.run("file.new", json!({})).unwrap();
    type_text(&mut app, "local");
    assert!(!app.host_owns(app.session.document_id()));
    let r = app.run("file.save", json!({})).unwrap();
    assert!(r.get("downloaded").is_some());
    assert!(!app.session.dirty);
    hand_in(&page, "d1", "A", "");
    app.host_tick(&page.ctx, 0.0);
    assert!(page.host.take_calls().is_empty(), "nothing opens without the hook");

    // Desktop: no download, no host; Save writes the file.
    let dir = std::env::temp_dir().join(format!("wordcraft-host-desktop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut desktop = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
    type_text(&mut desktop, "x");
    let path = dir.join("a.docx");
    desktop.run("file.save", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(path.exists() && !desktop.session.dirty);
    let _ = std::fs::remove_dir_all(&dir);
}
