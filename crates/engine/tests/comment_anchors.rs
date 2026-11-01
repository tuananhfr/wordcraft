//! Anchors of Cloud comments (M5) between two co-editing sessions: an anchor taken on one page is
//! placed again on either after the other's edits (kept, moved by a paragraph above, by text
//! typed before it, by a paragraph split; the paragraph deleted, the quote found elsewhere or
//! orphaned), inside tables, and on a reopened later version.

mod live_room;

use live_room::*;
use serde_json::json;
use wordcraft_doc::{Block, StoryRef};
use wordcraft_engine::Session;
use wordcraft_engine::io::{open_bytes, save_bytes};
use wordcraft_engine::live::Region;
use wordcraft_engine::live::anchor::{Anchor, Placed, QUOTE_MAX};

fn select(s: &mut Session, block: usize, from: usize, to: usize) -> Option<Anchor> {
    run(s, "select.range", json!({"anchor": {"block": block, "off": from}, "focus": {"block": block, "off": to}}));
    s.comment_anchor()
}

/// Where an anchor lands: (top-level block, path below it, the quoted text there).
fn at(s: &Session, a: &Anchor) -> Option<(usize, Vec<u32>, String)> {
    match s.place_comment_anchor(a) {
        Placed::At { start, end, anchor } => {
            assert_eq!(start.path, end.path);
            let p = s.doc.para_at(&start).expect("a paragraph");
            assert_eq!(p.text.get(start.off..end.off), Some(anchor.soft.quote.as_str()));
            Some((start.path.0[0] as usize, start.path.0[1..].to_vec(), anchor.soft.quote.clone()))
        }
        Placed::Orphaned => None,
    }
}

fn now(s: &Session, a: &Anchor) -> Anchor {
    match s.place_comment_anchor(a) {
        Placed::At { anchor, .. } => *anchor,
        Placed::Orphaned => panic!("orphaned"),
    }
}

fn region(s: &Session, k: usize) -> String {
    Region::of(&s.doc.body[k]).expect("an id").id()
}

#[test]
fn an_anchor_names_the_paragraph_the_characters_and_their_context() {
    let mut room = Room::new(2, &doc(&["Báo cáo năm nay", "Hai"]));
    // Bytes 5..9 are "cáo".
    let a = select(room.page(0), 0, 5, 9).expect("an anchor");
    assert_eq!(a.region.as_deref(), Some(region(&room.pages[0], 0).as_str()));
    assert_eq!((a.pos.path.as_slice(), a.pos.start, a.pos.end), (&[][..], 4, 7));
    assert_eq!((a.soft.quote.as_str(), a.soft.before.as_str(), a.soft.after.as_str(), a.soft.block), ("cáo", "Báo ", " năm nay", 0));
    assert_eq!((a.quote.as_str(), a.label.as_str(), a.order.as_slice()), ("cáo", "“cáo”", &[0, 4][..]));
    assert_eq!(Anchor::parse(&a.to_json()), Some(a.clone()));
    for s in &room.pages {
        assert_eq!(now(s, &a), a, "unchanged where nothing moved");
    }
}

#[test]
fn a_caret_anchors_its_word_and_a_long_or_wide_selection_is_cut() {
    let mut room = Room::new(1, &doc(&["một hai ba", "", "x".repeat(QUOTE_MAX + 50).as_str()]));
    caret(room.page(0), 0, 6);
    assert_eq!(room.page(0).comment_anchor().map(|a| a.soft.quote), Some("hai".into()));
    caret(room.page(0), 1, 0);
    assert_eq!(room.page(0).comment_anchor(), None, "an empty paragraph has nothing to anchor");
    run(room.page(0), "select.range", json!({"anchor": {"block": 0, "off": 6}, "focus": {"block": 2, "off": 3}}));
    let wide = room.page(0).comment_anchor().expect("an anchor");
    assert_eq!(wide.soft.quote, "hai ba", "cut at the end of the paragraph it starts in");
    let long = select(room.page(0), 2, 0, QUOTE_MAX + 50).expect("an anchor");
    assert_eq!(long.soft.quote.chars().count(), QUOTE_MAX);
}

#[test]
fn a_paragraph_added_above_by_the_other_page_moves_the_anchor_on_both() {
    let mut room = Room::new(2, &doc(&["Một", "Đoạn hai nay"]));
    let b = "Đoạn hai ".len();
    let a = select(room.page(0), 1, b, b + 3).expect("an anchor");
    assert_eq!(a.soft.quote, "nay");
    caret(room.page(1), 0, 0);
    run(room.page(1), "text.newParagraph", json!({}));
    run(room.page(1), "text.insert", json!({"text": "Mới"}));
    room.wait(20_000.0);
    room.converged();
    for s in &room.pages {
        assert_eq!(at(s, &a), Some((2, vec![], "nay".into())));
        let moved = now(s, &a);
        assert_eq!(moved.region, a.region, "the same paragraph");
        assert_eq!((moved.soft.block, moved.order[0]), (2, 2));
    }
}

