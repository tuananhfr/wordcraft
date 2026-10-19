//! Web draft autosave: when to hand each open document to the page's draft storage and when to
//! take its draft back ([`draft_bridge`](crate::draft_bridge) wires this to the app). Pure: the
//! caller passes the clock, so the timing is tested natively.

/// A draft waits for this long a pause in the editing.
pub const IDLE_MS: f64 = 2_500.0;
/// Continuous editing is still drafted this long after its first undrafted change.
pub const CHECKPOINT_MS: f64 = 30_000.0;
/// A document that takes long to serialize is drafted at most once per this many serializations' time.
pub const SERIALIZE_FACTOR: f64 = 10.0;
/// After a failed put, the next try waits this long instead of coming every frame.
pub const RETRY_MS: f64 = 30_000.0;

/// What a draft holds: the document (`document`), its edit (`rev`) and anything else the draft's
/// bytes or name depend on (`meta`: the save name, whether it has a password).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub document: u64,
    pub rev: u64,
    pub meta: u64,
}

/// An open document as of this frame.
#[derive(Clone, Copy, Debug)]
pub struct Observed {
    pub version: Version,
    /// Dirty, or a save of it is still in flight.
    pub unsaved: bool,
    /// An edit is half done (IME composition), so a snapshot now would be wrong.
    pub transient: bool,
}

/// A draft to put now: which document, under which key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Put {
    pub document: u64,
    pub key: String,
}

/// What to do this frame.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Serialize these documents and put them under their keys.
    pub put: Vec<Put>,
    /// Remove these keys' drafts.
    pub remove: Vec<String>,
    /// A draft is waiting for its moment: tick again soon, even without input.
    pub wait: bool,
}

/// The key a document's draft has until one is adopted from a restore. Document ids are unique
/// in the page, so the open documents' keys are too.
pub fn key_for(document: u64) -> String {
    format!("doc-{document}")
}

#[derive(Debug)]
struct Tracked {
    key: String,
    seen: Version,
    /// The version the shell holds, when it holds one this planner put (or adopted).
    drafted: Option<Version>,
    /// The shell holds a draft under `key`, so closing or saving the document removes it.
    stored: bool,
    last_change: f64,
    /// When the first change not yet drafted was seen.
    pending_since: Option<f64>,
    last_put: Option<f64>,
    serialize_ms: f64,
    retry_at: f64,
}

impl Tracked {
    fn new(key: String, seen: Version, now: f64) -> Tracked {
        Tracked { key, seen, drafted: None, stored: false, last_change: now, pending_since: None, last_put: None, serialize_ms: 0.0, retry_at: 0.0 }
    }
}

/// Decides, frame by frame, when each open document's draft is put and removed.
#[derive(Debug, Default)]
pub struct Planner {
    docs: Vec<Tracked>,
    /// Drafts of documents gone between two plans, removed by the next one.
    removals: Vec<String>,
}

impl Planner {
    /// What to do now, given the open documents (none: no document is open).
    pub fn plan(&mut self, now: f64, open: &[Observed]) -> Plan {
        let mut plan = Plan { remove: std::mem::take(&mut self.removals), ..Plan::default() };
        // Closed or replaced documents take their drafts with them.
        self.docs.retain(|t| {
            let still_open = open.iter().any(|o| o.version.document == t.seen.document);
            if !still_open && t.stored {
                plan.remove.push(t.key.clone());
            }
            still_open
        });
        for o in open {
            let at = match self.docs.iter().position(|t| t.seen.document == o.version.document) {
                Some(i) => i,
                None => {
                    self.docs.push(Tracked::new(key_for(o.version.document), o.version, now));
                    self.docs.len() - 1
                }
            };
            let Some(t) = self.docs.get_mut(at) else { continue };
            if o.version != t.seen {
                t.seen = o.version;
                t.last_change = now;
            }
            if !o.unsaved {
                if std::mem::take(&mut t.stored) {
                    plan.remove.push(t.key.clone());
                }
                t.drafted = None;
                t.pending_since = None;
                continue;
            }
            if t.drafted == Some(t.seen) {
                t.pending_since = None;
                continue;
            }
            let since = *t.pending_since.get_or_insert(now);
            let due = now - t.last_change >= IDLE_MS || now - since >= CHECKPOINT_MS;
            let paced = t.last_put.is_none_or(|last| now - last >= SERIALIZE_FACTOR * t.serialize_ms);
            if due && paced && !o.transient && now >= t.retry_at {
                plan.put.push(Put { document: o.version.document, key: t.key.clone() });
            } else {
                plan.wait = true;
            }
        }
        plan
    }

