//! Stable ids for live editing (ERPCons Office online, M3): paragraphs and table rows carry a
//! `w14:paraId` ([MS-DOCX] §2.6.2; one 31-bit space for `w:p` and `w:tr`, unique in a document),
//! and the list and comment ids a client makes carry its prefix so two people editing at once
//! never make the same one.
//!
//! - Ids read from a file are kept. Paragraphs and rows a file leaves without one (or with one an
//!   earlier block already has) get `1, 2, 3…` in document order, skipping ids in use
//!   ([`assign_missing`]): every client that opens the same bytes agrees.
//! - Ids made while editing are `prefix << 7 | n` (`n` 1–127) from the client's 24-bit prefix
//!   ([`ClientIds`]), never one the document already has. After 127 ids of one kind the client
//!   moves to a prefix derived from its own and a round number: 31 bits can't hold 24 prefix bits
//!   and an unbounded counter, so only the first 127 ids of each kind are distinct by
//!   construction; later ones are by a 2^-24 chance per round pair.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::table::Row;
use crate::{Block, Blocks, Document, Paragraph};

/// `w14:paraId` must be below 0x8000_0000 ([MS-DOCX] `ST_LongHexNumber` restriction).
pub const MAX_PARA_ID: u32 = 0x7FFF_FFFF;
const COUNTER_BITS: u32 = 7;
const PER_ROUND: u64 = (1 << COUNTER_BITS) - 1;
const PREFIX_MASK: u32 = 0xFF_FFFF;
const MAX_DEPTH: usize = 16;
/// Bounds a mint against a document that holds nearly every id of a round.
const MAX_ATTEMPTS: u32 = 1 << 20;

/// A `w14:paraId` value: 1 to 8 hex digits, not 0, below 0x8000_0000.
pub fn parse_para_id(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 8 {
        return None;
    }
    u32::from_str_radix(s, 16).ok().filter(|v| *v != 0 && *v <= MAX_PARA_ID)
}

/// Eight upper-case hex digits, as Word writes them.
pub fn format_para_id(id: u32) -> String {
    format!("{id:08X}")
}

/// What an id is for: each kind counts on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdKind {
    /// Paragraphs and table rows (`w14:paraId`).
    Block,
    /// `w:num/@w:numId` and `w:abstractNum/@w:abstractNumId`.
    List,
    /// `w:comment/@w:id`.
    Comment,
}

impl IdKind {
    fn slot(self) -> usize {
        match self {
            IdKind::Block => 0,
            IdKind::List => 1,
            IdKind::Comment => 2,
        }
    }
}

/// Whether `prefix` is what `setClientId` takes: exactly 6 lowercase hex digits.
pub fn is_client_prefix(prefix: &str) -> bool {
    prefix.len() == 6 && prefix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug)]
struct State {
    prefix: u32,
    made: [u64; 3],
}

/// The client's id source, shared by every session of the app (they hand it on when they swap
/// documents); counters live outside the document, so undo never makes an id come round again.
#[derive(Clone, Debug)]
pub struct ClientIds(Arc<Mutex<State>>);

impl Default for ClientIds {
    fn default() -> Self {
        ClientIds(Arc::new(Mutex::new(State { prefix: random_prefix(), made: [0; 3] })))
    }
}

impl ClientIds {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn prefix(&self) -> String {
        format!("{:06x}", self.state().prefix)
    }

    /// Uses `prefix` for ids made from now on; `false` (nothing changes) unless it is 6 lowercase
    /// hex digits. A new prefix starts its counters again.
    pub fn set_prefix(&self, prefix: &str) -> bool {
        let Some(bits) = is_client_prefix(prefix).then(|| u32::from_str_radix(prefix, 16).ok()).flatten() else { return false };
        let mut state = self.state();
        if state.prefix != bits {
            *state = State { prefix: bits, made: [0; 3] };
        }
        true
    }

