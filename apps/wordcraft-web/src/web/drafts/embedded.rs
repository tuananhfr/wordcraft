//! Embedded recovery in the browser: the parent's `init`/`open` messages, the IndexedDB store and
//! the draft bridge around [`crate::embedded_drafts::Recovery`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{Reflect, Uint8Array};
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use web_sys::{IdbDatabase, IdbTransactionMode, MessageEvent};
use wordcraft_ui_egui::draft_bridge::{DraftBridge, HiddenFlush, RestoredDraft, flush_hidden};

use super::super::font_store::done;
use crate::embedded_drafts::{HostMessage, Job, MAX_DRAFT_BYTES, Recovery, StoredDraft};

/// IndexedDB database of embedded recovery copies, apart from the shell's standalone drafts.
const DB_NAME: &str = "erpcons-embed-drafts";
const STORE: &str = "documents";

type Restores = Rc<RefCell<Vec<RestoredDraft>>>;

#[derive(Clone, Default)]
struct Store {
    recovery: Rc<RefCell<Recovery>>,
    // Kept open: a write when the page is hidden must not wait for a new connection.
    db: Rc<RefCell<Option<IdbDatabase>>>,
    busy: Rc<Cell<bool>>,
}

pub(super) struct Embedded {
    store: Store,
    restores: Restores,
}

fn property(target: &JsValue, name: &str) -> Option<JsValue> {
    Reflect::get(target, &JsValue::from_str(name)).ok().filter(|v| !v.is_undefined() && !v.is_null())
}

fn string(target: &JsValue, name: &str) -> Option<String> {
    property(target, name)?.as_string()
}

fn js(e: JsValue) -> String {
    format!("{e:?}")
}

/// The ERP page's origin, which the shell's host bridge checked against its allow-list.
fn parent_origin() -> Option<String> {
    string(&property(web_sys::window()?.as_ref(), "erpEmbed")?, "origin")
}

/// Opened inside the ERP page (`embed=erpcons`) or by a same-origin parent (`host=parent`).
pub(super) fn active() -> bool {
    parent_origin().is_some() || super::super::query().trim_start_matches('?').split('&').any(|p| p == "host=parent")
}

pub(super) fn connect() -> Option<(DraftBridge, Embedded)> {
    let window = web_sys::window()?;
    let store = Store::default();
    let restores = Restores::default();
    if let Err(e) = listen(&window, &store, &restores) {
        log::warn!("embedded recovery is off: {e}");
        return None;
    }
    // Tells the parent it may send `init` now (see packaging/web/README.md).
    if let (Some(origin), Ok(Some(parent))) = (parent_origin(), window.parent()) {
        let ready = js_sys::Object::new();
        for (field, value) in [("ns", JsValue::from_str("erp-office")), ("v", JsValue::from(1)), ("type", JsValue::from_str("drafts-ready"))] {
            let _ = Reflect::set(&ready, &JsValue::from_str(field), &value);
        }
        if let Err(e) = parent.post_message(&ready, &origin) {
            log::warn!("drafts-ready: {e:?}");
        }
    }
    let hidden = Rc::new(RefCell::new(HiddenFlush::default()));
    if let Err(e) = flush_when_hidden(&window, &hidden, &store) {
        log::warn!("embedded drafts won't be kept when the page is hidden: {e}");
    }
    let (available, put, remove) = (store.clone(), store.clone(), store.clone());
    let bridge = DraftBridge {
        observe: None,
        failed: Rc::default(),
        // The shell's `erpDrafts` stub reports unsaved changes to the ERP page, recovery or not.
        available: Box::new(move || super::shell().is_some() || available.recovery.try_borrow().is_ok_and(|r| r.ready())),
        put: Box::new(move |key, name, format, bytes| put.put(key, name, format, bytes)),
        remove: Box::new(move |key| remove.remove(key)),
        hidden,
        restores: restores.clone(),
    };
    Some((bridge, Embedded { store, restores }))
}

