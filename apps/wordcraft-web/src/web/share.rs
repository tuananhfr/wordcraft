//! The office shell's file sharing: `window.erpCanShareFiles(mime)`, asked once at start, and
//! `window.erpShare(bytes, name, mime)`. Absent (WordCraft on its own), Share… stays hidden.

use wasm_bindgen::{JsCast as _, JsValue};

fn shell_function(window: &web_sys::Window, name: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(window, &JsValue::from_str(name)).ok()?.dyn_into().ok()
}

/// The app's [`share`](wordcraft_ui_egui::Services::share) service, when the shell says this
/// browser can share a Word document. Chromium refuses Office types, so there it stays hidden.
pub fn connect() -> Option<Box<dyn Fn(&str, &[u8]) -> bool>> {
    let window = web_sys::window()?;
    let docx = JsValue::from_str(super::mime_for("document.docx"));
    let can = shell_function(&window, "erpCanShareFiles")?.call1(&window, &docx).ok()?.as_bool() == Some(true);
    can.then(|| Box::new(share) as Box<dyn Fn(&str, &[u8]) -> bool>)
}

/// Called within the click: the share sheet needs its user activation. `false` (no shell
/// function, or it threw or declined) leaves the copy to the app's download.
fn share(name: &str, bytes: &[u8]) -> bool {
    let Some(window) = web_sys::window() else { return false };
    let Some(erp_share) = shell_function(&window, "erpShare") else { return false };
    let args = js_sys::Array::of3(&js_sys::Uint8Array::from(bytes), &JsValue::from_str(name), &JsValue::from_str(super::mime_for(name)));
    erp_share.apply(&window, &args).ok().and_then(|v| v.as_bool()) == Some(true)
}
