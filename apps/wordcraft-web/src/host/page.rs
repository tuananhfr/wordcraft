//! Registers the app with `window.erpHost` and hands its documents to the app.

use std::rc::Rc;

use wasm_bindgen::{JsCast as _, JsValue};
use wordcraft_ui_egui::host_docs::{HostLink, HostOpen};

use super::bridge::{JsHost, call};
use super::result;

/// `flush()` resolves by then even if a save in flight never answers (the contract's limit).
const FLUSH_LIMIT_MS: i32 = 10_000;

pub(crate) struct Host {
    link: HostLink,
    erp_host: JsValue,
    ready_sent: bool,
}

/// Asked once, at start-up.
pub(crate) fn connect() -> Option<Host> {
    let window = web_sys::window()?;
    let erp_host = js_sys::Reflect::get(&window, &JsValue::from_str("erpHost")).ok().filter(|h| h.is_object())?;
    let version = js_sys::Reflect::get(&erp_host, &JsValue::from_str("version")).ok()?.as_f64();
    if version != Some(1.0) {
        return None;
    }
    let link = HostLink::new(Rc::new(JsHost { host: erp_host.clone() }));
    Some(Host { link, erp_host, ready_sent: false })
}

fn field(doc: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(doc, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

/// What `open(doc)` was given; missing fields leave the app to refuse the document.
fn host_open(doc: &JsValue) -> Option<HostOpen> {
    let doc_id = field(doc, "docId").as_string()?;
    Some(HostOpen {
        doc_id,
        version: field(doc, "version").as_f64().and_then(result::whole).unwrap_or(0),
        title: field(doc, "title").as_string().unwrap_or_default(),
        format: field(doc, "format").as_string().unwrap_or_default(),
        bytes: field(doc, "bytes").dyn_into::<js_sys::Uint8Array>().map(|b| b.to_vec()).unwrap_or_default(),
    })
}

impl Host {
    pub(crate) fn link(&self) -> HostLink {
        self.link.clone()
    }

    /// `erpHost.ready({open, flush, acknowledged})`, once the app can take a document (its first
    /// frame).
    pub(crate) fn announce(&mut self, ctx: &egui::Context) {
        if self.ready_sent {
            return;
        }
        self.ready_sent = true;
        let link = self.link.clone();
        let wake = ctx.clone();
        let open = wasm_bindgen::closure::Closure::<dyn FnMut(JsValue)>::new(move |doc: JsValue| {
            match host_open(&doc) {
                Some(doc) => link.open(doc),
                None => log::warn!("erpHost open: docId must be a string"),
            }
            wake.request_repaint();
        });
        let link = self.link.clone();
        let flush = wasm_bindgen::closure::Closure::<dyn FnMut() -> JsValue>::new(move || {
            let link = link.clone();
            let promise = js_sys::Promise::new(&mut |resolve: js_sys::Function, _reject: js_sys::Function| {
                if let Some(window) = web_sys::window() {
                    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, FLUSH_LIMIT_MS);
                }
                let resolve = resolve.clone();
                link.flush(Box::new(move || {
                    let _ = resolve.call0(&JsValue::UNDEFINED);
                }));
            });
            promise.into()
        });
        let link = self.link.clone();
        let wake = ctx.clone();
        let acknowledged = wasm_bindgen::closure::Closure::<dyn FnMut(JsValue, JsValue, JsValue)>::new(
            move |doc_id: JsValue, revision: JsValue, version: JsValue| {
                let count = |v: &JsValue| v.as_f64().and_then(result::whole);
                match (doc_id.as_string(), count(&revision), count(&version)) {
                    (Some(doc_id), Some(revision), Some(version)) => link.acknowledged(&doc_id, revision, version),
                    _ => log::warn!("erpHost acknowledged: expected a docId string and whole revision and version"),
                }
                wake.request_repaint();
            },
        );
        let handlers = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&handlers, &JsValue::from_str("open"), open.as_ref());
        let _ = js_sys::Reflect::set(&handlers, &JsValue::from_str("flush"), flush.as_ref());
        let _ = js_sys::Reflect::set(&handlers, &JsValue::from_str("acknowledged"), acknowledged.as_ref());
        if call(&self.erp_host, "ready", &js_sys::Array::of1(&handlers)).is_none() {
            log::warn!("erpHost.ready failed");
        }
        // The shell may call them for the rest of the page's life.
        open.forget();
        flush.forget();
        acknowledged.forget();
    }
}
