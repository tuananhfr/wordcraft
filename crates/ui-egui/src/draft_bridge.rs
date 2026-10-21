//! Web: the open documents' drafts in the page's draft storage (the ERPCons office shell's
//! `window.erpDrafts`). The host hands in a [`DraftBridge`]; [`drafts::Planner`] decides when to
//! put and remove; this module serializes, adopts restored drafts and keeps the snapshots the
//! page's hide/pagehide flush ([`flush_hidden`]) serializes without reaching into the app.

use std::cell::RefCell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;

use serde_json::json;
use wordcraft_engine::Session;

use crate::WordApp;
use crate::drafts::{Observed, Planner, Version};
use crate::open_documents::save_name_of;

/// What a draft's bytes are: the same Word package Ctrl+S writes.
pub const FORMAT: &str = "docx";

/// While a draft waits for its moment, the app ticks this often without input.
const WAIT_TICK: std::time::Duration = std::time::Duration::from_millis(1000);

/// `put(key, name, format, bytes)`.
pub type PutDraft = Box<dyn Fn(&str, &str, &str, &[u8]) -> Result<(), String>>;

/// The page's draft storage ([`crate::Services::drafts`]).
pub struct DraftBridge {
    /// Desktop storage restores copies through Save As; the web shell owns its recovery UI.
    #[cfg(not(target_arch = "wasm32"))]
    pub desktop_recovery: bool,
    /// Whether the page offers draft storage right now; asked every frame, so it must be cheap.
    pub available: Box<dyn Fn() -> bool>,
    pub put: PutDraft,
    pub remove: Box<dyn Fn(&str) -> Result<(), String>>,
    /// Shared with the host's hide/pagehide listeners.
    pub hidden: Rc<RefCell<HiddenFlush>>,
    /// Drafts the user chose to restore, opened on the next frames.
    pub restores: Rc<RefCell<Vec<RestoredDraft>>>,
}

/// A draft the page hands back to open.
pub struct RestoredDraft {
    pub key: String,
    pub name: String,
    pub format: String,
    pub bytes: Vec<u8>,
}

/// What a flush when the page is hidden needs, refreshed by the app at the end of every frame:
/// each unsaved document's latest undrafted state (a cheap clone: blocks are shared), and what the
/// last flush put, for the app to pick up.
#[derive(Default)]
pub struct HiddenFlush {
    snapshots: Vec<Snapshot>,
    flushed: Vec<Flushed>,
}

struct Snapshot {
    key: String,
    name: String,
    version: Version,
    doc: wordcraft_doc::Document,
    password: Option<wordcraft_engine::Password>,
    opts: wordcraft_engine::io::ExportOptions,
}

struct Flushed {
    key: String,
    version: Version,
    at: f64,
    serialize_ms: f64,
}

/// The page is being hidden or left: put the unsaved documents now, without waiting for a pause.
/// Called by the host outside any frame.
pub fn flush_hidden(cell: &RefCell<HiddenFlush>, put: &dyn Fn(&str, &str, &str, &[u8]) -> Result<(), String>) {
    let Ok(mut cell) = cell.try_borrow_mut() else { return };
    for s in std::mem::take(&mut cell.snapshots) {
        let t0 = crate::now_ms();
        let password = s.password.as_ref().map(wordcraft_engine::Password::as_str);
        match wordcraft_engine::io::save_bytes_with(&s.name, &s.doc, password, &s.opts).and_then(|bytes| put(&s.key, &s.name, FORMAT, &bytes)) {
            Ok(()) => {
                let at = crate::now_ms();
                cell.flushed.push(Flushed { key: s.key, version: s.version, at, serialize_ms: at - t0 });
            }
            Err(e) => log::warn!("couldn't keep a draft of {}: {e}", s.name),
        }
    }
}