    fn tracked(&mut self, document: u64) -> Option<&mut Tracked> {
        self.docs.iter_mut().find(|t| t.seen.document == document)
    }

    /// The draft of `version` was put under `key` (by [`Planner::plan`]'s put or a flush when the
    /// page was hidden); serializing it took `serialize_ms`.
    pub fn put_done(&mut self, key: &str, version: Version, now: f64, serialize_ms: f64) {
        // A flush can come before the plan that would have noticed the document.
        if self.tracked(version.document).is_none() {
            self.docs.push(Tracked::new(key.to_string(), version, now));
        }
        let Some(t) = self.tracked(version.document) else { return };
        t.drafted = Some(version);
        t.stored = true;
        t.last_put = Some(now);
        t.serialize_ms = serialize_ms.max(0.0);
        t.retry_at = 0.0;
        if version == t.seen {
            t.pending_since = None;
        }
    }

    /// A put of `document` failed: try again later, not on every frame.
    pub fn put_failed(&mut self, document: u64, now: f64) {
        if let Some(t) = self.tracked(document) {
            t.retry_at = now + RETRY_MS;
        }
    }

    /// A restored document took over the draft `key`, which already holds `version`. The other
    /// open documents keep their drafts.
    pub fn adopt(&mut self, key: String, version: Version, now: f64) {
        let mut t = Tracked::new(key.clone(), version, now);
        t.drafted = Some(version);
        t.stored = true;
        match self.tracked(version.document) {
            Some(d) => {
                let old = std::mem::replace(d, t);
                if old.stored && old.key != key {
                    self.removals.push(old.key);
                }
            }
            None => self.docs.push(t),
        }
    }