    /// A new id of `kind` for which `taken` is false; `None` only for a document that holds
    /// nearly every id there is.
    pub fn mint(&self, kind: IdKind, taken: impl Fn(u32) -> bool) -> Option<u32> {
        let mut state = self.state();
        let prefix = state.prefix;
        let slot = state.made.get_mut(kind.slot())?;
        for _ in 0..MAX_ATTEMPTS {
            let k = *slot;
            *slot = slot.saturating_add(1);
            let (round, n) = (k / PER_ROUND, (k % PER_ROUND) as u32 + 1);
            let id = (round_prefix(prefix, round) << COUNTER_BITS) | n;
            if !taken(id) {
                return Some(id);
            }
        }
        None
    }
}

/// The prefix of round `round`: the client's own first, then a mix of it and the round.
fn round_prefix(prefix: u32, round: u64) -> u32 {
    if round == 0 {
        return prefix;
    }
    // splitmix64 finaliser: rounds of different prefixes land apart.
    let mut z = (u64::from(prefix) << 32 | round).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) as u32) & PREFIX_MASK
}

/// A random prefix from the OS's hash seed and the clock. The web app has neither, so it sets
/// one from the browser's random source at start-up.
fn random_prefix() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    let bits = {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        h.write_u128(now);
        h.write_u32(std::process::id());
        h.finish()
    };
    #[cfg(target_arch = "wasm32")]
    let bits = {
        let probe = Box::new(0u8);
        std::ptr::addr_of!(*probe) as u64
    };
    (bits as u32) & PREFIX_MASK
}

#[derive(Clone, Copy)]
enum Item<'a> {
    Para(&'a Paragraph),
    Row(&'a Row),
}

impl Item<'_> {
    fn id(&self) -> Option<u32> {
        match self {
            Item::Para(p) => p.id,
            Item::Row(r) => r.id,
        }
    }
}

fn visit<'a>(bl: &'a Blocks, depth: usize, f: &mut dyn FnMut(Item<'a>)) {
    for b in bl {
        match &**b {
            Block::Para(p) => f(Item::Para(p)),
            Block::Table(t) if depth < MAX_DEPTH => {
                for r in &t.rows {
                    f(Item::Row(r));
                    for c in &r.cells {
                        visit(&c.blocks, depth + 1, f);
                    }
                }
            }
            Block::Table(_) => {}
        }
    }
}

/// How many items [`visit`] reports for `b`.
fn count(b: &Block, depth: usize) -> usize {
    match b {
        Block::Para(_) => 1,
        Block::Table(t) if depth < MAX_DEPTH => {
            t.rows.iter().map(|r| 1 + r.cells.iter().flat_map(|c| c.blocks.iter()).map(|cb| count(cb, depth + 1)).sum::<usize>()).sum()
        }
        Block::Table(_) => 0,
    }
}

impl Document {
    /// Every block list that holds paragraphs: body, parts, then the note separators.
    pub fn id_stories(&self) -> Vec<&Blocks> {
        let mut v: Vec<&Blocks> = vec![&self.body];
        v.extend(self.parts.values().map(|p| &p.blocks));
        for s in [&self.footnote_separators, &self.endnote_separators] {
            v.extend([&s.separator, &s.continuation_separator, &s.continuation_notice].into_iter().flatten());
        }
        v
    }

    fn id_stories_mut(&mut self) -> Vec<&mut Blocks> {
        let mut v: Vec<&mut Blocks> = vec![&mut self.body];
        v.extend(self.parts.values_mut().map(|p| &mut p.blocks));
        for s in [&mut self.footnote_separators, &mut self.endnote_separators] {
            let crate::NoteSeparators { separator, continuation_separator, continuation_notice } = s;
            v.extend([separator, continuation_separator, continuation_notice].into_iter().flatten());
        }
        v
    }

    /// Every paragraph and row id in the document, with how often it occurs.
    pub fn id_counts(&self) -> HashMap<u32, usize> {
        let mut m = HashMap::new();
        for st in self.id_stories() {
            visit(st, 0, &mut |it| {
                if let Some(id) = it.id() {
                    *m.entry(id).or_insert(0) += 1;
                }
            });
        }
        m
    }

    /// Ids of every list (`numId`s and `abstractNumId`s), for minting a new one.
    pub fn list_ids(&self) -> HashSet<u32> {
        self.numbering.nums.iter().map(|n| n.id).chain(self.numbering.abstracts.iter().map(|a| a.id)).collect()
    }
}