/// The app's side of drafting.
#[derive(Debug, Default)]
pub(crate) struct DraftState {
    planner: Planner,
    #[cfg(not(target_arch = "wasm32"))]
    recovered: usize,
    #[cfg(not(target_arch = "wasm32"))]
    recovery_errors: Vec<String>,
    /// A restored draft waiting for its password (the open happens in the Password dialog).
    restoring: Option<PendingRestore>,
}

#[derive(Debug)]
struct PendingRestore {
    key: String,
    name: String,
    /// The document shown when the restore began; another one shown means the restore's.
    document: u64,
}

/// A draft's version of `session`'s document: its id and revision, and what its name and bytes
/// depend on besides.
fn draft_version(session: &Session, untitled: u32) -> Version {
    let mut meta = std::collections::hash_map::DefaultHasher::new();
    save_name_of(session, untitled).hash(&mut meta);
    session.password.is_some().hash(&mut meta);
    Version { document: session.document_id(), rev: session.rev(), meta: meta.finish() }
}

impl WordApp {
    /// Keep startup failures visible alongside successful recoveries.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn report_desktop_recovery_errors(&mut self, errors: Vec<String>) {
        self.drafts.recovery_errors.extend(errors);
        self.desktop_recovery_status();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn desktop_recovery_status(&mut self) {
        let mut message = crate::i18n::fmt(tl!("Recovered {n} unsaved documents"), &[("n", &self.drafts.recovered.to_string())]);
        if !self.drafts.recovery_errors.is_empty() {
            message.push('\n');
            message.push_str(&self.drafts.recovery_errors.join("\n"));
        }
        self.status(message);
    }

