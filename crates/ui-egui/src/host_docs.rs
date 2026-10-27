//! Web (ERPCons office shell, `window.erpHost` v1, contract `erp-host.md`): documents that live
//! on the server. The host hands them in ([`HostLink::open`]); Ctrl+S, AutoSave and the host's
//! flush send them back through [`HostTransport::save`], and a document is saved only as of the
//! revision the host acknowledged. They are never drafted (`erpDrafts`): the host keeps its own
//! copy of unacknowledged bytes. Without [`crate::Services::host`] (desktop, the web app on its
//! own) nothing here runs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde_json::{Value, json};
use wordcraft_engine::Session;
use wordcraft_fonts::log_safe;

use crate::WordApp;
use crate::open_documents::{save_name_of, untitled_name};

mod autosave;
mod saves;
#[cfg(test)]
mod tests;

use saves::{Saves, Snapshot, Work};

/// What a save is for (`erpHost.save`'s `reason`), weakest first: coalesced triggers keep the
/// strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SaveReason {
    Auto,
    Flush,
    Manual,
}

impl SaveReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SaveReason::Auto => "auto",
            SaveReason::Flush => "flush",
            SaveReason::Manual => "manual",
        }
    }
}

/// The host's answer to a save (`SaveResult`).
#[derive(Clone, Debug, PartialEq)]
pub enum SaveOutcome {
    Saved {
        version: u64,
    },
    Conflict {
        latest_version: u64,
    },
    Offline,
    /// `message` is ready to show.
    Failed {
        retryable: bool,
        message: String,
    },
}

/// A document the host hands in (`open(doc)`).
pub struct HostOpen {
    pub doc_id: String,
    pub version: u64,
    /// The display name, never a path.
    pub title: String,
    /// The extension the server stored.
    pub format: String,
    pub bytes: Vec<u8>,
}

/// Gets a save's outcome.
pub type SaveDone = Box<dyn FnOnce(SaveOutcome)>;

/// The host's side: `erpHost.save`, `dirty`, `opened` and `closed`.
pub trait HostTransport {
    /// `done` gets the outcome later, never within this call.
    fn save(&self, doc_id: &str, bytes: &[u8], revision: u64, reason: SaveReason, done: SaveDone);
    fn dirty(&self, doc_id: &str, dirty: bool);
    fn opened(&self, doc_id: &str, ok: bool, reason: Option<&str>);
    fn closed(&self, doc_id: &str);
}

/// The app's host documents, shared with the host's callbacks, which run between frames.
#[derive(Clone)]
pub struct HostLink {
    transport: Rc<dyn HostTransport>,
    shared: Rc<Shared>,
}

#[derive(Default)]
struct Shared {
    saves: RefCell<Saves>,
    opens: RefCell<Vec<HostOpen>>,
    /// Outcomes, acknowledgements and flushes that arrived, for [`HostLink::pump`].
    settled: RefCell<Vec<(u64, u64, SaveOutcome)>>,
    acks: RefCell<Vec<(String, u64, u64)>>,
    flushes: RefCell<Vec<Box<dyn FnOnce()>>>,
    pumping: Cell<bool>,
    wake: RefCell<Option<egui::Context>>,
}

impl HostLink {
    pub fn new(transport: Rc<dyn HostTransport>) -> Self {
        HostLink { transport, shared: Rc::default() }
    }

    /// `open(doc)`: the document opens on the next frame.
    pub fn open(&self, doc: HostOpen) {
        if let Ok(mut opens) = self.shared.opens.try_borrow_mut() {
            opens.push(doc);
        }
        self.wake();
    }

    /// `flush()`: hand every document with unacknowledged edits to the host now, without waiting
    /// for a frame (the page may be hiding); `done` runs once each has been handed over.
    pub fn flush(&self, done: Box<dyn FnOnce()>) {
        if let Ok(mut flushes) = self.shared.flushes.try_borrow_mut() {
            flushes.push(done);
        }
        self.pump();
    }

    /// `acknowledged(docId, revision, version)`: the host's own retry saved `revision`.
    pub fn acknowledged(&self, doc_id: &str, revision: u64, version: u64) {
        if let Ok(mut acks) = self.shared.acks.try_borrow_mut() {
            acks.push((doc_id.to_string(), revision, version));
        }
        self.pump();
    }

