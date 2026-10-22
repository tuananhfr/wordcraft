//! Local recovery of a document the ERP page opened in the embedded editor (WORD-06): which
//! document it is, whether the host owns persistence, and which draft writes reach the embedded
//! store. The browser side (parent messages, IndexedDB) is `web/drafts/embedded.rs`; the contract
//! is in `packaging/web/README.md`.

use std::collections::VecDeque;

use sha2::{Digest as _, Sha256};

/// Largest recovery copy kept.
pub const MAX_DRAFT_BYTES: usize = 200 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Store key: SHA-256 of the parent origin and the host's document id, or of its file name.
    pub key: String,
    /// The host's file name.
    pub name: String,
    /// `drafts: "host"`: the host keeps the drafts, so nothing is stored or restored here.
    pub host_owned: bool,
}

impl Identity {
    pub fn new(origin: &str, name: &str, doc_id: Option<&str>, drafts: Option<&str>) -> Self {
        let doc_id = doc_id.filter(|s| !s.is_empty());
        // The kind tag keeps an id from colliding with a file name spelled the same.
        let (kind, discriminator) = match doc_id {
            Some(id) => ("id", id),
            None => ("name", name),
        };
        let digest = Sha256::digest(format!("{origin}\0{kind}\0{discriminator}"));
        let key = digest.iter().map(|b| format!("{b:02x}")).collect();
        Self { key, name: name.to_string(), host_owned: drafts == Some("host") }
    }
}

/// A recovery copy as stored, under the draft key the app gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDraft {
    pub key: String,
    pub name: String,
    pub format: String,
    pub bytes: Vec<u8>,
}

/// A store write, performed in order by the browser side.
#[derive(Debug, PartialEq, Eq)]
pub enum Job {
    Put { store_key: String, draft: StoredDraft },
    Delete { store_key: String },
}

impl Job {
    fn store_key(&self) -> &str {
        match self {
            Self::Put { store_key, .. } | Self::Delete { store_key } => store_key,
        }
    }
}

/// An `init` or `open` message from the parent, its origin and source already checked.
pub struct HostMessage<'a> {
    pub kind: &'a str,
    pub origin: &'a str,
    pub name: &'a str,
    pub doc_id: Option<&'a str>,
    pub drafts: Option<&'a str>,
}

#[derive(Default)]
pub struct Recovery {
    identity: Option<Identity>,
    init_pending: bool,
    generation: u64,
    loading: bool,
    loaded: bool,
    /// The one draft key stored for the identity: the restored copy's, or the first host document put.
    bound_key: Option<String>,
    jobs: VecDeque<Job>,
}

impl Recovery {
    /// Apply a parent message. True when the document changed: restores queued for the previous
    /// one must be dropped.
    pub fn host_message(&mut self, m: &HostMessage) -> bool {
        if !matches!(m.kind, "init" | "open") {
            return false;
        }
        let doc_id = m.doc_id.filter(|s| !s.is_empty());
        if m.name.is_empty() && doc_id.is_none() {
            return false;
        }
        // The shell's usual `open` (name and bytes only) following `init` is the same document.
        if m.kind == "open" && self.init_pending && doc_id.is_none() && m.drafts.is_none() {
            self.init_pending = false;
            return false;
        }
        self.init_pending = m.kind == "init";
        self.set_identity(Identity::new(m.origin, m.name, doc_id, m.drafts))
    }

    /// An older shell that sends no `init`: identify by the file it delivered.
    pub fn fallback(&mut self, origin: &str, name: &str) -> bool {
        if self.identity.is_some() || name.is_empty() {
            return false;
        }
        self.set_identity(Identity::new(origin, name, None, None))
    }

    fn set_identity(&mut self, identity: Identity) -> bool {
        if let Some(current) = &mut self.identity
            && current.key == identity.key
            && current.host_owned == identity.host_owned
        {
            current.name = identity.name;
            return false;
        }
        // Writes already queued carry their own store key and still complete.
        self.identity = Some(identity);
        self.generation = self.generation.wrapping_add(1);
        self.loading = false;
        self.loaded = false;
        self.bound_key = None;
        true
    }

    pub fn has_identity(&self) -> bool {
        self.identity.is_some()
    }

    fn local(&self) -> Option<&Identity> {
        self.identity.as_ref().filter(|id| !id.host_owned)
    }

    /// Local recovery is on and its stored copy has been looked up.
    pub fn ready(&self) -> bool {
        self.local().is_some() && self.loaded
    }

    /// Start looking up the stored copy: the generation to hand back and the store key.
    pub fn begin_load(&mut self) -> Option<(u64, String)> {
        if self.loaded || self.loading {
            return None;
        }
        let key = self.local()?.key.clone();
        self.loading = true;
        Some((self.generation, key))
    }

