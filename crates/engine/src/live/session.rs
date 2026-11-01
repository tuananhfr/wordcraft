//! The session side of co-editing: the lock gate every outermost edit passes, the shell's
//! answers, remote payloads, the caret report and per-user undo.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

use serde_json::{Value as Json, json};
use wordcraft_doc::ids::{block_id, format_para_id, parse_para_id};
use wordcraft_doc::{Document, Path, Pos, StoryRef};

use super::diff::{self, Scope};
use super::payload::{self, Wire};
use super::undo::Step;
use super::{Action, Call, DOC_IDLE_MS, Feedback, Held, IDLE_MS, LEAVE_MS, LOCK_DENIED, LOCKED_BY, Live, MAX_REGIONS, NOTHING_TO_UNDO, Region};
use crate::{CmdError, EditSnapshot, Selection, Session};

/// Commands that replace, save or print the document rather than edit it: never shared.
const NOT_EDITS: &[&str] = &[
    "file.open",
    "file.new",
    "file.newFromTemplate",
    "file.close",
    "file.recover",
    "file.save",
    "file.saveAs",
    "file.saveTemplate",
    "file.exportPdf",
    "file.exportPng",
    "file.print",
    "file.printPdf",
    "file.printMarkup",
];

/// One region's share of an edit: the top-level blocks it now holds and what they replace.
struct Run {
    region: Region,
    range: Range<usize>,
    remove: Vec<u32>,
    prev: Option<u32>,
    next: Option<u32>,
}

struct Runs {
    regions: Vec<Region>,
    runs: Vec<Run>,
    fresh: Vec<(u32, Region)>,
    old: Range<usize>,
    new: Range<usize>,
}

enum Plan {
    Doc,
    Runs(Box<Runs>),
}

enum Verdict {
    Unchanged,
    Committed(Option<Box<Step>>),
    Waiting,
    Refused,
}

/// How blocks `old` of `before` became blocks `new` of `after`, region by region; `None` when it
/// can't be told that way (a move, a block without an id, too many regions, a run in pieces),
/// which makes the edit a whole-document one.
fn plan_runs(live: &Live, before: &Document, after: &Document, old: Range<usize>, new: Range<usize>) -> Option<Plan> {
    let old_blocks = before.body.get(old.clone())?;
    let new_ids = diff::ids_of(after.body.get(new.clone())?)?;
    let mut region_of_old: HashMap<u32, Region> = HashMap::new();
    let mut regions: Vec<Region> = Vec::new();
    for b in old_blocks {
        let r = live.region_of(b)?;
        region_of_old.insert(block_id(b)?, r);
        if !regions.contains(&r) {
            regions.push(r);
        }
    }
    if regions.len() > MAX_REGIONS {
        return None;
    }
    let mut assigned: Vec<Option<Region>> = Vec::with_capacity(new_ids.len());
    let mut elsewhere: Option<HashSet<u32>> = None;
    for id in &new_ids {
        if let Some(r) = region_of_old.get(id) {
            assigned.push(Some(*r));
            continue;
        }
        let ids = elsewhere.get_or_insert_with(|| before.body.iter().filter_map(|b| block_id(b)).collect());
        if ids.contains(id) {
            return None;
        }
        assigned.push(None);
    }
    // A block made by this edit follows the region before it, else the one after it.
    let mut fresh = Vec::new();
    let mut last = None;
    for k in 0..assigned.len() {
        match assigned.get(k).copied().flatten() {
            Some(r) => last = Some(r),
            None => {
                let r = last.or_else(|| assigned.get(k..).into_iter().flatten().flatten().next().copied()).or(regions.first().copied())?;
                if let (Some(slot), Some(id)) = (assigned.get_mut(k), new_ids.get(k)) {
                    *slot = Some(r);
                    fresh.push((*id, r));
                }
                last = Some(r);
            }
        }
    }
    let after_ids = payload::body_ids(after);
    let mut runs = Vec::new();
    for r in &regions {
        let mut members: HashSet<u32> = live.held.get(r).map(|h| h.followers.iter().copied().collect()).unwrap_or_default();
        members.extend(r.block());
        members.extend(fresh.iter().filter(|(_, x)| x == r).map(|(id, _)| *id));
        let at: Vec<usize> = after_ids.iter().enumerate().filter(|(_, id)| id.is_some_and(|i| members.contains(&i))).map(|(k, _)| k).collect();
        let range = match (at.first(), at.last()) {
            (Some(&a), Some(&z)) if z + 1 - a == at.len() => a..z + 1,
            (None, _) => new.start..new.start,
            _ => return None,
        };
        let mut remove: BTreeSet<u32> = live.held.get(r).map(|h| h.sent.clone()).unwrap_or_default();
        remove.extend(r.block());
        let prev = range.start.checked_sub(1).and_then(|k| after_ids.get(k).copied().flatten());
        let next = after_ids.get(range.end).copied().flatten();
        runs.push(Run { region: *r, range, remove: remove.into_iter().collect(), prev, next });
    }
    // The runs, applied to the document as it was, must give this one (as the others apply them).
    let mut sim = payload::body_ids(before);
    for run in &runs {
        let (gone, at) = payload::place(&sim, &run.remove, run.prev, run.next);
        for k in gone.iter().rev() {
            sim.remove(*k);
        }
        let at = at.min(sim.len());
        sim.splice(at..at, after_ids.get(run.range.clone()).unwrap_or_default().iter().copied());
    }
    (sim == after_ids).then(|| Plan::Runs(Box::new(Runs { regions, runs, fresh, old, new })))
}

