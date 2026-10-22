//! Pure document draft state transitions for the shell bridge.
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub doc_id: String,
    pub name: String,
    pub revision: u64,
    pub dirty: bool,
    pub pending: bool,
    pub committed: bool,
    pub failed: bool,
}

#[derive(Default)]
pub struct States {
    documents: BTreeMap<String, Message>,
    active: Option<String>,
}

impl States {
    pub fn observe(&mut self, key: &str, name: &str, revision: u64, unsaved: bool) -> Vec<Message> {
        let revision = revision.min(9_007_199_254_740_991);
        let fresh = !self.documents.contains_key(key);
        let m = self.documents.entry(key.into()).or_insert_with(|| Message {
            doc_id: key.into(),
            name: name.into(),
            revision: 0,
            dirty: false,
            pending: false,
            committed: false,
            failed: false,
        });
        let edited = m.revision != revision;
        let renamed = m.name != name;
        m.name = name.into();
        let clean = !unsaved && (m.dirty || m.pending || m.committed || m.failed);
        if edited {
            m.revision = revision;
        }
        if edited || clean || fresh {
            m.dirty = unsaved;
            m.pending = false;
            m.committed = false;
            m.failed = false;
        }
        if edited || clean || fresh || renamed { self.messages(key) } else { Vec::new() }
    }

    pub fn switch(&mut self, key: Option<&str>) -> Vec<Message> {
        if self.active.as_deref() == key {
            return Vec::new();
        }
        self.active = key.map(str::to_owned);
        key.map_or_else(Vec::new, |key| self.messages(key))
    }

    pub fn checkpoint(&mut self, key: &str) -> Vec<Message> {
        let Some(m) = self.documents.get_mut(key) else { return Vec::new() };
        m.dirty = false;
        m.pending = true;
        m.committed = false;
        m.failed = false;
        self.messages(key)
    }

    pub fn revision(&self, key: &str) -> Option<u64> {
        self.documents.get(key).map(|m| m.revision)
    }

    pub fn complete(&mut self, key: &str, revision: u64, success: bool) -> Vec<Message> {
        let Some(m) = self.documents.get_mut(key) else { return Vec::new() };
        // A removed draft or a newer checkpoint owns its own outcome.
        if m.revision != revision || !m.pending {
            return self.messages(key);
        }
        m.pending = false;
        m.dirty = !success;
        m.committed = success;
        m.failed = !success;
        self.messages(key)
    }

    pub fn close(&mut self, key: &str) -> Vec<Message> {
        let Some(mut m) = self.documents.remove(key) else { return Vec::new() };
        m.dirty = false;
        m.pending = false;
        m.committed = false;
        m.failed = false;
        let mut messages = vec![m];
        if self.active.as_deref() == Some(key) {
            self.active = None;
        }
        if let Some(active) = self.active.as_ref().and_then(|key| self.documents.get(key)) {
            messages.push(active.clone());
        }
        messages
    }

    fn messages(&self, key: &str) -> Vec<Message> {
        let mut messages: Vec<_> = self.documents.get(key).cloned().into_iter().collect();
        if self.active.as_deref() != Some(key)
            && let Some(active) = self.active.as_ref().and_then(|key| self.documents.get(key))
        {
            messages.push(active.clone());
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_state_two_documents_and_close() {
        let mut s = States::default();
        s.observe("a", "A", 0, false);
        s.switch(Some("a"));
        s.observe("b", "B", 1, true);
        let pending = s.checkpoint("b");
        assert!(pending.first().is_some_and(|m| m.pending));
        assert_eq!(pending.last().map(|m| m.doc_id.as_str()), Some("a"));
        let complete = s.complete("b", 1, true);
        assert!(complete.first().is_some_and(|m| m.committed));
        assert_eq!(complete.last().map(|m| m.doc_id.as_str()), Some("a"));
        assert!(s.switch(Some("b")).first().is_some_and(|m| m.committed));
        let closed = s.close("a");
        assert!(closed.first().is_some_and(|m| !m.dirty && !m.pending && !m.committed && !m.failed));
        assert_eq!(closed.last().map(|m| m.doc_id.as_str()), Some("b"));
    }

    #[test]
    fn draft_state_stale_commit_and_clean() {
        let mut s = States::default();
        s.observe("adopted-id", "A", 1, true);
        s.checkpoint("adopted-id");
        s.observe("adopted-id", "A", 2, true);
        assert!(s.complete("adopted-id", 1, true).first().is_some_and(|m| m.revision == 2 && m.dirty && !m.committed));
        s.checkpoint("adopted-id");
        assert_eq!(s.revision("adopted-id"), Some(2));
        assert!(s.complete("adopted-id", 1, false).first().is_some_and(|m| m.pending && !m.failed));
        s.observe("adopted-id", "A", 2, false);
        assert!(s.complete("adopted-id", 2, true).first().is_some_and(|m| !m.committed && !m.pending && !m.dirty));
    }

    #[test]
    fn draft_state_failure_then_retry() {
        let mut s = States::default();
        s.observe("a", "A", 1, true);
        s.checkpoint("a");
        assert!(s.complete("a", 1, false).first().is_some_and(|m| m.failed && m.dirty));
        assert!(s.checkpoint("a").first().is_some_and(|m| m.pending && !m.failed));
        assert!(s.complete("a", 1, true).first().is_some_and(|m| m.committed && !m.failed));
    }
}
