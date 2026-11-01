//! What travels for a region (format `v:1`, see `ERPCONS-CHANGES.md`): a run of top-level blocks
//! with everything they point at outside themselves, document-local indexes made payload-local
//! (revisions, stories) or content-addressed (pictures), so equal runs give equal bytes; or, for
//! `*`, the whole document as a DOCX.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::Digest;
use wordcraft_doc::ids::{block_id, format_para_id, parse_para_id};
use wordcraft_doc::numbering::{AbstractNum, Num};
use wordcraft_doc::styles::Style;
use wordcraft_doc::{Block, Comment, Document, Part, Revision};

use super::walk::{ObjRef, comment_ids, obj_refs_mut, part_ids, revs_mut, style_and_list_refs};
use crate::cmd::insert::{base64_decode, base64_encode};

pub const VERSION: u32 = 1;
/// Most stories one run may carry (notes, text boxes, comments; hostile documents fan out).
const MAX_PARTS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireMedia {
    pub ext: String,
    pub len: usize,
    /// Base64 bytes when the room may not have them yet; otherwise only the key's hash travels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct RegionPayload {
    pub blocks: Vec<Block>,
    /// Revisions the blocks and stories use; their indexes point here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<Revision>,
    /// Notes, text boxes and comment texts, by payload-local id (1, 2… in order of use).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<u32, Part>,
    /// Comments anchored in the blocks, by their id (client-prefixed, so the same in the room).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub comments: BTreeMap<u32, Comment>,
    /// By wire key `sha256-<hex>.<ext>`, which is what the blocks' pictures hold.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub media: BTreeMap<String, WireMedia>,
    /// The styles and lists used, closed over `based_on`, links and list styles.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<Style>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nums: Vec<Num>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub abstracts: Vec<AbstractNum>,
}

/// SHA-256 of pictures, kept while their bytes are the same `Arc` (typing next to a picture
/// would otherwise hash it on every keystroke).
#[derive(Default)]
pub struct MediaHashes(HashMap<String, (usize, String)>);

impl MediaHashes {
    pub fn of(&mut self, key: &str, bytes: &Arc<Vec<u8>>) -> String {
        let ptr = Arc::as_ptr(bytes) as usize;
        if let Some((p, h)) = self.0.get(key)
            && *p == ptr
        {
            return h.clone();
        }
        let h = media_hash(bytes);
        self.0.insert(key.to_string(), (ptr, h.clone()));
        h
    }
}

pub fn media_hash(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn wire_key(hash: &str, ext: &str) -> String {
    format!("sha256-{hash}.{ext}")
}

pub fn hash_of_wire_key(key: &str) -> &str {
    key.trim_start_matches("sha256-").split('.').next().unwrap_or_default()
}

fn ext_of(key: &str) -> String {
    key.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).filter(|e| !e.is_empty() && e.len() <= 8).unwrap_or_else(|| "bin".into())
}