    /// The key to put the document under when the page is hidden right now: it is unsaved and
    /// has changed since its last put. Waits and transient edits don't count then.
    pub fn flush_due(&self, observed: Observed) -> Option<String> {
        if !observed.unsaved {
            return None;
        }
        match self.docs.iter().find(|t| t.seen.document == observed.version.document) {
            Some(t) => (t.drafted != Some(observed.version)).then(|| t.key.clone()),
            None => Some(key_for(observed.version.document)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(document: u64, rev: u64) -> Version {
        Version { document, rev, meta: 0 }
    }

    fn dirty(document: u64, rev: u64) -> Option<Observed> {
        Some(Observed { version: v(document, rev), unsaved: true, transient: false })
    }

    fn clean(document: u64, rev: u64) -> Option<Observed> {
        Some(Observed { version: v(document, rev), unsaved: false, transient: false })
    }

    /// Plans for these open documents, and records each put the way the app does when it succeeds.
    fn step_all(p: &mut Planner, now: f64, open: &[Observed]) -> Plan {
        let plan = p.plan(now, open);
        for put in &plan.put {
            if let Some(o) = open.iter().find(|o| o.version.document == put.document) {
                p.put_done(&put.key, o.version, now, 1.0);
            }
        }
        plan
    }

    /// [`step_all`] with at most one document open.
    fn step(p: &mut Planner, now: f64, o: Option<Observed>) -> Plan {
        let open: Vec<Observed> = o.into_iter().collect();
        step_all(p, now, &open)
    }

    /// The key of the single put planned, if any.
    fn put_key(plan: &Plan) -> Option<&str> {
        assert!(plan.put.len() <= 1, "{plan:?}");
        plan.put.first().map(|p| p.key.as_str())
    }

    #[test]
    fn a_clean_document_is_never_drafted() {
        let mut p = Planner::default();
        for t in [0.0, 5_000.0, 60_000.0] {
            assert_eq!(step(&mut p, t, clean(1, 1)), Plan::default());
        }
    }

    #[test]
    fn an_edit_is_drafted_after_a_pause() {
        let mut p = Planner::default();
        step(&mut p, 0.0, clean(1, 1));
        let plan = step(&mut p, 1_000.0, dirty(1, 2));
        assert_eq!(put_key(&plan), None);
        assert!(plan.wait, "waits for the pause without input");
        assert_eq!(put_key(&step(&mut p, 3_000.0, dirty(1, 2))), None, "only 2000 ms since the change");
        let plan = step(&mut p, 3_500.0, dirty(1, 2));
        assert_eq!(put_key(&plan), Some("doc-1"));
        assert!(!plan.wait);
        assert_eq!(step(&mut p, 10_000.0, dirty(1, 2)), Plan::default(), "nothing new to draft");
    }

    #[test]
    fn continuous_typing_is_drafted_at_the_checkpoint() {
        let mut p = Planner::default();
        let mut put_at = None;
        let mut t = 0.0;
        let mut rev = 1;
        while t <= 40_000.0 {
            rev += 1;
            if !step(&mut p, t, dirty(1, rev)).put.is_empty() && put_at.is_none() {
                put_at = Some(t);
            }
            t += 500.0;
        }
        assert_eq!(put_at, Some(30_000.0));
    }

    #[test]
    fn a_change_after_a_put_is_drafted_again() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        assert!(!step(&mut p, 2_500.0, dirty(1, 2)).put.is_empty());
        assert!(step(&mut p, 3_000.0, dirty(1, 3)).wait);
        assert_eq!(put_key(&step(&mut p, 5_500.0, dirty(1, 3))), Some("doc-1"));
    }

    #[test]
    fn saving_removes_the_draft_once() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        assert!(!step(&mut p, 2_500.0, dirty(1, 2)).put.is_empty());
        assert_eq!(step(&mut p, 3_000.0, clean(1, 2)).remove, vec!["doc-1".to_string()]);
        assert_eq!(step(&mut p, 4_000.0, clean(1, 2)), Plan::default());
        // Edited again (or a cancelled save marks it unsaved again): a new draft under the same key.
        step(&mut p, 5_000.0, dirty(1, 3));
        assert_eq!(put_key(&step(&mut p, 7_500.0, dirty(1, 3))), Some("doc-1"));
    }

    #[test]
    fn saving_a_document_that_was_never_drafted_removes_nothing() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        assert_eq!(step(&mut p, 1_000.0, clean(1, 2)), Plan::default());
    }

    #[test]
    fn a_dirty_flag_alone_is_enough_to_draft() {
        // A cancelled save picker marks the document unsaved again without an edit.
        let mut p = Planner::default();
        step(&mut p, 0.0, clean(1, 2));
        step(&mut p, 1_000.0, dirty(1, 2));
        assert_eq!(put_key(&step(&mut p, 3_500.0, dirty(1, 2))), Some("doc-1"));
    }