/// A body position as block id, path below it and offset: `0A1B2C3D:5`, `0A1B2C3D.1.0.0:3`.
fn pos_text(doc: &Document, p: &Pos) -> Option<String> {
    if p.story != StoryRef::Body {
        return None;
    }
    let (first, rest) = p.path.0.split_first()?;
    let id = block_id(doc.body.get(*first as usize)?)?;
    let mut s = format_para_id(id);
    for i in rest {
        s.push('.');
        s.push_str(&i.to_string());
    }
    Some(format!("{s}:{}", p.off))
}

fn pos_of(doc: &Document, text: &str) -> Option<Pos> {
    let (path, off) = text.rsplit_once(':')?;
    let mut parts = path.split('.');
    let id = parse_para_id(parts.next()?)?;
    let top = doc.body.iter().position(|b| block_id(b) == Some(id))?;
    let mut v = vec![u32::try_from(top).ok()?];
    for p in parts {
        v.push(p.parse().ok()?);
    }
    Some(doc.clamp(&Pos::new(StoryRef::Body, Path(v), off.parse().ok()?)))
}

/// `selectionChanged`'s range: anchor and focus as [`pos_text`], `;` between.
pub fn selection_range(doc: &Document, sel: &Selection) -> Option<String> {
    Some(format!("{};{}", pos_text(doc, &sel.anchor)?, pos_text(doc, &sel.focus)?))
}

/// A cursor's range in `doc` (anchor, focus); `None` when its blocks aren't here.
pub fn range_positions(doc: &Document, range: &str) -> Option<(Pos, Pos)> {
    let (a, f) = range.split_once(';')?;
    Some((pos_of(doc, a)?, pos_of(doc, f)?))
}

/// A selection end held by block id, to find again after the blocks around it moved.
struct Mark {
    id: u32,
    rest: Vec<u32>,
    off: usize,
}

fn mark(doc: &Document, p: &Pos) -> Option<Mark> {
    if p.story != StoryRef::Body {
        return None;
    }
    let (first, rest) = p.path.0.split_first()?;
    Some(Mark { id: block_id(doc.body.get(*first as usize)?)?, rest: rest.to_vec(), off: p.off })
}