/// Captures `blocks` (top-level blocks of `doc`); `inline` says which picture hashes travel with
/// their bytes.
pub fn capture(doc: &Document, blocks: &[Arc<Block>], hashes: &mut MediaHashes, inline: &mut dyn FnMut(&str) -> bool) -> RegionPayload {
    let mut p = RegionPayload { blocks: blocks.iter().map(|b| (**b).clone()).collect(), ..Default::default() };

    // Stories and comments reachable from the blocks, breadth first, numbered in order of use.
    let mut part_map: BTreeMap<u32, u32> = BTreeMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut queue: Vec<u32> = Vec::new();
    let mut comment_set: Vec<u32> = Vec::new();
    let discover = |b: &Block, queue: &mut Vec<u32>, comments: &mut Vec<u32>| {
        queue.extend(part_ids(b));
        for c in comment_ids(b) {
            if !comments.contains(&c) {
                comments.push(c);
            }
        }
    };
    for b in &p.blocks {
        discover(b, &mut queue, &mut comment_set);
    }
    let mut qi = 0;
    loop {
        while let Some(&id) = queue.get(qi) {
            qi += 1;
            if part_map.contains_key(&id) || order.len() >= MAX_PARTS {
                continue;
            }
            let Some(part) = doc.parts.get(&id) else { continue };
            part_map.insert(id, order.len() as u32 + 1);
            order.push(id);
            queue.extend(part.body.next);
            for b in &part.blocks {
                discover(b, &mut queue, &mut comment_set);
            }
        }
        // Comment texts (and replies to the comments found) are stories too.
        let before = queue.len();
        let replies: Vec<u32> = doc.comments.iter().filter(|(_, c)| c.parent.is_some_and(|x| comment_set.contains(&x))).map(|(id, _)| *id).collect();
        for r in replies {
            if !comment_set.contains(&r) {
                comment_set.push(r);
            }
        }
        for c in &comment_set {
            if let Some(cm) = doc.comments.get(c)
                && !part_map.contains_key(&cm.part)
                && !queue.contains(&cm.part)
            {
                queue.push(cm.part);
            }
        }
        if queue.len() == before {
            break;
        }
    }
    for id in &order {
        if let (Some(part), Some(local)) = (doc.parts.get(id), part_map.get(id)) {
            let mut part = part.clone();
            part.body.next = part.body.next.and_then(|n| part_map.get(&n).copied());
            p.parts.insert(*local, part);
        }
    }
    for c in &comment_set {
        if let Some(cm) = doc.comments.get(c) {
            let mut cm = cm.clone();
            cm.part = part_map.get(&cm.part).copied().unwrap_or(0);
            p.comments.insert(*c, cm);
        }
    }

    let mut rev_map: HashMap<u32, u32> = HashMap::new();
    let mut revisions: Vec<Revision> = Vec::new();
    let mut media: BTreeMap<String, WireMedia> = BTreeMap::new();
    let mut fix = |b: &mut Block| {
        revs_mut(b, &mut |i| {
            let n = *rev_map.entry(*i).or_insert_with(|| {
                revisions.push(doc.revisions.get(*i as usize).cloned().unwrap_or_default());
                revisions.len() as u32 - 1
            });
            *i = n;
        });
        obj_refs_mut(b, &mut |r| match r {
            ObjRef::Part(id) => *id = part_map.get(id).copied().unwrap_or(0),
            ObjRef::Media(key) => {
                if let Some(bytes) = doc.media.get(key.as_str()) {
                    let ext = ext_of(key);
                    let hash = hashes.of(key, bytes);
                    let wk = wire_key(&hash, &ext);
                    media.entry(wk.clone()).or_insert_with(|| WireMedia { ext, len: bytes.len(), data: inline(&hash).then(|| base64_encode(bytes)) });
                    *key = wk;
                }
            }
        });
    };
    for b in &mut p.blocks {
        fix(b);
    }
    for part in p.parts.values_mut() {
        for b in &mut part.blocks {
            fix(Arc::make_mut(b));
        }
    }
    p.revisions = revisions;
    p.media = media;

    // Style and list definitions, closed over what they refer to.
    let mut styles: Vec<String> = vec!["Normal".into()];
    let mut nums: Vec<u32> = Vec::new();
    for b in p.blocks.iter().chain(p.parts.values().flat_map(|pt| pt.blocks.iter().map(|b| &**b))) {
        style_and_list_refs(b, &mut styles, &mut nums);
    }
    let mut want: HashSet<String> = styles.into_iter().collect();
    let mut want_nums: HashSet<u32> = nums.into_iter().filter(|n| *n != 0).collect();
    loop {
        let size = (want.len(), want_nums.len());
        let mut more: Vec<String> = Vec::new();
        for st in doc.styles.styles.iter().filter(|s| want.contains(&s.id)) {
            more.extend(st.based_on.iter().chain(&st.next).chain(&st.linked).chain(&st.para.style).chain(&st.chr.style).cloned());
            want_nums.extend(st.para.numbering.iter().map(|n| n.num).filter(|n| *n != 0));
        }
        want.extend(more);
        for n in doc.numbering.nums.iter().filter(|n| want_nums.contains(&n.id)) {
            for (_, l) in &n.level_overrides {
                want.extend(l.style.iter().cloned());
            }
        }
        if (want.len(), want_nums.len()) == size {
            break;
        }
    }
    p.styles = doc.styles.styles.iter().filter(|s| want.contains(&s.id)).cloned().collect();
    p.nums = doc.numbering.nums.iter().filter(|n| want_nums.contains(&n.id)).cloned().collect();
    let abstracts: HashSet<u32> = p.nums.iter().map(|n| n.abstract_id).collect();
    p.abstracts = doc.numbering.abstracts.iter().filter(|a| abstracts.contains(&a.id)).cloned().collect();
    p
}

/// The canonical text of `blocks` of `doc`: equal for equal runs in any two documents.
pub fn fingerprint(doc: &Document, blocks: &[Arc<Block>], hashes: &mut MediaHashes) -> String {
    serde_json::to_string(&capture(doc, blocks, hashes, &mut |_| false)).unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("picture {0} is neither in the document nor in the payload")]
    MissingMedia(String),
    #[error("not a v1 payload: {0}")]
    Format(String),
}

