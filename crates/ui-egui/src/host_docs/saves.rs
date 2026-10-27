//! Each host document's saves (`erp-host.md` rules 1–6), kept outside the app: a flush when the
//! page hides, and the follow-up of a save that was in flight, go out when no frame will run.
//! Methods return the [`Work`] to do; the caller does it once it holds no borrow of this.

use super::autosave::Clock;
use super::{SaveOutcome, SaveReason};

/// A document as it is to be sent: its revision and what serializing it takes.
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub(crate) revision: u64,
    pub(crate) name: String,
    pub(crate) doc: wordcraft_doc::Document,
    pub(crate) password: Option<wordcraft_engine::Password>,
    pub(crate) opts: wordcraft_engine::io::ExportOptions,
}

/// Why AutoSave stopped for a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    Conflict,
    Failed,
}

/// A save to send.
pub(crate) struct Outgoing {
    /// The [`Doc::serial`] its outcome is for.
    pub(crate) serial: u64,
    pub(crate) doc_id: String,
    pub(crate) reason: SaveReason,
    pub(crate) snapshot: Snapshot,
}

/// What a change of state leaves to do: saves to send and flushes to resolve.
#[derive(Default)]
pub(crate) struct Work {
    pub(crate) sends: Vec<Outgoing>,
    pub(crate) done: Vec<Box<dyn FnOnce()>>,
}

impl Work {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.sends.is_empty() && self.done.is_empty()
    }

    pub(crate) fn append(&mut self, other: Work) {
        self.sends.extend(other.sends);
        self.done.extend(other.done);
    }
}

/// One host document.
pub(crate) struct Doc {
    /// Tells outcomes for a document reloaded in place (same `docId`) from the old one's.
    pub(crate) serial: u64,
    pub(crate) doc_id: String,
    /// The app's document ([`wordcraft_engine::Session::document_id`]).
    pub(crate) document: u64,
    /// The server version the document is based on.
    pub(crate) version: u64,
    /// The revision the host last acknowledged as saved.
    pub(crate) acked: u64,
    /// The newest revision seen.
    seen: u64,
    latest: Option<Snapshot>,
    /// The revision last sent, whatever came of it.
    sent: u64,
    /// Saving it would drop something the user must confirm first (a signature): only Ctrl+S,
    /// which asks, sends it.
    confirm: bool,
    in_flight: Option<u64>,
    /// Triggers while a save was in flight, as one follow-up of the strongest reason.
    queued: Option<SaveReason>,
    pub(crate) stopped: Option<Stop>,
    clock: Clock,
    /// What `erpHost.dirty` was last told.
    reported_dirty: bool,
}

impl Doc {
    pub(crate) fn dirty(&self) -> bool {
        self.seen != self.acked
    }

    /// No save in flight or waiting.
    pub(crate) fn idle(&self) -> bool {
        self.in_flight.is_none() && self.queued.is_none()
    }
}

/// A flush still waiting for a save in flight before it can hand over the newest revision. It
/// never waits for the host's answers: the host tracks those itself.
struct Flush {
    targets: Vec<Target>,
    done: Option<Box<dyn FnOnce()>>,
}

/// A document a flush waits for: done once `revision` has been handed to the host, or the
/// document stopped or closed.
struct Target {
    serial: u64,
    revision: u64,
}

#[derive(Default)]
pub(crate) struct Saves {
    docs: Vec<Doc>,
    flushes: Vec<Flush>,
    /// Acknowledged saves for the app to apply, as (document, revision).
    saved: Vec<(u64, u64)>,
    /// Non-retryable failures for the app to show, as (document, message).
    failures: Vec<(u64, String)>,
    next_serial: u64,
}

impl Saves {
    pub(crate) fn docs(&self) -> &[Doc] {
        &self.docs
    }

    pub(crate) fn by_doc_id(&self, doc_id: &str) -> Option<&Doc> {
        self.docs.iter().find(|d| d.doc_id == doc_id)
    }

    pub(crate) fn by_document(&self, document: u64) -> Option<&Doc> {
        self.docs.iter().find(|d| d.document == document)
    }

    fn by_document_mut(&mut self, document: u64) -> Option<&mut Doc> {
        self.docs.iter_mut().find(|d| d.document == document)
    }