fn unmark(doc: &Document, m: Option<&Mark>, p: &Pos, fallback: usize) -> Pos {
    let Some(m) = m else { return doc.clamp(p) };
    match doc.body.iter().position(|b| block_id(b) == Some(m.id)).and_then(|k| u32::try_from(k).ok()) {
        Some(k) => {
            let mut path = vec![k];
            path.extend(&m.rest);
            doc.clamp(&Pos::new(StoryRef::Body, Path(path), m.off))
        }
        None => doc.clamp(&Pos::new(StoryRef::Body, Path(vec![u32::try_from(fallback).unwrap_or(0)]), 0)),
    }
}

impl Session {
    /// Whether this session's document is shared region by region.
    pub fn live_on(&self) -> bool {
        self.live.is_some()
    }

    /// An outermost command while co-editing; `None` leaves it to the plain path.
    pub(crate) fn live_run(&mut self, id: &str, params: &Json) -> Option<Result<Json, CmdError>> {
        self.live.as_ref()?;
        if matches!(id, "edit.undo" | "edit.redo" | "edit.undoTo") {
            if self.granted().read_only {
                return Some(Err(CmdError::Disabled(format!("{id}: {}", crate::access::Refusal::ReadOnly.reason()))));
            }
            let count = if id == "edit.undoTo" { params.get("count").and_then(Json::as_u64).unwrap_or(1).clamp(1, 1000) } else { 1 };
            for _ in 0..count {
                if !self.live_undo(id == "edit.redo") {
                    break;
                }
            }
            return Some(Ok(json!({})));
        }
        if !self.registry.get(id).is_some_and(|s| s.mutates) || NOT_EDITS.contains(&id) {
            // Caret moves between edits waiting for a lock run again with them, or the edits
            // after them would land where the caret was before.
            if (id.starts_with("caret.") || id.starts_with("select."))
                && let Some(live) = self.live.as_mut()
                && !live.queue.is_empty()
            {
                live.queue.push(Action::Command(id.to_string(), params.clone()));
            }
            return None;
        }
        let snap = self.edit_snapshot();
        let sel = self.sel.clone();
        let document = self.document_id();
        let result = self.run_plain(id, params);
        if self.document_id() != document {
            // The command put another document here (Envelopes, Labels): not the shared one.
            self.live = None;
            return Some(result);
        }
        if result.is_ok() {
            let typing = id == "text.insert";
            if let Verdict::Committed(step) = self.live_gate(Action::Command(id.to_string(), params.clone()), snap, sel.clone()) {
                let after_sel = self.sel.clone();
                if let Some(live) = self.live.as_mut() {
                    live.redos.clear();
                    if let Some(mut step) = step.map(|b| *b) {
                        if let Step::Blocks { typing: t, .. } = &mut step {
                            *t = typing;
                        }
                        let joins = typing && live.typing_at.as_ref() == Some(&sel) && live.steps.last().is_some_and(|last| last.joins(&step));
                        match (joins, live.steps.pop()) {
                            (true, Some(last)) => live.steps.push(last.join(step)),
                            (_, last) => {
                                live.steps.extend(last);
                                live.record(step);
                            }
                        }
                    }
                    live.typing_at = typing.then_some(after_sel);
                }
            }
        }
        Some(result)
    }