/// What [`ensure_ids`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdReport {
    /// Paragraphs and rows that had no id.
    pub missing: usize,
    /// Later copies of an id already used (copied paragraphs, pastes).
    pub duplicates: usize,
}

impl IdReport {
    pub fn changed(&self) -> usize {
        self.missing + self.duplicates
    }
}

/// Gives the paragraphs and rows of a document just read their deterministic ids (see the module
/// doc); ids the file has are kept.
pub fn assign_missing(doc: &mut Document) -> IdReport {
    let mut next = 0u32;
    ensure_ids(doc, None, &mut |taken: &HashSet<u32>| {
        while next < MAX_PARA_ID {
            next += 1;
            if !taken.contains(&next) {
                return Some(next);
            }
        }
        None
    })
}

/// Gives every paragraph and row of `doc` that has no id, or an id an earlier one has, a new id
/// from `mint` (given the ids in use). Of copies of one id, the one whose content still equals its
/// holder in `before` keeps it (a paste above the original doesn't rename the original), else the
/// first. Only blocks holding a fix are copied, so undo snapshots keep sharing the rest.
pub fn ensure_ids(doc: &mut Document, before: Option<&Document>, mint: &mut dyn FnMut(&HashSet<u32>) -> Option<u32>) -> IdReport {
    let mut report = IdReport::default();
    let counts = doc.id_counts();
    let mut total = 0usize;
    for st in doc.id_stories() {
        visit(st, 0, &mut |it| {
            total += 1;
            if it.id().is_none() {
                report.missing += 1;
            }
        });
    }
    let duplicated = counts.values().any(|n| *n > 1);
    if report.missing == 0 && !duplicated {
        return report;
    }
    let mut prior_para: HashMap<u32, &Paragraph> = HashMap::new();
    let mut prior_row: HashMap<u32, &Row> = HashMap::new();
    if duplicated && let Some(b) = before {
        let bc = b.id_counts();
        let was_unique = |id: Option<u32>| id.filter(|i| bc.get(i) == Some(&1) && counts.get(i).is_some_and(|n| *n > 1));
        for st in b.id_stories() {
            visit(st, 0, &mut |it| match it {
                Item::Para(p) => prior_para.extend(was_unique(p.id).map(|i| (i, p))),
                Item::Row(r) => prior_row.extend(was_unique(r.id).map(|i| (i, r))),
            });
        }
    }
    let mut ids: Vec<Option<u32>> = Vec::with_capacity(total);
    let mut copies: HashMap<u32, Vec<(usize, bool)>> = HashMap::new();
    for st in doc.id_stories() {
        visit(st, 0, &mut |it| {
            let k = ids.len();
            ids.push(it.id());
            if let Some(id) = it.id().filter(|i| counts.get(i).is_some_and(|n| *n > 1)) {
                let same = match it {
                    Item::Para(p) => prior_para.get(&id).is_some_and(|q| *q == p),
                    Item::Row(r) => prior_row.get(&id).is_some_and(|q| *q == r),
                };
                copies.entry(id).or_default().push((k, same));
            }
        });
    }
    let keeper: HashMap<u32, usize> =
        copies.iter().filter_map(|(id, at)| at.iter().find(|(_, same)| *same).or(at.first()).map(|(k, _)| (*id, *k))).collect();
    let flags: Vec<bool> = ids
        .iter()
        .enumerate()
        .map(|(k, id)| match id {
            None => true,
            Some(id) => match keeper.get(id) {
                Some(&first) if first != k => {
                    report.duplicates += 1;
                    true
                }
                _ => false,
            },
        })
        .collect();
    let mut taken: HashSet<u32> = counts.into_keys().collect();
    let mut idx = 0usize;
    for st in doc.id_stories_mut() {
        fix(st, 0, &flags, &mut idx, &mut taken, mint);
    }
    report
}

fn fresh(mint: &mut dyn FnMut(&HashSet<u32>) -> Option<u32>, taken: &mut HashSet<u32>) -> Option<u32> {
    let id = mint(taken)?;
    taken.insert(id);
    Some(id)
}

