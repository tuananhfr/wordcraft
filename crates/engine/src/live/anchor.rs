//! Anchors of Cloud comments (ERPCons Office online M5, the `erpHost` comment calls): where a
//! thread sits in a document. The shell keeps them on its server; nothing here touches the file.
//!
//! An anchor names the region of the top-level block the selection starts in (`para:<id>` or
//! `table:<id>`, ids that saving and reopening keep), the paragraph inside it (`pos.path`, empty
//! for a paragraph region; row, cell, block… inside a table) and the selected characters there
//! (`pos.start..pos.end`, counted in characters of the paragraph's text, an inline object being
//! one). The soft part keeps the selected text, up to [`CONTEXT`] characters on each side and the
//! top-level block's index. Placing it again prefers the region, then the quote nearest the old
//! block (matching context first), then what is left of the region, else the thread is orphaned.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use wordcraft_doc::{Block, Document, Paragraph, Path, Pos, StoryRef};

use super::Region;
use crate::Session;

pub const VERSION: u32 = 1;
/// Characters of context kept before and after the quote.
pub const CONTEXT: usize = 32;
/// Longest quote an anchor takes: a longer selection is cut there, so anchors stay small.
pub const QUOTE_MAX: usize = 1_000;
/// Longest text of the quote shown in the comments panel.
const SHOWN_MAX: usize = 120;
const LABEL_MAX: usize = 60;
/// The shell compares at most this many numbers of `order`.
const ORDER_MAX: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub v: u32,
    /// `para:<id>` / `table:<id>`; `null` when the block has no id (nothing but the soft part).
    pub region: Option<String>,
    pub pos: AnchorPos,
    pub soft: Soft,
    #[serde(default)]
    pub label: String,
    /// The selected text as the comments panel shows it.
    #[serde(default)]
    pub quote: String,
    /// Top-level block index, the path inside it, the start: the shell lists threads in this order.
    #[serde(default)]
    pub order: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorPos {
    #[serde(default)]
    pub path: Vec<u32>,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Soft {
    pub quote: String,
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
    #[serde(default)]
    pub block: usize,
}

/// Where an anchor lies in the document now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placed {
    /// The body range `start..end`; `anchor` is how it reads at that place now.
    At {
        start: Pos,
        end: Pos,
        anchor: Box<Anchor>,
    },
    Orphaned,
}

impl Anchor {
    /// An anchor of this version, or `None` for any other shape.
    pub fn parse(json: &Json) -> Option<Anchor> {
        serde_json::from_value::<Anchor>(json.clone()).ok().filter(|a| a.v == VERSION && a.pos.start < a.pos.end && !a.soft.quote.is_empty())
    }

    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or(Json::Null)
    }
}

fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(b, _)| b)
}

fn char_at(text: &str, byte: usize) -> usize {
    text.get(..byte).map_or_else(|| text.chars().count(), |t| t.chars().count())
}

fn chars(text: &str, from: usize, to: usize) -> &str {
    text.get(byte_at(text, from)..byte_at(text, to)).unwrap_or("")
}