    /// Lets the edit just made (from `snap`, the selection then `sel`) stand only if its regions'
    /// locks are ours; otherwise takes it back and asks for them, or tells who holds one.
    fn live_gate(&mut self, action: Action, snap: EditSnapshot, sel: Selection) -> Verdict {
        let Some(live) = self.live.as_ref() else { return Verdict::Unchanged };
        let plan = match diff::scope(snap.doc(), &self.doc) {
            Scope::Unchanged => return Verdict::Unchanged,
            Scope::Doc => Plan::Doc,
            Scope::Blocks { old, new } => plan_runs(live, snap.doc(), &self.doc, old, new).unwrap_or(Plan::Doc),
        };
        let regions = match &plan {
            Plan::Doc => vec![Region::Doc],
            Plan::Runs(p) => p.regions.clone(),
        };
        let mut ask = Vec::new();
        let mut blocked = None;
        for r in &regions {
            if live.holds(*r) {
                continue;
            }
            if let Some(l) = live.blocker(*r) {
                blocked = Some(l.name.clone());
                break;
            }
            ask.push(*r);
        }
        let waiting = !live.queue.is_empty();
        if let Some(name) = blocked {
            self.restore(snap);
            if let Some(live) = self.live.as_mut() {
                live.feedback = Some(Feedback { text: LOCKED_BY, name });
            }
            return Verdict::Refused;
        }
        if waiting || !ask.is_empty() {
            self.restore(snap);
            if let Some(live) = self.live.as_mut() {
                if live.queue.is_empty() {
                    live.queue_sel = Some(sel);
                }
                live.queue.push(action);
                for r in ask {
                    if live.asked.insert(r) {
                        live.calls.push(Call::WillEdit(r.id()));
                    }
                }
            }
            return Verdict::Waiting;
        }
        let before = snap.doc().clone();
        // The others get the document as its DOCX reads back; so does this page, or the first
        // run sent after would carry differences only the round trip made.
        let docx = matches!(plan, Plan::Doc).then(|| crate::io::save_bytes("live.docx", &self.doc));
        if let Some(Ok(bytes)) = &docx {
            match crate::io::open_bytes("live.docx", bytes) {
                Ok(doc) => self.adopt(doc),
                Err(e) => log::warn!("co-editing: the document didn't read back: {e}"),
            }
        }
        let Some(live) = self.live.as_mut() else { return Verdict::Unchanged };
        let (now, online) = (live.now, live.online);
        let Live { held, owner, calls, hashes, known_media, .. } = live;
        let step = match plan {
            Plan::Doc => {
                match docx {
                    Some(Ok(bytes)) => calls.push(Call::Changed(Region::Doc.id(), payload::encode_doc(&bytes))),
                    Some(Err(e)) => log::warn!("co-editing: couldn't write the document for the room: {e}"),
                    None => {}
                }
                held.entry(Region::Doc).or_insert_with(|| Held { offline: !online, ..Held::default() }).used = now;
                Some(Step::Doc { before, after: Box::new(self.doc.clone()), sel })
            }
            Plan::Runs(p) => {
                let Runs { regions, runs, fresh, old, new } = *p;
                for r in &regions {
                    held.entry(*r).or_insert_with(|| Held { offline: !online, ..Held::default() }).used = now;
                }
                for (id, r) in fresh {
                    owner.insert(id, r);
                    if let Some(h) = held.get_mut(&r) {
                        h.followers.push(id);
                    }
                }
                // Styles and lists the edit added go along even when no block uses them yet.
                let new_styles: Vec<_> =
                    self.doc.styles.styles.iter().filter(|st| !before.styles.styles.iter().any(|b| b.id == st.id)).cloned().collect();
                let new_nums: Vec<_> =
                    self.doc.numbering.nums.iter().filter(|n| !before.numbering.nums.iter().any(|b| b.id == n.id)).cloned().collect();
                let new_abstracts: Vec<_> =
                    self.doc.numbering.abstracts.iter().filter(|a| !before.numbering.abstracts.iter().any(|b| b.id == a.id)).cloned().collect();
                for run in runs {
                    let blocks = self.doc.body.get(run.range.clone()).unwrap_or_default();
                    let Some(h) = held.get_mut(&run.region) else { continue };
                    let mut inlined = Vec::new();
                    let p = payload::capture(&self.doc, blocks, hashes, &mut |hash| {
                        let inline = !known_media.contains(hash);
                        if inline {
                            inlined.push(hash.to_string());
                        }
                        inline
                    });
                    let mut p = p;
                    for st in &new_styles {
                        if !p.styles.iter().any(|x| x.id == st.id) {
                            p.styles.push(st.clone());
                        }
                    }
                    for n in &new_nums {
                        if !p.nums.iter().any(|x| x.id == n.id) {
                            p.nums.push(n.clone());
                        }
                    }
                    for a in &new_abstracts {
                        if !p.abstracts.iter().any(|x| x.id == a.id) {
                            p.abstracts.push(a.clone());
                        }
                    }
                    h.media.extend(inlined);
                    h.sent.extend(blocks.iter().filter_map(|b| block_id(b)));
                    calls.push(Call::Changed(run.region.id(), payload::encode_run(&run.remove, run.prev, run.next, p)));
                }
                Step::blocks(before, &self.doc, old, new, sel, false, hashes)
            }
        };
        Verdict::Committed(step.map(Box::new))
    }

