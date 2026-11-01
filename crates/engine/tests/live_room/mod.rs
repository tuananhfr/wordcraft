//! The fake room of the region co-editing tests: sessions of one shared document and a room that
//! grants locks like the server (`*` clashes with everything, a region with itself, one ordered queue).

#![allow(dead_code)]

use serde_json::{Value, json};
use wordcraft_doc::{CharProps, Document, Paragraph, StoryRef, para_block};
use wordcraft_engine::Session;
use wordcraft_engine::io::{open_bytes, save_bytes};
use wordcraft_engine::live::payload::{MediaHashes, fingerprint};
use wordcraft_engine::live::{Call, Live, Lock, Region};

pub fn run(s: &mut Session, command: &str, params: Value) {
    s.run(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"));
}

pub fn doc(paras: &[&str]) -> Document {
    let mut d = Document::new();
    d.body = paras.iter().map(|t| para_block(Paragraph::with_text(t, CharProps::default()))).collect();
    // Every page opens the same bytes, so ids are the same everywhere.
    open_bytes("shared.docx", &save_bytes("shared.docx", &d).expect("save")).expect("open")
}

pub struct Room {
    pub pages: Vec<Session>,
    pub locks: Vec<(String, usize)>,
    pub clock: f64,
    /// Selections reported, per page.
    pub selections: Vec<Vec<(String, String)>>,
    /// Payloads sent, per region.
    pub sent: Vec<(String, String)>,
}

impl Room {
    pub fn new(n: usize, d: &Document) -> Room {
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

    pub fn page(&mut self, i: usize) -> &mut Session {
        &mut self.pages[i]
    }

    pub fn live(&mut self, i: usize) -> &mut Live {
        self.pages[i].live.as_mut().expect("co-editing")
    }

    /// Passes calls between the pages and the room until nothing moves.
    pub fn settle(&mut self) {
        assert!(self.try_settle(), "the room never settled");
    }

    pub fn try_settle(&mut self) -> bool {
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
    pub fn wait(&mut self, ms: f64) {
        self.settle();
        self.clock += ms;
        self.settle();
    }

    pub fn converged(&self) {
        let mut h = MediaHashes::default();
        let first = &self.pages[0].doc;
        let want = fingerprint(first, &first.body, &mut h);
        for (i, p) in self.pages.iter().enumerate().skip(1) {
            assert_eq!(fingerprint(&p.doc, &p.doc.body, &mut h), want, "page {i} differs: {:?} vs {:?}", texts(&p.doc), texts(first));
        }
    }
}

pub fn texts(d: &Document) -> Vec<String> {
    d.plain_text(StoryRef::Body).split('\n').map(str::to_string).collect()
}

pub fn caret(s: &mut Session, block: usize, off: usize) {
    run(s, "caret.set", json!({"pos": {"story": "body", "path": [block], "off": off}}));
}

pub fn type_at(s: &mut Session, block: usize, off: usize, text: &str) {
    caret(s, block, off);
    run(s, "text.insert", json!({"text": text}));
}

pub fn para_region(d: &Document, k: usize) -> String {
    Region::of(&d.body[k]).expect("id").id()
}
