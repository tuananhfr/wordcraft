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

/// Announces the app to the shell once it can open documents.
pub(super) struct Drafts {
    embedded: Option<embedded::Embedded>,
    restores: Rc<RefCell<Vec<RestoredDraft>>>,
    ready_sent: bool,
}

/// The bridge for the app's [`wordcraft_ui_egui::Services`].
pub(super) fn connect() -> Option<(DraftBridge, Drafts)> {
    if embedded::active() {
        return embedded::connect().map(|(bridge, embedded)| {
            let restores = bridge.restores.clone();
            (bridge, Drafts { restores, ready_sent: false, embedded: Some(embedded) })
        });
    }
    let hidden = Rc::new(RefCell::new(HiddenFlush::default()));
    let restores = Rc::new(RefCell::new(Vec::new()));
    if let Err(e) = flush_when_hidden(&hidden) {
        log::warn!("drafts won't be kept when the page is hidden: {e}");
    }
    let bridge =
        DraftBridge { available: Box::new(|| shell().is_some()), put: Box::new(put), remove: Box::new(remove), hidden, restores: restores.clone() };
    Some((bridge, Drafts { restores, ready_sent: false, embedded: None }))
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
        let Some(shell) = shell() else { return };
        self.ready_sent = true;
        let restores = self.restores.clone();
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
                    Ok(mut queue) => queue.push(RestoredDraft { key, name, format, bytes: bytes.to_vec() }),
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

fn call(shell: &JsValue, method: &str, args: &js_sys::Array) -> Result<(), String> {
    let f: js_sys::Function = js_sys::Reflect::get(shell, &JsValue::from_str(method))
        .map_err(|e| format!("{e:?}"))?
        .dyn_into()
        .map_err(|_| format!("erpDrafts.{method} is not a function"))?;
    f.apply(shell, args).map(|_| ()).map_err(|e| format!("{e:?}"))
}

fn put(key: &str, name: &str, format: &str, bytes: &[u8]) -> Result<(), String> {
    let shell = shell().ok_or("window.erpDrafts is missing")?;
    // `from` copies into JS memory: a view into wasm memory would go stale as it grows.
    let bytes = js_sys::Uint8Array::from(bytes);
    call(&shell, "put", &js_sys::Array::of4(&JsValue::from_str(key), &JsValue::from_str(name), &JsValue::from_str(format), &bytes))
}

fn remove(key: &str) -> Result<(), String> {
    let shell = shell().ok_or("window.erpDrafts is missing")?;
    call(&shell, "remove", &js_sys::Array::of1(&JsValue::from_str(key)))
}

/// Hiding the tab (switching away, minimising, a phone locking) and leaving the page put the
/// unsaved document at once: the page may never get another frame.
fn flush_when_hidden(hidden: &Rc<RefCell<HiddenFlush>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let js = |e: JsValue| format!("{e:?}");

    let cell = hidden.clone();
    let doc = document.clone();
    let on_visibility = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
        if doc.visibility_state() == web_sys::VisibilityState::Hidden {
            flush_hidden(&cell, &put);
        }
    });
    document.add_event_listener_with_callback("visibilitychange", on_visibility.as_ref().unchecked_ref()).map_err(js)?;
    // The listeners live as long as the page.
    on_visibility.forget();

    let cell = hidden.clone();
    let on_pagehide = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || flush_hidden(&cell, &put));
    window.add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref()).map_err(js)?;
    on_pagehide.forget();
    Ok(())
}