    /// `doc_id` opened as `document` at `revision`, saved as of then. A document reloaded under
    /// the same `doc_id` replaces the old entry and its saves; the host still hears its dirty
    /// state change back.
    pub(crate) fn add(&mut self, doc_id: &str, document: u64, version: u64, revision: u64) -> Work {
        let mut work = Work::default();
        let mut reported_dirty = false;
        if let Some(i) = self.docs.iter().position(|d| d.doc_id == doc_id) {
            let old = self.docs.remove(i);
            reported_dirty = old.reported_dirty;
            work.append(self.check_flushes());
        }
        self.next_serial = self.next_serial.wrapping_add(1);
        self.docs.push(Doc {
            serial: self.next_serial,
            doc_id: doc_id.to_string(),
            document,
            version,
            acked: revision,
            seen: revision,
            latest: None,
            sent: revision,
            confirm: false,
            in_flight: None,
            queued: None,
            stopped: None,
            clock: Clock::default(),
            reported_dirty,
        });
        work
    }

    /// The document closed: forget it. Its `doc_id`, for `erpHost.closed`.
    pub(crate) fn remove(&mut self, document: u64) -> Option<(String, Work)> {
        let i = self.docs.iter().position(|d| d.document == document)?;
        let doc = self.docs.remove(i);
        Some((doc.doc_id, self.check_flushes()))
    }

    /// The end of a frame saw `document` at `revision`; whether saving it needs the user's
    /// confirmation and its `snapshot` are asked for only when the revision is new. The dirty
    /// state when `erpHost.dirty` must hear it.
    pub(crate) fn observe(
        &mut self,
        document: u64,
        revision: u64,
        confirm: impl FnOnce() -> bool,
        now: f64,
        snapshot: impl FnOnce() -> Snapshot,
    ) -> Option<bool> {
        let doc = self.by_document_mut(document)?;
        if revision != doc.seen {
            doc.confirm = confirm();
            doc.seen = revision;
            doc.clock.edited(now);
            doc.latest = (revision != doc.acked).then(snapshot);
        }
        let dirty = doc.dirty();
        (dirty != doc.reported_dirty).then(|| {
            doc.reported_dirty = dirty;
            dirty
        })
    }

    /// AutoSave: the documents whose newest edits are unsent and have waited long enough.
    /// `blocked` documents (mid-composition) and those needing confirmation wait.
    pub(crate) fn autosave(&mut self, now: f64, blocked: impl Fn(u64) -> bool) -> Work {
        let due: Vec<u64> = self
            .docs
            .iter()
            .filter(|d| d.stopped.is_none() && !d.confirm && d.dirty() && d.seen != d.sent && d.clock.due(now) && !blocked(d.document))
            .map(|d| d.document)
            .collect();
        let mut work = Work::default();
        for document in due {
            work.append(self.save(document, SaveReason::Auto, None));
        }
        work
    }

    /// The soonest AutoSave is due, for waking the app.
    pub(crate) fn next_autosave(&self) -> Option<f64> {
        self.docs
            .iter()
            .filter(|d| d.stopped.is_none() && !d.confirm && d.dirty() && d.seen != d.sent)
            .filter_map(|d| d.clock.due_at())
            .reduce(f64::min)
    }

    /// Save `document` now, or right after the save in flight. `fresh` is its current state (a
    /// manual save sends even an unchanged document).
    pub(crate) fn save(&mut self, document: u64, reason: SaveReason, fresh: Option<Snapshot>) -> Work {
        let mut work = Work::default();
        let Some(doc) = self.by_document_mut(document) else { return work };
        if let Some(fresh) = fresh {
            doc.seen = fresh.revision;
            doc.latest = Some(fresh);
        }
        match doc.in_flight {
            Some(revision) if revision == doc.seen => {}
            Some(_) => doc.queued = doc.queued.max(Some(reason)),
            None => work.sends.extend(start(doc, reason)),
        }
        work
    }

    /// `flush()`: hand every document with unacknowledged edits to the host; `done` runs once each
    /// has been handed over (behind a save in flight). Stopped documents and those needing
    /// confirmation are skipped (rule 4).
    pub(crate) fn flush(&mut self, done: Box<dyn FnOnce()>) -> Work {
        let due: Vec<(u64, Target)> = self
            .docs
            .iter()
            .filter(|d| d.stopped.is_none() && !d.confirm && d.dirty())
            .map(|d| (d.document, Target { serial: d.serial, revision: d.seen }))
            .collect();
        let mut work = Work::default();
        for (document, _) in &due {
            work.append(self.save(*document, SaveReason::Flush, None));
        }
        self.flushes.push(Flush { targets: due.into_iter().map(|(_, t)| t).collect(), done: Some(done) });
        work.append(self.check_flushes());
        work
    }

