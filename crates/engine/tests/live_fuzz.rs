//! Fuzzing what other members send: honest region payloads and comment anchors, randomly mutated
//! (values swapped for huge, negative or odd ones, keys dropped, text cut or garbled, DOCX bytes
//! flipped) and given to `live_apply_remote` / `place_comment_anchor` under real and odd region
//! ids. Nothing may panic; a refused payload leaves the document as it was; an applied one leaves
//! a document whose paragraph ids are valid and unique and that still saves.
//!
//! Knobs: `WORDCRAFT_LIVE_FUZZ_CASES` (default 1500) and `WORDCRAFT_LIVE_FUZZ_SEED` (replay one seed).

mod live_room;

use std::collections::HashSet;

use live_room::*;
use serde_json::{Value as Json, json};
use wordcraft_doc::ids::{MAX_PARA_ID, block_id};
use wordcraft_doc::{Block, Document};
use wordcraft_engine::Session;
use wordcraft_engine::io::save_bytes;
use wordcraft_engine::live::Call;
use wordcraft_engine::live::anchor::Anchor;
use wordcraft_engine::live::payload::encode_doc;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a, T>(&mut self, list: &'a [T]) -> &'a T {
        &list[self.below(list.len())]
    }
}

fn odd_value(rng: &mut Rng) -> Json {
    match rng.below(15) {
        0 => Json::Null,
        1 => json!(u32::MAX),
        2 => json!(u64::MAX),
        3 => json!(-1),
        4 => json!(1e-30),
        5 => json!(1e300),
        6 => json!(""),
        7 => json!("FFFFFFFF"),
        8 => json!([]),
        9 => json!({}),
        10 => json!({"type": "commentStart", "id": 0}),
        11 => json!([[[[[[[[1]]]]]]]]),
        12 => json!("x".repeat(rng.below(5000))),
        13 => json!(true),
        _ => json!(rng.next() % 2_000_000),
    }
}

/// One random change somewhere in `v`.
fn mutate(rng: &mut Rng, v: &mut Json, depth: usize) {
    let descend = depth < 12 && rng.below(4) != 0;
    match v {
        Json::Object(m) if !m.is_empty() => {
            let keys: Vec<String> = m.keys().cloned().collect();
            let k = rng.pick(&keys).clone();
            match rng.below(6) {
                0 => {
                    m.remove(&k);
                }
                1 => {
                    m.insert(k, odd_value(rng));
                }
                _ if descend => mutate(rng, m.get_mut(&k).unwrap_or(&mut Json::Null), depth + 1),
                _ => {
                    m.insert(k, odd_value(rng));
                }
            }
        }
        Json::Array(list) if !list.is_empty() => {
            let i = rng.below(list.len());
            match rng.below(6) {
                0 => {
                    list.remove(i);
                }
                1 => {
                    let copy = list[i].clone();
                    list.extend(std::iter::repeat_n(copy, rng.below(64)));
                }
                _ if descend => mutate(rng, &mut list[i], depth + 1),
                _ => list[i] = odd_value(rng),
            }
        }
        _ => *v = odd_value(rng),
    }
}

fn numbers(v: &Json) -> usize {
    match v {
        Json::Number(_) => 1,
        Json::Array(list) => list.iter().map(numbers).sum(),
        Json::Object(m) => m.values().map(numbers).sum(),
        _ => 0,
    }
}

/// Swaps the `k`-th number in `v` for an odd one: paragraph ids, offsets, revision, part and
/// comment indexes are where a run hurts the document. Returns how many numbers are still to skip.
fn swap_number(rng: &mut Rng, v: &mut Json, k: usize) -> usize {
    match v {
        Json::Number(_) if k == 0 => {
            *v = rng.pick(&[json!(u32::MAX), json!(0x8000_0000u32), json!(0), json!(-1), json!(1e-30), json!(1e30), json!(1_000_000), json!(1)]).clone();
            usize::MAX
        }
        Json::Number(_) => k - 1,
        Json::Array(list) => {
            list.iter_mut().try_fold(k, |k, x| if k == usize::MAX { None } else { Some(swap_number(rng, x, k)) }).unwrap_or(usize::MAX)
        }
        Json::Object(m) => m.values_mut().try_fold(k, |k, x| if k == usize::MAX { None } else { Some(swap_number(rng, x, k)) }).unwrap_or(usize::MAX),
        _ => k,
    }
}

/// The payload text, sometimes garbled as text rather than as JSON.
fn garble(rng: &mut Rng, text: String) -> String {
    let chars: Vec<char> = text.chars().collect();
    match rng.below(8) {
        0 => chars[..rng.below(chars.len())].iter().collect(),
        1 => {
            let mut c = chars;
            let i = rng.below(c.len());
            c.insert(i, *rng.pick(&['"', '{', ']', '\\', '\u{0}', 'é', '9']));
            c.into_iter().collect()
        }
        _ => text,
    }
}

/// Honest payloads of page A: (region, payload).
fn corpus(room: &mut Room) -> Vec<(String, String)> {
    let steps: [(&str, Json); 9] = [
        ("caret.set", json!({"pos": {"story": "body", "path": [0], "off": 2}})),
        ("text.insert", json!({"text": "typed "})),
        ("text.newParagraph", json!({})),
        ("select.range", json!({"anchor": {"block": 2, "off": 0}, "focus": {"block": 2, "off": 4}})),
        ("review.newComment", json!({"text": "a comment"})),
        ("format.bold", json!({})),
        ("insert.table", json!({"rows": 2, "cols": 2})),
        ("insert.picture", json!({"data": PNG, "width": 20})),
        ("layout.orientation", json!({"value": "landscape"})),
    ];
    let mut out = Vec::new();
    for (command, params) in steps {
        run(room.page(0), command, params);
        for _ in 0..10 {
            for c in room.live(0).take_calls() {
                match c {
                    Call::WillEdit(r) => room.page(0).live_answer(&r, true),
                    Call::Changed(r, p) => out.push((r, p)),
                    _ => {}
                }
            }
        }
    }
    out
}