    fn wake(&self) {
        if let Ok(wake) = self.shared.wake.try_borrow()
            && let Some(ctx) = wake.as_ref()
        {
            ctx.request_repaint();
        }
    }

    fn with_saves<R>(&self, f: impl FnOnce(&mut Saves) -> R) -> Option<R> {
        self.shared.saves.try_borrow_mut().ok().map(|mut s| f(&mut s))
    }

    fn owns(&self, document: u64) -> bool {
        self.shared.saves.try_borrow().is_ok_and(|s| s.by_document(document).is_some())
    }

    /// Do `work`, then whatever the outcomes and flushes it led to leave to do. Re-entered (an
    /// outcome reported while sending), it only queues: the outer loop picks it up.
    fn run(&self, work: Work) {
        self.perform(work);
        self.pump();
    }

    fn pump(&self) {
        if self.shared.pumping.replace(true) {
            return;
        }
        loop {
            let settled = self.shared.settled.try_borrow_mut().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default();
            let acks = self.shared.acks.try_borrow_mut().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default();
            let flushes = self.shared.flushes.try_borrow_mut().map(|mut f| std::mem::take(&mut *f)).unwrap_or_default();
            if settled.is_empty() && acks.is_empty() && flushes.is_empty() {
                break;
            }
            let work = self.with_saves(|saves| {
                let mut work = Work::default();
                for (serial, revision, outcome) in settled {
                    work.append(saves.settle(serial, revision, outcome));
                }
                for (doc_id, revision, version) in acks {
                    work.append(saves.acknowledge(&doc_id, revision, version));
                }
                for done in flushes {
                    work.append(saves.flush(done));
                }
                work
            });
            self.perform(work.unwrap_or_default());
            self.wake();
        }
        self.shared.pumping.set(false);
    }

    fn perform(&self, work: Work) {
        for out in work.sends {
            let (serial, revision) = (out.serial, out.snapshot.revision);
            match serialize(&out.snapshot) {
                Ok(bytes) => {
                    let link = self.clone();
                    let done: SaveDone = Box::new(move |outcome| link.settled(serial, revision, outcome));
                    self.transport.save(&out.doc_id, &bytes, revision, out.reason, done);
                }
                Err(message) => {
                    if let Ok(mut settled) = self.shared.settled.try_borrow_mut() {
                        settled.push((serial, revision, SaveOutcome::Failed { retryable: false, message }));
                    }
                }
            }
        }
        // A flush resolves only once its saves have actually been handed to the host.
        for done in work.done {
            done();
        }
    }

    fn settled(&self, serial: u64, revision: u64, outcome: SaveOutcome) {
        if let Ok(mut settled) = self.shared.settled.try_borrow_mut() {
            settled.push((serial, revision, outcome));
        }
        self.pump();
    }
}

/// The bytes Save writes, encrypted with the document's password if it has one.
fn serialize(s: &Snapshot) -> Result<Vec<u8>, String> {
    let password = s.password.as_ref().map(wordcraft_engine::Password::as_str);
    wordcraft_engine::io::save_bytes_with(&s.name, &s.doc, password, &s.opts).map_err(|e| {
        log::warn!("couldn't write a {} for the host: {}", log_safe::file_label(&s.name, None), log_safe::scrub(&e, &s.name));
        tl!("Couldn't save the document. It is still unsaved.").to_string()
    })
}

/// The name a host document opens under: its title as a `.docx` file (a title is never a path).
fn host_file_name(title: &str) -> String {
    let clean: String = title.chars().map(|c| if matches!(c, '/' | '\\') || c.is_control() { '-' } else { c }).collect();
    let clean = clean.trim();
    let stem = if clean.is_empty() { untitled_name(1) } else { clean.to_string() };
    if stem.to_lowercase().ends_with(".docx") { stem } else { format!("{stem}.docx") }
}

/// What the app keeps about host documents besides their saves.
#[derive(Default)]
pub(crate) struct HostState {
    /// Commands held back until a host document's save is done ("Save changes?" › Save, Save
    /// All): (document, command, params).
    then: Vec<(u64, String, Value)>,
}

impl WordApp {
    /// Whether `document` came from the host (and saves there).
    pub(crate) fn host_owns(&self, document: u64) -> bool {
        self.services.host.as_ref().is_some_and(|h| h.owns(document))
    }