    /// The lookup's result (`None` also when it failed): the copy to restore, if any.
    pub fn finish_load(&mut self, generation: u64, found: Option<StoredDraft>) -> Option<StoredDraft> {
        if generation != self.generation || !self.loading {
            return None;
        }
        self.loading = false;
        self.loaded = true;
        let draft = found.filter(|d| d.bytes.len() <= MAX_DRAFT_BYTES)?;
        self.bound_key = Some(draft.key.clone());
        Some(draft)
    }

    /// A draft the app puts. Only the host's document is stored; an error while the stored copy is
    /// still being looked up makes the app retry.
    pub fn put(&mut self, key: &str, name: &str, format: &str, bytes: &[u8]) -> Result<(), String> {
        let Some(identity) = self.local() else { return Ok(()) };
        if !self.loaded {
            return Err("embedded recovery is still loading".into());
        }
        let host_document = match &self.bound_key {
            Some(bound) => bound == key,
            None => same_stem(&identity.name, name),
        };
        if !host_document {
            return Ok(());
        }
        if bytes.len() > MAX_DRAFT_BYTES {
            return Err("the document is too large for embedded recovery".into());
        }
        let store_key = identity.key.clone();
        self.bound_key = Some(key.to_string());
        self.jobs.retain(|job| job.store_key() != store_key);
        let draft = StoredDraft { key: key.into(), name: name.into(), format: format.into(), bytes: bytes.to_vec() };
        self.jobs.push_back(Job::Put { store_key, draft });
        Ok(())
    }

    /// The app no longer needs draft `key` (saved, closed or discarded).
    pub fn remove(&mut self, key: &str) {
        if self.bound_key.as_deref() != Some(key) {
            return;
        }
        self.bound_key = None;
        let Some(store_key) = self.local().map(|id| id.key.clone()) else { return };
        self.jobs.retain(|job| job.store_key() != store_key);
        self.jobs.push_back(Job::Delete { store_key });
    }

    pub fn next_job(&mut self) -> Option<Job> {
        self.jobs.pop_front()
    }
}

