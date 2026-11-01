//! Per-user undo while co-editing: each local edit keeps the document before it and a fingerprint
//! of what it left. Undo puts the old blocks back only while the blocks it left are still exactly
//! there; once someone else changed them the step is skipped, so undo never takes back others'
//! work.

use std::ops::Range;

use wordcraft_doc::Document;
use wordcraft_doc::ids::block_id;

use super::payload::{self, MediaHashes};
use crate::Selection;

#[derive(Clone)]
pub(crate) enum Step {
    Blocks {
        before: Document,
        old: Range<usize>,
        /// The top-level blocks the edit left, and their neighbours (to find an empty run).
        current: Vec<u32>,
        prev: Option<u32>,
        next: Option<u32>,
        after: String,
        sel: Selection,
        /// Typing, which later keystrokes in the same blocks join.
        typing: bool,
    },
    Doc {
        before: Document,
        after: Box<Document>,
        sel: Selection,
    },
}

/// Where `ids` stand as consecutive top-level blocks of `doc`, or where an empty run sits
/// between `prev` and `next`.
fn locate(doc: &Document, ids: &[u32], prev: Option<u32>, next: Option<u32>) -> Option<Range<usize>> {
    let pos = |id: u32| doc.body.iter().position(|b| block_id(b) == Some(id));
    match ids.first() {
        Some(&first) => {
            let start = pos(first)?;
            let ok = ids.iter().enumerate().all(|(k, id)| doc.body.get(start + k).is_some_and(|b| block_id(b) == Some(*id)));
            ok.then_some(start..start + ids.len())
        }
        None => {
            let at = match (prev, next) {
                (Some(p), _) => pos(p)? + 1,
                (None, Some(n)) => pos(n)?,
                (None, None) => return None,
            };
            let next_ok = next.is_none_or(|n| doc.body.get(at).is_some_and(|b| block_id(b) == Some(n)));
            next_ok.then_some(at..at)
        }
    }
}

impl Step {
    pub(crate) fn blocks(
        before: Document,
        after: &Document,
        old: Range<usize>,
        new: Range<usize>,
        sel: Selection,
        typing: bool,
        hashes: &mut MediaHashes,
    ) -> Option<Step> {
        let left = after.body.get(new.clone())?;
        let current = super::diff::ids_of(left)?;
        let prev = new.start.checked_sub(1).and_then(|k| after.body.get(k)).and_then(|b| block_id(b));
        let next = after.body.get(new.end).and_then(|b| block_id(b));
        let fp = payload::fingerprint(after, left, hashes);
        Some(Step::Blocks { before, old, current, prev, next, after: fp, sel, typing })
    }

    pub(crate) fn sel(&self) -> &Selection {
        match self {
            Step::Blocks { sel, .. } | Step::Doc { sel, .. } => sel,
        }
    }

    /// Whether what the step left is still exactly in `doc`.
    pub(crate) fn current(&self, doc: &Document, hashes: &mut MediaHashes) -> bool {
        match self {
            Step::Blocks { current, prev, next, after, .. } => {
                locate(doc, current, *prev, *next).and_then(|r| doc.body.get(r)).is_some_and(|left| payload::fingerprint(doc, left, hashes) == *after)
            }
            Step::Doc { after, .. } => *doc == **after,
        }
    }

    /// Puts back what the step replaced; false when it can't (not [`Step::current`]).
    pub(crate) fn revert(&self, doc: &mut Document, hashes: &mut MediaHashes) -> bool {
        match self {
            Step::Blocks { before, old, current, prev, next, .. } => {
                let Some(blocks) = before.body.get(old.clone()) else { return false };
                let back = payload::capture(before, blocks, hashes, &mut |_| true);
                let Some(range) = locate(doc, current, *prev, *next) else { return false };
                let gone: Vec<usize> = range.clone().collect();
                payload::replace(doc, &gone, range.start, &back).is_ok()
            }
            Step::Doc { before, .. } => {
                *doc = before.clone();
                true
            }
        }
    }

    /// Typing that continues `self`: same blocks, typed right after it.
    pub(crate) fn joins(&self, later: &Step) -> bool {
        match (self, later) {
            (Step::Blocks { current, typing: true, .. }, Step::Blocks { before, old, typing: true, .. }) => {
                before.body.get(old.clone()).and_then(super::diff::ids_of).is_some_and(|ids| ids == *current)
            }
            _ => false,
        }
    }

    /// `self` followed by `later` as one step: `self`'s before, `later`'s after.
    pub(crate) fn join(self, later: Step) -> Step {
        match (self, later) {
            (Step::Blocks { before, old, sel, .. }, Step::Blocks { current, prev, next, after, typing, .. }) => {
                Step::Blocks { before, old, current, prev, next, after, sel, typing }
            }
            (first, _) => first,
        }
    }
}