/// Stories owned by blocks (notes, text boxes, comment texts, linked boxes), transitively, and
/// the comments anchored in them.
fn owned_parts(doc: &Document, blocks: &[Arc<Block>]) -> (Vec<u32>, Vec<u32>) {
    let mut parts: Vec<u32> = Vec::new();
    let mut comments: Vec<u32> = Vec::new();
    let mut queue: Vec<u32> = Vec::new();
    for b in blocks {
        queue.extend(part_ids(b));
        comments.extend(comment_ids(b));
    }
    let mut qi = 0;
    while let Some(&id) = queue.get(qi) {
        qi += 1;
        if parts.contains(&id) || parts.len() >= MAX_PARTS {
            continue;
        }
        let Some(part) = doc.parts.get(&id) else { continue };
        parts.push(id);
        queue.extend(part.body.next);
        for b in &part.blocks {
            queue.extend(part_ids(b));
            comments.extend(comment_ids(b));
        }
    }
    comments.sort_unstable();
    comments.dedup();
    let replies: Vec<u32> = doc.comments.iter().filter(|(_, c)| c.parent.is_some_and(|x| comments.contains(&x))).map(|(id, _)| *id).collect();
    comments.extend(replies);
    comments.sort_unstable();
    comments.dedup();
    for c in &comments {
        if let Some(cm) = doc.comments.get(c)
            && !parts.contains(&cm.part)
        {
            parts.push(cm.part);
        }
    }
    (parts, comments)
}

/// Where a run lands in a body of top-level ids: the blocks `remove` names go, and the run takes
/// the place of the first of them; with none of them there, it goes after `prev`, else before
/// `next`, else at the end. Returns (indexes to remove, ascending; insertion index).
pub fn place(body: &[Option<u32>], remove: &[u32], prev: Option<u32>, next: Option<u32>) -> (Vec<usize>, usize) {
    let gone: Vec<usize> = body.iter().enumerate().filter(|(_, id)| id.is_some_and(|i| remove.contains(&i))).map(|(k, _)| k).collect();
    let at = match gone.first() {
        Some(&k) => k,
        None => {
            let find = |id: Option<u32>| id.and_then(|i| body.iter().position(|b| *b == Some(i)));
            find(prev).map(|k| k + 1).or_else(|| find(next)).unwrap_or(body.len())
        }
    };
    (gone, at)
}

/// Puts a run into `doc`: drops the blocks at `gone` (with their stories and comments), inserts
/// the payload's blocks at `at`. Returns the inserted range. Fails without touching the document.
pub fn replace(doc: &mut Document, gone: &[usize], at: usize, payload: &RegionPayload) -> Result<Range<usize>, ApplyError> {
    // Pictures first: the only input that can be missing.
    let mut local_by_hash: HashMap<String, String> = HashMap::new();
    if !payload.media.is_empty() {
        for (k, v) in &doc.media {
            local_by_hash.entry(media_hash(v)).or_insert_with(|| k.clone());
        }
    }
    let mut media_map: HashMap<String, String> = HashMap::new();
    let mut new_media: Vec<(String, Vec<u8>)> = Vec::new();
    for (wk, m) in &payload.media {
        let hash = hash_of_wire_key(wk).to_string();
        if let Some(local) = local_by_hash.get(&hash) {
            media_map.insert(wk.clone(), local.clone());
            continue;
        }
        let bytes =
            m.data.as_deref().and_then(base64_decode).filter(|b| media_hash(b) == hash).ok_or_else(|| ApplyError::MissingMedia(wk.clone()))?;
        new_media.push((wk.clone(), bytes));
    }
    for (wk, bytes) in new_media {
        let ext = payload.media.get(&wk).map(|m| m.ext.clone()).unwrap_or_else(|| "bin".into());
        let key = doc.add_media(bytes, &ext);
        media_map.insert(wk, key);
    }

    // Styles and lists the document lacks; a different definition of one it has stays local
    // (changing one is a whole-document edit).
    for st in &payload.styles {
        if !doc.styles.styles.iter().any(|s| s.id == st.id) {
            doc.styles.styles.push(st.clone());
        }
    }
    for a in &payload.abstracts {
        if !doc.numbering.abstracts.iter().any(|x| x.id == a.id) {
            doc.numbering.abstracts.push(a.clone());
        }
    }
    for n in &payload.nums {
        if !doc.numbering.nums.iter().any(|x| x.id == n.id) {
            doc.numbering.nums.push(n.clone());
        }
    }

    // Revisions: the same (kind, author, date, move) is the same revision.
    let rev_map: Vec<u32> = payload
        .revisions
        .iter()
        .map(|r| match doc.revisions.iter().position(|x| x == r) {
            Some(i) => i as u32,
            None => {
                doc.revisions.push(r.clone());
                doc.revisions.len() as u32 - 1
            }
        })
        .collect();

    // Stories: drop the old blocks', add the payload's under fresh ids.
    let old: Vec<Arc<Block>> = gone.iter().filter_map(|k| doc.body.get(*k).cloned()).collect();
    let (old_parts, old_comments) = owned_parts(doc, &old);
    for id in &old_parts {
        doc.parts.remove(id);
    }
    for c in &old_comments {
        doc.comments.remove(c);
    }
    let mut part_map: HashMap<u32, u32> = HashMap::new();
    for (local, part) in &payload.parts {
        part_map.insert(*local, doc.add_part(part.kind, Vec::new()));
    }
    let remap = |b: &mut Block| {
        revs_mut(b, &mut |i| *i = rev_map.get(*i as usize).copied().unwrap_or(*i));
        obj_refs_mut(b, &mut |r| match r {
            ObjRef::Part(id) => *id = part_map.get(id).copied().unwrap_or(0),
            ObjRef::Media(key) => {
                if let Some(k) = media_map.get(key.as_str()) {
                    *key = k.clone();
                }
            }
        });
    };
    for (local, part) in &payload.parts {
        let Some(id) = part_map.get(local) else { continue };
        let mut part = part.clone();
        part.body.next = part.body.next.and_then(|n| part_map.get(&n).copied());
        for b in &mut part.blocks {
            remap(Arc::make_mut(b));
        }
        doc.parts.insert(*id, part);
    }
    for (cid, c) in &payload.comments {
        let mut c = c.clone();
        c.part = part_map.get(&c.part).copied().unwrap_or(0);
        doc.comments.insert(*cid, c);
    }
    let blocks: Vec<Arc<Block>> = payload
        .blocks
        .iter()
        .map(|b| {
            let mut b = b.clone();
            remap(&mut b);
            Arc::new(b)
        })
        .collect();
    for k in gone.iter().rev() {
        if *k < doc.body.len() {
            doc.body.remove(*k);
        }
    }
    let at = at.min(doc.body.len());
    let n = blocks.len();
    doc.body.splice(at..at, blocks);
    if doc.body.is_empty() {
        doc.ensure_nonempty();
    }
    Ok(at..at + n)
}