    /// The save of `revision` that [`Outgoing::serial`] sent had `outcome`.
    pub(crate) fn settle(&mut self, serial: u64, revision: u64, outcome: SaveOutcome) -> Work {
        let mut work = Work::default();
        if let Some(doc) = self.docs.iter_mut().find(|d| d.serial == serial) {
            if doc.in_flight == Some(revision) {
                doc.in_flight = None;
            }
            match outcome {
                SaveOutcome::Saved { version } => {
                    doc.version = version;
                    doc.acked = doc.acked.max(revision);
                    self.saved.push((doc.document, revision));
                    // The server took this base: a manual save after a stop turns AutoSave back on.
                    doc.stopped = None;
                    if !doc.dirty() {
                        doc.latest = None;
                    }
                }
                SaveOutcome::Conflict { .. } => {
                    doc.stopped = Some(Stop::Conflict);
                    doc.queued = None;
                }
                SaveOutcome::Offline | SaveOutcome::Failed { retryable: true, .. } => {}
                SaveOutcome::Failed { retryable: false, message } => {
                    doc.stopped = Some(Stop::Failed);
                    doc.queued = None;
                    self.failures.push((doc.document, message));
                }
            }
            if let Some(reason) = doc.queued.take()
                && doc.in_flight.is_none()
                && doc.seen != revision
            {
                work.sends.extend(start(doc, reason));
            }
        }
        work.append(self.check_flushes());
        work
    }

    /// `acknowledged()`: the host's own retry saved `revision` of `doc_id` as `version`, like a
    /// `saved` for it. A duplicate, a stale one, one never sent and an unknown `doc_id` are
    /// ignored. Like `saved`, it turns a stopped AutoSave back on (rule 9).
    pub(crate) fn acknowledge(&mut self, doc_id: &str, revision: u64, version: u64) -> Work {
        let Some(doc) = self.docs.iter_mut().find(|d| d.doc_id == doc_id) else { return Work::default() };
        if revision <= doc.acked || revision > doc.sent {
            return Work::default();
        }
        doc.version = version;
        doc.acked = revision;
        doc.stopped = None;
        self.saved.push((doc.document, revision));
        if !doc.dirty() {
            doc.latest = None;
        }
        self.check_flushes()
    }

    pub(crate) fn take_saved(&mut self) -> Vec<(u64, u64)> {
        std::mem::take(&mut self.saved)
    }

    pub(crate) fn take_failures(&mut self) -> Vec<(u64, String)> {
        std::mem::take(&mut self.failures)
    }

    fn check_flushes(&mut self) -> Work {
        let docs = &self.docs;
        let satisfied = |t: &Target| {
            docs.iter().find(|d| d.serial == t.serial).is_none_or(|d| d.stopped.is_some() || d.acked >= t.revision || d.sent >= t.revision)
        };
        let mut work = Work::default();
        self.flushes.retain_mut(|f| {
            if !f.targets.iter().all(satisfied) {
                return true;
            }
            work.done.extend(f.done.take());
            false
        });
        work
    }
}