fn listen(window: &web_sys::Window, store: &Store, restores: &Restores) -> Result<(), String> {
    let recovery = store.recovery.clone();
    let restores = restores.clone();
    let listener = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(window) = web_sys::window() else { return };
        let Ok(Some(parent)) = window.parent() else { return };
        if event.source().is_none_or(|source| !js_sys::Object::is(source.as_ref(), parent.as_ref())) {
            return;
        }
        let allowed = parent_origin().or_else(|| window.location().origin().ok());
        if allowed.as_deref() != Some(event.origin().as_str()) {
            return;
        }
        let data = event.data();
        if string(&data, "ns").as_deref() != Some("erp-office") || property(&data, "v").and_then(|v| v.as_f64()) != Some(1.0) {
            return;
        }
        let (Some(kind), name, doc_id, drafts) = (string(&data, "type"), string(&data, "name"), string(&data, "docId"), string(&data, "drafts"))
        else {
            return;
        };
        let origin = event.origin();
        let message =
            HostMessage { kind: &kind, origin: &origin, name: name.as_deref().unwrap_or(""), doc_id: doc_id.as_deref(), drafts: drafts.as_deref() };
        let Ok(mut recovery) = recovery.try_borrow_mut() else { return };
        if recovery.host_message(&message)
            && let Ok(mut queue) = restores.try_borrow_mut()
        {
            queue.clear();
        }
    });
    window.add_event_listener_with_callback("message", listener.as_ref().unchecked_ref()).map_err(js)?;
    // The listener lives as long as the page.
    listener.forget();
    Ok(())
}

/// Hiding the tab or leaving the page puts the unsaved documents at once, as standalone drafts do.
fn flush_when_hidden(window: &web_sys::Window, hidden: &Rc<RefCell<HiddenFlush>>, store: &Store) -> Result<(), String> {
    let document = window.document().ok_or("no document")?;
    let (cell, put, doc) = (hidden.clone(), store.clone(), document.clone());
    let on_visibility = Closure::<dyn FnMut()>::new(move || {
        if doc.visibility_state() == web_sys::VisibilityState::Hidden {
            flush_hidden(&cell, &|key, name, format, bytes| put.put(key, name, format, bytes));
        }
    });
    document.add_event_listener_with_callback("visibilitychange", on_visibility.as_ref().unchecked_ref()).map_err(js)?;
    on_visibility.forget();
    let (cell, put) = (hidden.clone(), store.clone());
    let on_pagehide = Closure::<dyn FnMut()>::new(move || flush_hidden(&cell, &|key, name, format, bytes| put.put(key, name, format, bytes)));
    window.add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref()).map_err(js)?;
    on_pagehide.forget();
    Ok(())
}

