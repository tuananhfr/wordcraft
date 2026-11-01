//! What a local edit changed: the smallest range of top-level blocks to resend, or the whole
//! document when something outside the body's blocks changed (styles, list definitions,
//! sections, headers and footers, settings, properties…).

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use wordcraft_doc::ids::block_id;
use wordcraft_doc::{Block, Document};

use super::walk::{comment_ids, part_ids};

#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    Unchanged,
    /// Top-level blocks `old` of the document before became blocks `new` of the one after.
    Blocks {
        old: Range<usize>,
        new: Range<usize>,
    },
    Doc,
}

fn same(a: &Arc<Block>, b: &Arc<Block>) -> bool {
    Arc::ptr_eq(a, b) || a == b && ids_equal(a, b)
}

fn ids_equal(a: &Block, b: &Block) -> bool {
    let mut x = Vec::new();
    let mut y = Vec::new();
    collect_ids(a, &mut x, 0);
    collect_ids(b, &mut y, 0);
    x == y
}

fn collect_ids(b: &Block, out: &mut Vec<Option<u32>>, depth: usize) {
    match b {
        Block::Para(p) => out.push(p.id),
        Block::Table(t) if depth < 16 => {
            for r in &t.rows {
                out.push(r.id);
                for cb in r.cells.iter().flat_map(|c| c.blocks.iter()) {
                    collect_ids(cb, out, depth + 1);
                }
            }
        }
        Block::Table(_) => {}
    }
}

