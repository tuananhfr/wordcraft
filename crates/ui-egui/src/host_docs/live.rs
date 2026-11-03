//! Web (`erpHost` v3, a shell with `regionWillEdit`): region co-editing of the host document. The
//! shell's calls arrive as [`LiveMsg`]s between frames; each frame hands them to the shared
//! document's session ([`wordcraft_engine::live`]), ticks it and sends its calls back through
//! [`HostTransport::region_calls`]. A shell without v3 never sets [`HostLink::set_regions`] and
//! nothing here runs.

use std::rc::Rc;

use serde_json::Value;
use wordcraft_engine::Session;
use wordcraft_engine::live::{Cursor, Live, Lock};

use super::HostLink;
use crate::WordApp;

/// One of the shell's v3 calls, for the next frame.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveMsg {
    /// `regionWillEdit`'s promise resolved.
    Answer(String, bool),
    /// `applyRemote(region, payload, author)`.
    Apply(String, String),
    /// `showLocks(locks)`: the locks others hold.
    Locks(Vec<Lock>),
    Cursors(Vec<Cursor>),
    /// `showPresence(members)`: whether this page is among them.
    Presence(bool),
    /// `lockDenied(region)`.
    Denied(String),
    /// `isLeader(on)`.
    Leader(bool),
}

/// Answers a `regionConflict` question with `"mine"` or `"theirs"`.
pub type ConflictAnswer = Box<dyn FnOnce(&'static str)>;

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// `showLocks`' list: the locks others hold (ours, `mine: true`, are known from the answers).
pub fn locks_of(list: &Value) -> Vec<Lock> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|l| l.get("mine").and_then(Value::as_bool) != Some(true) && !text(l, "region").is_empty())
        .map(|l| Lock { region: text(l, "region"), holder: text(l, "holder"), name: text(l, "name"), colour: text(l, "colour") })
        .collect()
}

pub fn cursors_of(list: &Value) -> Vec<Cursor> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|c| Cursor {
            author: text(c, "author"),
            name: text(c, "name"),
            colour: text(c, "colour"),
            region: text(c, "region"),
            range: text(c, "range"),
        })
        .collect()
}

/// `showPresence`' members include this page.
pub fn present(members: &Value) -> bool {
    members.as_array().is_some_and(|m| m.iter().any(|x| x.get("you").and_then(Value::as_bool) == Some(true)))
}

impl HostLink {
    /// The shell takes region calls (`erpHost` v3, found by feature: its `version` stays 2).
    pub fn set_regions(&self) {
        self.shared.regions.set(true);
    }

    pub fn regions(&self) -> bool {
        self.shared.regions.get()
    }

    pub fn live_message(&self, message: LiveMsg) {
        if let Ok(mut inbox) = self.shared.live_inbox.try_borrow_mut() {
            inbox.push(message);
        }
        self.wake();
    }

    /// `regionConflict(region)`: asked in a window on the next frame.
    pub fn conflict(&self, region: String, answer: ConflictAnswer) {
        if let Ok(mut q) = self.shared.conflicts.try_borrow_mut() {
            q.push_back((region, answer));
        }
        self.wake();
    }
}

impl WordApp {
    /// The session whose document the shell shares, shown or in a background tab.
    pub(crate) fn live_session(&mut self) -> Option<&mut Session> {
        if self.session.live.is_some() {
            return Some(&mut self.session);
        }
        self.docs.parked_sessions_mut().find(|s| s.live.is_some())
    }

    /// The host document `document` is shared region by region from now on.
    pub(super) fn start_live(&mut self, link: &HostLink, document: u64) {
        if !link.regions() {
            return;
        }
        if let Some(s) = self.session_mut(document)
            && s.live.is_none()
        {
            s.live = Some(Live::new(&s.doc));
        }
    }

    /// Every frame: the shell's calls in, the session's calls out, feedback in the status bar.
    pub(super) fn host_live_tick(&mut self, ctx: &egui::Context, link: &HostLink, now: f64) {
        if !link.regions() {
            return;
        }
        let messages = link.shared.live_inbox.try_borrow_mut().map(|mut m| std::mem::take(&mut *m)).unwrap_or_default();
        let Some(session) = self.live_session() else { return };
        for message in messages {
            match message {
                LiveMsg::Answer(region, granted) => session.live_answer(&region, granted),
                LiveMsg::Apply(region, payload) => {
                    if let Err(e) = session.live_apply_remote(&region, &payload) {
                        log::warn!("erpHost.applyRemote: not applied: {e}");
                    }
                }
                LiveMsg::Denied(region) => session.live_lock_denied(&region),
                other => {
                    let Some(live) = session.live.as_mut() else { continue };
                    match other {
                        LiveMsg::Locks(locks) => live.set_locks(locks),
                        LiveMsg::Cursors(cursors) => live.set_cursors(cursors),
                        LiveMsg::Presence(online) => live.set_online(online),
                        LiveMsg::Leader(leader) => live.set_leader(leader),
                        _ => {}
                    }
                }
            }
        }
        session.live_tick(now);
        let Some(live) = session.live.as_mut() else { return };
        let calls = live.take_calls();
        let feedback = live.take_feedback();
        if !calls.is_empty() {
            let back = link.clone();
            link.transport.region_calls(calls, Rc::new(move |region, granted| back.live_message(LiveMsg::Answer(region, granted))));
        }
        if let Some(f) = feedback {
            let text = f.fill(tl!(f.text));
            self.status(text);
        }
        // Regions are handed back on a timer, so frames keep coming while nothing happens.
        ctx.request_repaint_after(std::time::Duration::from_millis(1_000));
        self.ask_conflict(ctx, link);
    }

    /// The oldest `regionConflict` question, until the user answers it.
    fn ask_conflict(&mut self, ctx: &egui::Context, link: &HostLink) {
        let Some(region) = link.shared.conflicts.try_borrow().ok().and_then(|q| q.front().map(|(r, _)| r.clone())) else { return };
        let answer = super::conflict_dialog::show(ctx, &region);
        if let Some(answer) = answer
            && let Some((_, done)) = link.shared.conflicts.try_borrow_mut().ok().and_then(|mut q| q.pop_front())
        {
            done(answer);
            ctx.request_repaint();
        }
    }
}