    /// `regionWillEdit`'s answer. Once every lock asked for is granted the edits that waited run
    /// again, from where the first of them started.
    pub fn live_answer(&mut self, region: &str, granted: bool) {
        let Some(r) = Region::parse(region) else { return };
        let Some(live) = self.live.as_mut() else { return };
        let asked = live.asked.remove(&r);
        if granted {
            let now = live.now;
            live.held.entry(r).or_insert_with(|| Held { used: now, ..Held::default() });
        } else if asked {
            live.queue.clear();
            live.queue_sel = None;
            live.feedback = Some(Feedback::plain(LOCK_DENIED));
        }
        if !live.asked.is_empty() || live.queue.is_empty() {
            return;
        }
        let queue = std::mem::take(&mut live.queue);
        if let Some(sel) = live.queue_sel.take() {
            self.sel = sel;
            self.clamp_selection();
        }
        for action in queue {
            match action {
                Action::Command(id, params) => {
                    if let Err(e) = self.run(&id, &params) {
                        log::warn!("co-editing: {id} failed when its lock came: {e}");
                    }
                }
                Action::Undo(redo) => {
                    self.live_undo(redo);
                }
            }
        }
    }

    /// `lockDenied(region)`: the lock is gone (expired, or refused for an update).
    pub fn live_lock_denied(&mut self, region: &str) {
        let Some(r) = Region::parse(region) else { return };
        let Some(live) = self.live.as_mut() else { return };
        live.drop_held(r);
        live.asked.remove(&r);
        live.queue.clear();
        live.queue_sel = None;
        live.feedback = Some(Feedback::plain(LOCK_DENIED));
    }

    /// `applyRemote(region, payload)`: another editor's region content replaces ours.
    pub fn live_apply_remote(&mut self, region: &str, payload: &str) -> Result<(), String> {
        if self.live.is_none() {
            return Err("not co-editing".into());
        }
        Region::parse(region).ok_or_else(|| format!("unknown region {region:?}"))?;
        let marks = (mark(&self.doc, &self.sel.anchor), mark(&self.doc, &self.sel.focus));
        let fallback = match payload::decode(payload).map_err(|e| e.to_string())? {
            Wire::Run { remove, prev, next, payload } => {
                let (gone, at) = payload::place(&payload::body_ids(&self.doc), &remove, prev, next);
                let range = payload::replace(&mut self.doc, &gone, at, &payload).map_err(|e| e.to_string())?;
                if let Some(live) = self.live.as_mut() {
                    live.known_media.extend(payload.media.keys().map(|k| payload::hash_of_wire_key(k).to_string()));
                }
                range.start
            }
            Wire::Doc(bytes) => {
                self.adopt(crate::io::open_bytes("live.docx", &bytes)?);
                self.touch();
                return Ok(());
            }
        };
        self.remark(marks, fallback);
        self.touch();
        Ok(())
    }