    /// Remove recovery copies only after the user has approved desktop shutdown.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn discard_desktop_drafts(&mut self) {
        let plan = self.drafts.planner.plan(crate::now_ms(), &[]);
        if let Some(bridge) = &self.services.drafts {
            for key in plan.remove {
                if let Err(e) = (bridge.remove)(&key) {
                    log::warn!("couldn't remove recovery draft: {e}");
                }
            }
        }
    }

    /// Draft the open documents when they are due, remove drafts no longer needed and open
    /// restored ones (every frame, from `logic`).
    pub(crate) fn drafts_tick(&mut self, ctx: &egui::Context, now: f64) {
        let Some(bridge) = &self.services.drafts else { return };
        if !(bridge.available)() {
            return;
        }
        let flushed = bridge.hidden.try_borrow_mut().map(|mut c| std::mem::take(&mut c.flushed)).unwrap_or_default();
        for f in flushed {
            self.drafts.planner.put_done(&f.key, f.version, f.at, f.serialize_ms);
        }
        self.open_restored_draft(ctx, now);
        let plan = self.drafts.planner.plan(now, &self.draft_observed());
        if let Some(bridge) = &self.services.drafts {
            for key in &plan.remove {
                if let Err(e) = (bridge.remove)(key) {
                    log::warn!("couldn't remove the draft {key}: {e}");
                }
            }
        }
        for put in &plan.put {
            self.put_draft(put.document, &put.key, now);
        }
        if plan.wait {
            ctx.request_repaint_after(WAIT_TICK);
        }
    }

    /// Keep what a flush when the page is hidden would put up to date (end of every frame).
    pub(crate) fn refresh_draft_snapshot(&self) {
        let Some(bridge) = &self.services.drafts else { return };
        let due: Vec<(String, Version)> = if (bridge.available)() {
            self.draft_observed().into_iter().filter_map(|o| self.drafts.planner.flush_due(o).map(|key| (key, o.version))).collect()
        } else {
            Vec::new()
        };
        let Ok(mut cell) = bridge.hidden.try_borrow_mut() else { return };
        let mut kept = std::mem::take(&mut cell.snapshots);
        for (key, version) in due {
            if let Some(i) = kept.iter().position(|s| s.version == version && s.key == key) {
                cell.snapshots.push(kept.swap_remove(i));
            } else if let Some((session, untitled)) = self.find_document(version.document) {
                cell.snapshots.push(Snapshot {
                    key,
                    name: save_name_of(session, untitled),
                    version,
                    doc: session.doc.clone(),
                    password: session.password.clone(),
                    opts: wordcraft_engine::cmd::file::export_options(&self.session, &json!({})),
                });
            }
        }
    }

    /// Every open document, shown or not. Unsaved: dirty, or a save of it still in the browser's
    /// save picker.
    fn draft_observed(&self) -> Vec<Observed> {
        if !self.has_document() {
            return Vec::new();
        }
        let shown = self.session.document_id();
        self.document_tabs()
            .iter()
            .filter_map(|t| self.find_document(t.document))
            .map(|(session, untitled)| Observed {
                version: draft_version(session, untitled),
                unsaved: session.dirty || self.picker_save_pending(session.document_id()),
                transient: session.document_id() == shown && !self.canvas.ime_preedit.is_empty(),
            })
            .collect()
    }

    /// Serialize document `document` as Ctrl+S does, minus `stamp_save` (a draft must not change
    /// the document), and encrypted with its password when it has one. Whether the draft was kept.
    fn put_draft(&mut self, document: u64, key: &str, now: f64) -> bool {
        let Some(bridge) = &self.services.drafts else { return false };
        let Some((session, untitled)) = self.find_document(document) else { return false };
        let name = save_name_of(session, untitled);
        let version = draft_version(session, untitled);
        let password = session.password.as_ref().map(wordcraft_engine::Password::as_str);
        // Print Markup and the markup options are the user's, so the shown session has them.
        let opts = wordcraft_engine::cmd::file::export_options(&self.session, &json!({}));
        let t0 = crate::now_ms();
        let put =
            wordcraft_engine::io::save_bytes_with(&name, &session.doc, password, &opts).and_then(|bytes| (bridge.put)(key, &name, FORMAT, &bytes));
        let serialize_ms = crate::now_ms() - t0;
        match put {
            Ok(()) => {
                self.drafts.planner.put_done(key, version, now, serialize_ms);
                true
            }
            Err(e) => {
                log::warn!("couldn't keep a draft of {name}: {e}");
                self.drafts.planner.put_failed(document, now);
                false
            }
        }
    }

    /// Open the next restored draft, one per frame, after any still waiting for its password.
    fn open_restored_draft(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(p) = self.drafts.restoring.take() {
            if self.session.document_id() != p.document {
                self.adopt_restored(p.key, p.name, now);
            } else if matches!(self.dialog, Some(crate::dialogs::Dialog::Password { .. })) {
                self.drafts.restoring = Some(p);
                return;
            }
            // Otherwise the password was cancelled: the draft stays with the page.
        }
        let Some(bridge) = &self.services.drafts else { return };
        let (next, more) = match bridge.restores.try_borrow_mut() {
            Ok(mut queue) if !queue.is_empty() => {
                let next = queue.remove(0);
                (next, !queue.is_empty())
            }
            _ => return,
        };
        if more {
            ctx.request_repaint();
        }
        self.restore_draft(next, now);
    }

    /// Open a restored draft as a file drop would: beside the open documents, which keep their
    /// own drafts (an untouched blank start document is replaced). Refused past the open
    /// documents limit, when the draft stays with the page.
    fn restore_draft(&mut self, draft: RestoredDraft, now: f64) {
        if draft.format != FORMAT {
            log::warn!("the draft {} is {}, not a Word document; not restored", draft.key, draft.format);
            #[cfg(not(target_arch = "wasm32"))]
            self.report_desktop_recovery_errors(vec![crate::i18n::fmt(
                tl!("Could not recover {name}: {error}"),
                &[("name", &draft.name), ("error", &draft.format)],
            )]);
            return;
        }
        // The bytes are a Word package whatever the name says; the engine reads the name's format.
        let ext = std::path::Path::new(&draft.name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let path = if wordcraft_engine::io::is_word_package(&ext) { draft.name.clone() } else { format!("{}.{FORMAT}", draft.name) };
        let document = self.session.document_id();
        let params = json!({"path": path, "data": wordcraft_engine::cmd::insert::base64_encode(&draft.bytes)});
        match self.execute_user("file.open", params) {
            Ok(v) if v.get("pending").is_some_and(|p| !p.is_null()) => {
                self.drafts.restoring = Some(PendingRestore { key: draft.key, name: draft.name, document });
            }
            Ok(_) => self.adopt_restored(draft.key, draft.name, now),
            Err(e) => {
                log::warn!("couldn't restore the draft {}: {e}", draft.name);
                #[cfg(not(target_arch = "wasm32"))]
                self.report_desktop_recovery_errors(vec![crate::i18n::fmt(
                    tl!("Could not recover {name}: {error}"),
                    &[("name", &draft.name), ("error", &e)],
                )]);
            }
        }
    }

    /// The restored document is unsaved, named as the draft was, and already drafted under its key.
    fn adopt_restored(&mut self, key: String, name: String, now: f64) {
        #[cfg(target_arch = "wasm32")]
        {
            self.session.path = Some(name.into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.services.drafts.as_ref().is_some_and(|b| b.desktop_recovery) {
            let stem = std::path::Path::new(&name).file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
            let title = crate::i18n::fmt(tl!("Recovered — {name}"), &[("name", &stem)]);
            self.session.path = None;
            self.session.suggested_save_path = Some(format!("{title}.docx").into());
            self.autosave_path = None;
            self.drafts.recovered = self.drafts.recovered.saturating_add(1);
            self.desktop_recovery_status();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !self.services.drafts.as_ref().is_some_and(|b| b.desktop_recovery) {
            self.session.path = Some(name.into());
        }
        self.session.dirty = true;
        self.ui.backstage = false;
        let version = draft_version(&self.session, self.untitled);
        self.drafts.planner.adopt(key, version, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use wordcraft_engine::Session;

    type Calls = Rc<RefCell<Vec<(String, String, String, Vec<u8>)>>>;

    struct Page {
        puts: Calls,
        removes: Rc<RefCell<Vec<String>>>,
        hidden: Rc<RefCell<HiddenFlush>>,
        restores: Rc<RefCell<Vec<RestoredDraft>>>,
    }

    fn app_with_page() -> (WordApp, Page) {
        let puts: Calls = Rc::default();
        let removes: Rc<RefCell<Vec<String>>> = Rc::default();
        let hidden: Rc<RefCell<HiddenFlush>> = Rc::default();
        let restores: Rc<RefCell<Vec<RestoredDraft>>> = Rc::default();
        let (p, r) = (puts.clone(), removes.clone());
        let bridge = DraftBridge {
            #[cfg(not(target_arch = "wasm32"))]
            desktop_recovery: false,
            available: Box::new(|| true),
            put: Box::new(move |key, name, format, bytes| {
                p.borrow_mut().push((key.into(), name.into(), format.into(), bytes.to_vec()));
                Ok(())
            }),
            remove: Box::new(move |key| {
                r.borrow_mut().push(key.into());
                Ok(())
            }),
            hidden: hidden.clone(),
            restores: restores.clone(),
        };
        let app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services { drafts: Some(bridge), ..Default::default() });
        (app, Page { puts, removes, hidden, restores })
    }

    fn type_text(app: &mut WordApp, text: &str) {
        let r = app.execute("text.insert", json!({ "text": text }));
        assert!(r.is_ok(), "{r:?}");
    }

    fn docx(text: &str) -> Vec<u8> {
        let doc = wordcraft_doc::Document::from_text(text);
        wordcraft_engine::io::save_bytes_with("x.docx", &doc, None, &Default::default()).unwrap_or_default()
    }

    #[test]
    fn an_edited_document_is_put_after_a_pause_and_removed_once_saved() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        app.drafts_tick(&ctx, 0.0);
        type_text(&mut app, "Hello");
        app.drafts_tick(&ctx, 1_000.0);
        assert!(page.puts.borrow().is_empty());
        let modified = app.session.doc.core.modified.clone();
        app.drafts_tick(&ctx, 3_600.0);
        {
            let puts = page.puts.borrow();
            assert_eq!(puts.len(), 1);
            let (key, name, format, bytes) = &puts[0];
            assert_eq!(key, &format!("doc-{}", app.session.document_id()));
            assert_eq!(name, "Document1.docx");
            assert_eq!(format, "docx");
            assert!(bytes.starts_with(b"PK"), "a Word package");
        }
        assert_eq!(app.session.doc.core.modified, modified, "drafting doesn't stamp a save");
        assert!(app.session.dirty);
        app.session.dirty = false;
        app.drafts_tick(&ctx, 4_000.0);
        app.drafts_tick(&ctx, 5_000.0);
        assert_eq!(page.removes.borrow().len(), 1);
    }

    #[test]
    fn a_protected_document_is_drafted_encrypted() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        type_text(&mut app, "Secret");
        assert!(app.execute("file.encrypt", json!({"password": "pw-123"})).is_ok());
        app.drafts_tick(&ctx, 0.0);
        app.drafts_tick(&ctx, 3_000.0);
        let puts = page.puts.borrow();
        let bytes = puts.last().map(|p| p.3.clone()).unwrap_or_default();
        assert!(bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]), "an encrypted (compound file) package");
    }

    #[test]
    fn a_composition_holds_the_draft_back() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        type_text(&mut app, "Vi");
        app.drafts_tick(&ctx, 0.0);
        app.canvas.ime_preedit = "ệ".into();
        app.drafts_tick(&ctx, 5_000.0);
        assert!(page.puts.borrow().is_empty());
        app.canvas.ime_preedit.clear();
        app.drafts_tick(&ctx, 5_100.0);
        assert_eq!(page.puts.borrow().len(), 1);
    }

    #[test]
    fn hiding_the_page_puts_at_once_and_counts_as_the_draft() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        app.drafts_tick(&ctx, 0.0);
        type_text(&mut app, "Quick");
        app.refresh_draft_snapshot();
        flush_hidden(&page.hidden, &|key, name, format, bytes| {
            page.puts.borrow_mut().push((key.into(), name.into(), format.into(), bytes.to_vec()));
            Ok(())
        });
        assert_eq!(page.puts.borrow().len(), 1);
        app.drafts_tick(&ctx, 10_000.0);
        assert_eq!(page.puts.borrow().len(), 1, "the flushed version isn't put again");
        app.refresh_draft_snapshot();
        flush_hidden(&page.hidden, &|_, _, _, _| Err("nothing new to flush".into()));
        app.session.dirty = false;
        app.drafts_tick(&ctx, 11_000.0);
        assert_eq!(page.removes.borrow().len(), 1);
    }

    #[test]
    fn a_restored_draft_opens_unsaved_under_its_key() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        assert!(app.start_screen());
        page.restores.borrow_mut().push(RestoredDraft {
            key: "tab-a/3".into(),
            name: "Report.docx".into(),
            format: "docx".into(),
            bytes: docx("Recovered"),
        });
        app.drafts_tick(&ctx, 0.0);
        assert!(app.has_document());
        assert!(!app.ui.backstage);
        assert!(app.session.dirty);
        assert_eq!(app.session.path.as_deref(), Some(std::path::Path::new("Report.docx")));
        assert!(app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("Recovered"));
        assert!(app.dialog.is_none(), "no Save changes prompt");
        app.drafts_tick(&ctx, 60_000.0);
        assert!(page.puts.borrow().is_empty(), "already drafted");
        type_text(&mut app, "More ");
        app.drafts_tick(&ctx, 61_000.0);
        app.drafts_tick(&ctx, 64_000.0);
        assert_eq!(page.puts.borrow().last().map(|p| (p.0.clone(), p.1.clone())), Some(("tab-a/3".to_string(), "Report.docx".to_string())));
    }

    #[test]
    fn other_formats_are_not_restored() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        let document = app.session.document_id();
        page.restores.borrow_mut().push(RestoredDraft { key: "k".into(), name: "Book1.xlsx".into(), format: "xlsx".into(), bytes: vec![1, 2, 3] });
        app.drafts_tick(&ctx, 0.0);
        assert_eq!(app.session.document_id(), document);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn desktop_startup_recovers_copies_and_shutdown_removes_their_drafts() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        app.services.drafts.as_mut().unwrap().desktop_recovery = true;
        for name in ["/real/Report.docx", "Other.docx"] {
            page.restores.borrow_mut().push(RestoredDraft {
                key: name.into(),
                name: name.into(),
                format: FORMAT.into(),
                bytes: docx("Recovered content"),
            });
        }
        app.drafts_tick(&ctx, 0.0);
        assert!(app.session.path.is_none());
        assert!(app.autosave_path.is_none());
        assert_eq!(app.title_stem(), "Recovered — Report");
        assert!(app.session.dirty);
        app.drafts_tick(&ctx, 1.0);
        assert_eq!(app.document_count(), 2);
        assert!(app.document_tabs().iter().all(|t| t.title.starts_with("Recovered — ") && t.dirty));
        assert_eq!(app.status_msg.as_ref().map(|s| s.0.as_str()), Some("Recovered 2 unsaved documents"));
        app.execute_user("file.save", json!({})).unwrap();
        assert!(app.session.path.is_none(), "Save asks for a destination rather than overwriting the source");
        app.discard_desktop_drafts();
        assert_eq!(page.removes.borrow().len(), 2);
        app.discard_desktop_drafts();
        assert_eq!(page.removes.borrow().len(), 2);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn desktop_corrupt_recovery_reports_failure_and_keeps_the_draft() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        app.services.drafts.as_mut().unwrap().desktop_recovery = true;
        let document = app.session.document_id();
        page.restores.borrow_mut().push(RestoredDraft {
            key: "broken".into(),
            name: "Broken.docx".into(),
            format: FORMAT.into(),
            bytes: vec![0, 1, 2],
        });
        app.drafts_tick(&ctx, 0.0);
        assert_eq!(app.session.document_id(), document);
        assert!(app.status_msg.as_ref().is_some_and(|s| s.0.contains("Could not recover Broken.docx")));
        assert!(page.removes.borrow().is_empty());
    }

    #[test]
    fn without_draft_storage_nothing_happens() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        if let Some(b) = app.services.drafts.as_mut() {
            b.available = Box::new(|| false);
        }
        type_text(&mut app, "Hello");
        app.drafts_tick(&ctx, 0.0);
        app.drafts_tick(&ctx, 10_000.0);
        app.refresh_draft_snapshot();
        flush_hidden(&page.hidden, &|_, _, _, _| Err("must not be called".into()));
        assert!(page.puts.borrow().is_empty());
    }

    #[test]
    fn restoring_next_to_an_unsaved_document_opens_a_tab_and_keeps_its_draft() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        app.drafts_tick(&ctx, 0.0);
        type_text(&mut app, "Mine");
        let mine = app.session.document_id();
        page.restores.borrow_mut().push(RestoredDraft {
            key: "tab-b/1".into(),
            name: "Old.docx".into(),
            format: "docx".into(),
            bytes: docx("Theirs"),
        });
        app.drafts_tick(&ctx, 500.0);
        assert!(app.dialog.is_none(), "nothing to ask: the unsaved document stays open");
        assert_eq!(app.document_count(), 2);
        assert!(app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("Theirs"));
        assert!(app.session.dirty, "the restored draft opens unsaved");
        assert_ne!(app.session.document_id(), mine, "a document id of its own");
        assert!(app.find_document(mine).is_some_and(|(s, _)| s.dirty && s.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("Mine")));
        app.drafts_tick(&ctx, 3_000.0);
        assert_eq!(
            page.puts.borrow().iter().map(|p| p.0.clone()).collect::<Vec<_>>(),
            [format!("doc-{mine}")],
            "the one in the background is drafted too"
        );
        assert!(page.removes.borrow().is_empty());
    }

    /// Restored untitled drafts keep their names, so a new document counts on past them rather
    /// than taking "Document2" a second time; like Word, a closed one's number isn't reused.
    #[test]
    fn a_new_document_is_never_titled_like_an_open_one() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        for (key, name) in [("t|doc-10", "Document2.docx"), ("t|doc-12", "Document1.docx")] {
            page.restores.borrow_mut().push(RestoredDraft { key: key.into(), name: name.into(), format: "docx".into(), bytes: docx(name) });
        }
        app.drafts_tick(&ctx, 0.0);
        app.drafts_tick(&ctx, 100.0);
        let titles = |app: &WordApp| app.document_tabs().iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&app), ["Document2", "Document1"]);
        app.run("file.new", json!({})).unwrap();
        assert_eq!(titles(&app), ["Document2", "Document1", "Document3"]);
        type_text(&mut app, "New");
        app.drafts_tick(&ctx, 1_000.0);
        app.drafts_tick(&ctx, 4_000.0);
        assert!(page.puts.borrow().iter().any(|p| p.1 == "Document3.docx"), "drafted under its own name too");

        app.execute("ui.closeDocument", json!({"index": 1})).unwrap();
        app.run("file.new", json!({})).unwrap();
        assert_eq!(titles(&app), ["Document2", "Document3", "Document4"], "Document1's number isn't reused");
    }

    /// QA repro: "Khôi phục tất cả" of Tài liệu1 and Tài liệu2, then File › New.
    #[test]
    fn a_new_document_counts_past_restored_vietnamese_titles() {
        crate::i18n::set_current(crate::i18n::Lang::from_code("vi").unwrap());
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        for (key, name) in [("t|doc-12", "Tài liệu2.docx"), ("t|doc-10", "Tài liệu1.docx")] {
            page.restores.borrow_mut().push(RestoredDraft { key: key.into(), name: name.into(), format: "docx".into(), bytes: docx(name) });
        }
        app.drafts_tick(&ctx, 0.0);
        app.drafts_tick(&ctx, 100.0);
        app.run("file.new", json!({})).unwrap();
        let titles: Vec<String> = app.document_tabs().iter().map(|t| t.title.clone()).collect();
        crate::i18n::set_current(crate::i18n::Lang::EN);
        assert_eq!(titles, ["Tài liệu2", "Tài liệu1", "Tài liệu3"]);
    }

    #[test]
    fn restoring_several_drafts_opens_a_tab_for_each() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        assert!(app.start_screen());
        for (key, name) in [("t|doc-1", "A.docx"), ("t|doc-2", "B.docx"), ("t|doc-3", "C.docx")] {
            page.restores.borrow_mut().push(RestoredDraft { key: key.into(), name: name.into(), format: "docx".into(), bytes: docx(name) });
        }
        for t in 0..3 {
            app.drafts_tick(&ctx, f64::from(t) * 100.0);
        }
        let tabs = app.document_tabs();
        assert_eq!(tabs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["A", "B", "C"], "the start screen's placeholder was replaced");
        assert!(tabs.iter().all(|t| t.dirty));
        let mut ids: Vec<u64> = tabs.iter().map(|t| t.document).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3, "unique document ids");
        // Each is drafted under the key it came back with, and only once edited again.
        app.drafts_tick(&ctx, 60_000.0);
        assert!(page.puts.borrow().is_empty());
        assert!(app.execute("ui.activateDocument", json!({"index": 0})).is_ok());
        type_text(&mut app, "More ");
        app.drafts_tick(&ctx, 61_000.0);
        app.drafts_tick(&ctx, 64_000.0);
        assert_eq!(
            page.puts.borrow().iter().map(|p| (p.0.clone(), p.1.clone())).collect::<Vec<_>>(),
            [("t|doc-1".to_string(), "A.docx".to_string())]
        );
        // Closing one removes its draft only.
        assert!(app.execute("ui.closeDocument", json!({"index": 1})).is_ok());
        app.drafts_tick(&ctx, 65_000.0);
        assert_eq!(*page.removes.borrow(), ["t|doc-2".to_string()]);
    }

    #[test]
    fn every_open_document_is_drafted_under_its_own_key() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        app.drafts_tick(&ctx, 0.0);
        type_text(&mut app, "First");
        let first = app.session.document_id();
        assert!(app.execute("file.new", json!({})).is_ok());
        type_text(&mut app, "Second");
        let second = app.session.document_id();
        assert_ne!(first, second);
        app.drafts_tick(&ctx, 500.0);
        app.drafts_tick(&ctx, 3_000.0);
        let mut keys: Vec<(String, String)> = page.puts.borrow().iter().map(|p| (p.0.clone(), p.1.clone())).collect();
        keys.sort();
        let mut want = vec![(format!("doc-{first}"), "Document1.docx".to_string()), (format!("doc-{second}"), "Document2.docx".to_string())];
        want.sort();
        assert_eq!(keys, want, "the document in the background is drafted, under its own name");
        // Saving the background one (a picker that writes at once) removes only its draft.
        assert!(app.execute("ui.activateDocument", json!({"document": first})).is_ok());
        app.session.dirty = false;
        app.drafts_tick(&ctx, 4_000.0);
        assert_eq!(*page.removes.borrow(), [format!("doc-{first}")]);
        // Closing the other one, unsaved, removes its draft too.
        assert!(app.execute("ui.closeDocument", json!({"document": second})).is_ok());
        app.drafts_tick(&ctx, 5_000.0);
        assert_eq!(*page.removes.borrow(), [format!("doc-{first}"), format!("doc-{second}")]);
    }

    #[test]
    fn hiding_the_page_puts_every_unsaved_document() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        app.drafts_tick(&ctx, 0.0);
        type_text(&mut app, "One");
        assert!(app.execute("file.new", json!({})).is_ok());
        assert!(app.execute("file.new", json!({})).is_ok());
        type_text(&mut app, "Three");
        app.refresh_draft_snapshot();
        flush_hidden(&page.hidden, &|key, name, format, bytes| {
            page.puts.borrow_mut().push((key.into(), name.into(), format.into(), bytes.to_vec()));
            Ok(())
        });
        let mut names: Vec<String> = page.puts.borrow().iter().map(|p| p.1.clone()).collect();
        names.sort();
        assert_eq!(names, ["Document1.docx", "Document3.docx"], "the untouched second one has nothing to keep");
        app.drafts_tick(&ctx, 10_000.0);
        assert_eq!(page.puts.borrow().len(), 2, "the flushed versions aren't put again");
    }

    #[test]
    fn a_restore_past_the_limit_is_refused_and_the_draft_stays() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        type_text(&mut app, "x");
        for _ in 1..crate::open_documents::MAX_DOCUMENTS {
            assert!(app.execute("file.new", json!({})).is_ok());
        }
        page.restores.borrow_mut().push(RestoredDraft {
            key: "t|doc-9".into(),
            name: "Late.docx".into(),
            format: "docx".into(),
            bytes: docx("Late"),
        });
        app.drafts_tick(&ctx, 0.0);
        assert_eq!(app.document_count(), crate::open_documents::MAX_DOCUMENTS);
        assert!(app.document_tabs().iter().all(|t| t.title != "Late"));
        assert!(app.status_msg.as_ref().is_some_and(|(m, _)| m.contains("up to 10 documents")), "{:?}", app.status_msg);
        assert!(page.removes.borrow().is_empty(), "the shell keeps the draft");
    }
}