/// File names naming the same document: case aside (as the shell's save capture compares them),
/// and the extension aside (a `.doc` the host sent is drafted as `.docx`). An unnamed identity
/// (`init` with only a `docId`) takes the first document drafted.
fn same_stem(host: &str, name: &str) -> bool {
    let stem = |s: &str| s.rsplit_once('.').map_or(s, |(stem, _)| stem).to_lowercase();
    host.is_empty() || stem(host) == stem(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const ERP: &str = "https://erp.example";

    /// The browser's IndexedDB store across reloads.
    type Disk = BTreeMap<String, StoredDraft>;

    fn init<'a>(doc_id: Option<&'a str>, name: &'a str, drafts: Option<&'a str>) -> HostMessage<'a> {
        HostMessage { kind: "init", origin: ERP, name, doc_id, drafts }
    }

    fn drain(r: &mut Recovery, disk: &mut Disk) {
        while let Some(job) = r.next_job() {
            match job {
                Job::Put { store_key, draft } => disk.insert(store_key, draft),
                Job::Delete { store_key } => disk.remove(&store_key),
            };
        }
    }

    /// A page load: the parent's messages, then the stored copy's lookup.
    fn page(disk: &Disk, messages: &[HostMessage]) -> (Recovery, Option<StoredDraft>) {
        let mut r = Recovery::default();
        for m in messages {
            r.host_message(m);
        }
        let restored = r.begin_load().and_then(|(generation, key)| r.finish_load(generation, disk.get(&key).cloned()));
        (r, restored)
    }

    #[test]
    fn embedded_edit_reload_restores_the_same_document_id() {
        let mut disk = Disk::new();
        let (mut first, restored) = page(&disk, &[init(Some("doc-1"), "Hợp đồng.docx", None)]);
        assert!(restored.is_none() && first.ready());
        first.put("draft-1", "Hợp đồng.docx", "docx", b"edited").unwrap();
        drain(&mut first, &mut disk);
        // Reload: the host renamed the file but keeps its id; the shell's own `open` follows.
        let open = HostMessage { kind: "open", origin: ERP, name: "Hợp đồng (bản 2).docx", doc_id: None, drafts: None };
        let (mut second, restored) = page(&disk, &[init(Some("doc-1"), "Hợp đồng (bản 2).docx", Some("local")), open]);
        let restored = restored.unwrap();
        assert_eq!(
            (restored.key.as_str(), restored.name.as_str(), restored.format.as_str(), restored.bytes.as_slice()),
            ("draft-1", "Hợp đồng.docx", "docx", &b"edited"[..])
        );
        // The restored copy keeps its record; saving it removes the record.
        second.put("draft-1", "Hợp đồng.docx", "docx", b"edited more").unwrap();
        drain(&mut second, &mut disk);
        assert_eq!(disk.values().next().unwrap().bytes, b"edited more");
        second.remove("draft-1");
        drain(&mut second, &mut disk);
        assert!(disk.is_empty());
    }

    #[test]
    fn a_different_document_id_or_parent_does_not_restore() {
        let mut disk = Disk::new();
        let (mut first, _) = page(&disk, &[init(Some("doc-1"), "a.docx", None)]);
        first.put("draft-1", "a.docx", "docx", b"edited").unwrap();
        drain(&mut first, &mut disk);
        assert!(page(&disk, &[init(Some("doc-2"), "a.docx", None)]).1.is_none());
        let other = HostMessage { kind: "init", origin: "https://other.example", name: "a.docx", doc_id: Some("doc-1"), drafts: None };
        assert!(page(&disk, &[other]).1.is_none());
        let key = Identity::new(ERP, "a.docx", Some("doc-1"), None).key;
        assert!(key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit()), "{key}");
        // Without an id the file name identifies it: an id-less page of the same name is another key.
        assert!(page(&disk, &[init(None, "a.docx", None)]).1.is_none());
    }

    #[test]
    fn without_init_the_delivered_file_name_identifies_the_document() {
        let mut disk = Disk::new();
        let mut first = Recovery::default();
        assert!(!first.ready() && first.begin_load().is_none());
        assert!(first.fallback(ERP, "Báo cáo.docx") && first.has_identity());
        assert!(!first.fallback(ERP, "Khác.docx"), "an identity is never replaced by the fallback");
        let (generation, _) = first.begin_load().unwrap();
        assert!(first.finish_load(generation, None).is_none());
        first.put("draft-1", "Báo cáo.docx", "docx", b"edited").unwrap();
        drain(&mut first, &mut disk);
        let mut reload = Recovery::default();
        reload.fallback(ERP, "BÁO CÁO.docx");
        let (generation, key) = reload.begin_load().unwrap();
        assert!(reload.finish_load(generation, disk.get(&key).cloned()).is_none(), "the key is the exact name");
        let mut reload = Recovery::default();
        reload.fallback(ERP, "Báo cáo.docx");
        let (generation, key) = reload.begin_load().unwrap();
        assert_eq!(reload.finish_load(generation, disk.get(&key).cloned()).unwrap().bytes, b"edited");
    }

    #[test]
    fn host_owned_flag_disables_local_drafts() {
        let mut disk = Disk::new();
        let (mut first, _) = page(&disk, &[init(Some("doc-1"), "a.docx", None)]);
        first.put("draft-1", "a.docx", "docx", b"edited").unwrap();
        drain(&mut first, &mut disk);
        let (mut owned, restored) = page(&disk, &[init(Some("doc-1"), "a.docx", Some("host"))]);
        assert!(restored.is_none() && !owned.ready());
        owned.put("draft-2", "a.docx", "docx", b"other").unwrap();
        owned.remove("draft-1");
        assert!(owned.next_job().is_none());
        assert_eq!(disk.values().next().unwrap().bytes, b"edited", "the host-owned page leaves the store alone");
    }

    #[test]
    fn only_the_host_document_is_stored_and_puts_wait_for_the_lookup() {
        let disk = Disk::new();
        let mut r = Recovery::default();
        r.host_message(&init(Some("doc-1"), "HopDong.doc", None));
        let (generation, _) = r.begin_load().unwrap();
        assert!(r.put("draft-1", "HopDong.docx", "docx", b"x").is_err(), "retried after the lookup");
        r.finish_load(generation, disk.get("missing").cloned());
        r.put("other", "Notes.docx", "docx", b"x").unwrap();
        assert!(r.next_job().is_none(), "another tab is not the host's document");
        r.put("draft-1", "hopdong.docx", "docx", b"1").unwrap();
        r.put("draft-1", "hopdong.docx", "docx", b"2").unwrap();
        assert!(matches!(r.next_job(), Some(Job::Put { draft, .. }) if draft.bytes == b"2"), "only the latest copy is written");
        assert!(r.next_job().is_none());
        // A late lookup of a document the page has left is ignored.
        assert!(r.host_message(&init(Some("doc-2"), "b.docx", None)));
        assert!(r.finish_load(generation, Some(StoredDraft { key: "k".into(), name: "a".into(), format: "docx".into(), bytes: vec![1] })).is_none());
    }
}
