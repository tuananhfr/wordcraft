//! Region co-editing (ERPCons Office online, `erpHost` v3): every edit of the shared document
//! names the regions it touched ([`Region`]: top-level paragraphs and tables, or `*`), runs only
//! once their locks are ours, and goes to the other editors as payloads holding each region's
//! whole content; theirs are applied here. Undo takes back only this user's steps. A [`Session`]
//! without [`Session::live`] (the desktop app, a shell without v3) never gets here.
//!
//! The host bridge drains [`Live::take_calls`] into the shell and feeds its answers back through
//! the `live_*` methods of [`Session`].

pub mod diff;
pub mod payload;
pub mod region;
mod session;
mod undo;
mod walk;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::Value as Json;
use wordcraft_doc::Document;

pub use region::Region;
pub use session::{range_positions, selection_range};

use crate::Selection;
use payload::MediaHashes;

/// A call the fork makes on the shell.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    /// `regionWillEdit(region)`; answer with [`crate::Session::live_answer`].
    WillEdit(String),
    /// `regionChanged(region, payload)`.
    Changed(String, String),
    /// `regionDidEndEdit(region)`.
    DidEndEdit(String),
    /// `selectionChanged(region, range)`.
    Selection(String, String),
}

/// A lock another member holds (`showLocks`).
#[derive(Clone, Debug, PartialEq)]
pub struct Lock {
    pub region: String,
    pub holder: String,
    pub name: String,
    pub colour: String,
}

/// Another member's selection (`showCursors`); `range` is [`session::selection_range`]'s form.
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    pub author: String,
    pub name: String,
    pub colour: String,
    pub region: String,
    pub range: String,
}

/// Something to tell the user (English; the front end translates `text`), with `{name}`
/// standing for `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct Feedback {
    pub text: &'static str,
    pub name: String,
}

impl Feedback {
    fn plain(text: &'static str) -> Feedback {
        Feedback { text, name: String::new() }
    }
    /// `text` (already translated) with the name put in.
    pub fn fill(&self, text: &str) -> String {
        text.replace("{name}", if self.name.is_empty() { "?" } else { &self.name })
    }
}

pub const LOCKED_BY: &str = "Someone else is editing this part: {name}.";
pub const LOCK_DENIED: &str = "Someone else is editing this part; your change was not made.";
pub const NOTHING_TO_UNDO: &str = "Nothing of yours left to undo.";

/// An edit touching more regions than this takes `*` instead (the server holds at most 64 locks
/// per member, and asking for dozens one by one is slower than one whole-document lock).
const MAX_REGIONS: usize = 24;
/// A held region is handed back once the caret has been elsewhere this long, or nobody edited
/// it for `IDLE_MS`; `*` soon after its edit.
const LEAVE_MS: f64 = 3_000.0;
const IDLE_MS: f64 = 15_000.0;
const DOC_IDLE_MS: f64 = 2_000.0;
const STEPS_KEPT: usize = 100;

/// What waits for a lock: run again once every lock asked for is granted.
#[derive(Clone, Debug)]
enum Action {
    Command(String, Json),
    Undo(bool),
}

/// A region whose lock is ours.
#[derive(Clone, Debug, Default)]
struct Held {
    /// Blocks this client made while holding it, which travel in its runs (nobody else knows
    /// them, so they need no lock of their own until it is handed back).
    followers: Vec<u32>,
    /// Every block a run of this lock sent: the next run replaces them too, since the shell keeps
    /// only a region's newest unsent payload.
    sent: BTreeSet<u32>,
    /// Picture hashes sent with their bytes under this lock (the room has them once it ends).
    media: HashSet<String>,
    used: f64,
    /// Taken while not in the room: the server never granted it.
    offline: bool,
}

/// The co-editing state of the shared document ([`crate::Session::live`]).
pub struct Live {
    calls: Vec<Call>,
    held: BTreeMap<Region, Held>,
    /// Followers' regions, by block id.
    owner: HashMap<u32, Region>,
    /// `regionWillEdit` asked and not answered yet.
    asked: BTreeSet<Region>,
    /// Edits waiting for those answers, in order, and the selection the first one started from.
    queue: Vec<Action>,
    queue_sel: Option<Selection>,
    others: Vec<Lock>,
    cursors: Vec<Cursor>,
    feedback: Option<Feedback>,
    /// Picture hashes everyone in the room has.
    known_media: HashSet<String>,
    hashes: MediaHashes,
    steps: Vec<undo::Step>,
    redos: Vec<undo::Step>,
    reported: Option<(String, String)>,
    /// The selection the last typing step left, which the next keystroke joins if typed there.
    typing_at: Option<Selection>,
    online: bool,
    leader: bool,
    now: f64,
}

