//! Web: the open document's draft in the page's draft storage (the ERPCons office shell's
//! `window.erpDrafts`). The host hands in a [`DraftBridge`]; [`drafts::Planner`] decides when to
//! put and remove; this module serializes, adopts restored drafts and keeps the snapshot the
//! page's hide/pagehide flush ([`flush_hidden`]) serializes without reaching into the app.

use std::cell::RefCell;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;

use serde_json::json;

use crate::WordApp;
use crate::drafts::{Observed, Planner, Version};

/// What a draft's bytes are: the same Word package Ctrl+S writes.
pub const FORMAT: &str = "docx";

/// While a draft waits for its moment, the app ticks this often without input.
const WAIT_TICK: std::time::Duration = std::time::Duration::from_millis(1000);

/// `put(key, name, format, bytes)`.
pub type PutDraft = Box<dyn Fn(&str, &str, &str, &[u8]) -> Result<(), String>>;

/// The page's draft storage ([`crate::Services::drafts`]).
pub struct DraftBridge {
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
/// the unsaved document's latest undrafted state (a cheap clone: blocks are shared), and what the
/// last flush put, for the app to pick up.
#[derive(Default)]
pub struct HiddenFlush {
    snapshot: Option<Snapshot>,
    flushed: Option<Flushed>,
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

/// The page is being hidden or left: put the unsaved document now, without waiting for a pause.
/// Called by the host outside any frame.
pub fn flush_hidden(cell: &RefCell<HiddenFlush>, put: &dyn Fn(&str, &str, &str, &[u8]) -> Result<(), String>) {
    let Ok(mut cell) = cell.try_borrow_mut() else { return };
    let Some(s) = cell.snapshot.take() else { return };
    let t0 = crate::now_ms();
    let password = s.password.as_ref().map(wordcraft_engine::Password::as_str);
    match wordcraft_engine::io::save_bytes_with(&s.name, &s.doc, password, &s.opts).and_then(|bytes| put(&s.key, &s.name, FORMAT, &bytes)) {
        Ok(()) => {
            let at = crate::now_ms();
            cell.flushed = Some(Flushed { key: s.key, version: s.version, at, serialize_ms: at - t0 });
        }
        Err(e) => log::warn!("couldn't keep a draft of {}: {e}", s.name),
    }
}

/// The app's side of drafting.
#[derive(Debug, Default)]
pub(crate) struct DraftState {
    planner: Planner,
    /// A restored draft waiting for its password (the open happens in the Password dialog).
    restoring: Option<PendingRestore>,
}

#[derive(Debug)]
struct PendingRestore {
    key: String,
    name: String,
    /// The document open when the restore began; another one open means the restore's.
    document: u64,
}

impl WordApp {
    /// Draft the open document when it is due, remove drafts no longer needed and open restored
    /// ones (every frame, from `logic`).
    pub(crate) fn drafts_tick(&mut self, ctx: &egui::Context, now: f64) {
        let Some(bridge) = &self.services.drafts else { return };
        if !(bridge.available)() {
            return;
        }
        let flushed = bridge.hidden.try_borrow_mut().ok().and_then(|mut c| c.flushed.take());
        if let Some(f) = flushed {
            self.drafts.planner.put_done(&f.key, f.version, f.at, f.serialize_ms);
        }
        self.open_restored_draft(ctx, now);
        let plan = self.drafts.planner.plan(now, self.draft_observed());
        if let Some(bridge) = &self.services.drafts {
            for key in &plan.remove {
                if let Err(e) = (bridge.remove)(key) {
                    log::warn!("couldn't remove the draft {key}: {e}");
                }
            }
        }
        if let Some(key) = plan.put {
            self.put_draft(&key, now);
        }
        if plan.wait {
            ctx.request_repaint_after(WAIT_TICK);
        }
    }

    /// Keep what a flush when the page is hidden would put up to date (end of every frame).
    pub(crate) fn refresh_draft_snapshot(&self) {
        let Some(bridge) = &self.services.drafts else { return };
        let due = if (bridge.available)() {
            self.draft_observed().and_then(|o| self.drafts.planner.flush_due(o).map(|key| (key, o.version)))
        } else {
            None
        };
        let Ok(mut cell) = bridge.hidden.try_borrow_mut() else { return };
        match due {
            None => cell.snapshot = None,
            Some((key, version)) => {
                if cell.snapshot.as_ref().is_none_or(|s| s.version != version || s.key != key) {
                    cell.snapshot = Some(Snapshot {
                        key,
                        name: self.default_save_name(),
                        version,
                        doc: self.session.doc.clone(),
                        password: self.session.password.clone(),
                        opts: wordcraft_engine::cmd::file::export_options(&self.session, &json!({})),
                    });
                }
            }
        }
    }

    /// Unsaved: dirty, or a save of it still in the browser's save picker.
    fn draft_observed(&self) -> Option<Observed> {
        self.has_document().then(|| Observed {
            version: self.draft_version(),
            unsaved: self.session.dirty || self.picker_save_pending(self.session.document_id()),
            transient: !self.canvas.ime_preedit.is_empty(),
        })
    }

    fn draft_version(&self) -> Version {
        let mut meta = std::collections::hash_map::DefaultHasher::new();
        self.default_save_name().hash(&mut meta);
        self.session.password.is_some().hash(&mut meta);
        Version { document: self.session.document_id(), rev: self.session.rev(), meta: meta.finish() }
    }

