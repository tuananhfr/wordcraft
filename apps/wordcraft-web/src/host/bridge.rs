//! The calls into `window.erpHost`. Its answers never reach the log: they may name the document.

use wasm_bindgen::{JsCast as _, JsValue};
use wordcraft_ui_egui::host_docs::{HostTransport, SaveDone, SaveOutcome, SaveReason};

use super::result;

pub(super) struct JsHost {
    pub(super) host: JsValue,
}

pub(super) fn call(host: &JsValue, method: &str, args: &js_sys::Array) -> Option<JsValue> {
    let f: js_sys::Function = js_sys::Reflect::get(host, &JsValue::from_str(method)).ok()?.dyn_into().ok()?;
    f.apply(host, args).ok()
}

fn outcome_of(value: &JsValue) -> SaveOutcome {
    let json = js_sys::JSON::stringify(value).ok().and_then(|s| s.as_string());
    json.and_then(|s| serde_json::from_str(&s).ok()).and_then(|v| result::save_outcome(&v)).unwrap_or_else(|| {
        log::warn!("erpHost.save answered outside the contract");
        result::unreadable()
    })
}

impl HostTransport for JsHost {
    fn save(&self, doc_id: &str, bytes: &[u8], revision: u64, reason: SaveReason, done: SaveDone) {
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &JsValue::from_str("revision"), &JsValue::from_f64(revision.min(result::MAX_SAFE) as f64));
        let _ = js_sys::Reflect::set(&options, &JsValue::from_str("reason"), &JsValue::from_str(reason.as_str()));
        // `from` copies into JS memory: a view into wasm memory would go stale as it grows.
        let args = js_sys::Array::of3(&JsValue::from_str(doc_id), &js_sys::Uint8Array::from(bytes), &options);
        let promise = call(&self.host, "save", &args).and_then(|v| v.dyn_into::<js_sys::Promise>().ok());
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match promise {
                Some(promise) => match wasm_bindgen_futures::JsFuture::from(promise).await {
                    Ok(value) => outcome_of(&value),
                    Err(_) => {
                        log::warn!("erpHost.save was rejected");
                        result::unreadable()
                    }
                },
                None => {
                    log::warn!("erpHost.save is missing or returned no promise");
                    result::unreadable()
                }
            };
            done(outcome);
        });
    }

    fn dirty(&self, doc_id: &str, dirty: bool) {
        if call(&self.host, "dirty", &js_sys::Array::of2(&JsValue::from_str(doc_id), &JsValue::from_bool(dirty))).is_none() {
            log::warn!("erpHost.dirty failed");
        }
    }

    fn opened(&self, doc_id: &str, ok: bool, reason: Option<&str>) {
        let reason = reason.map_or(JsValue::UNDEFINED, JsValue::from_str);
        if call(&self.host, "opened", &js_sys::Array::of3(&JsValue::from_str(doc_id), &JsValue::from_bool(ok), &reason)).is_none() {
            log::warn!("erpHost.opened failed");
        }
    }

    fn closed(&self, doc_id: &str) {
        if call(&self.host, "closed", &js_sys::Array::of1(&JsValue::from_str(doc_id))).is_none() {
            log::warn!("erpHost.closed failed");
        }
    }
}