fn fix(bl: &mut Blocks, depth: usize, flags: &[bool], idx: &mut usize, taken: &mut HashSet<u32>, mint: &mut dyn FnMut(&HashSet<u32>) -> Option<u32>) {
    for b in bl.iter_mut() {
        let n = count(b, depth);
        let end = idx.saturating_add(n);
        if !flags.get(*idx..end).is_some_and(|f| f.contains(&true)) {
            *idx = end;
            continue;
        }
        match Arc::make_mut(b) {
            Block::Para(p) => {
                if flags.get(*idx) == Some(&true) {
                    p.id = fresh(mint, taken);
                }
                *idx += 1;
            }
            Block::Table(t) if depth < MAX_DEPTH => {
                for r in &mut t.rows {
                    if flags.get(*idx) == Some(&true) {
                        r.id = fresh(mint, taken);
                    }
                    *idx += 1;
                    for c in &mut r.cells {
                        fix(&mut c.blocks, depth + 1, flags, idx, taken, mint);
                    }
                }
            }
            Block::Table(_) => {}
        }
    }
}

/// The id of a top-level block for regions: a paragraph's own, a table's first row's (OOXML
/// tables carry none of their own).
pub fn block_id(b: &Block) -> Option<u32> {
    match b {
        Block::Para(p) => p.id,
        Block::Table(t) => t.rows.first().and_then(|r| r.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::para_block;
    use crate::props::CharProps;
    use crate::table::Table;

    fn para(text: &str, id: Option<u32>) -> Arc<Block> {
        let mut p = Paragraph::with_text(text, CharProps::default());
        p.id = id;
        para_block(p)
    }

    fn body_ids(doc: &Document) -> Vec<Option<u32>> {
        let mut v = Vec::new();
        visit(&doc.body, 0, &mut |it| v.push(it.id()));
        v
    }

    fn client(prefix: &str) -> ClientIds {
        let ids = ClientIds::default();
        assert!(ids.set_prefix(prefix));
        ids
    }

    #[test]
    fn para_ids_parse_and_format_as_word_writes_them() {
        assert_eq!(parse_para_id("1A2B3C4D"), Some(0x1A2B_3C4D));
        assert_eq!(parse_para_id("7fffffff"), Some(MAX_PARA_ID));
        for bad in ["", "0", "00000000", "80000000", "FFFFFFFF", "123456789", "xyz"] {
            assert_eq!(parse_para_id(bad), None, "{bad:?}");
        }
        assert_eq!(format_para_id(0x2A), "0000002A");
    }

    #[test]
    fn prefixes_are_six_lowercase_hex_digits() {
        assert!(is_client_prefix("0a9f3c"));
        for bad in ["", "0a9f3", "0a9f3c1", "0A9F3C", "0a9g3c"] {
            assert!(!is_client_prefix(bad), "{bad:?}");
        }
        let ids = ClientIds::default();
        assert!(is_client_prefix(&ids.prefix()));
        assert!(!ids.set_prefix("nope"));
        assert!(ids.set_prefix("00beef"));
        assert_eq!(ids.prefix(), "00beef");
    }

    #[test]
    fn minted_ids_carry_the_prefix_and_stay_below_the_word_limit() {
        let ids = client("abcdef");
        let first = ids.mint(IdKind::Block, |_| false).unwrap();
        assert_eq!(first, 0xabcdef << 7 | 1);
        assert_eq!(ids.mint(IdKind::Block, |i| i == (0xabcdef << 7 | 2)), Some(0xabcdef << 7 | 3), "a taken id is skipped");
        assert_eq!(ids.mint(IdKind::List, |_| false), Some(0xabcdef << 7 | 1), "each kind counts on its own");
        let max = client("ffffff");
        for _ in 0..1000 {
            assert!(max.mint(IdKind::Comment, |_| false).unwrap() <= MAX_PARA_ID);
        }
    }

    #[test]
    fn two_prefixes_never_make_the_same_id_in_their_first_round() {
        let (a, b) = (client("000001"), client("000002"));
        let mut all = HashSet::new();
        for _ in 0..PER_ROUND {
            assert!(all.insert(a.mint(IdKind::Block, |_| false).unwrap()));
            assert!(all.insert(b.mint(IdKind::Block, |_| false).unwrap()));
        }
        // Later rounds move to derived prefixes, apart from either client's own.
        let next = a.mint(IdKind::Block, |_| false).unwrap();
        assert!(next >> COUNTER_BITS != 1 && next >> COUNTER_BITS != 2, "{next:x}");
        assert!(!all.contains(&next));
    }

    #[test]
    fn missing_ids_are_numbered_in_order_and_file_ids_are_kept() {
        let mut doc = Document::new();
        doc.body = vec![para("a", None), para("b", Some(2)), para("c", None), para("d", Some(0x1234))];
        let mut t = Table::new(2, 1, 100.0);
        t.rows[1].id = Some(7);
        doc.body.insert(1, Arc::new(Block::Table(t)));
        let mut again = doc.clone();
        assert_eq!(assign_missing(&mut doc), IdReport { missing: 5, duplicates: 0 });
        // Body order: a, row 0, its cell's paragraph, row 1 (7), its paragraph, b (2), c, d.
        assert_eq!(body_ids(&doc), [Some(1), Some(3), Some(4), Some(7), Some(5), Some(2), Some(6), Some(0x1234)]);
        assign_missing(&mut again);
        assert_eq!(body_ids(&again), body_ids(&doc), "every client opening the same bytes agrees");
        assert_eq!(assign_missing(&mut doc), IdReport::default(), "nothing left to do");
    }

    #[test]
    fn a_copy_gets_a_new_id_and_the_original_keeps_its_own() {
        let mut before = Document::new();
        before.body = vec![para("original", Some(10)), para("other", Some(11))];
        let mut doc = before.clone();
        // A paste of the first paragraph above it, then an edit of the copy.
        doc.body.insert(0, para("original edited", Some(10)));
        let ids = client("123456");
        let report = ensure_ids(&mut doc, Some(&before), &mut |taken| ids.mint(IdKind::Block, |i| taken.contains(&i)));
        assert_eq!(report, IdReport { missing: 0, duplicates: 1 });
        assert_eq!(body_ids(&doc), [Some(0x123456 << 7 | 1), Some(10), Some(11)]);
        // Without a before, the first copy keeps it.
        let mut doc = before.clone();
        doc.body.push(para("original", Some(10)));
        ensure_ids(&mut doc, None, &mut |taken| ids.mint(IdKind::Block, |i| taken.contains(&i)));
        assert_eq!(body_ids(&doc), [Some(10), Some(11), Some(0x123456 << 7 | 2)]);
    }

    #[test]
    fn a_split_gives_the_new_paragraph_a_new_id_and_a_merge_keeps_the_first() {
        let mut p = Paragraph::with_text("Hello world", CharProps::default());
        p.id = Some(5);
        let tail = p.split_off(5).unwrap();
        assert_eq!((p.id, tail.id), (Some(5), None), "a split in the middle: the head keeps it");
        let mut q = Paragraph::with_text("Hello", CharProps::default());
        q.id = Some(6);
        let tail = q.split_off(0).unwrap();
        assert_eq!((q.id, tail.id), (None, Some(6)), "Enter at the start: the text keeps it, the empty paragraph above gets a new one");
        let mut empty = Paragraph::new();
        empty.id = Some(8);
        let tail = empty.split_off(0).unwrap();
        assert_eq!((empty.id, tail.id), (Some(8), None));
        let mut a = Paragraph::with_text("a", CharProps::default());
        a.id = Some(1);
        let mut b = Paragraph::with_text("b", CharProps::default());
        b.id = Some(2);
        a.append(b);
        assert_eq!(a.id, Some(1));
    }

    #[test]
    fn ids_never_count_in_paragraph_or_row_equality() {
        let mut a = Paragraph::with_text("same", CharProps::default());
        let mut b = a.clone();
        a.id = Some(1);
        b.id = Some(2);
        assert!(a == b);
        let t = Table::new(1, 1, 100.0);
        let (mut r1, mut r2) = (t.rows[0].clone(), t.rows[0].clone());
        r1.id = Some(1);
        r2.id = Some(2);
        assert!(r1 == r2);
    }

    #[test]
    fn a_table_is_known_by_its_first_rows_id() {
        let mut t = Table::new(2, 1, 100.0);
        t.rows[0].id = Some(42);
        assert_eq!(block_id(&Block::Table(t)), Some(42));
        assert_eq!(block_id(&para("x", Some(3))), Some(3));
    }
}
