//! `erpHost` v3 through the app: two pages of one shared document and a fake room between them
//! that grants locks like the server.

use serde_json::json;
use wordcraft_engine::live::{Call, Region};

use super::LiveMsg;
use super::live::{cursors_of, locks_of, present};
use super::tests::{Page, body, hand_in, type_text, web_app};
use crate::WordApp;

fn page() -> (WordApp, Page) {
    let (mut app, page) = web_app();
    page.link.set_regions();
    hand_in(&page, "d1", "Biên bản", "Đoạn một");
    app.host_tick(&page.ctx, 0.0);
    page.host.take_calls();
    page.link.live_message(LiveMsg::Presence(true));
    app.host_tick(&page.ctx, 0.0);
    (app, page)
}

/// Frames on both pages until the room has nothing left to pass on.
fn exchange(pages: &mut [(WordApp, Page); 2], locks: &mut Vec<(String, usize)>, now: f64) {
    for _ in 0..20 {
        let mut moved = false;
        for i in 0..2 {
            let (app, page) = &mut pages[i];
            app.host_tick(&page.ctx, now);
            let calls = std::mem::take(&mut *page.host.regions.borrow_mut());
            let answer = page.host.answer.borrow().clone();
            for call in calls {
                match call {
                    Call::WillEdit(r) => {
                        moved = true;
                        let want = Region::parse(&r).expect("region");
                        let free = !locks.iter().any(|(h, by)| *by != i && Region::parse(h).expect("held").clashes(&want));
                        if free {
                            locks.push((r.clone(), i));
                        }
                        if let Some(answer) = &answer {
                            answer(r, free);
                        }
                    }
                    Call::Changed(r, p) => {
                        moved = true;
                        pages[1 - i].1.link.live_message(LiveMsg::Apply(r, p));
                    }
                    Call::DidEndEdit(r) => {
                        moved = true;
                        locks.retain(|(h, by)| !(h == &r && *by == i));
                    }
                    Call::Selection(..) => {}
                }
            }
        }
        for (i, (_, page)) in pages.iter().enumerate() {
            let shown = json!(locks.iter().map(|(r, by)| json!({"region": r, "holder": format!("m{by}"), "name": format!("Người {by}"), "colour": "#22aa66", "mine": *by == i})).collect::<Vec<_>>());
            page.link.live_message(LiveMsg::Locks(locks_of(&shown)));
        }
        if !moved {
            return;
        }
    }
}

#[test]
fn the_shared_document_is_co_edited_only_with_a_v3_shell() {
    let (app, _page) = page();
    assert!(app.session.live_on());
    let (mut plain, page) = web_app();
    hand_in(&page, "d1", "Biên bản", "Đoạn một");
    plain.host_tick(&page.ctx, 0.0);
    assert!(!plain.session.live_on());
}

#[test]
fn typing_locks_the_paragraph_first_and_reaches_the_other_page() {
    let mut pages = [page(), page()];
    let mut locks = Vec::new();
    type_text(&mut pages[0].0, "A ");
    assert!(!body(&pages[0].0).contains("A "), "nothing shows before the lock is ours");
    exchange(&mut pages, &mut locks, 0.0);
    assert_eq!(locks.len(), 1);
    assert!(body(&pages[1].0).starts_with("A Đoạn một"), "{}", body(&pages[1].0));
    // The other page can't type there, and the status bar says who is.
    type_text(&mut pages[1].0, "B");
    exchange(&mut pages, &mut locks, 0.0);
    let status = pages[1].0.status_msg.clone().map(|(s, _)| s).unwrap_or_default();
    assert!(status.contains("Người 0"), "{status}");
    assert!(!body(&pages[1].0).contains('B'));
    // Left alone, the paragraph is handed back.
    exchange(&mut pages, &mut locks, 20_000.0);
    assert!(locks.is_empty(), "{locks:?}");
}

#[test]
fn undo_on_a_page_keeps_the_other_pages_edit() {
    let mut pages = [page(), page()];
    let mut locks = Vec::new();
    type_text(&mut pages[0].0, "Mine ");
    exchange(&mut pages, &mut locks, 0.0);
    exchange(&mut pages, &mut locks, 20_000.0);
    pages[1].0.run("caret.docEnd", json!({})).expect("caret");
    pages[1].0.run("text.newParagraph", json!({})).expect("enter");
    type_text(&mut pages[1].0, "Theirs");
    exchange(&mut pages, &mut locks, 20_000.0);
    exchange(&mut pages, &mut locks, 40_000.0);
    pages[0].0.run("edit.undo", json!({})).expect("undo");
    exchange(&mut pages, &mut locks, 40_000.0);
    for (app, _) in &pages {
        let text = body(app);
        assert!(!text.contains("Mine"), "{text}");
        assert!(text.contains("Theirs"), "{text}");
    }
}

#[test]
fn locks_cursors_and_presence_read_from_the_shells_lists() {
    let locks = locks_of(&json!([
        {"region": "para:00000001", "holder": "m1", "name": "An", "colour": "#ff0000", "mine": false},
        {"region": "*", "holder": "m0", "name": "Me", "colour": "#00ff00", "mine": true},
        {"holder": "m3"}
    ]));
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].name, "An");
    let cursors =
        cursors_of(&json!([{"author": "m1", "name": "An", "colour": "#ff0000", "region": "para:00000001", "range": "00000001:2;00000001:5"}]));
    assert_eq!(cursors[0].range, "00000001:2;00000001:5");
    assert!(cursors_of(&json!(null)).is_empty());
    assert!(present(&json!([{"id": "m0", "you": true}])));
    assert!(!present(&json!([{"id": "m1"}])));
    let (mut app, page) = page();
    page.link.live_message(LiveMsg::Cursors(cursors));
    page.link.live_message(LiveMsg::Leader(true));
    app.host_tick(&page.ctx, 0.0);
    let live = app.session.live.as_ref().expect("co-editing");
    assert_eq!(live.cursors()[0].name, "An");
    assert!(live.is_leader());
}