/// A run's wire form (`k: "run"`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunWire {
    pub v: u32,
    pub k: String,
    /// The top-level blocks the run replaces: the region's own and every block an earlier run of
    /// the same lock sent (eight hex digits each).
    pub r: Vec<String>,
    /// The blocks around the run on the sender, to place it when none of `r` is there.
    #[serde(default)]
    pub prev: Option<String>,
    #[serde(default)]
    pub next: Option<String>,
    pub p: RegionPayload,
}

/// The whole document's wire form (`k: "doc"`): a DOCX, base64.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocWire {
    pub v: u32,
    pub k: String,
    pub x: String,
}

pub enum Wire {
    Run { remove: Vec<u32>, prev: Option<u32>, next: Option<u32>, payload: Box<RegionPayload> },
    Doc(Vec<u8>),
}

pub fn encode_run(remove: &[u32], prev: Option<u32>, next: Option<u32>, payload: RegionPayload) -> String {
    let wire = RunWire {
        v: VERSION,
        k: "run".into(),
        r: remove.iter().map(|i| format_para_id(*i)).collect(),
        prev: prev.map(format_para_id),
        next: next.map(format_para_id),
        p: payload,
    };
    serde_json::to_string(&wire).unwrap_or_default()
}

pub fn encode_doc(docx: &[u8]) -> String {
    serde_json::to_string(&DocWire { v: VERSION, k: "doc".into(), x: base64_encode(docx) }).unwrap_or_default()
}

pub fn decode(payload: &str) -> Result<Wire, ApplyError> {
    let head: serde_json::Value = serde_json::from_str(payload).map_err(|e| ApplyError::Format(e.to_string()))?;
    if head.get("v").and_then(serde_json::Value::as_u64) != Some(u64::from(VERSION)) {
        return Err(ApplyError::Format("version".into()));
    }
    match head.get("k").and_then(serde_json::Value::as_str) {
        Some("run") => {
            let w: RunWire = serde_json::from_value(head).map_err(|e| ApplyError::Format(e.to_string()))?;
            let id = |s: &String| parse_para_id(s).ok_or_else(|| ApplyError::Format(format!("block id {s}")));
            Ok(Wire::Run {
                remove: w.r.iter().map(id).collect::<Result<_, _>>()?,
                prev: w.prev.as_ref().map(id).transpose()?,
                next: w.next.as_ref().map(id).transpose()?,
                payload: Box::new(w.p),
            })
        }
        Some("doc") => {
            let w: DocWire = serde_json::from_value(head).map_err(|e| ApplyError::Format(e.to_string()))?;
            base64_decode(&w.x).map(Wire::Doc).ok_or_else(|| ApplyError::Format("base64".into()))
        }
        _ => Err(ApplyError::Format("kind".into())),
    }
}

/// The top-level ids of a body, for [`place`].
pub fn body_ids(doc: &Document) -> Vec<Option<u32>> {
    doc.body.iter().map(|b| block_id(b)).collect()
}