fn blocks_same(a: &[Arc<Block>], b: &[Arc<Block>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
}

/// Top-level block index of the block that owns each story (notes, text boxes, comment texts).
fn story_owners(doc: &Document) -> HashMap<u32, usize> {
    let mut m = HashMap::new();
    for (i, b) in doc.body.iter().enumerate() {
        let mut queue = part_ids(b);
        for c in comment_ids(b) {
            if let Some(cm) = doc.comments.get(&c) {
                queue.push(cm.part);
            }
        }
        let mut qi = 0;
        while let Some(&id) = queue.get(qi) {
            qi += 1;
            if m.contains_key(&id) || qi > 4096 {
                continue;
            }
            m.insert(id, i);
            if let Some(p) = doc.parts.get(&id) {
                queue.extend(p.body.next);
                for pb in &p.blocks {
                    queue.extend(part_ids(pb));
                }
            }
        }
    }
    m
}

fn comment_owners(doc: &Document) -> HashMap<u32, usize> {
    let mut m = HashMap::new();
    for (i, b) in doc.body.iter().enumerate() {
        for c in comment_ids(b) {
            m.entry(c).or_insert(i);
        }
    }
    m
}

/// Styles that only gained new ones travel with the blocks that use them.
fn styles_grew_only(a: &wordcraft_doc::styles::StyleSheet, b: &wordcraft_doc::styles::StyleSheet) -> bool {
    if a == b {
        return true;
    }
    let mut kept = b.clone();
    kept.styles.retain(|s| a.styles.iter().any(|x| x.id == s.id));
    kept == *a
}

fn lists_grew_only(a: &wordcraft_doc::numbering::Numbering, b: &wordcraft_doc::numbering::Numbering) -> bool {
    if a == b {
        return true;
    }
    let mut kept = b.clone();
    kept.nums.retain(|n| a.nums.iter().any(|x| x.id == n.id));
    kept.abstracts.retain(|n| a.abstracts.iter().any(|x| x.id == n.id));
    kept == *a
}

/// Whether anything outside the body's blocks and the stories they own changed.
fn doc_level(before: &Document, after: &Document) -> bool {
    let Document {
        body: _,
        last_section,
        parts: _,
        styles,
        numbering,
        comments: _,
        revisions: _,
        settings,
        core,
        sources,
        custom_props,
        footnote_separators,
        endnote_separators,
        media: _,
        passthrough,
    } = before;
    *last_section != after.last_section
        || !styles_grew_only(styles, &after.styles)
        || !lists_grew_only(numbering, &after.numbering)
        || *settings != after.settings
        || *core != after.core
        || *sources != after.sources
        || *custom_props != after.custom_props
        || *footnote_separators != after.footnote_separators
        || *endnote_separators != after.endnote_separators
        || *passthrough != after.passthrough
}

/// Compares a document before and after a command.
pub fn scope(before: &Document, after: &Document) -> Scope {
    if doc_level(before, after) {
        return Scope::Doc;
    }
    let (a, b) = (&before.body, &after.body);
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a.get(pre).zip(b.get(pre)).is_some_and(|(x, y)| same(x, y)) {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a.get(a.len() - 1 - suf).zip(b.get(b.len() - 1 - suf)).is_some_and(|(x, y)| same(x, y)) {
        suf += 1;
    }

    // Stories and comments: attributed to the block that owns them, else the whole document.
    let mut extra_old: Vec<usize> = Vec::new();
    let mut extra_new: Vec<usize> = Vec::new();
    let parts_changed = before.parts.len() != after.parts.len()
        || before
            .parts
            .iter()
            .zip(&after.parts)
            .any(|((ka, pa), (kb, pb))| ka != kb || pa.kind != pb.kind || pa.body != pb.body || !blocks_same(&pa.blocks, &pb.blocks));
    if parts_changed {
        let owners_a = story_owners(before);
        let owners_b = story_owners(after);
        let keys: BTreeSet<u32> = before.parts.keys().chain(after.parts.keys()).copied().collect();
        for k in keys {
            let changed = match (before.parts.get(&k), after.parts.get(&k)) {
                (Some(x), Some(y)) => x.kind != y.kind || x.body != y.body || !blocks_same(&x.blocks, &y.blocks),
                (None, None) => false,
                _ => true,
            };
            if !changed {
                continue;
            }
            match (owners_a.get(&k), owners_b.get(&k)) {
                (None, None) => return Scope::Doc,
                (oa, ob) => {
                    extra_old.extend(oa);
                    extra_new.extend(ob);
                }
            }
        }
    }
    if before.comments != after.comments {
        let (ca, cb) = (comment_owners(before), comment_owners(after));
        let keys: BTreeSet<u32> = before.comments.keys().chain(after.comments.keys()).copied().collect();
        for c in keys {
            if before.comments.get(&c) == after.comments.get(&c) {
                continue;
            }
            match (ca.get(&c), cb.get(&c)) {
                (None, None) => return Scope::Doc,
                (oa, ob) => {
                    extra_old.extend(oa);
                    extra_new.extend(ob);
                }
            }
        }
    }

    let (mut lo_a, mut hi_a) = (pre, a.len() - suf);
    let (mut lo_b, mut hi_b) = (pre, b.len() - suf);
    // Body untouched (a note, text box or comment edited): the owners alone are the range.
    if lo_a == hi_a && lo_b == hi_b && (!extra_old.is_empty() || !extra_new.is_empty()) {
        let all = extra_old.iter().chain(&extra_new);
        let (lo, hi) = (all.clone().copied().min().unwrap_or(0), all.copied().max().map_or(0, |m| m + 1));
        (lo_a, hi_a, lo_b, hi_b) = (lo, hi.min(a.len()), lo, hi.min(b.len()));
        extra_old.clear();
        extra_new.clear();
    }
    // An owner of a changed story joins the range (same offset on both sides outside it).
    for i in extra_old {
        if i < lo_a {
            let k = lo_a - i;
            lo_a -= k;
            lo_b = lo_b.saturating_sub(k);
        } else if i >= hi_a {
            let k = i + 1 - hi_a;
            hi_a += k;
            hi_b = (hi_b + k).min(b.len());
        }
    }
    for i in extra_new {
        if i < lo_b {
            let k = lo_b - i;
            lo_b -= k;
            lo_a = lo_a.saturating_sub(k);
        } else if i >= hi_b {
            let k = i + 1 - hi_b;
            hi_b += k;
            hi_a = (hi_a + k).min(a.len());
        }
    }
    if lo_a >= hi_a && lo_b >= hi_b {
        // New styles or lists with no block to carry them.
        let grew = before.styles != after.styles || before.numbering != after.numbering;
        return if grew { Scope::Doc } else { Scope::Unchanged };
    }
    // A pure insertion takes its neighbour along, so the change names a region that exists.
    if lo_a == hi_a {
        if lo_a > 0 && lo_b > 0 {
            lo_a -= 1;
            lo_b -= 1;
        } else if hi_a < a.len() && hi_b < b.len() {
            hi_a += 1;
            hi_b += 1;
        }
    }
    if lo_a == hi_a {
        return Scope::Doc;
    }
    // A section break moving in or out changes pagination and which headers apply.
    let sec = |bl: &[Arc<Block>]| bl.iter().filter_map(|x| x.as_para()).filter_map(|p| p.section.clone()).collect::<Vec<_>>();
    if sec(a.get(lo_a..hi_a).unwrap_or_default()) != sec(b.get(lo_b..hi_b).unwrap_or_default()) {
        return Scope::Doc;
    }
    Scope::Blocks { old: lo_a..hi_a, new: lo_b..hi_b }
}

/// The top-level ids of `blocks`; `None` if one has none.
pub fn ids_of(blocks: &[Arc<Block>]) -> Option<Vec<u32>> {
    blocks.iter().map(|b| block_id(b)).collect()
}
