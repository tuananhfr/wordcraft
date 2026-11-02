//! What another editor's run may do, checked before it touches the document. The server checks
//! only that the sender holds the lock string it names, so the receiver is the one place that
//! keeps a run to its region: the blocks it replaces, the ids it brings and its sections.

use std::collections::HashSet;
use std::sync::Arc;

use wordcraft_doc::ids::{MAX_PARA_ID, block_id};
use wordcraft_doc::{Block, Document, InlineObject};

use super::Region;
use super::payload::RegionPayload;
use super::walk::MAX_DEPTH;

/// Revisions one run may carry: each is matched against the document's, which only grow.
pub(super) const MAX_REVISIONS: usize = 4096;

/// A run of `region` that would replace the blocks at `gone` of `doc` with `payload`. Besides
/// its own block a run replaces the blocks earlier runs of its lock brought, which a page that
/// reloaded since can't tell from others: `locked` says which top-level blocks someone else's
/// lock or this page's covers, and those it may not replace.
pub(super) fn check_run(doc: &Document, region: Region, locked: &dyn Fn(u32) -> bool, gone: &[usize], payload: &RegionPayload) -> Result<(), &'static str> {
    let own = region.block().ok_or("a run names no paragraph or table")?;
    if gone.iter().filter_map(|k| doc.body.get(*k)).filter_map(|b| block_id(b)).any(|id| id != own && locked(id)) {
        return Err("the run replaces blocks someone else is editing");
    }
    if payload.revisions.len() > MAX_REVISIONS {
        return Err("the run carries too many revisions");
    }
    let stories = payload.parts.values().flat_map(|p| p.blocks.iter().map(|b| &**b));
    if !payload.blocks.iter().chain(stories).all(|b| within_depth(b, 0)) {
        return Err("the run nests too deep");
    }

    // Ids: every top-level block has one, none repeats, and those not replaced are new here.
    let old: Vec<Arc<Block>> = gone.iter().filter_map(|k| doc.body.get(*k).cloned()).collect();
    let mut replaced = Vec::new();
    for b in &old {
        ids_in(b, &mut replaced);
    }
    let replaced: HashSet<u32> = replaced.into_iter().flatten().collect();
    let mut elsewhere = Vec::new();
    for (k, b) in doc.body.iter().enumerate() {
        if !gone.contains(&k) {
            ids_in(b, &mut elsewhere);
        }
    }
    let elsewhere: HashSet<u32> = elsewhere.into_iter().flatten().collect();
    let mut seen = HashSet::new();
    for b in &payload.blocks {
        if block_id(b).is_none() {
            return Err("a block of the run has no id");
        }
        let mut ids = Vec::new();
        ids_in(b, &mut ids);
        for id in ids.into_iter().flatten() {
            if !(1..=MAX_PARA_ID).contains(&id) || !seen.insert(id) || (elsewhere.contains(&id) && !replaced.contains(&id)) {
                return Err("the run brings a block id that is invalid, repeated or taken");
            }
        }
    }

    // Section breaks moving in or out take `*` (see `diff::scope`).
    let sections = |blocks: &mut dyn Iterator<Item = &Block>| blocks.filter_map(|b| b.as_para()).filter_map(|p| p.section.clone()).collect::<Vec<_>>();
    if sections(&mut old.iter().map(|b| &**b)) != sections(&mut payload.blocks.iter()) {
        return Err("the run changes section breaks");
    }

    Ok(())
}

/// Tables and drawing groups nest no deeper than the walks over references go.
fn within_depth(b: &Block, depth: usize) -> bool {
    fn group_ok(o: &InlineObject, depth: usize) -> bool {
        match o {
            InlineObject::Group { children, .. } => depth < MAX_DEPTH && children.iter().all(|c| group_ok(&c.obj, depth + 1)),
            _ => true,
        }
    }
    match b {
        Block::Para(p) => p.objects.iter().all(|o| group_ok(o, 0)),
        Block::Table(t) => {
            depth < MAX_DEPTH && t.rows.iter().flat_map(|r| r.cells.iter()).flat_map(|c| c.blocks.iter()).all(|cb| within_depth(cb, depth + 1))
        }
    }
}

/// Paragraph and row ids of a block, nested ones too (bounded by [`within_depth`] for payloads).
fn ids_in(b: &Block, out: &mut Vec<Option<u32>>) {
    match b {
        Block::Para(p) => out.push(p.id),
        Block::Table(t) => {
            for r in &t.rows {
                out.push(r.id);
                for cb in r.cells.iter().flat_map(|c| c.blocks.iter()) {
                    ids_in(cb, out);
                }
            }
        }
    }
}
