//! Cloud comments on the page (web, a shell with the `erpHost` comment calls): a light amber tint
//! over each open thread's text and a marker in the page's right margin; a click on either names
//! the thread to the shell. Marks are never part of the document. Without them nothing is drawn.

use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use wordcraft_doc::Pos;
use wordcraft_engine::Session;
use wordcraft_engine::live::anchor::{Anchor, Placed};
use wordcraft_layout::DocLayout;

use crate::WordApp;

const AMBER: Color32 = Color32::from_rgb(0xE3, 0x9B, 0x00);

/// An open thread the shell shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MarkThread {
    pub(crate) id: String,
    pub(crate) anchor: Anchor,
}

#[derive(Default)]
pub(crate) struct CommentMarks {
    /// The host document the shell's threads are on.
    document: Option<u64>,
    threads: Vec<MarkThread>,
    placed: Vec<(String, Pos, Pos)>,
    /// (document, revision, generation) `placed` is for: placing again on every frame would
    /// search the document for each thread.
    key: Option<(u64, u64, u64)>,
    generation: u64,
    /// The marks as last painted, on screen.
    hits: Vec<(String, Rect)>,
    clicked: Option<String>,
    pub(crate) focused: Option<String>,
}

impl CommentMarks {
    pub(crate) fn document(&self) -> Option<u64> {
        self.document
    }

    /// The threads are on `document` from now on (the shared or newest host document).
    pub(crate) fn target(&mut self, document: Option<u64>) {
        if self.document != document {
            self.document = document;
            self.key = None;
            self.placed.clear();
            self.hits.clear();
        }
    }

    pub(crate) fn show(&mut self, threads: Vec<MarkThread>) {
        if self.focused.as_ref().is_some_and(|f| !threads.iter().any(|t| &t.id == f)) {
            self.focused = None;
        }
        self.threads = threads;
        self.generation = self.generation.wrapping_add(1);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    /// Places the threads again when `session` is their document and it changed since.
    pub(crate) fn refresh(&mut self, session: &Session) {
        let document = session.document_id();
        if self.document != Some(document) {
            return;
        }
        let key = (document, session.rev(), self.generation);
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.placed = self
            .threads
            .iter()
            .filter_map(|t| match session.place_comment_anchor(&t.anchor) {
                Placed::At { start, end, .. } => Some((t.id.clone(), start, end)),
                Placed::Orphaned => None,
            })
            .collect();
    }

    /// Where each open thread lies now: (thread, start, end).
    #[cfg(test)]
    pub(crate) fn placed(&self) -> &[(String, Pos, Pos)] {
        &self.placed
    }

    /// A click at `p` on the page: the smallest mark under it is the thread clicked.
    pub(crate) fn click_at(&mut self, p: Pos2) -> bool {
        let hit = self.hits.iter().filter(|(_, r)| r.contains(p)).min_by(|a, b| a.1.area().total_cmp(&b.1.area()));
        if let Some((id, _)) = hit {
            self.clicked = Some(id.clone());
        }
        hit.is_some()
    }

    pub(crate) fn take_clicked(&mut self) -> Option<String> {
        self.clicked.take()
    }

    #[cfg(test)]
    pub(crate) fn hits(&self) -> &[(String, Rect)] {
        &self.hits
    }
}

/// The marks of the shown document over the pages (`rects`, on screen at `scale`).
pub(crate) fn paint(app: &mut WordApp, painter: &Painter, layout: &DocLayout, rects: &[Rect], scale: f32) {
    let marks = &mut app.host.marks;
    marks.hits.clear();
    if marks.is_empty() {
        return;
    }
    marks.refresh(&app.session);
    if marks.document != Some(app.session.document_id()) {
        return;
    }
    let (doc, hint) = (&app.session.doc, app.session.page_hint);
    for (id, a, b) in &marks.placed {
        let focused = marks.focused.as_deref() == Some(id.as_str());
        let pieces = crate::live_overlay::page_frames(doc, layout, rects, scale, hint, a, b);
        for (i, (pi, f)) in pieces.iter().enumerate() {
            painter.rect_filled(*f, 1.0, AMBER.gamma_multiply(if focused { 0.32 } else { 0.14 }));
            painter.hline(f.x_range(), f.max.y - 0.5, Stroke::new(if focused { 2.0 } else { 1.0 }, AMBER));
            marks.hits.push((id.clone(), *f));
            let Some(page) = rects.get(*pi).filter(|_| i == 0) else { continue };
            let flag = Rect::from_min_size(pos2(page.max.x - 22.0, f.min.y), vec2(14.0, 11.0));
            painter.rect_filled(flag, 2.0, AMBER);
            let tail = pos2(flag.min.x + 4.0, flag.max.y);
            painter.add(egui::Shape::convex_polygon(vec![tail, tail + vec2(5.0, 0.0), tail + vec2(0.0, 4.0)], AMBER, Stroke::NONE));
            if focused {
                painter.rect_stroke(flag.expand(1.5), 3.0, Stroke::new(1.5, AMBER.gamma_multiply(0.6)), StrokeKind::Outside);
            }
            marks.hits.push((id.clone(), flag.expand(3.0)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_picks_the_smallest_mark_under_it() {
        let mut marks = CommentMarks {
            hits: vec![
                ("wide".into(), Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 20.0))),
                ("word".into(), Rect::from_min_size(pos2(40.0, 0.0), vec2(30.0, 20.0))),
            ],
            ..Default::default()
        };
        assert!(marks.click_at(pos2(50.0, 10.0)));
        assert_eq!(marks.take_clicked().as_deref(), Some("word"));
        assert!(marks.click_at(pos2(150.0, 10.0)));
        assert_eq!(marks.take_clicked().as_deref(), Some("wide"));
        assert!(!marks.click_at(pos2(300.0, 10.0)));
        assert_eq!(marks.take_clicked(), None);
    }
}