    /// Every frame, from `logic`: open what the host handed in, show save outcomes, AutoSave.
    pub(crate) fn host_tick(&mut self, ctx: &egui::Context, now: f64) {
        let Some(link) = self.services.host.clone() else { return };
        if let Ok(mut wake) = link.shared.wake.try_borrow_mut()
            && wake.is_none()
        {
            *wake = Some(ctx.clone());
        }
        let opens = link.shared.opens.try_borrow_mut().map(|mut o| std::mem::take(&mut *o)).unwrap_or_default();
        for open in opens {
            self.open_host_document(&link, open);
        }
        self.apply_host_outcomes(&link);
        self.host_observe(now);
        let work = link.with_saves(|s| s.autosave(now, |d| self.host_autosave_waits(d))).unwrap_or_default();
        link.run(work);
        self.apply_host_outcomes(&link);
        if let Some(at) = link.with_saves(|s| s.next_autosave()).flatten() {
            ctx.request_repaint_after(std::time::Duration::from_millis((at - now).clamp(0.0, autosave::CEILING_MS) as u64));
        }
    }

    /// End of every frame (and from [`Self::host_tick`]): what each host document now is, for
    /// AutoSave, a flush and `erpHost.dirty`; tabs that closed tell `erpHost.closed`.
    pub(crate) fn host_observe(&mut self, now: f64) {
        let Some(link) = self.services.host.clone() else { return };
        let documents: Vec<u64> = link.with_saves(|s| s.docs().iter().map(|d| d.document).collect()).unwrap_or_default();
        let opts = wordcraft_engine::cmd::file::export_options(&self.session, &json!({}));
        for document in documents {
            let Some((session, untitled)) = self.find_document(document) else {
                self.then_drop(document);
                if let Some((doc_id, work)) = link.with_saves(|s| s.remove(document)).flatten() {
                    link.transport.closed(&doc_id);
                    link.run(work);
                }
                continue;
            };
            let revision = session.rev();
            let confirm = || !crate::loss_warning::lost_items(wordcraft_engine::compatibility::DocFacts::of(session), "docx").is_empty();
            let snapshot = || snapshot_of(session, untitled, opts);
            let report = link.with_saves(|s| s.observe(document, revision, confirm, now, snapshot)).flatten();
            if let Some(dirty) = report
                && let Some(doc_id) = link.with_saves(|s| s.by_document(document).map(|d| d.doc_id.clone())).flatten()
            {
                link.transport.dirty(&doc_id, dirty);
            }
        }
    }

    /// AutoSave waits while the document's text is being composed (an IME). A save that would warn
    /// first (a signature it would drop) is left out by [`Saves::observe`]: the user's Ctrl+S asks.
    fn host_autosave_waits(&self, document: u64) -> bool {
        document == self.session.document_id() && !self.canvas.ime_preedit.is_empty()
    }

    /// Ctrl+S / Save on a host document: send it to the host (rule 9). `None` for other documents.
    pub(crate) fn host_save(&mut self) -> Option<Result<Value, String>> {
        let link = self.services.host.clone()?;
        let document = self.session.document_id();
        if !link.owns(document) {
            return None;
        }
        self.session.stamp_save();
        let opts = wordcraft_engine::cmd::file::export_options(&self.session, &json!({}));
        let fresh = snapshot_of(&self.session, self.untitled, opts);
        let work = link.with_saves(|s| s.save(document, SaveReason::Manual, Some(fresh))).unwrap_or_default();
        // Outcomes are applied by the next frame's tick: held-back commands mustn't run inside
        // this save, which may be one of a background document.
        link.run(work);
        Some(Ok(json!({"pending": true, "host": true})))
    }

    /// Hold `then` back until the host document's save is done. False for other documents.
    pub(crate) fn host_continue_after_save(&mut self, document: u64, then: String, params: Value) -> bool {
        if !self.host_owns(document) {
            return false;
        }
        self.host.then.push((document, then, params));
        true
    }

    fn then_drop(&mut self, document: u64) {
        self.host.then.retain(|(d, ..)| *d != document);
    }

