//! Region co-editing between sessions of one shared document, through a fake room that grants
//! locks like the server (`*` clashes with everything, a region with itself, one ordered queue).

use serde_json::{Value, json};
use wordcraft_doc::{Block, CharProps, Document, Paragraph, StoryRef, para_block};
use wordcraft_engine::Session;
use wordcraft_engine::io::{open_bytes, save_bytes};
use wordcraft_engine::live::payload::{MediaHashes, fingerprint};
use wordcraft_engine::live::{Call, LOCK_DENIED, LOCKED_BY, Live, Lock, NOTHING_TO_UNDO, Region};

fn run(s: &mut Session, command: &str, params: Value) {
    s.run(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
}

fn doc(paras: &[&str]) -> Document {
    let mut d = Document::new();
    d.body = paras.iter().map(|t| para_block(Paragraph::with_text(t, CharProps::default()))).collect();
    // Every page opens the same bytes, so ids are the same everywhere.
    open_bytes("shared.docx", &save_bytes("shared.docx", &d).expect("save")).expect("open")
}

struct Room {
    pages: Vec<Session>,
    locks: Vec<(String, usize)>,
    clock: f64,
    /// Selections reported, per page.
    selections: Vec<Vec<(String, String)>>,
    /// Payloads sent, per region.
    sent: Vec<(String, String)>,
}

impl Room {
    fn new(n: usize, d: &Document) -> Room {
        let pages = (0..n)
            .map(|i| {
                let mut s = Session::new(d.clone());
                assert!(s.set_client_prefix(&format!("a0000{i}")));
                let mut live = Live::new(&s.doc);
                live.set_online(true);
                s.live = Some(live);
                s
            })
            .collect();
        Room { pages, locks: Vec::new(), clock: 1_000.0, selections: vec![Vec::new(); n], sent: Vec::new() }
    }

    fn page(&mut self, i: usize) -> &mut Session {
        &mut self.pages[i]
    }

    fn live(&mut self, i: usize) -> &mut Live {
        self.pages[i].live.as_mut().expect("co-editing")
    }

    /// Passes calls between the pages and the room until nothing moves.
    fn settle(&mut self) {
        assert!(self.try_settle(), "the room never settled");
    }

    fn try_settle(&mut self) -> bool {
        for _ in 0..100 {
            let mut moved = false;
            let n = self.pages.len();
            let mut answers: Vec<(usize, String, bool)> = Vec::new();
            let mut applies: Vec<(usize, String, String)> = Vec::new();
            for i in 0..n {
                self.pages[i].live_tick(self.clock);
                for call in self.live(i).take_calls() {
                    moved = true;
                    match call {
                        Call::WillEdit(r) => {
                            let want = Region::parse(&r).expect("region id");
                            let free = !self.locks.iter().any(|(h, by)| *by != i && Region::parse(h).expect("held").clashes(&want));
                            if free && !self.locks.contains(&(r.clone(), i)) {
                                self.locks.push((r.clone(), i));
                            }
                            answers.push((i, r, free));
                        }
                        Call::Changed(r, p) => {
                            let held = self.locks.iter().any(|(h, by)| *by == i && (*h == r || h == "*"));
                            assert!(held, "page {i} sent {r} without its lock ({:?})", self.locks);
                            self.sent.push((r.clone(), p.clone()));
                            applies.extend((0..n).filter(|j| *j != i).map(|j| (j, r.clone(), p.clone())));
                        }
                        Call::DidEndEdit(r) => self.locks.retain(|(h, by)| !(*h == r && *by == i)),
                        Call::Selection(r, range) => self.selections[i].push((r, range)),
                    }
                }
            }
            for (j, r, p) in applies {
                self.pages[j].live_apply_remote(&r, &p).unwrap_or_else(|e| panic!("page {j} applying {r}: {e}"));
            }
            for j in 0..n {
                let others = self
                    .locks
                    .iter()
                    .filter(|(_, by)| *by != j)
                    .map(|(r, by)| Lock { region: r.clone(), holder: format!("m{by}"), name: format!("User {by}"), colour: "#336699".into() })
                    .collect();
                self.live(j).set_locks(others);
            }
            for (i, r, granted) in answers {
                self.pages[i].live_answer(&r, granted);
            }
            if !moved {
                return true;
            }
        }
        false
    }

    /// Time passes (regions left behind are handed back).
    fn wait(&mut self, ms: f64) {
        self.settle();
        self.clock += ms;
        self.settle();
    }

    fn converged(&self) {
        let mut h = MediaHashes::default();
        let first = &self.pages[0].doc;
        let want = fingerprint(first, &first.body, &mut h);
        for (i, p) in self.pages.iter().enumerate().skip(1) {
            assert_eq!(fingerprint(&p.doc, &p.doc.body, &mut h), want, "page {i} differs: {:?} vs {:?}", texts(&p.doc), texts(first));
        }
    }
}

fn texts(d: &Document) -> Vec<String> {
    d.plain_text(StoryRef::Body).split('\n').map(str::to_string).collect()
}

fn caret(s: &mut Session, block: usize, off: usize) {
    run(s, "caret.set", json!({"pos": {"story": "body", "path": [block], "off": off}}));
}

fn type_at(s: &mut Session, block: usize, off: usize, text: &str) {
    caret(s, block, off);
    run(s, "text.insert", json!({"text": text}));
}

fn para_region(d: &Document, k: usize) -> String {
    Region::of(&d.body[k]).expect("id").id()
}

#[test]
fn edits_of_different_paragraphs_converge() {
    let mut room = Room::new(2, &doc(&["One", "Two", "Three"]));
    type_at(room.page(0), 0, 3, " A");
    type_at(room.page(1), 2, 5, " B");
    room.settle();
    room.converged();
    assert_eq!(texts(&room.pages[1].doc)[..3], ["One A", "Two", "Three B"]);
    let mut regions: Vec<String> = room.locks.iter().map(|(r, _)| r.clone()).collect();
    regions.sort();
    let d = &room.pages[0].doc;
    let mut want = vec![para_region(d, 0), para_region(d, 2)];
    want.sort();
    assert_eq!(regions, want);
}

#[test]
fn keystrokes_typed_before_the_lock_comes_all_land_in_order() {
    let mut room = Room::new(2, &doc(&["Hello"]));
    caret(room.page(0), 0, 5);
    for ch in [",", " ", "w", "o", "r", "l", "d"] {
        run(room.page(0), "text.insert", json!({"text": ch}));
    }
    // Nothing shows before the lock is ours.
    assert_eq!(texts(&room.pages[0].doc)[0], "Hello");
    room.settle();
    assert_eq!(texts(&room.pages[0].doc)[0], "Hello, world");
    room.converged();
}

#[test]
fn a_paragraph_someone_holds_refuses_the_others() {
    let mut room = Room::new(2, &doc(&["Shared", "Other"]));
    type_at(room.page(0), 0, 6, "!");
    room.settle();
    type_at(room.page(1), 0, 0, "x");
    assert_eq!(texts(&room.pages[1].doc)[0], "Shared!");
    let f = room.live(1).take_feedback().expect("told why");
    assert_eq!((f.text, f.name.as_str()), (LOCKED_BY, "User 0"));
    // Another paragraph is free.
    type_at(room.page(1), 1, 5, "?");
    room.settle();
    room.converged();
    assert_eq!(texts(&room.pages[0].doc)[..2], ["Shared!", "Other?"]);
}

#[test]
fn a_denied_lock_takes_the_edit_back() {
    let mut room = Room::new(2, &doc(&["Race"]));
    type_at(room.page(0), 0, 4, " won");
    type_at(room.page(1), 0, 0, "Lost ");
    room.settle();
    assert_eq!(room.live(1).take_feedback().map(|f| f.text), Some(LOCK_DENIED));
    room.converged();
    assert_eq!(texts(&room.pages[1].doc)[0], "Race won");
}

#[test]
fn a_new_paragraph_travels_with_the_one_it_came_from() {
    let mut room = Room::new(2, &doc(&["First half second half", "Next"]));
    caret(room.page(0), 0, 10);
    run(room.page(0), "text.newParagraph", json!({}));
    run(room.page(0), "text.insert", json!({"text": "NEW "}));
    run(room.page(0), "text.newParagraph", json!({}));
    room.settle();
    room.converged();
    let d = &room.pages[1].doc;
    assert_eq!(texts(d)[..4], ["First half", "NEW ", " second half", "Next"]);
    // Only the paragraph that was there is locked; the new ones need none.
    assert_eq!(room.locks, [(para_region(d, 0), 0)]);
    // Once handed back, a new paragraph is a region of its own.
    caret(room.page(0), 3, 0);
    room.wait(4_000.0);
    assert!(room.locks.is_empty(), "{:?}", room.locks);
    type_at(room.page(1), 1, 0, "B: ");
    room.settle();
    assert_eq!(room.locks, [(para_region(&room.pages[0].doc, 1), 1)]);
    room.converged();
}

#[test]
fn deleting_across_paragraphs_takes_both_and_converges() {
    let mut room = Room::new(2, &doc(&["Alpha", "Beta", "Gamma"]));
    run(room.page(0), "select.range", json!({"anchor": {"story": "body", "path": [0], "off": 3}, "focus": {"story": "body", "path": [1], "off": 2}}));
    run(room.page(0), "text.backspace", json!({}));
    room.settle();
    room.converged();
    assert_eq!(texts(&room.pages[1].doc)[..2], ["Alpta", "Gamma"]);
    assert_eq!(room.locks.len(), 2, "{:?}", room.locks);
}

#[test]
fn a_table_is_one_region() {
    let mut room = Room::new(2, &doc(&["Before", "After"]));
    caret(room.page(0), 0, 6);
    run(room.page(0), "insert.table", json!({"rows": 2, "cols": 2}));
    room.wait(20_000.0);
    room.converged();
    let k = room.pages[1].doc.body.iter().position(|b| matches!(**b, Block::Table(_))).expect("the table arrived");
    let table = Region::of(&room.pages[1].doc.body[k]).expect("table id");
    assert!(matches!(table, Region::Table(_)));
    // Typing in a cell locks the table.
    run(room.page(1), "caret.set", json!({"pos": {"story": "body", "path": [k, 1, 1, 0], "off": 0}}));
    run(room.page(1), "text.insert", json!({"text": "cell"}));
    room.settle();
    assert_eq!(room.locks, [(table.id(), 1)]);
    room.converged();
    run(room.page(0), "caret.set", json!({"pos": {"story": "body", "path": [k, 0, 0, 0], "off": 0}}));
    run(room.page(0), "text.insert", json!({"text": "no"}));
    assert_eq!(room.live(0).take_feedback().map(|f| f.text), Some(LOCKED_BY));
}

#[test]
fn document_wide_edits_take_the_star_lock() {
    let mut room = Room::new(2, &doc(&["Page"]));
    run(room.page(0), "layout.orientation", json!({"value": "landscape"}));
    room.settle();
    assert_eq!(room.locks, [("*".to_string(), 0)]);
    assert!(room.pages[1].doc.last_section.landscape, "the other page turned too");
    room.converged();
    // Nobody else edits while `*` is held.
    type_at(room.page(1), 0, 0, "x");
    assert_eq!(room.live(1).take_feedback().map(|f| f.text), Some(LOCKED_BY));
    room.wait(3_000.0);
    assert!(room.locks.is_empty());
    let modified = json!({"style": "Normal", "chr": {"bold": true}});
    run(room.page(1), "styles.modify", modified);
    room.settle();
    room.converged();
    assert_eq!(room.sent.last().map(|(r, _)| r.as_str()), Some("*"));
}

#[test]
fn undo_takes_back_only_this_users_steps() {
    let mut room = Room::new(2, &doc(&["Mine", "Theirs"]));
    type_at(room.page(0), 0, 4, " 1");
    room.settle();
    type_at(room.page(1), 1, 6, " 2");
    room.settle();
    run(room.page(0), "edit.undo", json!({}));
    room.settle();
    room.converged();
    assert_eq!(texts(&room.pages[1].doc)[..2], ["Mine", "Theirs 2"]);
    run(room.page(0), "edit.redo", json!({}));
    room.settle();
    room.converged();
    assert_eq!(texts(&room.pages[1].doc)[..2], ["Mine 1", "Theirs 2"]);
}

#[test]
fn undo_skips_a_step_someone_else_overwrote() {
    let mut room = Room::new(2, &doc(&["Text"]));
    type_at(room.page(0), 0, 4, " mine");
    room.wait(20_000.0);
    type_at(room.page(1), 0, 0, "Their ");
    room.wait(20_000.0);
    run(room.page(0), "edit.undo", json!({}));
    room.settle();
    assert_eq!(room.live(0).take_feedback().map(|f| f.text), Some(NOTHING_TO_UNDO));
    room.converged();
    assert_eq!(texts(&room.pages[0].doc)[0], "Their Text mine");
}

#[test]
fn typing_is_one_undo_step() {
    let mut room = Room::new(2, &doc(&["Word"]));
    caret(room.page(0), 0, 4);
    for ch in [" ", "a", "b", "c"] {
        run(room.page(0), "text.insert", json!({"text": ch}));
        room.settle();
    }
    run(room.page(0), "edit.undo", json!({}));
    room.settle();
    room.converged();
    assert_eq!(texts(&room.pages[1].doc)[0], "Word");
}

#[test]
fn undo_of_a_split_brings_the_paragraph_back_everywhere() {
    let mut room = Room::new(2, &doc(&["Split here", "Tail"]));
    caret(room.page(0), 0, 5);
    run(room.page(0), "text.newParagraph", json!({}));
    room.settle();
    assert_eq!(texts(&room.pages[1].doc)[..3], ["Split", " here", "Tail"]);
    run(room.page(0), "edit.undo", json!({}));
    room.settle();
    room.converged();
    assert_eq!(texts(&room.pages[1].doc)[..2], ["Split here", "Tail"]);
}

#[test]
fn remote_edits_keep_the_caret_on_its_paragraph() {
    let mut room = Room::new(2, &doc(&["One", "Two", "Three"]));
    caret(room.page(1), 2, 3);
    caret(room.page(0), 0, 3);
    run(room.page(0), "text.newParagraph", json!({}));
    run(room.page(0), "text.insert", json!({"text": "inserted"}));
    room.settle();
    let s = &room.pages[1];
    assert_eq!(s.sel.focus.path.0, [3]);
    assert_eq!(s.sel.focus.off, 3);
}

#[test]
fn ids_survive_save_and_open_after_remote_applies() {
    let mut room = Room::new(2, &doc(&["Keep", "ids"]));
    caret(room.page(0), 0, 4);
    run(room.page(0), "text.newParagraph", json!({}));
    run(room.page(0), "text.insert", json!({"text": "fresh"}));
    room.settle();
    let d = &room.pages[1].doc;
    let reopened = open_bytes("x.docx", &save_bytes("x.docx", d).expect("save")).expect("open");
    let ids = |d: &Document| d.body.iter().map(|b| Region::of(b)).collect::<Vec<_>>();
    assert_eq!(ids(&reopened), ids(&room.pages[0].doc));
    // A payload made after the reopen still finds its paragraph.
    let mut third = Session::new(reopened);
    third.live = Some(Live::new(&third.doc));
    let (r, p) = room.sent.last().cloned().expect("sent");
    third.live_apply_remote(&r, &p).expect("applies");
    assert_eq!(texts(&third.doc)[..3], ["Keep", "fresh", "ids"]);
}

#[test]
fn without_co_editing_nothing_changes() {
    let mut s = Session::new(doc(&["Plain"]));
    type_at(&mut s, 0, 5, " text");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(texts(&s.doc)[0], "Plain");
    assert!(!s.live_on());
}

#[test]
fn our_selection_is_reported_once_per_change() {
    let mut room = Room::new(2, &doc(&["Look here", "And there"]));
    caret(room.page(0), 1, 4);
    room.settle();
    room.settle();
    let d = room.pages[0].doc.clone();
    let id = Region::of(&d.body[1]).and_then(|r| r.block()).expect("id");
    let want = (para_region(&d, 1), format!("{id:08X}:4;{id:08X}:4"));
    assert_eq!(room.selections[0].iter().filter(|s| **s == want).count(), 1, "{:?}", room.selections[0]);
    let (a, f) = wordcraft_engine::live::range_positions(&room.pages[1].doc, &want.1).expect("found on the other page");
    assert_eq!((a.path.0, f.off), (vec![1], 4));
}

#[test]
fn offline_edits_need_no_lock_and_still_go_out() {
    let mut room = Room::new(2, &doc(&["Away"]));
    room.live(0).set_online(false);
    type_at(room.page(0), 0, 4, " offline");
    assert_eq!(texts(&room.pages[0].doc)[0], "Away offline");
    let calls = room.live(0).take_calls();
    assert!(calls.iter().any(|c| matches!(c, Call::Changed(..))));
    assert!(!calls.iter().any(|c| matches!(c, Call::WillEdit(..))));
}

#[test]
fn a_picture_travels_with_its_bytes_until_the_room_has_them() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let mut room = Room::new(2, &doc(&["Picture: ", "After"]));
    caret(room.page(0), 0, 9);
    run(room.page(0), "insert.picture", json!({"data": PNG, "width": 20}));
    run(room.page(0), "caret.end", json!({}));
    run(room.page(0), "text.insert", json!({"text": " caption"}));
    room.settle();
    room.converged();
    assert_eq!(room.pages[1].doc.media.len(), 1, "the picture arrived");
    // Every payload of that lock carries the bytes: the shell keeps only the newest one.
    let with_bytes = room.sent.iter().filter(|(_, p)| p.contains("\"data\"")).count();
    assert_eq!(with_bytes, room.sent.len());
    room.wait(20_000.0);
    let before = room.sent.len();
    type_at(room.page(0), 0, 0, "A ");
    room.settle();
    let (_, last) = room.sent.last().expect("sent");
    assert!(room.sent.len() > before && !last.contains("\"data\""), "once handed back, only its hash travels");
    room.converged();
}

