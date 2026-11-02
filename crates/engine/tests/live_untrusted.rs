//! Region ids and payloads of other editors reach this page through the server unchecked (it
//! only checks who holds which lock string): whatever a peer sends, a run stays inside its
//! region, and a refused payload leaves the document as it was.

mod live_room;

use std::sync::Arc;

use live_room::*;
use serde_json::{Value, json};
use wordcraft_doc::ids::block_id;
use wordcraft_doc::{Block, SectionProps, Table};
use wordcraft_engine::Session;
use wordcraft_engine::io::save_bytes;
use wordcraft_engine::live::{Call, Lock};
use wordcraft_engine::live::payload::encode_doc;

/// What page `who` sends after `edit`, its locks granted, without delivering it.
fn sent(room: &mut Room, who: usize, edit: impl FnOnce(&mut Session)) -> Vec<(String, String)> {
    edit(room.page(who));
    let mut out = Vec::new();
    for _ in 0..10 {
        for c in room.live(who).take_calls() {
            match c {
                Call::WillEdit(r) => room.page(who).live_answer(&r, true),
                Call::Changed(r, p) => out.push((r, p)),
                _ => {}
            }
        }
    }
    out
}

/// A's run for typing at the start of paragraph `k`: (region, payload as JSON).
fn run_of(room: &mut Room, k: usize) -> (String, Value) {
    let sent = sent(room, 0, |s| type_at(s, k, 0, "A: "));
    let (r, p) = sent.into_iter().find(|(r, _)| r.starts_with("para:")).expect("a run");
    (r, serde_json::from_str(&p).unwrap())
}

fn id_of(s: &Session, k: usize) -> u32 {
    block_id(&s.doc.body[k]).unwrap()
}

fn snapshot(s: &Session) -> String {
    serde_json::to_string(&s.doc).unwrap()
}

/// B refuses `payload` for `region` and its document stays as it was.
fn refused(room: &mut Room, region: &str, payload: &str) -> String {
    let before = snapshot(&room.pages[1]);
    let err = room.page(1).live_apply_remote(region, payload).expect_err(&format!("{region} {payload:.300}"));
    assert_eq!(snapshot(&room.pages[1]), before, "{region}: refused, yet changed");
    err
}