fn snapshot(s: &Session) -> String {
    serde_json::to_string(&s.doc).unwrap()
}

/// Paragraph and row ids stay unique and within `w14:paraId`, and the document still saves.
fn sound(d: &Document) {
    fn ids(b: &Block, out: &mut Vec<Option<u32>>) {
        match b {
            Block::Para(p) => out.push(p.id),
            Block::Table(t) => {
                for r in &t.rows {
                    out.push(r.id);
                    for cb in r.cells.iter().flat_map(|c| c.blocks.iter()) {
                        ids(cb, out);
                    }
                }
            }
        }
    }
    let mut all = Vec::new();
    for b in &d.body {
        assert!(block_id(b).is_some(), "a top-level block without an id");
        ids(b, &mut all);
    }
    let mut seen = HashSet::new();
    for id in all.into_iter().flatten() {
        assert!((1..=MAX_PARA_ID).contains(&id) && seen.insert(id), "id {id:08X}");
    }
    save_bytes("fuzz.docx", d).expect("an applied payload leaves a document that saves");
}

fn regions(rng: &mut Rng, honest: &str, d: &Document) -> String {
    let other = d.body.get(rng.below(d.body.len())).and_then(|b| block_id(b)).map(|id| format!("para:{id:08X}")).unwrap_or_default();
    let odd = [other.as_str(), "para:0000abcd", "para:ABCD", "para:00000000", "para:FFFFFFFF", "table:00000001", "*", "", "cell:f1:A1", "para:"];
    if rng.below(3) == 0 { rng.pick(&odd).to_string() } else { honest.to_string() }
}

fn seed_count() -> (u64, u64) {
    if let Some(seed) = std::env::var("WORDCRAFT_LIVE_FUZZ_SEED").ok().and_then(|s| s.parse().ok()) {
        return (seed, seed + 1);
    }
    let cases = std::env::var("WORDCRAFT_LIVE_FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(1500);
    (1, 1 + cases)
}

#[test]
fn mutated_payloads_never_panic_and_refusals_change_nothing() {
    let mut room = Room::new(2, &doc(&["One", "Two words", "Three more words"]));
    let docx = save_bytes("a.docx", &room.pages[0].doc).unwrap();
    let honest = corpus(&mut room);
    assert!(honest.iter().any(|(r, _)| r.starts_with("para:")) && honest.iter().any(|(r, _)| r == "*"), "{honest:?}");
    let (from, to) = seed_count();
    let mut applied = 0;
    for seed in from..to {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (region, text) = if rng.below(10) == 0 {
            // A whole-document payload whose DOCX bytes are damaged.
            let mut bytes = docx.clone();
            for _ in 0..1 + rng.below(8) {
                let i = rng.below(bytes.len());
                bytes[i] = rng.next() as u8;
            }
            ("*".to_string(), encode_doc(&bytes))
        } else {
            let (region, payload) = rng.pick(&honest).clone();
            let mut v: Json = serde_json::from_str(&payload).unwrap();
            for _ in 0..1 + rng.below(4) {
                if rng.below(2) == 0 {
                    let k = rng.below(numbers(&v));
                    swap_number(&mut rng, &mut v, k);
                } else {
                    mutate(&mut rng, &mut v, 0);
                }
            }
            (regions(&mut rng, &region, &room.pages[1].doc), garble(&mut rng, v.to_string()))
        };
        let b = room.page(1);
        let before = snapshot(b);
        match b.live_apply_remote(&region, &text) {
            Err(_) => assert_eq!(snapshot(b), before, "seed {seed}: refused, yet changed ({region})"),
            Ok(()) => {
                applied += 1;
                sound(&b.doc);
            }
        }
    }
    eprintln!("live fuzz: {applied} of {} mutated payloads applied", to - from);
    assert!(to - from < 50 || applied > 0, "no mutated payload was ever applied: the fuzzer only hits the parser");
}

#[test]
fn mutated_anchors_never_panic_and_change_nothing() {
    let mut room = Room::new(2, &doc(&["Báo cáo năm nay", "Hai"]));
    run(room.page(0), "select.range", json!({"anchor": {"block": 0, "off": 5}, "focus": {"block": 0, "off": 9}}));
    let honest = room.pages[0].comment_anchor().expect("an anchor").to_json();
    let (from, to) = seed_count();
    for seed in from..to {
        let mut rng = Rng(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93) | 1);
        let mut v = honest.clone();
        for _ in 0..1 + rng.below(4) {
            if rng.below(2) == 0 {
                let k = rng.below(numbers(&v));
                swap_number(&mut rng, &mut v, k);
            } else {
                mutate(&mut rng, &mut v, 0);
            }
        }
        let s = &room.pages[1];
        let before = snapshot(s);
        if let Some(anchor) = Anchor::parse(&v) {
            let _ = s.place_comment_anchor(&anchor);
        }
        assert_eq!(snapshot(s), before, "seed {seed}");
    }
}