/// Every mutating command, from a caret and from a selection across paragraphs: what reaches the
/// other page makes the same document, and what changes more than the body's blocks takes `*`.
#[test]
fn every_command_converges_through_its_payloads() {
    let sample = wordcraft_engine::sample::sample_document();
    let reg = wordcraft_engine::cmd::registry();
    let skip = ["insert.picture", "insert.textFromFile", "edit.undo", "edit.redo", "edit.undoTo", "edit.repeat"];
    let (mut ran, mut star) = (0, Vec::new());
    let mut diverged = Vec::new();
    for spec in reg.all() {
        if !spec.mutates || spec.id.starts_with("file.") || skip.contains(&spec.id) {
            continue;
        }
        for start in 0..2 {
            let mut room = Room::new(2, &sample);
            match start {
                0 => caret(room.page(0), 2, 40),
                _ => run(
                    room.page(0),
                    "select.range",
                    json!({"anchor": {"story": "body", "path": [3], "off": 2}, "focus": {"story": "body", "path": [5], "off": 4}}),
                ),
            }
            if room.page(0).run(spec.id, &json!({})).is_err() || !room.pages[0].live_on() {
                continue;
            }
            ran += 1;
            let settled = room.try_settle();
            room.clock += 20_000.0;
            if !settled || !room.try_settle() {
                diverged.push((spec.id, start + 10));
                continue;
            }
            if room.sent.iter().any(|(r, _)| r == "*") && !star.contains(&spec.id) {
                star.push(spec.id);
            }
            let mut h = MediaHashes::default();
            let (a, b) = (&room.pages[0].doc, &room.pages[1].doc);
            let styles = |d: &Document| {
                let mut v: Vec<String> = d.styles.styles.iter().map(|s| serde_json::to_string(s).unwrap_or_default()).collect();
                v.sort();
                v
            };
            if fingerprint(a, &a.body, &mut h) != fingerprint(b, &b.body, &mut h) || a.last_section != b.last_section || styles(a) != styles(b) {
                diverged.push((spec.id, start));
            }
        }
    }
    eprintln!("ran {ran}; whole-document ({}): {star:?}", star.len());
    assert!(ran > 100, "only {ran} commands ran");
    assert!(diverged.is_empty(), "diverged: {diverged:?}");
}