    /// Puts `doc` in place of the document, the selection kept on its blocks.
    fn adopt(&mut self, mut doc: Document) {
        let marks = (mark(&self.doc, &self.sel.anchor), mark(&self.doc, &self.sel.focus));
        let fallback = self.sel.focus.path.0.first().map_or(0, |k| *k as usize);
        doc.ensure_nonempty();
        wordcraft_doc::ids::assign_missing(&mut doc);
        self.doc = doc;
        if let Some(live) = self.live.as_mut() {
            live.learn_media(&self.doc);
        }
        self.remark(marks, fallback);
    }

    fn remark(&mut self, marks: (Option<Mark>, Option<Mark>), fallback: usize) {
        self.sel.anchor = unmark(&self.doc, marks.0.as_ref(), &self.sel.anchor, fallback);
        self.sel.focus = unmark(&self.doc, marks.1.as_ref(), &self.sel.focus, fallback);
        self.column = None;
        self.widened = None;
        self.also_selected.clear();
        self.clamp_selection();
    }

    /// Every frame: reports this user's selection when it moved, and hands back regions the caret
    /// left or nobody edits any more.
    pub fn live_tick(&mut self, now: f64) {
        let Some(live) = self.live.as_mut() else { return };
        live.now = now;
        let caret = match (&self.sel.focus.story, self.sel.focus.path.0.first()) {
            (StoryRef::Body, Some(k)) => self.doc.body.get(*k as usize).and_then(|b| live.region_of(b)),
            _ => None,
        };
        if let (Some(region), Some(range)) = (caret, selection_range(&self.doc, &self.sel)) {
            let report = (region.id(), range);
            if live.reported.as_ref() != Some(&report) {
                live.calls.push(Call::Selection(report.0.clone(), report.1.clone()));
                live.reported = Some(report);
            }
        }
        if !live.queue.is_empty() || !live.asked.is_empty() {
            return;
        }
        let gone: Vec<Region> = live
            .held
            .iter()
            .filter(|(r, h)| {
                let idle = now - h.used;
                match r {
                    Region::Doc => idle >= DOC_IDLE_MS,
                    _ => idle >= IDLE_MS || (caret != Some(**r) && idle >= LEAVE_MS),
                }
            })
            .map(|(r, _)| *r)
            .collect();
        for r in gone {
            live.release(r);
        }
    }

    /// Undo (or redo) this user's newest step that is still current; steps others overwrote are
    /// dropped. False when there was none left.
    fn live_undo(&mut self, redo: bool) -> bool {
        let step = loop {
            let Some(live) = self.live.as_mut() else { return false };
            let Live { steps, redos, hashes, .. } = live;
            let stack = if redo { redos } else { steps };
            let Some(step) = stack.last() else {
                live.feedback = Some(Feedback::plain(NOTHING_TO_UNDO));
                return false;
            };
            if step.current(&self.doc, hashes) {
                break step.clone();
            }
            stack.pop();
        };
        let snap = self.edit_snapshot();
        let sel = self.sel.clone();
        let reverted = match self.live.as_mut() {
            Some(live) => step.revert(&mut self.doc, &mut live.hashes),
            None => false,
        };
        if !reverted {
            self.restore(snap);
            return false;
        }
        self.sel = step.sel().clone();
        self.clamp_selection();
        self.touch();
        let verdict = self.live_gate(Action::Undo(redo), snap, sel);
        let Some(live) = self.live.as_mut() else { return false };
        live.typing_at = None;
        match verdict {
            Verdict::Committed(reverse) => {
                let (from, to) = if redo { (&mut live.redos, &mut live.steps) } else { (&mut live.steps, &mut live.redos) };
                from.pop();
                to.extend(reverse.map(|b| *b));
                true
            }
            Verdict::Waiting => true,
            Verdict::Refused => false,
            Verdict::Unchanged => {
                if redo {
                    live.redos.pop()
                } else {
                    live.steps.pop()
                };
                true
            }
        }
    }
}