    /// Serialize as Ctrl+S does, minus `stamp_save` (a draft must not change the document), and
    /// encrypted with the document's password when it has one. Whether the draft was kept.
    fn put_draft(&mut self, key: &str, now: f64) -> bool {
        let Some(bridge) = &self.services.drafts else { return false };
        let name = self.default_save_name();
        let version = self.draft_version();
        let password = self.session.password.as_ref().map(wordcraft_engine::Password::as_str);
        let opts = wordcraft_engine::cmd::file::export_options(&self.session, &json!({}));
        let t0 = crate::now_ms();
        let put = wordcraft_engine::io::save_bytes_with(&name, &self.session.doc, password, &opts)
            .and_then(|bytes| (bridge.put)(key, &name, FORMAT, &bytes));
        let serialize_ms = crate::now_ms() - t0;
        match put {
            Ok(()) => {
                self.drafts.planner.put_done(key, version, now, serialize_ms);
                true
            }
            Err(e) => {
                log::warn!("couldn't keep a draft of {name}: {e}");
                self.drafts.planner.put_failed(now);
                false
            }
        }
    }

    /// Open the next restored draft, one per frame, after any still waiting for its password.
    fn open_restored_draft(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(p) = self.drafts.restoring.take() {
            if self.session.document_id() != p.document {
                self.adopt_restored(p.key, p.name, now);
            } else if matches!(self.dialog, Some(crate::dialogs::Dialog::Password { .. } | crate::dialogs::Dialog::SaveChanges { .. }))
                || self.picker_save_pending(p.document)
            {
                self.drafts.restoring = Some(p);
                return;
            }
            // Otherwise the password or the Save Changes prompt was cancelled: the draft stays with the page.
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

    /// Open a restored draft as a file drop would, but without asking to save the open document:
    /// that one's draft is put now and left with the page instead (adopting doesn't remove it).
    /// If that draft can't be kept, the usual Save Changes prompt asks first.
    fn restore_draft(&mut self, draft: RestoredDraft, now: f64) {
        if draft.format != FORMAT {
            log::warn!("the draft {} is {}, not a Word document; not restored", draft.key, draft.format);
            return;
        }
        let kept = match self.draft_observed().and_then(|o| self.drafts.planner.flush_due(o)) {
            Some(key) => self.put_draft(&key, now),
            None => true,
        };
        // The bytes are a Word package whatever the name says; the engine reads the name's format.
        let ext = std::path::Path::new(&draft.name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let path = if wordcraft_engine::io::is_word_package(&ext) { draft.name.clone() } else { format!("{}.{FORMAT}", draft.name) };
        let document = self.session.document_id();
        let params = json!({"path": path, "data": wordcraft_engine::cmd::insert::base64_encode(&draft.bytes)});
        let opened = if kept { self.execute_user("file.open", params) } else { self.run("file.open", params) };
        match opened {
            Ok(v) if v.get("pending").is_some_and(|p| !p.is_null()) => {
                self.drafts.restoring = Some(PendingRestore { key: draft.key, name: draft.name, document });
            }
            Ok(_) => self.adopt_restored(draft.key, draft.name, now),
            Err(e) => log::warn!("couldn't restore the draft {}: {e}", draft.name),
        }
    }

    /// The restored document is unsaved, named as the draft was, and already drafted under its key.
    fn adopt_restored(&mut self, key: String, name: String, now: f64) {
        self.session.path = Some(name.into());
        self.session.dirty = true;
        self.ui.backstage = false;
        let version = self.draft_version();
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
    fn restoring_over_an_unsaved_document_keeps_that_ones_draft() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        app.drafts_tick(&ctx, 0.0);
        type_text(&mut app, "Mine");
        let mine = format!("doc-{}", app.session.document_id());
        page.restores.borrow_mut().push(RestoredDraft {
            key: "tab-b/1".into(),
            name: "Old.docx".into(),
            format: "docx".into(),
            bytes: docx("Theirs"),
        });
        app.drafts_tick(&ctx, 500.0);
        assert_eq!(page.puts.borrow().first().map(|p| p.0.clone()), Some(mine));
        app.drafts_tick(&ctx, 1_000.0);
        assert!(page.removes.borrow().is_empty(), "the replaced document's draft stays with the page");
    }

    #[test]
    fn restoring_over_an_unsaved_document_whose_draft_failed_asks_first() {
        let ctx = egui::Context::default();
        let (mut app, page) = app_with_page();
        if let Some(b) = app.services.drafts.as_mut() {
            b.put = Box::new(|_, _, _, _| Err("quota".into()));
        }
        app.drafts_tick(&ctx, 0.0);
        type_text(&mut app, "Mine");
        let mine = app.session.document_id();
        page.restores.borrow_mut().push(RestoredDraft { key: "tab-b/1".into(), name: "Old.docx".into(), format: "docx".into(), bytes: docx("Theirs") });
        app.drafts_tick(&ctx, 500.0);
        assert_eq!(app.session.document_id(), mine, "not replaced while its only copy is in memory");
        assert!(matches!(app.dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })));
        app.drafts_tick(&ctx, 1_000.0);
        assert!(matches!(app.dialog, Some(crate::dialogs::Dialog::SaveChanges { .. })), "the restore waits for the answer");
        assert!(app.answer_save_changes(crate::SaveChoice::DontSave).is_ok());
        app.drafts_tick(&ctx, 1_500.0);
        assert!(app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("Theirs"));
        assert!(app.session.dirty, "the restored draft opens unsaved");
        assert_eq!(app.session.path.as_deref(), Some(std::path::Path::new("Old.docx")));
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
}