impl Live {
    /// Starts co-editing `doc` as it was opened (everyone in the room opened the same bytes).
    pub fn new(doc: &Document) -> Live {
        let mut live = Live {
            calls: Vec::new(),
            held: BTreeMap::new(),
            owner: HashMap::new(),
            asked: BTreeSet::new(),
            queue: Vec::new(),
            queue_sel: None,
            others: Vec::new(),
            cursors: Vec::new(),
            feedback: None,
            known_media: HashSet::new(),
            hashes: MediaHashes::default(),
            steps: Vec::new(),
            redos: Vec::new(),
            reported: None,
            typing_at: None,
            online: false,
            leader: false,
            now: 0.0,
        };
        live.learn_media(doc);
        live
    }

    /// The document was replaced (a reload): what was held, asked or undoable belongs to the old
    /// one.
    pub fn reset(&mut self, doc: &Document) {
        for r in std::mem::take(&mut self.held).into_keys() {
            self.calls.push(Call::DidEndEdit(r.id()));
        }
        self.owner.clear();
        self.asked.clear();
        self.queue.clear();
        self.queue_sel = None;
        self.steps.clear();
        self.redos.clear();
        self.reported = None;
        self.typing_at = None;
        self.known_media.clear();
        self.learn_media(doc);
    }

    fn learn_media(&mut self, doc: &Document) {
        for (k, v) in &doc.media {
            let h = self.hashes.of(k, v);
            self.known_media.insert(h);
        }
    }

    pub fn take_calls(&mut self) -> Vec<Call> {
        std::mem::take(&mut self.calls)
    }

    pub fn take_feedback(&mut self) -> Option<Feedback> {
        self.feedback.take()
    }

    /// `showPresence`: whether this page is in the room. Locks taken while out of it were never
    /// the server's, so they go when it comes back.
    pub fn set_online(&mut self, online: bool) {
        if online && !self.online {
            let gone: Vec<Region> = self.held.iter().filter(|(_, h)| h.offline).map(|(r, _)| *r).collect();
            for r in gone {
                self.drop_held(r);
            }
        }
        self.online = online;
    }

    pub fn online(&self) -> bool {
        self.online
    }

    pub fn set_leader(&mut self, leader: bool) {
        self.leader = leader;
    }

    pub fn is_leader(&self) -> bool {
        self.leader
    }

    /// `showLocks`: the locks others hold (ours are known from the answers).
    pub fn set_locks(&mut self, others: Vec<Lock>) {
        self.others = others;
    }

    pub fn others(&self) -> &[Lock] {
        &self.others
    }

    pub fn set_cursors(&mut self, cursors: Vec<Cursor>) {
        self.cursors = cursors;
    }

    pub fn cursors(&self) -> &[Cursor] {
        &self.cursors
    }

    /// Whether an edit of `r` can go ahead now.
    pub fn holds(&self, r: Region) -> bool {
        !self.online || self.held.contains_key(&r) || self.held.contains_key(&Region::Doc)
    }

    /// Who holds a lock clashing with `r`, if someone else does.
    pub fn blocker(&self, r: Region) -> Option<&Lock> {
        self.others.iter().find(|l| Region::parse(&l.region).is_some_and(|x| x.clashes(&r)))
    }

    /// The region a top-level block belongs to here: its own, or the held region it follows.
    pub fn region_of(&self, b: &wordcraft_doc::Block) -> Option<Region> {
        wordcraft_doc::ids::block_id(b).and_then(|id| self.owner.get(&id).copied()).or_else(|| Region::of(b))
    }

    fn drop_held(&mut self, r: Region) -> Option<Held> {
        let h = self.held.remove(&r)?;
        for f in &h.followers {
            self.owner.remove(f);
        }
        Some(h)
    }

    /// Hands `r` back to the room.
    fn release(&mut self, r: Region) {
        if let Some(h) = self.drop_held(r) {
            self.known_media.extend(h.media);
            if !h.offline {
                self.calls.push(Call::DidEndEdit(r.id()));
            }
        }
    }

    fn record(&mut self, step: undo::Step) {
        self.steps.push(step);
        if self.steps.len() > STEPS_KEPT {
            self.steps.remove(0);
        }
    }
}
