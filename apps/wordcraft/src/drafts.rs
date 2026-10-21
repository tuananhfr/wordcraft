//! Desktop recovery packages, atomically replaced beside the settings file.
use std::cell::RefCell;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use wordcraft_ui_egui::draft_bridge::{DraftBridge, HiddenFlush, RestoredDraft};
use wordcraft_ui_egui::i18n::t;

const MAX_COUNT: usize = wordcraft_ui_egui::open_documents::MAX_DOCUMENTS;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 256 * 1024 * 1024;
const MAX_HEADER: usize = 4096;

struct Store {
    dir: PathBuf,
    _lock: std::fs::File,
}

impl Store {
    fn create(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_millis();
        for _ in 0..5 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|e| e.to_string())?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let id = format!("{}-{stamp}-{suffix}", std::process::id());
            let lock_path = root.join(format!("{id}.lock"));
            let lock = match std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&lock_path) {
                Ok(lock) => lock,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            };
            if lock.try_lock().is_err() {
                continue;
            }
            let path = root.join(id);
            if let Err(e) = std::fs::create_dir(&path) {
                drop(lock);
                let _ = std::fs::remove_file(lock_path);
                return Err(e.to_string());
            }
            return Ok(Self { dir: path, _lock: lock });
        }
        Err(t("Invalid recovery draft key.").into())
    }

    fn recover(&self, root: &Path) -> Result<(Vec<RestoredDraft>, Vec<String>), String> {
        let mut restores = Vec::new();
        let mut errors = Vec::new();
        let mut total = 0u64;
        for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let lock_path = entry.path();
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() || lock_path.extension().is_none_or(|e| e != "lock") {
                continue;
            }
            let Some(id) = lock_path.file_stem().and_then(|s| s.to_str()) else { continue };
            if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
                continue;
            }
            let path = root.join(id);
            if path == self.dir {
                continue;
            }
            let Ok(lock) = std::fs::OpenOptions::new().read(true).write(true).open(&lock_path) else { continue };
            if lock.try_lock().is_err() {
                continue;
            }
            match path.try_exists() {
                Ok(false) => {
                    // A new owner may not have created its directory yet.
                    let old = lock
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|time| time.elapsed().ok())
                        .is_some_and(|age| age > std::time::Duration::from_secs(60));
                    drop(lock);
                    if old {
                        let _ = std::fs::remove_file(&lock_path);
                    }
                    continue;
                }
                Err(e) => {
                    errors.push(format!("{}: {e}", path.display()));
                    continue;
                }
                Ok(true) => {}
            }
            let dead = Self { dir: path.clone(), _lock: lock };
            let result = (|| -> Result<(), String> {
                for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if entry.file_name().to_string_lossy().starts_with(".draft-write-") && entry.file_type().map_err(|e| e.to_string())?.is_file() {
                        std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
                    }
                }
                for (source, size) in dead.entries()? {
                    total = total.saturating_add(size);
                    let result = (|| -> Result<RestoredDraft, String> {
                        if total > MAX_TOTAL {
                            return Err(t("Recovery storage exceeds its size limit.").into());
                        }
                        let mut draft = dead.read(&source)?;
                        let source_key = draft.key;
                        let mut random = [0u8; 16];
                        getrandom::fill(&mut random).map_err(|e| e.to_string())?;
                        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
                        draft.key = format!("desktop-{suffix}");
                        self.put(&draft.key, &draft.name, &draft.format, &draft.bytes)?;
                        dead.remove(&source_key)?;
                        Ok(draft)
                    })();
                    match result {
                        Ok(draft) => restores.push(draft),
                        Err(e) => errors.push(format!("{}: {e}", source.display())),
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                errors.push(format!("{}: {e}", path.display()));
            }
            let gone = std::fs::remove_dir(&path).is_ok();
            drop(dead);
            if gone {
                let _ = std::fs::remove_file(lock_path);
            }
        }
        Ok((restores, errors))
    }
}

