//! Walks over a block's document-level references: revision indexes, stories in
//! `Document::parts`, media keys, comments, styles and lists.

use wordcraft_doc::props::CharProps;
use wordcraft_doc::table::{Cell, Row, Table};
use wordcraft_doc::{Block, InlineObject, Paragraph};

pub(super) const MAX_DEPTH: usize = 16;

/// A node a mutable walk stops at.
pub(crate) enum NodeMut<'a> {
    Para(&'a mut Paragraph),
    Table(&'a mut Table),
    Row(&'a mut Row),
    Cell(&'a mut Cell),
}

pub(crate) fn walk_mut(b: &mut Block, f: &mut dyn FnMut(NodeMut<'_>)) {
    walk_mut_at(b, 0, f);
}

fn walk_mut_at(b: &mut Block, depth: usize, f: &mut dyn FnMut(NodeMut<'_>)) {
    match b {
        Block::Para(p) => f(NodeMut::Para(p)),
        Block::Table(t) => {
            f(NodeMut::Table(t));
            if depth >= MAX_DEPTH {
                return;
            }
            for r in &mut t.rows {
                f(NodeMut::Row(r));
                for c in &mut r.cells {
                    f(NodeMut::Cell(c));
                    for cb in &mut c.blocks {
                        walk_mut_at(std::sync::Arc::make_mut(cb), depth + 1, f);
                    }
                }
            }
        }
    }
}

/// Every paragraph of a block, tables descended.
pub(crate) fn paras<'a>(b: &'a Block, f: &mut dyn FnMut(&'a Paragraph)) {
    paras_at(b, 0, f);
}

fn paras_at<'a>(b: &'a Block, depth: usize, f: &mut dyn FnMut(&'a Paragraph)) {
    match b {
        Block::Para(p) => f(p),
        Block::Table(t) if depth < MAX_DEPTH => {
            for cb in t.rows.iter().flat_map(|r| r.cells.iter()).flat_map(|c| c.blocks.iter()) {
                paras_at(cb, depth + 1, f);
            }
        }
        Block::Table(_) => {}
    }
}

fn char_revs(c: &mut CharProps, f: &mut dyn FnMut(&mut u32)) {
    if let Some(i) = c.ins.as_mut() {
        f(i);
    }
    if let Some(i) = c.del.as_mut() {
        f(i);
    }
    if let Some(ch) = c.fmt_change.as_mut() {
        f(&mut ch.rev);
    }
}

/// Every `Document::revisions` index a block holds.
pub(crate) fn revs_mut(b: &mut Block, f: &mut dyn FnMut(&mut u32)) {
    walk_mut(b, &mut |n| match n {
        NodeMut::Para(p) => {
            for r in &mut p.runs {
                char_revs(&mut r.props, f);
            }
            char_revs(&mut p.mark, f);
            if let Some(ch) = p.props.fmt_change.as_mut() {
                f(&mut ch.rev);
            }
            if let Some(ch) = p.props.num_change.as_mut() {
                f(&mut ch.rev);
            }
            if let Some(ch) = p.section.as_mut().and_then(|s| s.fmt_change.as_mut()) {
                f(&mut ch.rev);
            }
            for o in &mut p.objects {
                if let InlineObject::Ruby { props, .. } = o {
                    char_revs(props, f);
                }
            }
        }
        NodeMut::Table(t) => {
            if let Some(ch) = t.props.fmt_change.as_mut() {
                f(&mut ch.rev);
            }
        }
        NodeMut::Row(r) => {
            if let Some(ch) = r.props.fmt_change.as_mut() {
                f(&mut ch.rev);
            }
        }
        NodeMut::Cell(c) => {
            if let Some(ch) = c.props.fmt_change.as_mut() {
                f(&mut ch.rev);
            }
        }
    });
}

/// What an inline object points at outside its paragraph.
pub(crate) enum ObjRef<'a> {
    /// A note's or text box's story id.
    Part(&'a mut u32),
    Media(&'a mut String),
}

fn obj_refs(o: &mut InlineObject, depth: usize, f: &mut dyn FnMut(ObjRef<'_>)) {
    match o {
        InlineObject::Image { media, .. } => f(ObjRef::Media(media)),
        InlineObject::Shape { story: Some(id), .. } => f(ObjRef::Part(id)),
        InlineObject::NoteRef { id, .. } => f(ObjRef::Part(id)),
        InlineObject::Group { children, .. } if depth < MAX_DEPTH => {
            for c in children {
                obj_refs(&mut c.obj, depth + 1, f);
            }
        }
        _ => {}
    }
}

/// Every part id and media key a block's inline objects hold (not comments: their ids are
/// `Document::comments` keys).
pub(crate) fn obj_refs_mut(b: &mut Block, f: &mut dyn FnMut(ObjRef<'_>)) {
    walk_mut(b, &mut |n| {
        if let NodeMut::Para(p) = n {
            for o in &mut p.objects {
                obj_refs(o, 0, f);
            }
        }
    });
}

/// Part ids a block refers to (notes, text boxes), read-only.
pub(crate) fn part_ids(b: &Block) -> Vec<u32> {
    let mut out = Vec::new();
    fn collect(o: &InlineObject, depth: usize, out: &mut Vec<u32>) {
        match o {
            InlineObject::Shape { story: Some(id), .. } | InlineObject::NoteRef { id, .. } => out.push(*id),
            InlineObject::Group { children, .. } if depth < MAX_DEPTH => {
                for c in children {
                    collect(&c.obj, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    paras(b, &mut |p| {
        for o in &p.objects {
            collect(o, 0, &mut out);
        }
    });
    out
}

/// Comment ids anchored in a block.
pub(crate) fn comment_ids(b: &Block) -> Vec<u32> {
    let mut out = Vec::new();
    paras(b, &mut |p| {
        for o in &p.objects {
            if let InlineObject::CommentStart { id } | InlineObject::CommentEnd { id } = o
                && !out.contains(id)
            {
                out.push(*id);
            }
        }
    });
    out
}

/// Style ids and list (`num`) ids a block uses directly.
pub(crate) fn style_and_list_refs(b: &Block, styles: &mut Vec<String>, nums: &mut Vec<u32>) {
    fn table_styles(b: &Block, depth: usize, styles: &mut Vec<String>) {
        if let Block::Table(t) = b {
            styles.extend(t.props.style.iter().cloned());
            if depth < MAX_DEPTH {
                for cb in t.rows.iter().flat_map(|r| r.cells.iter()).flat_map(|c| c.blocks.iter()) {
                    table_styles(cb, depth + 1, styles);
                }
            }
        }
    }
    table_styles(b, 0, styles);
    paras(b, &mut |p| {
        styles.extend(p.props.style.iter().cloned());
        styles.extend(p.mark.style.iter().cloned());
        styles.extend(p.runs.iter().filter_map(|r| r.props.style.clone()));
        nums.extend(p.props.numbering.iter().map(|n| n.num));
    });
}
