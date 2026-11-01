//! Stable ids for live editing: paragraph and row ids survive edits, undo/redo and save/open;
//! the ids two clients make at once (paragraphs, lists, comments) never collide.

use std::collections::HashSet;

use serde_json::{Value, json};
use wordcraft_doc::{Document, Pos};
use wordcraft_engine::Session;
use wordcraft_engine::io::{open_bytes, save_bytes};

fn run(s: &mut Session, command: &str, params: Value) -> Value {
    s.run(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"))
}

fn session(prefix: &str, doc: Document) -> Session {
    let mut s = Session::new(doc);
    assert!(s.set_client_prefix(prefix));
    s
}

fn reopen(prefix: &str, s: &Session) -> Session {
    let bytes = save_bytes("live.docx", &s.doc).expect("save");
    session(prefix, open_bytes("live.docx", &bytes).expect("open"))
}

fn ids(s: &Session) -> Vec<u32> {
    s.doc.body.iter().filter_map(|b| b.as_para()).map(|p| p.id.expect("every paragraph has an id")).collect()
}

fn texts(s: &Session) -> Vec<String> {
    s.doc.body.iter().filter_map(|b| b.as_para()).map(|p| p.plain_text().to_string()).collect()
}

fn caret(s: &mut Session, block: usize, off: usize) {
    run(s, "caret.set", json!({"pos": {"story": "body", "path": [block], "off": off}}));
}

fn prefix_of(id: u32) -> u32 {
    id >> 7
}

/// A document every client opens: three paragraphs a file left without ids.
fn shared_bytes() -> Vec<u8> {
    save_bytes("base.docx", &Document::from_text("Alpha\nBeta\nGamma")).expect("save")
}

#[test]
fn a_file_without_ids_gets_the_same_ids_on_every_client() {
    let bytes = shared_bytes();
    let a = session("aaaaaa", open_bytes("b.docx", &bytes).unwrap());
    let b = session("bbbbbb", open_bytes("b.docx", &bytes).unwrap());
    assert_eq!(ids(&a), [1, 2, 3]);
    assert_eq!(ids(&a), ids(&b));
    // Saved and opened again by anyone: the same ids, never rewritten.
    assert_eq!(ids(&reopen("cccccc", &a)), [1, 2, 3]);
}

#[test]
fn ids_survive_typing_enter_undo_redo_and_save() {
    let mut s = session("0c0c0c", open_bytes("b.docx", &shared_bytes()).unwrap());
    caret(&mut s, 1, 2);
    run(&mut s, "text.insert", json!({"text": "xx"}));
    assert_eq!(ids(&s), [1, 2, 3], "typing keeps the id");
    // Enter in the middle: the head keeps its id, the new paragraph gets one with the prefix.
    run(&mut s, "text.newParagraph", json!({}));
    let after_split = ids(&s);
    assert_eq!(texts(&s), ["Alpha", "Bexx", "ta", "Gamma"]);
    assert_eq!(after_split[..2], [1, 2]);
    assert_eq!(prefix_of(after_split[2]), 0x0c0c0c);
    assert_eq!(after_split[3], 3);
    // Enter at the start of a paragraph: the text keeps its id, the empty one above is new.
    caret(&mut s, 3, 0);
    run(&mut s, "text.newParagraph", json!({}));
    let after_enter = ids(&s);
    assert_eq!(texts(&s)[3..], ["".to_string(), "Gamma".to_string()]);
    assert_eq!(after_enter[4], 3, "Gamma keeps its id");
    assert_eq!(prefix_of(after_enter[3]), 0x0c0c0c);
    // Backspace at the start of "ta" merges it into "Bexx", which keeps its id.
    caret(&mut s, 2, 0);
    run(&mut s, "text.backspace", json!({}));
    assert_eq!(texts(&s)[1], "Bexxta");
    assert_eq!(ids(&s)[1], 2);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(ids(&s), after_enter, "undo brings the ids back");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(ids(&s), after_split);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(ids(&s), after_enter, "redo too: no new ids");
    assert_eq!(ids(&reopen("d0d0d0", &s)), after_enter, "save and open keep them");
    let unique: HashSet<u32> = after_enter.iter().copied().collect();
    assert_eq!(unique.len(), after_enter.len());
}

#[test]
fn a_pasted_copy_gets_a_new_id_and_the_original_keeps_its_own() {
    let mut s = session("0d0d0d", open_bytes("b.docx", &shared_bytes()).unwrap());
    run(&mut s, "select.range", json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": 1, "off": 0}}));
    run(&mut s, "edit.copy", json!({}));
    caret(&mut s, 2, 0);
    run(&mut s, "edit.paste", json!({}));
    assert_eq!(texts(&s), ["Alpha", "Beta", "Alpha", "Gamma"]);
    let got = ids(&s);
    assert_eq!([got[0], got[1], got[3]], [1, 2, 3]);
    assert_eq!(prefix_of(got[2]), 0x0d0d0d);
    // Text pasted at the start of a paragraph leaves it its id.
    run(&mut s, "select.range", json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": 0, "off": 2}}));
    run(&mut s, "edit.copy", json!({}));
    caret(&mut s, 1, 0);
    run(&mut s, "edit.paste", json!({}));
    assert_eq!(texts(&s)[1], "AlBeta");
    assert_eq!(ids(&s), got);
}

#[test]
fn table_rows_get_ids_and_a_table_is_known_by_its_first_row() {
    let mut s = session("0e0e0e", Document::new());
    run(&mut s, "insert.table", json!({"rows": 2, "cols": 2}));
    let t = s.doc.body.iter().find_map(|b| match &**b {
        wordcraft_doc::Block::Table(t) => Some(t.clone()),
        _ => None,
    });
    let t = t.expect("table inserted");
    let rows: Vec<u32> = t.rows.iter().map(|r| r.id.expect("row id")).collect();
    assert!(rows.iter().all(|id| prefix_of(*id) == 0x0e0e0e), "{rows:x?}");
    let reopened = reopen("0f0f0f", &s);
    let t2 = reopened.doc.body.iter().find_map(|b| match &**b {
        wordcraft_doc::Block::Table(t) => Some(t.rows.iter().map(|r| r.id).collect::<Vec<_>>()),
        _ => None,
    });
    assert_eq!(t2, Some(rows.iter().copied().map(Some).collect()));
}

/// The spike's `concurrent_new_lists_collide_on_list_ids`, now passing: two clients on the same
/// document each start a list, add a comment and new paragraphs; none of their ids collide.
#[test]
fn two_clients_editing_at_once_never_make_the_same_id() {
    let bytes = shared_bytes();
    let mut made: Vec<(&str, HashSet<u32>, HashSet<u32>, HashSet<u32>)> = Vec::new();
    for (prefix, block) in [("a1a1a1", 0usize), ("a1a1a2", 2)] {
        let mut s = session(prefix, open_bytes("b.docx", &bytes).unwrap());
        let (lists_before, paras_before) = (s.doc.list_ids(), ids(&s));
        caret(&mut s, block, 0);
        run(&mut s, "para.numbering", json!({}));
        caret(&mut s, block, 2);
        run(&mut s, "select.range", json!({"anchor": {"block": block, "off": 0}, "focus": {"block": block, "off": 3}}));
        run(&mut s, "review.newComment", json!({"text": "?"}));
        for _ in 0..5 {
            run(&mut s, "caret.docEnd", json!({}));
            // Enter on an empty list item would end the list instead of adding a paragraph.
            run(&mut s, "text.insert", json!({"text": "x"}));
            run(&mut s, "text.newParagraph", json!({}));
        }
        let lists: HashSet<u32> = s.doc.list_ids().difference(&lists_before).copied().collect();
        let comments: HashSet<u32> = s.doc.comments.keys().copied().collect();
        let paras: HashSet<u32> = ids(&s).into_iter().filter(|i| !paras_before.contains(i)).collect();
        assert_eq!(lists.len(), 2, "a new list: its abstract list and its list");
        assert_eq!(comments.len(), 1);
        assert_eq!(paras.len(), 5);
        made.push((prefix, lists, comments, paras));
    }
    let (a, b) = (&made[0], &made[1]);
    assert!(a.1.is_disjoint(&b.1), "list ids {:?} / {:?}", a.1, b.1);
    assert!(a.2.is_disjoint(&b.2), "comment ids {:?} / {:?}", a.2, b.2);
    assert!(a.3.is_disjoint(&b.3), "paragraph ids {:?} / {:?}", a.3, b.3);
}

#[test]
fn a_bad_prefix_changes_nothing() {
    let mut s = session("123abc", Document::new());
    for bad in ["", "123ABC", "12345", "1234567", "12345g"] {
        assert!(!s.set_client_prefix(bad), "{bad:?}");
    }
    assert_eq!(s.client_prefix(), "123abc");
    run(&mut s, "text.insert", json!({"text": "x"}));
    run(&mut s, "text.newParagraph", json!({}));
    assert!(s.doc.para_at(&Pos::body(1, 0)).and_then(|p| p.id).is_some_and(|id| prefix_of(id) == 0x123abc));
}