#[test]
fn a_whole_document_payload_needs_the_star_lock_and_a_docx() {
    let mut room = Room::new(2, &doc(&["One", "Two"]));
    let evil = encode_doc(&save_bytes("x.docx", &doc(&["Replaced"])).unwrap());
    let para = para_region(&room.pages[1].doc, 0);
    // A lock on one paragraph doesn't let its holder replace the document.
    refused(&mut room, &para, &evil);
    let rtf = json!({"v": 1, "k": "doc", "x": "e1xydGYxIHNlY3JldH0="}).to_string();
    let err = refused(&mut room, "*", &rtf);
    assert!(!err.contains("secret"), "{err}");
    let err = refused(&mut room, &para, r#"{"v":1,"k":"run","r":["secret text"],"p":{}}"#);
    assert!(!err.contains("secret"), "{err}");
    room.page(1).live_apply_remote("*", &evil).unwrap();
    assert_eq!(texts(&room.pages[1].doc)[0], "Replaced");
}

#[test]
fn an_oversized_payload_is_refused_unread() {
    let mut room = Room::new(2, &doc(&["One"]));
    let para = para_region(&room.pages[1].doc, 0);
    let big = json!({"v": 1, "k": "doc", "x": "A".repeat(3 << 20)}).to_string();
    refused(&mut room, "*", &big);
    refused(&mut room, &para, &big);
}

#[test]
fn a_run_replaces_no_block_someone_else_is_editing() {
    let mut room = Room::new(3, &doc(&["One", "Two", "Three", "Four"]));
    let (region, honest) = run_of(&mut room, 0);
    let ids: Vec<String> = (0..4).map(|k| format!("{:08X}", id_of(&room.pages[1], k))).collect();
    // B types in the third paragraph, C holds the fourth.
    sent(&mut room, 1, |s| type_at(s, 2, 0, "B: "));
    let c_lock = Lock { region: para_region(&room.pages[1].doc, 3), holder: "m2".into(), name: "User 2".into(), colour: "#336699".into() };
    room.live(1).set_locks(vec![c_lock]);
    for r in [json!([ids[0], ids[2]]), json!([ids[0], ids[3]]), json!(ids)] {
        let mut p = honest.clone();
        p["r"] = r;
        refused(&mut room, &region, &p.to_string());
    }
    room.page(1).live_apply_remote(&region, &honest.to_string()).unwrap();
    assert_eq!(texts(&room.pages[1].doc)[..4], ["A: One", "Two", "B: Three", "Four"]);
}

#[test]
fn a_run_brings_only_new_or_replaced_block_ids() {
    let mut room = Room::new(2, &doc(&["One", "Two", "Three"]));
    let (region, honest) = run_of(&mut room, 0);
    let taken = id_of(&room.pages[1], 2);
    let block = honest["p"]["blocks"][0].clone();
    let with_id = |id: Value| {
        let mut p = honest.clone();
        p["p"]["blocks"][0]["id"] = id;
        p.to_string()
    };
    // Past `w14:paraId`'s range (it would freeze the page's next edit and break the DOCX), taken
    // by another paragraph, or missing.
    for id in [json!(0x8000_0000u32), json!(u32::MAX), json!(taken)] {
        refused(&mut room, &region, &with_id(id));
    }
    let mut p = honest.clone();
    p["p"]["blocks"][0].as_object_mut().unwrap().remove("id");
    refused(&mut room, &region, &p.to_string());
    let mut p = honest.clone();
    p["p"]["blocks"] = json!([block.clone(), block]);
    refused(&mut room, &region, &p.to_string());
}

#[test]
fn a_run_cannot_move_a_section_break() {
    let mut room = Room::new(2, &doc(&["One", "Two"]));
    let (region, mut p) = run_of(&mut room, 0);
    p["p"]["blocks"][0]["section"] = serde_json::to_value(SectionProps::default()).unwrap();
    refused(&mut room, &region, &p.to_string());
}

#[test]
fn a_run_nests_no_deeper_than_its_references_are_walked() {
    let mut room = Room::new(2, &doc(&["One"]));
    let (region, mut p) = run_of(&mut room, 0);
    // 17 tables: one past the depth the walks over references go (`walk::MAX_DEPTH`), yet within
    // serde's own recursion limit.
    let mut t = Table::new(1, 1, 100.0);
    for _ in 0..16 {
        let mut outer = Table::new(1, 1, 100.0);
        outer.rows[0].cells[0].blocks = vec![Arc::new(Block::Table(t))];
        t = outer;
    }
    t.rows[0].id = Some(0x0123_4567);
    p["p"]["blocks"].as_array_mut().unwrap().push(serde_json::to_value(Block::Table(t)).unwrap());
    refused(&mut room, &region, &p.to_string());
}

#[test]
fn a_run_touches_no_comment_anchored_elsewhere() {
    let mut room = Room::new(2, &doc(&["One", "Two words"]));
    run(room.page(0), "select.range", json!({"anchor": {"block": 1, "off": 0}, "focus": {"block": 1, "off": 3}}));
    run(room.page(0), "review.newComment", json!({"text": "keep me"}));
    room.wait(20_000.0);
    room.converged();
    let (&cid, c) = room.pages[1].doc.comments.iter().next().expect("the comment arrived");
    let mut forged = serde_json::to_value(c).unwrap();
    forged["resolved"] = json!(true);
    forged["author"] = json!("Mallory");
    let (region, honest) = run_of(&mut room, 0);
    let kept = |room: &Room| room.pages[1].doc.comments.get(&cid).is_some_and(|c| c.author != "Mallory" && !c.resolved);
    let mut p = honest.clone();
    p["p"]["comments"] = json!({ cid.to_string(): forged });
    room.page(1).live_apply_remote(&region, &p.to_string()).unwrap();
    assert!(kept(&room), "a run rewrote another paragraph's comment");
    // Anchored inside the run too, the next run's removal of that anchor keeps it.
    let mut p = honest.clone();
    p["p"]["blocks"][0]["objects"] = json!([{"type": "commentStart", "id": cid}]);
    room.page(1).live_apply_remote(&region, &p.to_string()).unwrap();
    room.page(1).live_apply_remote(&region, &honest.to_string()).unwrap();
    assert!(kept(&room), "a run deleted another paragraph's comment");
}
