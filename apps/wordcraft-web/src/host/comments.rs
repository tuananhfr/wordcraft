//! The `erpHost` comment calls on the page (M5): `anchorOfSelection`, `showComments`,
//! `focusAnchor` and `resolveAnchors` given to `erpHost.ready`, and `erpHost.commentClicked` when
//! a mark is clicked. What they do is in [`wordcraft_ui_egui::host_docs`].

use serde_json::Value;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wordcraft_ui_egui::host_docs::{CommentMsg, HostLink};

use super::bridge::call;

/// The shell shows Cloud comments (found by feature, like the region calls).
pub(super) fn speaks_comments(host: &JsValue) -> bool {
    js_sys::Reflect::get(host, &JsValue::from_str("commentClicked")).is_ok_and(|f| f.is_function())
}

fn json_of(value: &JsValue) -> Value {
    js_sys::JSON::stringify(value).ok().and_then(|s| s.as_string()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

fn js_of(json: &Value) -> JsValue {
    js_sys::JSON::parse(&json.to_string()).unwrap_or(JsValue::NULL)
}

fn set(handlers: &js_sys::Object, handler: &str, f: JsValue) {
    if js_sys::Reflect::set(handlers, &JsValue::from_str(handler), &f).is_err() {
        log::warn!("erpHost.ready: couldn't add {handler}");
    }
}

/// Adds the comment handlers to those given to `erpHost.ready`. `anchorOfSelection` answers at
/// once (the selection as of the last frame); `resolveAnchors` returns a promise answered on the
/// next frame, after pending `applyRemote` calls.
pub(super) fn add_handlers(handlers: &js_sys::Object, link: &HostLink) {
    let l = link.clone();
    let anchor = Closure::<dyn FnMut() -> JsValue>::new(move || js_of(&l.anchor_of_selection()));
    set(handlers, "anchorOfSelection", anchor.into_js_value());
    let l = link.clone();
    let show = Closure::<dyn FnMut(JsValue)>::new(move |threads: JsValue| l.comment_message(CommentMsg::Show(json_of(&threads))));
    set(handlers, "showComments", show.into_js_value());
    let l = link.clone();
    let focus = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |anchor: JsValue, thread: JsValue| {
        l.comment_message(CommentMsg::Focus(json_of(&anchor), thread.as_string()));
    });
    set(handlers, "focusAnchor", focus.into_js_value());
    let l = link.clone();
    let resolve = Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |threads: JsValue| {
        let threads = json_of(&threads);
        let l = l.clone();
        js_sys::Promise::new(&mut |resolve: js_sys::Function, _reject: js_sys::Function| {
            let threads = threads.clone();
            l.comment_message(CommentMsg::Resolve(
                threads,
                Box::new(move |updates| {
                    if resolve.call1(&JsValue::UNDEFINED, &js_of(&updates)).is_err() {
                        log::warn!("erpHost.resolveAnchors: couldn't answer");
                    }
                }),
            ));
        })
        .into()
    });
    set(handlers, "resolveAnchors", resolve.into_js_value());
}

/// `erpHost.commentClicked(threadId)`.
pub(super) fn clicked(host: &JsValue, thread_id: &str) {
    if call(host, "commentClicked", &js_sys::Array::of1(&JsValue::from_str(thread_id))).is_none() {
        log::warn!("erpHost.commentClicked failed");
    }
}