impl Store {
    fn put(&self, key: &str, name: &str, format: &str, bytes: &[u8]) -> Result<(), String> {
        if super::shell().is_some() {
            super::put(key, name, format, bytes)?;
        }
        self.recovery.try_borrow_mut().map_err(|_| "embedded recovery is busy".to_string())?.put(key, name, format, bytes)?;
        self.drain();
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<(), String> {
        if super::shell().is_some() {
            super::remove(key)?;
        }
        self.recovery.try_borrow_mut().map_err(|_| "embedded recovery is busy".to_string())?.remove(key);
        self.drain();
        Ok(())
    }

    /// Perform the queued writes in order, one task at a time.
    fn drain(&self) {
        if self.busy.replace(true) {
            return;
        }
        let store = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(job) = store.recovery.try_borrow_mut().ok().and_then(|mut r| r.next_job()) {
                if let Err(e) = store.write(job).await {
                    log::warn!("embedded draft: {e}");
                }
            }
            store.busy.set(false);
        });
    }

    async fn database(&self) -> Result<IdbDatabase, String> {
        if let Some(db) = self.db.try_borrow().ok().and_then(|db| db.clone()) {
            return Ok(db);
        }
        let factory = web_sys::window().ok_or("no window")?.indexed_db().map_err(js)?.ok_or("IndexedDB unavailable")?;
        let request = factory.open_with_u32(DB_NAME, 1).map_err(js)?;
        let upgrade_request = request.clone();
        let upgrade = Closure::<dyn FnMut()>::new(move || {
            if let Ok(db) = upgrade_request.result().and_then(|v| v.dyn_into::<IdbDatabase>())
                && !db.object_store_names().contains(STORE)
                && let Err(e) = db.create_object_store(STORE)
            {
                log::warn!("embedded draft store: {e:?}");
            }
        });
        request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        let result = done(&request).await;
        request.set_onupgradeneeded(None);
        let db = result?.dyn_into::<IdbDatabase>().map_err(js)?;
        if let Ok(mut slot) = self.db.try_borrow_mut() {
            *slot = Some(db.clone());
        }
        Ok(db)
    }

    async fn write(&self, job: Job) -> Result<(), String> {
        let db = self.database().await?;
        let result = async {
            let store = db.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite).map_err(js)?.object_store(STORE).map_err(js)?;
            let request = match job {
                Job::Put { store_key, draft } => store.put_with_key(&record(&draft)?, &JsValue::from_str(&store_key)),
                Job::Delete { store_key } => store.delete(&JsValue::from_str(&store_key)),
            }
            .map_err(js)?;
            done(&request).await.map(|_| ())
        }
        .await;
        if result.is_err()
            && let Ok(mut slot) = self.db.try_borrow_mut()
        {
            // The browser may have closed the connection (storage cleared): reopen next time.
            *slot = None;
        }
        result
    }

    async fn load(&self, key: &str) -> Result<Option<StoredDraft>, String> {
        let db = self.database().await?;
        let store = db.transaction_with_str(STORE).map_err(js)?.object_store(STORE).map_err(js)?;
        let value = done(&store.get(&JsValue::from_str(key)).map_err(js)?).await?;
        if value.is_undefined() || value.is_null() {
            return Ok(None);
        }
        let (Some(key), Some(name), Some(format), Some(bytes)) =
            (string(&value, "key"), string(&value, "name"), string(&value, "format"), property(&value, "bytes"))
        else {
            return Err("damaged recovery record".into());
        };
        let bytes = bytes.dyn_into::<Uint8Array>().map_err(|_| "damaged recovery record".to_string())?;
        if bytes.length() as usize > MAX_DRAFT_BYTES {
            return Err("recovery record too large".into());
        }
        Ok(Some(StoredDraft { key, name, format, bytes: bytes.to_vec() }))
    }
}

fn record(draft: &StoredDraft) -> Result<JsValue, String> {
    let value = js_sys::Object::new();
    for (field, text) in [("key", &draft.key), ("name", &draft.name), ("format", &draft.format)] {
        Reflect::set(&value, &JsValue::from_str(field), &JsValue::from_str(text)).map_err(js)?;
    }
    // `from` copies into JS memory: a view into wasm memory would go stale as it grows.
    Reflect::set(&value, &JsValue::from_str("bytes"), &Uint8Array::from(draft.bytes.as_slice())).map_err(js)?;
    Ok(value.into())
}

impl Embedded {
    /// Every frame: identify the document (older shells: by the delivered file) and look up its
    /// stored copy once, queueing it for restore.
    pub(super) fn tick(&mut self, ctx: &egui::Context) {
        let recovery = &self.store.recovery;
        if recovery.try_borrow().is_ok_and(|r| !r.has_identity())
            && let Some(window) = web_sys::window()
            && let Some(embed) = property(window.as_ref(), "erpEmbed")
            && let Some(origin) = string(&embed, "origin")
            && let Some(original_name) = property(&embed, "originalName").and_then(|v| v.dyn_into::<js_sys::Function>().ok())
            && let Some(name) = original_name.call0(&embed).ok().and_then(|v| v.as_string())
            && let Ok(mut r) = recovery.try_borrow_mut()
        {
            r.fallback(&origin, &name);
        }
        let Some((generation, key)) = recovery.try_borrow_mut().ok().and_then(|mut r| r.begin_load()) else { return };
        let (store, restores, ctx) = (self.store.clone(), self.restores.clone(), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let found = store.load(&key).await.unwrap_or_else(|e| {
                log::warn!("embedded recovery: {e}");
                None
            });
            let restored = store.recovery.try_borrow_mut().ok().and_then(|mut r| r.finish_load(generation, found));
            if let Some(d) = restored
                && let Ok(mut queue) = restores.try_borrow_mut()
            {
                queue.push(RestoredDraft { key: d.key, name: d.name, format: d.format, bytes: d.bytes });
            }
            ctx.request_repaint();
        });
    }
}
