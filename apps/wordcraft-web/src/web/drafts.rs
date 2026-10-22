//! The ERPCons office shell's draft storage, `window.erpDrafts` (`put`, `remove`, `ready`): the
//! shell keeps the drafts and shows all recovery UI; this only passes bytes both ways. Absent
//! (WordCraft opened on its own), drafting is off. Embedded in the ERP page (`embed=erpcons`) or
//! with `?host=parent`, the app keeps its own recovery copy instead ([`embedded`]).

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wordcraft_ui_egui::draft_bridge::{DraftBridge, HiddenFlush, RestoredDraft, flush_hidden};

mod embedded;

use crate::draft_state::{Message, States};
use wordcraft_ui_egui::draft_bridge::DraftObservation;

#[derive(Default)]
struct Tracking {
    states: States,
    keys: std::collections::BTreeSet<String>,
    failed: Rc<RefCell<Vec<String>>>,
    restored: std::collections::BTreeSet<String>,
    ctx: Option<egui::Context>,
}

fn send(messages: Vec<Message>) {
    let Some(shell) = shell() else { return };
    let Ok(function) = js_sys::Reflect::get(&shell, &JsValue::from_str("state")) else { return };
    let Some(function) = function.dyn_ref::<js_sys::Function>() else { return };
    for m in messages {
        let value = js_sys::Object::new();
        let fields = [
            ("docId", JsValue::from_str(&m.doc_id)),
            ("name", JsValue::from_str(&m.name)),
            ("revision", JsValue::from_f64(m.revision as f64)),
            ("dirty", JsValue::from_bool(m.dirty)),
            ("pending", JsValue::from_bool(m.pending)),
            ("committed", JsValue::from_bool(m.committed)),
            ("failed", JsValue::from_bool(m.failed)),
        ];
        if fields.into_iter().all(|(key, v)| js_sys::Reflect::set(&value, &JsValue::from_str(key), &v).is_ok()) {
            let _ = function.call1(&shell, &value);
        }
    }
}

fn observe(tracking: &RefCell<Tracking>, inputs: Vec<DraftObservation>) {
    let Ok(mut tracking) = tracking.try_borrow_mut() else { return };
    let active = inputs.iter().find(|d| d.active).map(|d| d.key.as_str());
    send(tracking.states.switch(active));
    let keys: std::collections::BTreeSet<_> = inputs.iter().map(|d| d.key.clone()).collect();
    let closed: Vec<_> = tracking.keys.difference(&keys).cloned().collect();
    for key in closed {
        send(tracking.states.close(&key));
    }
    for d in inputs {
        send(tracking.states.observe(&d.key, &d.name, d.revision, d.unsaved));
        if tracking.restored.remove(&d.key) {
            tracking.states.checkpoint(&d.key);
            send(tracking.states.complete(&d.key, d.revision, true));
        }
    }
    tracking.keys = keys;
}

/// Announces the app to the shell once it can open documents.
pub(super) struct Drafts {
    embedded: Option<embedded::Embedded>,
    restores: Rc<RefCell<Vec<RestoredDraft>>>,
    ready_sent: bool,
    tracking: Option<Rc<RefCell<Tracking>>>,
}

/// The bridge for the app's [`wordcraft_ui_egui::Services`].
pub(super) fn connect() -> Option<(DraftBridge, Drafts)> {
    if embedded::active() {
        return embedded::connect().map(|(bridge, embedded)| {
            let restores = bridge.restores.clone();
            (bridge, Drafts { restores, ready_sent: false, embedded: Some(embedded), tracking: None })
        });
    }
    let tracking = Rc::new(RefCell::new(Tracking::default()));
    let hidden = Rc::new(RefCell::new(HiddenFlush::default()));
    let restores = Rc::new(RefCell::new(Vec::new()));
    if let Err(e) = flush_when_hidden(&hidden, &tracking) {
        log::warn!("drafts won't be kept when the page is hidden: {e}");
    }
    let failed = tracking.try_borrow().ok()?.failed.clone();
    let observed = tracking.clone();
    let stored_tracking = tracking.clone();
    let bridge = DraftBridge {
        available: Box::new(|| shell().is_some()),
        failed,
        observe: Some(Box::new(move |inputs| observe(&observed, inputs))),
        put: Box::new(move |key, name, format, bytes| tracked_put(&tracking, key, name, format, bytes)),
        remove: Box::new(remove),
        hidden,
        restores: restores.clone(),
    };
    Some((bridge, Drafts { restores, ready_sent: false, embedded: None, tracking: Some(stored_tracking) }))
}

