//! Co-editing on the page (web, `erpHost` v3): an outline with the holder's name around each
//! paragraph or table another editor has locked (every page's edge for `*`), and the other
//! editors' carets and selections. Nothing is drawn without co-editing.

use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};
use wordcraft_doc::ids::block_id;
use wordcraft_doc::{Block, Document, Path, Pos, StoryRef};
use wordcraft_engine::live::{Region, range_positions};
use wordcraft_layout::DocLayout;

use crate::WordApp;
use crate::live_tags::{TagLook, named_by_own_lock, tag};

/// `#rrggbb` as the shell sends member colours; anything else gets a neutral blue.
pub(crate) fn colour(css: &str) -> Color32 {
    let hex = css.trim().trim_start_matches('#');
    let channel = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    match (hex.len(), channel(0), channel(2), channel(4)) {
        (6, Some(r), Some(g), Some(b)) => Color32::from_rgb(r, g, b),
        _ => Color32::from_rgb(0x33, 0x66, 0xcc),
    }
}

/// A lock's label: above the frame's top-left corner, or when compact in the margin beside its
/// first line, so the chip covers no text (above, it would sit on the line before).
fn lock_tag_at(frame: Rect, page: Option<&Rect>, size: Vec2, look: TagLook) -> Pos2 {
    let beside = pos2(frame.left() - 6.0 - size.x, frame.top() - 2.0);
    match page {
        Some(p) if look.compact && beside.x >= p.left() => beside,
        _ => pos2(frame.left() - 4.0, frame.top() - 3.0 - size.y),
    }
}

/// The top-level region a body position is in.
fn region_at(doc: &Document, at: &Pos) -> Option<Region> {
    if at.story != StoryRef::Body {
        return None;
    }
    let k = usize::try_from(*at.path.0.first()?).ok()?;
    doc.body.get(k).and_then(|b| Region::of(b))
}

fn screen(pr: Rect, scale: f32, r: wordcraft_geom::Rect) -> Rect {
    Rect::from_min_size(pos2(pr.min.x + r.x * scale, pr.min.y + r.y * scale), vec2(r.w * scale, r.h * scale))
}

/// The first and last place of top-level block `k`.
fn block_ends(doc: &Document, k: usize) -> Option<(Pos, Pos)> {
    let top = u32::try_from(k).ok()?;
    let at = |path: Vec<u32>, off: usize| doc.clamp(&Pos::new(StoryRef::Body, Path(path), off));
    match &**doc.body.get(k)? {
        Block::Para(_) => Some((at(vec![top], 0), at(vec![top], usize::MAX))),
        Block::Table(t) => {
            let rows = t.rows.len().checked_sub(1)?;
            let cells = t.rows.last()?.cells.len().checked_sub(1)?;
            let blocks = t.rows.last()?.cells.last()?.blocks.len().saturating_sub(1);
            let n = |v: usize| u32::try_from(v).unwrap_or(0);
            Some((at(vec![top, 0, 0, 0], 0), at(vec![top, n(rows), n(cells), n(blocks)], usize::MAX)))
        }
    }
}

/// `a..b` on screen, one rectangle per page.
fn frames(doc: &Document, layout: &DocLayout, rects: &[Rect], scale: f32, hint: usize, a: &Pos, b: &Pos) -> Vec<Rect> {
    page_frames(doc, layout, rects, scale, hint, a, b).into_iter().map(|(_, r)| r).collect()
}

/// `a..b` on screen, one rectangle per page, with the page's index.
pub(crate) fn page_frames(doc: &Document, layout: &DocLayout, rects: &[Rect], scale: f32, hint: usize, a: &Pos, b: &Pos) -> Vec<(usize, Rect)> {
    let mut out: Vec<(usize, Rect)> = Vec::new();
    for (pi, sr) in layout.selection_rects(doc, a, b, hint) {
        let Some(pr) = rects.get(pi) else { continue };
        let s = screen(*pr, scale, sr);
        match out.iter_mut().find(|(p, _)| *p == pi) {
            Some((_, u)) => *u = u.union(s),
            None => out.push((pi, s)),
        }
    }
    out
}