#[test]
fn text_typed_before_the_quote_shifts_it_and_a_split_takes_it_to_the_new_paragraph() {
    let mut room = Room::new(2, &doc(&["Mở đầu rồi kết luận"]));
    let text = room.pages[0].doc.para(StoryRef::Body, &wordcraft_doc::Path::top(0)).expect("para").text.clone();
    let b = text.find("kết").expect("found");
    let a = select(room.page(0), 0, b, text.len()).expect("an anchor");
    assert_eq!(a.soft.quote, "kết luận");
    type_at(room.page(1), 0, 0, "Thêm ");
    room.wait(20_000.0);
    let shifted = now(&room.pages[0], &a);
    assert_eq!((shifted.region.clone(), shifted.pos.start), (a.region.clone(), a.pos.start + 5));
    // The other page splits the paragraph before the quote.
    let text = room.pages[1].doc.para(StoryRef::Body, &wordcraft_doc::Path::top(0)).expect("para").text.clone();
    caret(room.page(1), 0, text.find("rồi").expect("found"));
    run(room.page(1), "text.newParagraph", json!({}));
    room.wait(20_000.0);
    room.converged();
    for s in &room.pages {
        let (k, _, quote) = at(s, &a).expect("found again");
        assert_eq!(quote, "kết luận");
        assert_eq!(now(s, &a).region.as_deref(), Some(region(s, k).as_str()));
        assert_eq!(s.doc.body.len(), 2);
    }
}

#[test]
fn a_deleted_paragraph_finds_its_quote_elsewhere_or_orphans_the_thread() {
    let mut room = Room::new(2, &doc(&["Tổng cộng", "Xoá đoạn này", "Khác", "Ghi chú: Tổng cộng"]));
    let found = select(room.page(0), 0, 0, "Tổng cộng".len()).expect("an anchor");
    let lost = select(room.page(0), 1, 0, "Xoá".len()).expect("an anchor");
    for k in [1, 0] {
        run(room.page(1), "select.range", json!({"anchor": {"block": k, "off": 0}, "focus": {"block": k + 1, "off": 0}}));
        run(room.page(1), "text.delete", json!({}));
        room.wait(20_000.0);
    }
    room.converged();
    for s in &room.pages {
        assert_eq!(at(s, &lost), None);
        let (k, _, quote) = at(s, &found).expect("the same words elsewhere");
        assert_eq!((k, quote.as_str()), (1, "Tổng cộng"));
    }
}

#[test]
fn an_edited_quote_stays_on_what_is_left_of_its_paragraph() {
    let mut room = Room::new(2, &doc(&["Giá là một trăm đồng"]));
    let text = room.pages[0].doc.para(StoryRef::Body, &wordcraft_doc::Path::top(0)).expect("para").text.clone();
    let b = text.find("một").expect("found");
    let a = select(room.page(0), 0, b, b + "một trăm".len()).expect("an anchor");
    run(room.page(1), "select.range", json!({"anchor": {"block": 0, "off": b}, "focus": {"block": 0, "off": b + "một".len()}}));
    run(room.page(1), "text.insert", json!({"text": "hai"}));
    room.wait(20_000.0);
    let kept = now(&room.pages[0], &a);
    assert_eq!(kept.region, a.region);
    assert_eq!((kept.pos.start, kept.soft.quote.as_str()), (a.pos.start, "hai trăm"), "up to where its context starts again");
    // Deleting the quote with its context leaves nothing to point at.
    run(room.page(1), "select.range", json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": 0, "off": usize::MAX}}));
    run(room.page(1), "text.insert", json!({"text": "Khác hẳn"}));
    room.wait(20_000.0);
    assert_eq!(at(&room.pages[0], &a), None);
}

#[test]
fn a_cell_of_a_table_anchors_the_table_region_and_its_path() {
    let mut room = Room::new(2, &doc(&["Trước", "Sau"]));
    caret(room.page(0), 0, "Trước".len());
    run(room.page(0), "insert.table", json!({"rows": 2, "cols": 2}));
    room.wait(20_000.0);
    let k = room.pages[1].doc.body.iter().position(|b| matches!(**b, Block::Table(_))).expect("a table");
    run(room.page(1), "caret.set", json!({"pos": {"story": "body", "path": [k, 1, 1, 0], "off": 0}}));
    run(room.page(1), "text.insert", json!({"text": "ô cuối"}));
    room.wait(20_000.0);
    run(
        room.page(0),
        "select.range",
        json!({"anchor": {"story": "body", "path": [k, 1, 1, 0], "off": 0}, "focus": {"story": "body", "path": [k, 1, 1, 0], "off": "ô".len()}}),
    );
    let a = room.page(0).comment_anchor().expect("an anchor");
    assert_eq!(a.region.as_deref(), Some(region(&room.pages[0], k).as_str()));
    assert!(a.region.as_deref().is_some_and(|r| r.starts_with("table:")));
    assert_eq!(a.pos.path, [1, 1, 0]);
    // A paragraph above moves the table; the anchor follows it into the same cell.
    caret(room.page(1), 0, 0);
    run(room.page(1), "text.newParagraph", json!({}));
    room.wait(20_000.0);
    assert_eq!(at(&room.pages[0], &a), Some((k + 1, vec![1, 1, 0], "ô".into())));
}

#[test]
fn a_reopened_later_version_keeps_the_paragraph_ids_and_the_anchors() {
    let mut room = Room::new(2, &doc(&["Một", "Hai ba"]));
    let a = select(room.page(0), 1, 4, 6).expect("an anchor");
    type_at(room.page(1), 0, 0, "Đầu ");
    room.wait(20_000.0);
    let bytes = save_bytes("x.docx", &room.pages[1].doc).expect("save");
    let later = Session::new(open_bytes("x.docx", &bytes).expect("open"));
    assert_eq!(now(&later, &a), a, "same paragraph, same characters");
    // Without its id (a block that never had one) the soft part still finds it.
    let mut orphan_id = a.clone();
    orphan_id.region = None;
    assert_eq!(at(&later, &orphan_id), Some((1, vec![], "ba".into())));
}
