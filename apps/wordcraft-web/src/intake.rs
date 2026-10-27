//! The office shell's `window.erpIntakeResult(name, size, ok)`: how each file dropped on the page
//! went. Files from the Open picker share the inbox, but only drops can be the shell's.

use std::cell::RefCell;
use std::rc::Rc;

/// Dropped files read into the inbox and not reported yet, as (name, byte length).
#[derive(Clone, Default)]
pub(crate) struct Drops(Rc<RefCell<Vec<(String, usize)>>>);

impl Drops {
    pub(crate) fn arrived(&self, name: &str, size: usize) {
        if let Ok(mut drops) = self.0.try_borrow_mut() {
            drops.push((name.to_string(), size));
        }
    }

    /// Whether `name` of `size` bytes was dropped and not reported yet; it counts as reported now.
    pub(crate) fn take(&self, name: &str, size: usize) -> bool {
        let Ok(mut drops) = self.0.try_borrow_mut() else { return false };
        let Some(i) = drops.iter().position(|(n, s)| n == name && *s == size) else { return false };
        drops.remove(i);
        true
    }

    /// The app's [`intake_result`](wordcraft_ui_egui::Services::intake_result) service.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn service(&self) -> Box<dyn Fn(&str, usize, bool)> {
        let drops = self.clone();
        Box::new(move |name, size, ok| {
            if drops.take(name, size) {
                report(name, size, ok);
            }
        })
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn report(name: &str, size: usize, ok: bool) {
    use wasm_bindgen::{JsCast as _, JsValue};

    let Some(window) = web_sys::window() else { return };
    let Ok(value) = js_sys::Reflect::get(&window, &JsValue::from_str("erpIntakeResult")) else { return };
    let Ok(callback) = value.dyn_into::<js_sys::Function>() else { return };
    if let Err(error) = callback.call3(&window, &JsValue::from_str(name), &JsValue::from_f64(size as f64), &JsValue::from_bool(ok)) {
        log::warn!("erpIntakeResult failed: {error:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::Drops;

    #[test]
    fn each_drop_is_reported_once_and_picked_files_never() {
        let drops = Drops::default();
        drops.arrived("Báo cáo.docx", 120);
        drops.arrived("Báo cáo.docx", 120);
        assert!(!drops.take("Báo cáo.docx", 121), "another file of the same name");
        assert!(!drops.take("picked.docx", 120), "a file from the Open picker");
        assert!(drops.take("Báo cáo.docx", 120));
        assert!(drops.take("Báo cáo.docx", 120), "the same file dropped twice is reported twice");
        assert!(!drops.take("Báo cáo.docx", 120));
    }
}