    /// Saved documents become clean as of the revision the host acknowledged (rule 1); failures
    /// that stop AutoSave show as a failed save (rule 6); held-back commands go on once their
    /// document's save is done, or stop if it is still unsaved.
    fn apply_host_outcomes(&mut self, link: &HostLink) {
        let (saved, failures) = link.with_saves(|s| (s.take_saved(), s.take_failures())).unwrap_or_default();
        for (document, revision) in saved {
            if let Some(session) = self.session_mut(document)
                && session.rev() == revision
            {
                session.dirty = false;
                session.doc.passthrough.remove("wordcraft:package.signed");
            }
            self.save_failures.saved(document);
        }
        for (document, message) in failures {
            self.note_save_failed(document, &message);
            self.status(message);
        }
        let ready: Vec<usize> = self
            .host
            .then
            .iter()
            .enumerate()
            .filter(|(_, (document, ..))| link.with_saves(|s| s.by_document(*document).is_none_or(saves::Doc::idle)).unwrap_or(false))
            .map(|(i, _)| i)
            .collect();
        for i in ready.into_iter().rev() {
            let (document, then, params) = self.host.then.remove(i);
            let clean = self.find_document(document).is_some_and(|(s, _)| !s.dirty);
            let by_document = then == "ui.closeDocument" || then == crate::close_many::CLOSE_DOCUMENTS;
            if clean && (by_document || document == self.session.document_id()) {
                let _ = self.execute_user(&then, params);
            } else {
                self.stop_save_all(document);
            }
        }
    }

    fn session_mut(&mut self, document: u64) -> Option<&mut Session> {
        if self.session.document_id() == document {
            return Some(&mut self.session);
        }
        self.docs.parked_sessions_mut().find(|s| s.document_id() == document)
    }

    /// Open a document the host handed in, beside the open ones, or in place of the one it
    /// reloads (same `docId`: the latest version after a conflict). Reported once to `opened`.
    fn open_host_document(&mut self, link: &HostLink, open: HostOpen) {
        if !open.format.eq_ignore_ascii_case("docx") {
            link.transport.opened(&open.doc_id, false, Some(REFUSED_FORMAT));
            return;
        }
        let params = json!({"path": host_file_name(&open.title), "data": wordcraft_engine::cmd::insert::base64_encode(&open.bytes)});
        let reloads = link.with_saves(|s| s.by_doc_id(&open.doc_id).map(|d| d.document)).flatten().filter(|d| self.document_index(*d).is_some());
        let opened = match reloads {
            Some(document) => self.reload_host_document(document, &params),
            None => self.execute("file.open", params).map(|_| self.session.document_id()),
        };
        match opened {
            Ok(document) => {
                let revision = self.find_document(document).map_or(0, |(s, _)| s.rev());
                let work = link.with_saves(|s| s.add(&open.doc_id, document, open.version, revision)).unwrap_or_default();
                link.run(work);
                self.ui.backstage = false;
                link.transport.opened(&open.doc_id, true, None);
            }
            Err(e) => {
                link.transport.opened(&open.doc_id, false, Some(host_open_refusal(&e)));
            }
        }
    }

    /// Replace `document` with the reloaded bytes, keeping its tab; its unsaved edits go (the
    /// host asked the user first).
    fn reload_host_document(&mut self, document: u64, params: &Value) -> Result<u64, String> {
        let shown = self.session.document_id() == document;
        let reloaded = self.in_document(document, |app| -> Result<u64, String> {
            app.session.run("file.open", params).map_err(|e| e.to_string())?;
            let mut fresh = app.canvas.fresh_view();
            app.canvas.swap_view(&mut fresh);
            app.autosave_path = None;
            crate::alt_chunk_notice::opened(app);
            crate::font_notice::opened(app);
            Ok(app.session.document_id())
        });
        let reloaded = reloaded.unwrap_or_else(|| Err("the document closed".into()))?;
        self.then_drop(document);
        self.save_failures.saved(document);
        if shown {
            self.document_switched();
        }
        Ok(reloaded)
    }
}

/// `opened`'s reason for a document the editor doesn't take. A key of the shell's intake
/// messages: the shell words it.
const REFUSED_FORMAT: &str = "unsupported";

/// Why a document the host handed in didn't open, as `opened`'s reason: unreadable bytes and the
/// document limit are `parserFailed`.
fn host_open_refusal(error: &str) -> &'static str {
    if wordcraft_engine::io::needs_password(error) || wordcraft_engine::io::wrong_password(error) { REFUSED_FORMAT } else { "parserFailed" }
}

fn snapshot_of(session: &Session, untitled: u32, opts: wordcraft_engine::io::ExportOptions) -> Snapshot {
    Snapshot { revision: session.rev(), name: save_name_of(session, untitled), doc: session.doc.clone(), password: session.password.clone(), opts }
}