/// Others' locks and selections over the pages (`rects`, on screen at `scale`).
pub(crate) fn paint(app: &WordApp, painter: &Painter, layout: &DocLayout, rects: &[Rect], scale: f32) {
    let Some(live) = app.session.live.as_ref() else { return };
    let doc = &app.session.doc;
    let hint = app.session.page_hint;
    let look = TagLook::at_zoom(scale / crate::canvas::PX_PER_PT);
    for lock in live.others() {
        let c = colour(&lock.colour);
        let Some(region) = Region::parse(&lock.region) else { continue };
        let Some(id) = region.block() else {
            for r in rects {
                painter.rect_stroke(r.shrink(1.5), 0.0, Stroke::new(3.0, c), StrokeKind::Inside);
            }
            if let Some(first) = rects.first() {
                let at = first.left_top() + vec2(8.0, 22.0);
                tag(painter, &lock.name, c, look, |size| pos2(at.x, at.y - size.y - 1.0));
            }
            continue;
        };
        let Some(k) = doc.body.iter().position(|b| block_id(b) == Some(id)) else { continue };
        let Some((a, b)) = block_ends(doc, k) else { continue };
        let fr = page_frames(doc, layout, rects, scale, hint, &a, &b);
        for (_, f) in &fr {
            let f = f.expand2(vec2(4.0, 2.0));
            painter.rect_filled(f, 2.0, c.gamma_multiply(0.08));
            painter.rect_stroke(f, 2.0, Stroke::new(2.0, c), StrokeKind::Outside);
        }
        if let Some((page, first)) = fr.first() {
            tag(painter, &lock.name, c, look, |size| lock_tag_at(*first, rects.get(*page), size, look));
        }
    }
    for cursor in live.cursors() {
        let Some((a, f)) = range_positions(doc, &cursor.range) else { continue };
        let c = colour(&cursor.colour);
        if a != f {
            for r in frames(doc, layout, rects, scale, hint, &a, &f) {
                painter.rect_filled(r, 0.0, c.gamma_multiply(0.18));
            }
        }
        let Some(caret) = layout.caret_on(&f, hint) else { continue };
        let Some(pr) = rects.get(caret.page) else { continue };
        let x = pr.min.x + caret.x * scale;
        let (y0, y1) = (pr.min.y + caret.top * scale, pr.min.y + (caret.top + caret.height) * scale);
        painter.line_segment([pos2(x, y0), pos2(x, y1)], Stroke::new(2.0, c));
        // The holder's own lock label already names them there; a second label would overlap it.
        if !named_by_own_lock(&cursor.author, region_at(doc, &f), live.others()) {
            tag(painter, &cursor.name, c, look, |size| pos2(x, y0 - size.y - 1.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcraft_engine::live::{Cursor, Live, Lock};

    #[test]
    fn member_colours_parse_and_fall_back() {
        assert_eq!(colour("#ff8000"), Color32::from_rgb(0xff, 0x80, 0x00));
        assert_eq!(colour("nope"), Color32::from_rgb(0x33, 0x66, 0xcc));
        assert_eq!(colour("#ff80"), Color32::from_rgb(0x33, 0x66, 0xcc));
    }

    fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }

    /// The sample document with a lock on block 2 by `m1` ("An Nguyễn") and `author`'s caret in block `caret_in`.
    fn locked_with_caret(author: &str, name: &str, caret_in: usize) -> wordcraft_engine::Session {
        let mut session = wordcraft_engine::Session::new(wordcraft_engine::sample::sample_document());
        let mut live = Live::new(&session.doc);
        let id = |k: usize| Region::of(&session.doc.body[k]).and_then(|r| r.block()).expect("id");
        let (locked, typing) = (id(2), id(caret_in));
        live.set_locks(vec![Lock { region: Region::Para(locked).id(), holder: "m1".into(), name: "An Nguyễn".into(), colour: "#cc3300".into() }]);
        live.set_cursors(vec![Cursor {
            author: author.into(),
            name: name.into(),
            colour: "#0066cc".into(),
            region: Region::Para(typing).id(),
            range: format!("{typing:08X}:2;{typing:08X}:9"),
        }]);
        session.live = Some(live);
        session
    }

    /// The texts painted in the last of a few frames of a `width` wide window.
    fn drawn_at(session: wordcraft_engine::Session, width: f32) -> Vec<String> {
        let mut app = WordApp::new(session, Default::default());
        let ctx = egui::Context::default();
        let mut drawn = Vec::new();
        for i in 0..6 {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 900.0))),
                time: Some(f64::from(i) * 0.1),
                ..Default::default()
            };
            app.raw_input_hook(&mut input);
            let mut out = ctx.run_ui(input, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            drawn.clear();
            for s in &out.shapes {
                texts(&s.shape, &mut drawn);
            }
            out.textures_delta.clear();
        }
        drawn
    }

    #[test]
    fn others_locks_and_carets_are_drawn_with_their_names() {
        let drawn = drawn_at(locked_with_caret("m2", "Bình", 3), 1440.0);
        assert!(drawn.iter().any(|t| t == "An Nguyễn"), "the lock's holder is named");
        assert!(drawn.iter().any(|t| t == "Bình"), "the other caret is named");
    }

    #[test]
    fn a_caret_inside_its_owners_lock_is_named_once() {
        let drawn = drawn_at(locked_with_caret("m1", "An Nguyễn", 2), 1440.0);
        assert_eq!(drawn.iter().filter(|t| *t == "An Nguyễn").count(), 1, "{drawn:?}");
        let drawn = drawn_at(locked_with_caret("m1", "An Nguyễn", 3), 1440.0);
        assert_eq!(drawn.iter().filter(|t| *t == "An Nguyễn").count(), 2, "a caret outside the lock keeps its own name: {drawn:?}");
    }

    /// What co-editing adds to the texts painted at `width`, against the same document without it.
    fn labels_at(session: wordcraft_engine::Session, width: f32) -> Vec<String> {
        let bare = wordcraft_engine::Session::new(wordcraft_engine::sample::sample_document());
        let mut labels = drawn_at(session, width);
        for t in drawn_at(bare, width) {
            if let Some(i) = labels.iter().position(|l| *l == t) {
                labels.remove(i);
            }
        }
        labels.sort();
        labels
    }

    #[test]
    fn a_phone_wide_page_names_members_by_their_initial() {
        assert_eq!(labels_at(locked_with_caret("m2", "Trần Bình", 3), 360.0), ["B", "N"], "no full name over tiny text");
        assert_eq!(labels_at(locked_with_caret("m2", "Trần Bình", 3), 768.0), ["An Nguyễn", "Trần Bình"], "a tablet keeps full names");
    }

    #[test]
    fn a_tables_ends_are_its_first_and_last_cells() {
        let mut doc = Document::from_text("Before");
        let table = wordcraft_doc::Table::new(2, 3, 400.0);
        doc.body.insert(1, std::sync::Arc::new(Block::Table(table)));
        let (a, b) = block_ends(&doc, 1).expect("a table");
        assert_eq!(a.path.0, [1, 0, 0, 0]);
        assert_eq!(b.path.0, [1, 1, 2, 0]);
        let (a, _) = block_ends(&doc, 0).expect("a paragraph");
        assert_eq!(a.path.0, [0]);
    }
}