/// The cost per keystroke on a 5,000-paragraph document: the id pass every edit runs, a plain
/// keystroke, and one through co-editing (diff, payload, undo fingerprint). Printed; the bound is
/// loose, only to catch something going quadratic.
#[test]
fn keystroke_cost_on_a_large_document() {
    use std::time::Instant;
    let paras: Vec<String> = (0..5_000).map(|i| format!("Paragraph {i} with some ordinary words in it, enough to look like text.")).collect();
    let refs: Vec<&str> = paras.iter().map(String::as_str).collect();
    let d = doc(&refs);
    let mut copy = d.clone();
    let t = Instant::now();
    for _ in 0..100 {
        wordcraft_doc::ids::ensure_ids(&mut copy, Some(&d), &mut |_| None);
    }
    let ids = t.elapsed().as_secs_f64() * 1000.0 / 100.0;
    let mut plain = Session::new(d.clone());
    caret(&mut plain, 2_500, 3);
    let t = Instant::now();
    for _ in 0..100 {
        run(&mut plain, "text.insert", json!({"text": "x"}));
    }
    let plain_ms = t.elapsed().as_secs_f64() * 1000.0 / 100.0;
    let mut room = Room::new(2, &d);
    caret(room.page(0), 2_500, 3);
    run(room.page(0), "text.insert", json!({"text": "x"}));
    room.settle();
    let t = Instant::now();
    for _ in 0..100 {
        run(room.page(0), "text.insert", json!({"text": "x"}));
    }
    let live_ms = t.elapsed().as_secs_f64() * 1000.0 / 100.0;
    let t = Instant::now();
    room.settle();
    let apply_ms = t.elapsed().as_secs_f64() * 1000.0 / 100.0;
    room.converged();
    eprintln!(
        "5000 paragraphs, per keystroke: ensure_ids {ids:.3} ms, plain {plain_ms:.3} ms, co-editing {live_ms:.3} ms, remote apply {apply_ms:.3} ms"
    );
    assert!(live_ms < 250.0, "co-editing keystroke took {live_ms} ms");
}