/// Send the newest snapshot of `doc`; nothing without one.
fn start(doc: &mut Doc, reason: SaveReason) -> Option<Outgoing> {
    let snapshot = doc.latest.clone()?;
    doc.in_flight = Some(snapshot.revision);
    doc.sent = snapshot.revision;
    doc.queued = None;
    doc.clock.sent();
    Some(Outgoing { serial: doc.serial, doc_id: doc.doc_id.clone(), reason, snapshot })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    fn snap(revision: u64) -> Snapshot {
        Snapshot { revision, name: "a.docx".into(), doc: wordcraft_doc::Document::new(), password: None, opts: Default::default() }
    }

    fn opened() -> Saves {
        let mut s = Saves::default();
        assert!(s.add("d1", 7, 3, 10).is_empty());
        s
    }

    fn sent(work: &Work) -> Vec<(u64, SaveReason)> {
        work.sends.iter().map(|o| (o.snapshot.revision, o.reason)).collect()
    }

    fn serial(s: &Saves) -> u64 {
        s.docs()[0].serial
    }

    #[test]
    fn autosave_waits_for_the_pause_then_sends_the_newest_revision() {
        let mut s = opened();
        assert_eq!(s.observe(7, 11, || false, 0.0, || snap(11)), Some(true), "first difference from the acknowledged revision");
        assert_eq!(s.observe(7, 12, || false, 1_000.0, || snap(12)), None, "already reported dirty");
        assert!(s.autosave(3_999.0, |_| false).is_empty());
        assert_eq!(s.next_autosave(), Some(4_000.0));
        assert_eq!(sent(&s.autosave(4_000.0, |_| false)), vec![(12, SaveReason::Auto)]);
        assert!(s.autosave(10_000.0, |_| false).is_empty(), "sent and not edited since");
    }

    #[test]
    fn clean_only_as_of_the_revision_sent() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        let w = s.autosave(5_000.0, |_| false);
        assert_eq!(sent(&w), vec![(11, SaveReason::Auto)]);
        s.observe(7, 12, || false, 5_500.0, || snap(12));
        let w = s.settle(serial(&s), 11, SaveOutcome::Saved { version: 4 });
        assert!(w.is_empty(), "no follow-up was asked for yet");
        let d = s.by_document(7).unwrap();
        assert_eq!((d.acked, d.version), (11, 4));
        assert!(d.dirty(), "the edit typed during the request stays unsaved");
        assert_eq!(s.observe(7, 12, || false, 5_600.0, || snap(12)), None, "still dirty, no new report");
        assert_eq!(sent(&s.autosave(8_500.0, |_| false)), vec![(12, SaveReason::Auto)], "and triggers the next AutoSave");
        s.settle(serial(&s), 12, SaveOutcome::Saved { version: 5 });
        assert_eq!(s.observe(7, 12, || false, 9_000.0, || snap(12)), Some(false));
    }

    #[test]
    fn triggers_while_a_save_is_in_flight_coalesce_into_one_follow_up() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        assert_eq!(sent(&s.save(7, SaveReason::Manual, Some(snap(11)))), vec![(11, SaveReason::Manual)]);
        for (i, rev) in (12..16).enumerate() {
            let t = 100.0 * i as f64;
            s.observe(7, rev, || false, t, || snap(rev));
            assert!(s.save(7, SaveReason::Manual, Some(snap(rev))).is_empty(), "one in flight");
            assert!(s.autosave(t + 50_000.0, |_| false).is_empty(), "one in flight");
        }
        let w = s.settle(serial(&s), 11, SaveOutcome::Saved { version: 4 });
        assert_eq!(sent(&w), vec![(15, SaveReason::Manual)], "a single follow-up of the newest revision, at once");
        assert!(s.settle(serial(&s), 15, SaveOutcome::Saved { version: 5 }).is_empty());
        assert!(!s.by_document(7).unwrap().dirty());
    }

    #[test]
    fn a_follow_up_is_dropped_when_nothing_changed_since() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.save(7, SaveReason::Auto, None);
        assert!(s.save(7, SaveReason::Auto, None).is_empty());
        assert!(s.settle(serial(&s), 11, SaveOutcome::Saved { version: 4 }).is_empty());
        assert!(s.by_document(7).unwrap().idle());
    }

    #[test]
    fn a_conflict_stops_autosave_keeps_the_document_dirty_and_is_never_retried() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.autosave(5_000.0, |_| false);
        s.observe(7, 12, || false, 5_100.0, || snap(12));
        s.save(7, SaveReason::Auto, None);
        let w = s.settle(serial(&s), 11, SaveOutcome::Conflict { latest_version: 9 });
        assert!(w.is_empty(), "the queued follow-up is dropped");
        let d = s.by_document(7).unwrap();
        assert_eq!(d.stopped, Some(Stop::Conflict));
        assert!(d.dirty());
        s.observe(7, 13, || false, 6_000.0, || snap(13));
        assert!(s.autosave(1e9, |_| false).is_empty(), "AutoSave stays off");
        assert_eq!(s.next_autosave(), None);
        let flushed = Rc::new(Cell::new(false));
        let f = flushed.clone();
        let w = s.flush(Box::new(move || f.set(true)));
        assert!(w.sends.is_empty(), "a flush doesn't retry it either");
        w.done.into_iter().for_each(|d| d());
        assert!(flushed.get());
        assert_eq!(sent(&s.save(7, SaveReason::Manual, Some(snap(13)))), vec![(13, SaveReason::Manual)], "the user's own save still goes");
        s.settle(serial(&s), 13, SaveOutcome::Saved { version: 10 });
        assert_eq!(s.by_document(7).unwrap().stopped, None);
    }

    #[test]
    fn offline_and_retryable_failures_keep_the_document_dirty_and_autosave_on() {
        for outcome in [SaveOutcome::Offline, SaveOutcome::Failed { retryable: true, message: "thử lại".into() }] {
            let mut s = opened();
            s.observe(7, 11, || false, 0.0, || snap(11));
            s.autosave(5_000.0, |_| false);
            assert!(s.settle(serial(&s), 11, outcome).is_empty());
            let d = s.by_document(7).unwrap();
            assert!(d.dirty() && d.stopped.is_none());
            assert!(s.autosave(1e9, |_| false).is_empty(), "the shell owns the retry of the same revision");
            assert!(s.take_failures().is_empty(), "nothing to show");
            s.observe(7, 12, || false, 10_000.0, || snap(12));
            assert_eq!(sent(&s.autosave(13_000.0, |_| false)), vec![(12, SaveReason::Auto)], "a new edit saves again");
        }
    }

    #[test]
    fn a_non_retryable_failure_stops_autosave_and_is_shown() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.autosave(5_000.0, |_| false);
        s.settle(serial(&s), 11, SaveOutcome::Failed { retryable: false, message: "Tệp quá lớn".into() });
        assert_eq!(s.take_failures(), vec![(7, "Tệp quá lớn".to_string())]);
        let d = s.by_document(7).unwrap();
        assert_eq!(d.stopped, Some(Stop::Failed));
        assert!(d.dirty());
        s.observe(7, 12, || false, 6_000.0, || snap(12));
        assert!(s.autosave(1e9, |_| false).is_empty());
    }

    #[test]
    fn flush_resolves_once_every_unacknowledged_document_is_handed_over() {
        let mut s = opened();
        s.add("d2", 8, 1, 20);
        s.add("d3", 9, 1, 30);
        s.add("d4", 10, 1, 40);
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.observe(8, 21, || false, 0.0, || snap(21));
        s.observe(10, 41, || true, 0.0, || snap(41));
        // d1 has a save in flight of an older revision; d3 is clean; d4 would drop a signature.
        s.save(7, SaveReason::Auto, None);
        s.observe(7, 12, || false, 10.0, || snap(12));
        let flushed = Rc::new(Cell::new(0));
        let f = flushed.clone();
        let w = s.flush(Box::new(move || f.set(f.get() + 1)));
        assert_eq!(sent(&w), vec![(21, SaveReason::Flush)], "d1 waits behind its save in flight; d3 and d4 aren't sent");
        assert!(w.done.is_empty(), "d1 hasn't handed revision 12 over yet");
        let s1 = s.by_doc_id("d1").unwrap().serial;
        let s2 = s.by_doc_id("d2").unwrap().serial;
        let w = s.settle(s1, 11, SaveOutcome::Offline);
        assert_eq!(sent(&w), vec![(12, SaveReason::Flush)], "the follow-up goes as soon as the save in flight is answered");
        assert_eq!(w.done.len(), 1, "and the flush resolves without waiting for its answer, nor d2's");
        w.done.into_iter().for_each(|d| d());
        assert_eq!(flushed.get(), 1);

        assert!(s.settle(s2, 21, SaveOutcome::Offline).is_empty());
        let again = Rc::new(Cell::new(false));
        let a = again.clone();
        let w = s.flush(Box::new(move || a.set(true)));
        assert_eq!(sent(&w), vec![(21, SaveReason::Flush)], "d2 is still unacknowledged after going offline; d1's revision 12 is in flight");
        assert_eq!(w.done.len(), 1, "everything is handed over at once");
        w.done.into_iter().for_each(|d| d());
        assert!(again.get());
        assert!(s.settle(s2, 21, SaveOutcome::Saved { version: 2 }).is_empty());
    }

    #[test]
    fn a_document_needing_confirmation_is_never_autosaved() {
        let mut s = opened();
        s.observe(7, 11, || true, 0.0, || snap(11));
        assert!(s.autosave(1e9, |_| false).is_empty());
        assert_eq!(s.next_autosave(), None);
        assert_eq!(sent(&s.save(7, SaveReason::Manual, Some(snap(11)))), vec![(11, SaveReason::Manual)], "Ctrl+S, which asks, still sends");
    }

    #[test]
    fn a_closed_or_reloaded_document_no_longer_holds_a_flush_up() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.save(7, SaveReason::Auto, None);
        s.observe(7, 12, || false, 10.0, || snap(12));
        let w = s.flush(Box::new(|| {}));
        assert!(w.is_empty(), "waiting behind the save in flight");
        let (doc_id, w) = s.remove(7).unwrap();
        assert_eq!(doc_id, "d1");
        assert_eq!(w.done.len(), 1);

        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        let old = serial(&s);
        s.save(7, SaveReason::Auto, None);
        s.observe(7, 12, || false, 10.0, || snap(12));
        assert!(s.flush(Box::new(|| {})).is_empty());
        let w = s.add("d1", 70, 9, 40);
        assert_eq!(w.done.len(), 1, "the reload replaced the document the flush waited for");
        assert!(s.settle(old, 11, SaveOutcome::Saved { version: 8 }).is_empty(), "the old document's outcome is ignored");
        let d = s.by_doc_id("d1").unwrap();
        assert_eq!((d.document, d.acked, d.version), (70, 40, 9));
        assert_eq!(s.observe(70, 40, || false, 1.0, || snap(40)), Some(false), "the host hears the reloaded document is clean");
    }

    #[test]
    fn acknowledged_after_going_offline_saves_that_revision() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.autosave(5_000.0, |_| false);
        s.settle(serial(&s), 11, SaveOutcome::Offline);
        assert!(s.acknowledge("d1", 11, 4).is_empty());
        assert_eq!(s.take_saved(), vec![(7, 11)]);
        let d = s.by_document(7).unwrap();
        assert_eq!((d.acked, d.version, d.dirty()), (11, 4, false));
        assert_eq!(s.observe(7, 11, || false, 6_000.0, || snap(11)), Some(false), "the host hears it is clean");
    }

    #[test]
    fn acknowledged_cleans_only_as_of_its_revision() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.autosave(5_000.0, |_| false);
        s.settle(serial(&s), 11, SaveOutcome::Offline);
        s.observe(7, 12, || false, 5_500.0, || snap(12));
        s.acknowledge("d1", 11, 4);
        let d = s.by_document(7).unwrap();
        assert_eq!(d.acked, 11);
        assert!(d.dirty(), "the edit made after the send stays unsaved");
        assert_eq!(sent(&s.autosave(8_500.0, |_| false)), vec![(12, SaveReason::Auto)]);
    }

    #[test]
    fn stale_unsent_and_unknown_acknowledgements_are_ignored() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.autosave(5_000.0, |_| false);
        s.settle(serial(&s), 11, SaveOutcome::Offline);
        s.observe(7, 12, || false, 5_500.0, || snap(12));
        s.autosave(9_000.0, |_| false);
        s.settle(serial(&s), 12, SaveOutcome::Offline);
        s.acknowledge("d1", 12, 6);
        s.take_saved();
        s.acknowledge("d1", 11, 5);
        s.acknowledge("d1", 99, 7);
        s.acknowledge("other", 12, 8);
        assert!(s.take_saved().is_empty());
        let d = s.by_document(7).unwrap();
        assert_eq!((d.acked, d.version), (12, 6), "an older revision, one never sent or another docId changes nothing");
    }

    #[test]
    fn acknowledged_while_stopped_turns_autosave_back_on() {
        let mut s = opened();
        s.observe(7, 11, || false, 0.0, || snap(11));
        s.autosave(5_000.0, |_| false);
        s.settle(serial(&s), 11, SaveOutcome::Offline);
        s.observe(7, 12, || false, 5_500.0, || snap(12));
        s.autosave(8_500.0, |_| false);
        s.settle(serial(&s), 12, SaveOutcome::Conflict { latest_version: 9 });
        assert_eq!(s.by_document(7).unwrap().stopped, Some(Stop::Conflict));
        assert!(s.acknowledge("d1", 11, 4).is_empty());
        let d = s.by_document(7).unwrap();
        assert_eq!((d.stopped, d.acked, d.version), (None, 11, 4));
        assert!(d.dirty(), "revision 12 is still unsaved");
        s.observe(7, 13, || false, 9_000.0, || snap(13));
        assert_eq!(s.next_autosave(), Some(12_000.0));
        assert_eq!(sent(&s.autosave(12_000.0, |_| false)), vec![(13, SaveReason::Auto)], "AutoSave runs again");
    }
}