/// Every paragraph inside top-level block `block`, with its path below that block.
fn paragraphs(block: &Block) -> Vec<(Vec<u32>, &Paragraph)> {
    fn walk<'a>(block: &'a Block, at: Vec<u32>, out: &mut Vec<(Vec<u32>, &'a Paragraph)>) {
        match block {
            Block::Para(p) => out.push((at, p)),
            Block::Table(t) => {
                for (r, row) in t.rows.iter().enumerate() {
                    for (c, cell) in row.cells.iter().enumerate() {
                        for (b, inner) in cell.blocks.iter().enumerate() {
                            let mut path = at.clone();
                            path.extend([r, c, b].map(|i| u32::try_from(i).unwrap_or(u32::MAX)));
                            walk(inner, path, out);
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(block, Vec::new(), &mut out);
    out
}

fn para_in<'a>(block: &'a Block, path: &[u32]) -> Option<&'a Paragraph> {
    match (block, path) {
        (Block::Para(p), []) => Some(p),
        (Block::Table(t), [r, c, b, rest @ ..]) => {
            let inner = t.rows.get(*r as usize)?.cells.get(*c as usize)?.blocks.get(*b as usize)?;
            para_in(inner, rest)
        }
        _ => None,
    }
}

fn shown(text: &str, max: usize) -> String {
    let clean: String = text.chars().filter(|c| *c != wordcraft_doc::para::OBJ).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max { clean } else { format!("{}…", clean.chars().take(max - 1).collect::<String>()) }
}

/// The anchor of characters `start..end` of the paragraph at `path` inside top-level block `k`.
fn build(doc: &Document, k: usize, path: &[u32], start: usize, end: usize) -> Option<Anchor> {
    let block = doc.body.get(k)?;
    let text = &para_in(block, path)?.text;
    let len = text.chars().count();
    let end = end.min(len).min(start + QUOTE_MAX);
    if start >= end {
        return None;
    }
    let quote = chars(text, start, end).to_string();
    let display = shown(&quote, SHOWN_MAX);
    let mut order: Vec<u32> = std::iter::once(k).chain(path.iter().map(|i| *i as usize)).map(|i| u32::try_from(i).unwrap_or(u32::MAX)).collect();
    order.truncate(ORDER_MAX - 1);
    order.push(u32::try_from(start).unwrap_or(u32::MAX));
    Some(Anchor {
        v: VERSION,
        region: Region::of(block).map(|r| r.id()),
        pos: AnchorPos { path: path.to_vec(), start, end },
        soft: Soft {
            quote,
            before: chars(text, start.saturating_sub(CONTEXT), start).to_string(),
            after: chars(text, end, (end + CONTEXT).min(len)).to_string(),
            block: k,
        },
        label: format!("“{}”", shown(&display, LABEL_MAX)),
        quote: display,
        order,
    })
}

/// One place the quote occurs: (top-level block, path below it, start in characters).
type Hit = (usize, Vec<u32>, usize);

/// Occurrences of the soft quote in top-level blocks `blocks`, best first: matching context,
/// then nearest block `near`, then the old paragraph and offset `old`, then document order.
fn search(doc: &Document, soft: &Soft, blocks: impl Iterator<Item = usize>, near: usize, old: (&[u32], usize)) -> Option<Hit> {
    let mut best: Option<((u8, usize, bool, usize, usize, Vec<u32>, usize), Hit)> = None;
    for k in blocks {
        let Some(block) = doc.body.get(k) else { continue };
        for (path, p) in paragraphs(block) {
            // Character offsets counted on from the previous hit: from the start each time made a
            // short quote in a long paragraph quadratic, on every keystroke.
            let mut last = (0, 0);
            for (b, _) in p.text.match_indices(soft.quote.as_str()) {
                let before = p.text.get(..b).is_some_and(|t| t.ends_with(&soft.before));
                let after = p.text.get(b + soft.quote.len()..).is_some_and(|t| t.starts_with(&soft.after));
                let start = last.1 + p.text.get(last.0..b).map_or(0, |t| t.chars().count());
                last = (b, start);
                let key = (u8::from(!before) + u8::from(!after), k.abs_diff(near), path != old.0, start.abs_diff(old.1), k, path.clone(), start);
                if best.as_ref().is_none_or(|(seen, _)| key < *seen) {
                    best = Some((key, (k, path.clone(), start)));
                }
            }
        }
    }
    best.map(|(_, hit)| hit)
}

/// Where an edited quote starting at character `start` of `text` ends now: before its old
/// `after` context when that still follows, else at the old end. `None` when neither context is
/// still there, or nothing is left between them.
fn edited_end(text: &str, soft: &Soft, start: usize, old_end: usize) -> Option<usize> {
    let len = text.chars().count();
    if start >= len {
        return None;
    }
    let before = !soft.before.is_empty() && chars(text, 0, start).ends_with(&soft.before);
    let rest = chars(text, start, len);
    let after = (!soft.after.is_empty()).then(|| rest.find(&soft.after)).flatten().map(|b| start + char_at(rest, b));
    let end = match (before, after) {
        (_, Some(end)) => end,
        (true, None) => old_end.min(len),
        (false, None) => return None,
    };
    (end > start).then_some(end)
}

fn place(doc: &Document, anchor: &Anchor) -> Placed {
    let at = |(k, path, start): Hit, end: usize| -> Placed {
        let Some(new) = build(doc, k, &path, start, end) else { return Placed::Orphaned };
        let full = |off: usize| -> Option<Pos> {
            let text = &para_in(doc.body.get(k)?, &path)?.text;
            let mut p = vec![u32::try_from(k).ok()?];
            p.extend(&path);
            Some(Pos::new(StoryRef::Body, Path(p), byte_at(text, off)))
        };
        match (full(new.pos.start), full(new.pos.end)) {
            (Some(start), Some(end)) => Placed::At { start, end, anchor: Box::new(new) },
            _ => Placed::Orphaned,
        }
    };
    let width = anchor.soft.quote.chars().count();
    let old = (anchor.pos.path.as_slice(), anchor.pos.start);
    let region = anchor.region.as_deref().and_then(Region::parse).filter(|r| r.block().is_some());
    let found = region.and_then(|r| doc.body.iter().position(|b| Region::of(b) == Some(r)));
    if let Some(k) = found {
        let block = doc.body.get(k).map(|b| &**b);
        let here = block.and_then(|b| para_in(b, &anchor.pos.path));
        if let Some(p) = here
            && chars(&p.text, anchor.pos.start, anchor.pos.end) == anchor.soft.quote
        {
            return at((k, anchor.pos.path.clone(), anchor.pos.start), anchor.pos.end);
        }
        if let Some(hit) = search(doc, &anchor.soft, std::iter::once(k), k, old) {
            let end = hit.2 + width;
            return at(hit, end);
        }
        // A paragraph split or a move takes the quote to another block: look there before
        // settling for what is left of the region.
        if let Some(hit) = search(doc, &anchor.soft, 0..doc.body.len(), k, old) {
            let end = hit.2 + width;
            return at(hit, end);
        }
        // The quote itself was edited: keep what now stands between its old context.
        if let Some(p) = here
            && let Some(end) = edited_end(&p.text, &anchor.soft, anchor.pos.start, anchor.pos.end)
        {
            return at((k, anchor.pos.path.clone(), anchor.pos.start), end);
        }
        return Placed::Orphaned;
    }
    match search(doc, &anchor.soft, 0..doc.body.len(), anchor.soft.block, old) {
        Some(hit) => {
            let end = hit.2 + width;
            at(hit, end)
        }
        None => Placed::Orphaned,
    }
}

/// The word around character `at` of `text`, as a character range; `None` between words.
fn word_at(text: &str, at: usize) -> Option<(usize, usize)> {
    let cs: Vec<char> = text.chars().collect();
    let word = |c: &char| c.is_alphanumeric() || *c == '_';
    let mut start = at.min(cs.len());
    while start > 0 && cs.get(start - 1).is_some_and(word) {
        start -= 1;
    }
    let mut end = at.min(cs.len());
    while cs.get(end).is_some_and(word) {
        end += 1;
    }
    (start < end).then_some((start, end))
}

impl Session {
    /// The anchor of the selection, cut to the paragraph it starts in; a caret anchors the word
    /// it is in. `None` outside the body or with nothing to anchor.
    pub fn comment_anchor(&self) -> Option<Anchor> {
        let (a, b) = self.sel.ordered();
        if a.story != StoryRef::Body {
            return None;
        }
        let (first, path) = a.path.0.split_first()?;
        let k = *first as usize;
        let text = &para_in(self.doc.body.get(k)?, path)?.text;
        let start = char_at(text, a.off);
        let (start, end) = if a == b {
            word_at(text, start)?
        } else if b.story == a.story && b.path == a.path {
            (start, char_at(text, b.off))
        } else {
            (start, text.chars().count())
        };
        build(&self.doc, k, path, start, end)
    }

    /// Where `anchor` lies in this session's document now.
    pub fn place_comment_anchor(&self, anchor: &Anchor) -> Placed {
        place(&self.doc, anchor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_offsets_count_characters_not_bytes() {
        let text = "Báo cáo \u{FFFC} năm";
        assert_eq!(chars(text, 4, 7), "cáo");
        assert_eq!(char_at(text, byte_at(text, 9)), 9);
        assert_eq!(chars(text, 8, 9), "\u{FFFC}");
        assert_eq!(shown("a \u{FFFC}  b\n c", 10), "a b c");
        assert_eq!(shown(&"x".repeat(10), 5), "xxxx…");
    }

    #[test]
    fn a_caret_takes_the_word_it_is_in() {
        assert_eq!(word_at("một hai ba", 5), Some((4, 7)));
        assert_eq!(word_at("một hai ba", 4), Some((4, 7)));
        assert_eq!(word_at("một hai ba", 7), Some((4, 7)));
        assert_eq!(word_at("a  b", 2), None);
        assert_eq!(word_at("", 0), None);
    }

    #[test]
    fn anchors_round_trip_as_json_and_refuse_other_shapes() {
        let a = Anchor {
            v: VERSION,
            region: Some("para:00001A2B".into()),
            pos: AnchorPos { path: vec![], start: 4, end: 7 },
            soft: Soft { quote: "cáo".into(), before: "Báo ".into(), after: " năm".into(), block: 2 },
            label: "“cáo”".into(),
            quote: "cáo".into(),
            order: vec![2, 4],
        };
        let json = a.to_json();
        assert_eq!(json["pos"], serde_json::json!({"path": [], "start": 4, "end": 7}));
        assert_eq!(Anchor::parse(&json), Some(a));
        for bad in [
            serde_json::json!({"kind": "document"}),
            serde_json::json!({"v": 1, "region": null, "pos": {"start": 3, "end": 3}, "soft": {"quote": "x"}}),
        ] {
            assert_eq!(Anchor::parse(&bad), None);
        }
    }
}