impl Drafts {
    /// `erpDrafts.ready(restore)`, once. `restore` only queues the draft: opening it inside the
    /// shell's callback would run the app outside a frame.
    pub(super) fn announce_ready(&mut self, ctx: &egui::Context) {
        if let Some(embedded) = &mut self.embedded {
            embedded.tick(ctx);
            return;
        }
        if self.ready_sent {
            return;
        }
        if let Some(tracking) = &self.tracking
            && let Ok(mut tracking) = tracking.try_borrow_mut()
        {
            tracking.ctx = Some(ctx.clone());
        }
        let Some(shell) = shell() else { return };
        self.ready_sent = true;
        let restores = self.restores.clone();
        let tracking = self.tracking.clone();
        let ctx = ctx.clone();
        let restore = wasm_bindgen::closure::Closure::<dyn FnMut(JsValue, JsValue, JsValue, JsValue)>::new(
            move |key: JsValue, name: JsValue, format: JsValue, bytes: JsValue| {
                let (Some(key), Some(name), Some(format)) = (key.as_string(), name.as_string(), format.as_string()) else {
                    log::warn!("erpDrafts restore: key, name and format must be strings");
                    return;
                };
                let Ok(bytes) = bytes.dyn_into::<js_sys::Uint8Array>() else {
                    log::warn!("erpDrafts restore: bytes must be a Uint8Array");
                    return;
                };
                match restores.try_borrow_mut() {
                    Ok(mut queue) => {
                        if let Some(tracking) = &tracking
                            && let Ok(mut tracking) = tracking.try_borrow_mut()
                        {
                            tracking.restored.insert(key.clone());
                        }
                        queue.push(RestoredDraft { key, name, format, bytes: bytes.to_vec() });
                    }
                    Err(_) => log::warn!("erpDrafts restore: busy; draft {key} not restored"),
                }
                ctx.request_repaint();
            },
        );
        if let Err(e) = call(&shell, "ready", &js_sys::Array::of1(restore.as_ref())) {
            log::warn!("erpDrafts.ready failed: {e}");
        }
        // The shell may call `restore` at any time for the rest of the page's life.
        restore.forget();
    }
}

/// Looked up at each use: the shell may install or drop it at any time.
fn shell() -> Option<JsValue> {
    let window = web_sys::window()?;
    let shell = js_sys::Reflect::get(&window, &JsValue::from_str("erpDrafts")).ok()?;
    (!shell.is_undefined() && !shell.is_null()).then_some(shell)
}

fn call(shell: &JsValue, method: &str, args: &js_sys::Array) -> Result<JsValue, String> {
    let f: js_sys::Function = js_sys::Reflect::get(shell, &JsValue::from_str(method))
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| format!("erpDrafts.{method} is not a function"))?;
    f.apply(shell, args).map_err(|e| format!("{e:?}"))
}

fn tracked_put(tracking: &Rc<RefCell<Tracking>>, key: &str, name: &str, format: &str, bytes: &[u8]) -> Result<(), String> {
    let shell = shell().ok_or("window.erpDrafts is missing")?;
    // `from` copies into JS memory: a view into wasm memory would go stale as it grows.
    let bytes = js_sys::Uint8Array::from(bytes);
    let state_available = js_sys::Reflect::get(&shell, &JsValue::from_str("state")).ok().is_some_and(|v| v.is_function());
    let revision = if state_available {
        tracking.try_borrow_mut().ok().and_then(|mut t| {
            send(t.states.checkpoint(key));
            t.states.revision(key)
        })
    } else {
        None
    };
    let result = call(&shell, "put", &js_sys::Array::of4(&JsValue::from_str(key), &JsValue::from_str(name), &JsValue::from_str(format), &bytes));
    let Some(revision) = revision else { return result.map(|_| ()) };
    let tracking = tracking.clone();
    let key = key.to_owned();
    wasm_bindgen_futures::spawn_local(async move {
        let success = match result {
            Ok(value) => match value.dyn_into::<js_sys::Promise>() {
                Ok(promise) => wasm_bindgen_futures::JsFuture::from(promise).await.ok().and_then(|v| v.as_bool()) == Some(true),
                Err(_) => false,
            },
            Err(_) => false,
        };
        if let Ok(mut t) = tracking.try_borrow_mut() {
            send(t.states.complete(&key, revision, success));
            if !success && let Ok(mut failed) = t.failed.try_borrow_mut() {
                failed.push(key);
            }
            if let Some(ctx) = &t.ctx {
                ctx.request_repaint();
            }
        }
    });
    Ok(())
}

fn put(key: &str, name: &str, format: &str, bytes: &[u8]) -> Result<(), String> {
    let shell = shell().ok_or("window.erpDrafts is missing")?;
    let bytes = js_sys::Uint8Array::from(bytes);
    call(&shell, "put", &js_sys::Array::of4(&JsValue::from_str(key), &JsValue::from_str(name), &JsValue::from_str(format), &bytes)).map(|_| ())
}

fn remove(key: &str) -> Result<(), String> {
    let shell = shell().ok_or("window.erpDrafts is missing")?;
    call(&shell, "remove", &js_sys::Array::of1(&JsValue::from_str(key))).map(|_| ())
}

/// Hiding the tab (switching away, minimising, a phone locking) and leaving the page put the
/// unsaved document at once: the page may never get another frame.
fn flush_when_hidden(hidden: &Rc<RefCell<HiddenFlush>>, tracking: &Rc<RefCell<Tracking>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let js = |e: JsValue| format!("{e:?}");

    let visibility_tracking = tracking.clone();
    let cell = hidden.clone();
    let doc = document.clone();
    let on_visibility = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
        if doc.visibility_state() == web_sys::VisibilityState::Hidden {
            flush_hidden(&cell, &|key, name, format, bytes| tracked_put(&visibility_tracking, key, name, format, bytes));
        }
    });
    document.add_event_listener_with_callback("visibilitychange", on_visibility.as_ref().unchecked_ref()).map_err(js)?;
    // The listeners live as long as the page.
    on_visibility.forget();

    let cell = hidden.clone();
    let tracking = tracking.clone();
    let on_pagehide = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
        flush_hidden(&cell, &|key, name, format, bytes| tracked_put(&tracking, key, name, format, bytes));
    });
    window.add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref()).map_err(js)?;
    on_pagehide.forget();
    Ok(())
}