    #[test]
    fn replacing_or_closing_the_document_removes_its_draft() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        assert!(!step(&mut p, 2_500.0, dirty(1, 2)).put.is_empty());
        let plan = step(&mut p, 3_000.0, clean(2, 1));
        assert_eq!(plan.remove, vec!["doc-1".to_string()]);
        step(&mut p, 4_000.0, dirty(2, 2));
        assert_eq!(put_key(&step(&mut p, 6_500.0, dirty(2, 2))), Some("doc-2"), "the new document has its own key");
        assert_eq!(step(&mut p, 7_000.0, None).remove, vec!["doc-2".to_string()]);
        assert_eq!(step(&mut p, 8_000.0, None), Plan::default());
    }

    #[test]
    fn a_transient_edit_holds_the_draft_back() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        let composing = Some(Observed { version: v(1, 2), unsaved: true, transient: true });
        let plan = step(&mut p, 5_000.0, composing);
        assert_eq!(put_key(&plan), None);
        assert!(plan.wait);
        assert_eq!(put_key(&step(&mut p, 5_100.0, dirty(1, 3))), None, "the commit is a change and waits for its pause");
        assert!(!step(&mut p, 7_600.0, dirty(1, 3)).put.is_empty());
    }

    #[test]
    fn a_slow_serialization_spaces_the_drafts_out() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        let open: Vec<Observed> = dirty(1, 2).into_iter().collect();
        assert!(!p.plan(2_500.0, &open).put.is_empty());
        p.put_done("doc-1", v(1, 2), 2_500.0, 1_000.0);
        step(&mut p, 3_000.0, dirty(1, 3));
        assert_eq!(put_key(&step(&mut p, 6_000.0, dirty(1, 3))), None, "paused, but not 10 × 1000 ms since the last put");
        assert!(!step(&mut p, 12_500.0, dirty(1, 3)).put.is_empty());
    }

    #[test]
    fn a_failed_put_is_retried_later_not_every_frame() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        let open: Vec<Observed> = dirty(1, 2).into_iter().collect();
        assert!(!p.plan(2_500.0, &open).put.is_empty());
        p.put_failed(1, 2_500.0);
        let plan = p.plan(2_600.0, &open);
        assert!(plan.put.is_empty());
        assert!(plan.wait);
        assert!(!p.plan(2_500.0 + RETRY_MS, &open).put.is_empty());
    }

    #[test]
    fn a_restored_document_keeps_its_key_and_is_not_put_again() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        assert!(!step(&mut p, 2_500.0, dirty(1, 2)).put.is_empty());
        p.adopt("old-tab-7".into(), v(2, 1), 3_000.0);
        // The restored one is already drafted; document 1, gone by the next plan, was closed.
        assert_eq!(step(&mut p, 3_000.0, dirty(2, 1)), Plan { remove: vec!["doc-1".into()], ..Plan::default() });
        assert_eq!(step(&mut p, 60_000.0, dirty(2, 1)), Plan::default());
        step(&mut p, 61_000.0, dirty(2, 2));
        assert_eq!(put_key(&step(&mut p, 63_500.0, dirty(2, 2))), Some("old-tab-7"));
        assert_eq!(step(&mut p, 64_000.0, clean(2, 2)).remove, vec!["old-tab-7".to_string()]);
    }

    #[test]
    fn a_restored_document_saved_unchanged_removes_its_draft() {
        let mut p = Planner::default();
        p.adopt("old-tab-7".into(), v(2, 1), 0.0);
        assert_eq!(step(&mut p, 1_000.0, clean(2, 1)).remove, vec!["old-tab-7".to_string()]);
    }

    #[test]
    fn a_hidden_page_flushes_without_waiting() {
        let mut p = Planner::default();
        step(&mut p, 0.0, clean(1, 1));
        assert_eq!(p.flush_due(Observed { version: v(1, 1), unsaved: false, transient: false }), None);
        step(&mut p, 100.0, dirty(1, 2));
        let due = Observed { version: v(1, 2), unsaved: true, transient: true };
        assert_eq!(p.flush_due(due).as_deref(), Some("doc-1"), "no idle wait, and a composition doesn't hold it back");
        p.put_done("doc-1", v(1, 2), 200.0, 1.0);
        assert_eq!(p.flush_due(due), None);
        assert_eq!(step(&mut p, 10_000.0, dirty(1, 2)), Plan::default(), "the flushed draft counts as put");
        assert_eq!(step(&mut p, 11_000.0, clean(1, 2)).remove, vec!["doc-1".to_string()]);
    }

    #[test]
    fn a_flush_of_a_document_not_yet_planned_is_still_removed_later() {
        let mut p = Planner::default();
        let o = Observed { version: v(5, 1), unsaved: true, transient: false };
        let key = p.flush_due(o);
        assert_eq!(key.as_deref(), Some("doc-5"));
        p.put_done("doc-5", v(5, 1), 0.0, 1.0);
        assert_eq!(step(&mut p, 1_000.0, clean(5, 1)).remove, vec!["doc-5".to_string()]);
    }

    #[test]
    fn a_flush_for_a_new_document_removes_the_replaced_ones_draft() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        assert!(!step(&mut p, 2_500.0, dirty(1, 2)).put.is_empty());
        // Document 2 replaced it and was flushed before the next plan saw it.
        p.put_done("doc-2", v(2, 2), 3_000.0, 1.0);
        let plan = step(&mut p, 3_100.0, dirty(2, 2));
        assert_eq!(plan.remove, vec!["doc-1".to_string()]);
        assert!(plan.put.is_empty());
    }

    #[test]
    fn a_name_or_password_change_is_a_change() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        assert!(!step(&mut p, 2_500.0, dirty(1, 2)).put.is_empty());
        let renamed = Some(Observed { version: Version { meta: 9, ..v(1, 2) }, unsaved: true, transient: false });
        step(&mut p, 3_000.0, renamed);
        assert_eq!(put_key(&step(&mut p, 5_500.0, renamed)), Some("doc-1"));
    }

    fn open(docs: &[Option<Observed>]) -> Vec<Observed> {
        docs.iter().flatten().copied().collect()
    }

    #[test]
    fn every_open_document_is_drafted_under_its_own_key() {
        let mut p = Planner::default();
        step_all(&mut p, 0.0, &open(&[dirty(1, 2), clean(2, 1), dirty(3, 4)]));
        let plan = step_all(&mut p, 2_500.0, &open(&[dirty(1, 2), clean(2, 1), dirty(3, 4)]));
        let keys: Vec<&str> = plan.put.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys, ["doc-1", "doc-3"], "the clean one isn't");
        assert_eq!(plan.put.iter().map(|p| p.document).collect::<Vec<_>>(), [1, 3]);
        // Only the edited one is put again; the others are already drafted.
        step_all(&mut p, 3_000.0, &open(&[dirty(1, 3), clean(2, 1), dirty(3, 4)]));
        let plan = step_all(&mut p, 5_500.0, &open(&[dirty(1, 3), clean(2, 1), dirty(3, 4)]));
        assert_eq!(put_key(&plan), Some("doc-1"));
    }

    #[test]
    fn saving_or_closing_one_document_removes_only_its_draft() {
        let mut p = Planner::default();
        step_all(&mut p, 0.0, &open(&[dirty(1, 2), dirty(2, 2), dirty(3, 2)]));
        assert_eq!(step_all(&mut p, 2_500.0, &open(&[dirty(1, 2), dirty(2, 2), dirty(3, 2)])).put.len(), 3);
        let plan = step_all(&mut p, 3_000.0, &open(&[dirty(1, 2), clean(2, 2), dirty(3, 2)]));
        assert_eq!(plan.remove, vec!["doc-2".to_string()], "saved");
        let plan = step_all(&mut p, 3_500.0, &open(&[clean(2, 2), dirty(3, 2)]));
        assert_eq!(plan.remove, vec!["doc-1".to_string()], "closed");
        assert!(plan.put.is_empty());
        assert_eq!(step_all(&mut p, 4_000.0, &open(&[dirty(3, 2)])), Plan::default(), "a clean document closed has no draft to remove");
    }

    #[test]
    fn restoring_next_to_an_unsaved_document_keeps_both_drafts() {
        let mut p = Planner::default();
        step(&mut p, 0.0, dirty(1, 2));
        assert!(!step(&mut p, 2_500.0, dirty(1, 2)).put.is_empty());
        p.adopt("old-tab|doc-4".into(), v(7, 1), 3_000.0);
        let plan = step_all(&mut p, 3_100.0, &open(&[dirty(1, 2), dirty(7, 1)]));
        assert_eq!(plan, Plan::default(), "nothing removed, nothing put again");
        step_all(&mut p, 4_000.0, &open(&[dirty(1, 2), dirty(7, 2)]));
        let plan = step_all(&mut p, 6_500.0, &open(&[dirty(1, 2), dirty(7, 2)]));
        assert_eq!(put_key(&plan), Some("old-tab|doc-4"));
    }
}