impl Store {
    fn path(&self, key: &str) -> Result<PathBuf, String> {
        if key.is_empty() || key.len() > 200 || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(t("Invalid recovery draft key.").into());
        }
        Ok(self.dir.join(format!("{key}.draft")))
    }

    fn entries(&self) -> Result<Vec<(PathBuf, u64)>, String> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&self.dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().extension().is_some_and(|e| e == "draft") {
                let meta = entry.metadata().map_err(|e| e.to_string())?;
                if !meta.is_file() || entries.len() >= MAX_COUNT {
                    return Err(t("Recovery storage exceeds its document limit.").into());
                }
                entries.push((entry.path(), meta.len()));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    fn put(&self, key: &str, name: &str, format: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.path(key)?;
        if name.len() > MAX_HEADER || format != wordcraft_ui_egui::draft_bridge::FORMAT {
            return Err(t("Invalid recovery draft metadata.").into());
        }
        let header = serde_json::to_vec(&serde_json::json!({"name":name,"format":format})).map_err(|e| e.to_string())?;
        let size = (header.len() as u64).saturating_add(1).saturating_add(bytes.len() as u64);
        if header.len() > MAX_HEADER || size > MAX_BYTES {
            return Err(t("The recovery draft exceeds its size limit.").into());
        }
        let entries = self.entries()?;
        let others: Vec<_> = entries.iter().filter(|(p, _)| p != &path).collect();
        let total = others.iter().fold(size, |n, (_, size)| n.saturating_add(*size));
        if others.len() >= MAX_COUNT || total > MAX_TOTAL {
            return Err(t("Recovery storage is full.").into());
        }
        let mut temp = tempfile::Builder::new().prefix(".draft-write-").tempfile_in(&self.dir).map_err(|e| e.to_string())?;
        temp.write_all(&header).and_then(|_| temp.write_all(b"\n")).and_then(|_| temp.write_all(bytes)).map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        self.sync()
    }

    fn sync(&self) -> Result<(), String> {
        #[cfg(unix)]
        std::fs::File::open(&self.dir).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<(), String> {
        match std::fs::remove_file(self.path(key)?) {
            Ok(()) => self.sync(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn read(&self, path: &Path) -> Result<RestoredDraft, String> {
        let mut bytes = Vec::new();
        std::fs::File::open(path).map_err(|e| e.to_string())?.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(t("The recovery draft exceeds its size limit.").into());
        }
        let end = bytes.iter().take(MAX_HEADER + 1).position(|b| *b == b'\n').ok_or_else(|| t("Invalid recovery draft header.").to_string())?;
        let header: serde_json::Value =
            serde_json::from_slice(bytes.get(..end).ok_or_else(|| t("Invalid recovery draft header.").to_string())?).map_err(|e| e.to_string())?;
        let key = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| t("Invalid recovery draft key.").to_string())?.to_string();
        self.path(&key)?;
        Ok(RestoredDraft {
            key,
            name: header.get("name").and_then(|v| v.as_str()).ok_or_else(|| t("The recovery draft has no name.").to_string())?.into(),
            format: header.get("format").and_then(|v| v.as_str()).ok_or_else(|| t("The recovery draft has no format.").to_string())?.into(),
            bytes: bytes.get(end.saturating_add(1)..).ok_or_else(|| t("Invalid recovery draft content.").to_string())?.to_vec(),
        })
    }
}

pub fn connect(dir: PathBuf) -> Result<(DraftBridge, Vec<String>), String> {
    let store = Rc::new(Store::create(&dir)?);
    let (restores, errors) = store.recover(&dir)?;
    // New document ids restart with the process; recovered keys must remain distinct.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let prefix = format!("desktop-{stamp}-{}-", std::process::id());
    let storage_key = move |key: &str| if key.starts_with("desktop-") { key.to_string() } else { format!("{prefix}{key}") };
    let put_key = storage_key.clone();
    let put_store = store.clone();
    Ok((
        DraftBridge {
            desktop_recovery: true,
            available: Box::new(|| true),
            put: Box::new(move |key, name, format, bytes| put_store.put(&put_key(key), name, format, bytes)),
            remove: Box::new(move |key| store.remove(&storage_key(key))),
            hidden: Rc::new(RefCell::new(HiddenFlush::default())),
            restores: Rc::new(RefCell::new(restores)),
        },
        errors,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_replacement_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::create(dir.path()).unwrap();
        store.put("desktop-1", "Report.docx", "docx", b"first").unwrap();
        store.put("desktop-1", "Report.docx", "docx", b"latest").unwrap();
        let draft = store.read(&store.path("desktop-1").unwrap()).unwrap();
        assert_eq!(
            (draft.key.as_str(), draft.name.as_str(), draft.format.as_str(), draft.bytes.as_slice()),
            ("desktop-1", "Report.docx", "docx", b"latest".as_slice())
        );
        assert_eq!(std::fs::read_dir(&store.dir).unwrap().count(), 1);
        store.remove("desktop-1").unwrap();
        store.remove("desktop-1").unwrap();
        assert!(store.entries().unwrap().is_empty());
    }

    #[test]
    fn limits_preserve_existing_drafts_and_reject_paths() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::create(dir.path()).unwrap();
        assert!(store.put("../outside", "a", "docx", b"x").is_err());
        assert!(store.put("large", &"x".repeat(MAX_HEADER + 1), "docx", b"x").is_err());
        for n in 0..MAX_COUNT {
            store.put(&format!("doc-{n}"), "a.docx", "docx", b"x").unwrap();
        }
        assert!(store.put("extra", "a.docx", "docx", b"x").is_err());
        store.put("doc-0", "a.docx", "docx", b"updated").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(store.path("doc-1").unwrap()).unwrap();
        file.set_len(MAX_BYTES + 1).unwrap();
        assert!(store.read(&store.path("doc-1").unwrap()).is_err());
        file.set_len(MAX_TOTAL).unwrap();
        assert!(store.put("doc-0", "a.docx", "docx", b"x").is_err());
        assert_eq!(store.entries().unwrap().len(), MAX_COUNT);
    }

    #[test]
    fn live_instances_are_isolated_and_dead_drafts_are_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let (first, _) = connect(dir.path().into()).unwrap();
        (first.put)("doc-1", "First.docx", "docx", b"first").unwrap();
        let first_dir = std::fs::read_dir(dir.path()).unwrap().filter_map(Result::ok).find(|e| e.file_type().unwrap().is_dir()).unwrap().path();
        let (second, errors) = connect(dir.path().into()).unwrap();
        assert!(errors.is_empty());
        assert!(second.restores.borrow().is_empty());
        (second.put)("doc-1", "Second.docx", "docx", b"second").unwrap();
        // Even a known foreign key cannot delete another instance's draft.
        let foreign =
            std::fs::read_dir(&first_dir).unwrap().filter_map(Result::ok).find(|e| e.path().extension().is_some_and(|e| e == "draft")).unwrap();
        let key = foreign.path().file_stem().unwrap().to_str().unwrap().to_string();
        (second.remove)(&key).unwrap();
        assert!(foreign.path().exists());
        (second.remove)("doc-1").unwrap();
        drop(first);
        let (third, errors) = connect(dir.path().into()).unwrap();
        assert!(errors.is_empty());
        assert!(!first_dir.exists());
        assert!(!first_dir.with_extension("lock").exists());
        let draft = third.restores.borrow_mut().pop().unwrap();
        assert_eq!(draft.bytes, b"first");
        assert_ne!(draft.key, key);
        (third.remove)(&draft.key).unwrap();
        drop(third);
        let (fourth, errors) = connect(dir.path().into()).unwrap();
        assert!(errors.is_empty());
        assert!(fourth.restores.borrow().is_empty());
        // The second store is still live and writable after other stores clean up.
        (second.put)("doc-1", "Second.docx", "docx", b"still protected").unwrap();
    }

    #[test]
    fn draft_orphan_locks_respect_startup_grace_period() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh.lock");
        let old = dir.path().join("old.lock");
        std::fs::File::create(&fresh).unwrap();
        let file = std::fs::File::create(&old).unwrap();
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(120)).unwrap();
        drop(file);
        let unowned = dir.path().join("unowned");
        std::fs::create_dir(&unowned).unwrap();
        std::fs::write(unowned.join("ignored.draft"), b"untouched").unwrap();
        let store = Store::create(dir.path()).unwrap();
        let (drafts, errors) = store.recover(dir.path()).unwrap();
        assert!(drafts.is_empty());
        assert!(errors.is_empty());
        assert!(fresh.exists());
        assert!(!old.exists());
        assert!(unowned.join("ignored.draft").exists());
    }

    #[test]
    fn startup_queues_valid_drafts_and_reports_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let (bridge, errors) = connect(dir.path().into()).unwrap();
        assert!(errors.is_empty());
        (bridge.put)("doc-1", "Draft.docx", "docx", b"package").unwrap();
        let owned = std::fs::read_dir(dir.path()).unwrap().filter_map(Result::ok).find(|e| e.file_type().unwrap().is_dir()).unwrap().path();
        std::fs::write(owned.join("bad.draft"), b"corrupt").unwrap();
        std::fs::write(owned.join(".draft-write-interrupted"), b"partial").unwrap();
        drop(bridge);
        let (restored, errors) = connect(dir.path().into()).unwrap();
        assert!(!owned.join(".draft-write-interrupted").exists());
        assert_eq!(errors.len(), 1);
        assert!(owned.join("bad.draft").exists());
        assert!(owned.with_extension("lock").exists());
        let queue = restored.restores.borrow();
        let draft = queue.first().unwrap();
        assert_eq!(draft.name, "Draft.docx");
        assert_eq!(draft.bytes, b"package");
        assert!(draft.key.starts_with("desktop-"));
        (restored.remove)(&draft.key).unwrap();
        drop(queue);
        drop(restored);
        let (next, errors) = connect(dir.path().into()).unwrap();
        assert!(next.restores.borrow().is_empty());
        assert_eq!(errors.len(), 1);
    }
}
