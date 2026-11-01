//! `erpHost` v3 on the page: the region handlers given to `erpHost.ready`, and the region calls
//! the editor makes. What they do is in [`wordcraft_ui_egui::host_docs`].

use std::rc::Rc;

use serde_json::Value;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wordcraft_engine::live::Call;
use wordcraft_ui_egui::host_docs::{HostLink, LiveMsg, cursors_of, locks_of, present};

use super::bridge::call;

/// The shell takes region calls (found by feature: its `version` stays 2).
pub(super) fn speaks_regions(host: &JsValue) -> bool {
    js_sys::Reflect::get(host, &JsValue::from_str("regionWillEdit")).is_ok_and(|f| f.is_function())
}

fn json_of(value: &JsValue) -> Value {
    js_sys::JSON::stringify(value).ok().and_then(|s| s.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

fn set(handlers: &js_sys::Object, name: &str, f: JsValue) {
    if js_sys::Reflect::set(handlers, &JsValue::from_str(name), &f).is_err() {
        log::warn!("erpHost.ready: couldn't add {name}");
    }
}

/// Adds `applyRemote`, `showPresence`, `showLocks`, `showCursors`, `lockDenied`, `isLeader` and
/// `regionConflict` to the handlers given to `erpHost.ready`.
pub(super) fn add_handlers(handlers: &js_sys::Object, link: &HostLink) {
    let l = link.clone();
    let apply = Closure::<dyn FnMut(JsValue, JsValue, JsValue)>::new(move |region: JsValue, payload: JsValue, _author: JsValue| {
        match (region.as_string(), payload.as_string()) {
            (Some(r), Some(p)) => l.live_message(LiveMsg::Apply(r, p)),
            _ => log::warn!("erpHost.applyRemote: expected a region and a payload string"),
        }
    });
    set(handlers, "applyRemote", apply.into_js_value());
    let l = link.clone();
    let presence = Closure::<dyn FnMut(JsValue)>::new(move |members: JsValue| l.live_message(LiveMsg::Presence(present(&json_of(&members)))));
    set(handlers, "showPresence", presence.into_js_value());
    let l = link.clone();
    let locks = Closure::<dyn FnMut(JsValue)>::new(move |list: JsValue| l.live_message(LiveMsg::Locks(locks_of(&json_of(&list)))));
    set(handlers, "showLocks", locks.into_js_value());
    let l = link.clone();
    let cursors = Closure::<dyn FnMut(JsValue)>::new(move |list: JsValue| l.live_message(LiveMsg::Cursors(cursors_of(&json_of(&list)))));
    set(handlers, "showCursors", cursors.into_js_value());
    let l = link.clone();
    let denied = Closure::<dyn FnMut(JsValue)>::new(move |region: JsValue| {
        if let Some(r) = region.as_string() {
            l.live_message(LiveMsg::Denied(r));
        }
    });
    set(handlers, "lockDenied", denied.into_js_value());
    let l = link.clone();
    let leader = Closure::<dyn FnMut(JsValue)>::new(move |on: JsValue| l.live_message(LiveMsg::Leader(on.as_bool() == Some(true))));
    set(handlers, "isLeader", leader.into_js_value());
    let l = link.clone();
    let conflict = Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |region: JsValue| {
        let region = region.as_string().unwrap_or_default();
        let l = l.clone();
        js_sys::Promise::new(&mut |resolve: js_sys::Function, _reject: js_sys::Function| {
            l.conflict(
                region.clone(),
                Box::new(move |answer| {
                    if resolve.call1(&JsValue::UNDEFINED, &JsValue::from_str(answer)).is_err() {
                        log::warn!("erpHost.regionConflict: couldn't answer");
                    }
                }),
            );
        })
        .into()
    });
    set(handlers, "regionConflict", conflict.into_js_value());
}

/// Makes the editor's calls on the shell; `regionWillEdit`'s promise goes to `answer`.
pub(super) fn perform(host: &JsValue, calls: Vec<Call>, answer: Rc<dyn Fn(String, bool)>) {
    for c in calls {
        match c {
            Call::WillEdit(region) => {
                let asked = call(host, "regionWillEdit", &js_sys::Array::of1(&JsValue::from_str(&region)));
                let answer = answer.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let granted = match asked {
                        Some(v) if v.is_instance_of::<js_sys::Promise>() => {
                            match wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(v)).await {
                                Ok(v) => v.as_bool() == Some(true),
                                Err(_) => {
                                    log::warn!("erpHost.regionWillEdit was rejected");
                                    false
                                }
                            }
                        }
                        Some(v) => v.as_bool() == Some(true),
                        None => false,
                    };
                    answer(region, granted);
                });
            }
            Call::Changed(region, payload) => {
                if call(host, "regionChanged", &js_sys::Array::of2(&JsValue::from_str(&region), &JsValue::from_str(&payload))).is_none() {
                    log::warn!("erpHost.regionChanged failed");
                }
            }
            Call::DidEndEdit(region) => {
                if call(host, "regionDidEndEdit", &js_sys::Array::of1(&JsValue::from_str(&region))).is_none() {
                    log::warn!("erpHost.regionDidEndEdit failed");
                }
            }
            Call::Selection(region, range) => {
                if call(host, "selectionChanged", &js_sys::Array::of2(&JsValue::from_str(&region), &JsValue::from_str(&range))).is_none() {
                    log::warn!("erpHost.selectionChanged failed");
                }
            }
        }
    }
}
