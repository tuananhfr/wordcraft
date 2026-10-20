//! Browser capability calls stay synchronous until their promises have started.

use js_sys::{Array, Function, Promise, Reflect};
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use super::{ClipboardBridge, MAX_FORMATS, MAX_IMAGE_BYTES, MAX_IMAGES, MAX_TEXT_BYTES, Notice, PendingPaste, native_text_target};

fn property(target: &JsValue, name: &str) -> Result<JsValue, JsValue> {
    Reflect::get(target, &JsValue::from_str(name))
}

fn promise(target: &JsValue, method: &str, args: &Array) -> Result<Promise, JsValue> {
    property(target, method)?.dyn_into::<Function>()?.apply(target, args)?.dyn_into()
}

fn clipboard() -> Result<JsValue, JsValue> {
    let window = web_sys::window().ok_or(JsValue::UNDEFINED)?;
    property(&property(window.as_ref(), "navigator")?, "clipboard")
}

fn formats(values: Array) -> Vec<String> {
    values.iter().take(MAX_FORMATS).filter_map(|v| v.as_string()).map(|s| s.chars().take(128).collect()).collect()
}

fn focused_html_editor() -> bool {
    let Some(element) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.active_element()) else {
        return false;
    };
    let editable = element.dyn_ref::<web_sys::HtmlElement>().is_some_and(|e| e.is_content_editable());
    native_text_target(&element.tag_name(), &element.id(), editable)
}

async fn bytes(blob: &web_sys::Blob) -> Result<Vec<u8>, JsValue> {
    let buffer = JsFuture::from(blob.array_buffer()).await?;
    let buffer = js_sys::Uint8Array::new(&buffer);
    if buffer.length() as usize > MAX_IMAGE_BYTES {
        return Err(JsValue::UNDEFINED);
    }
    Ok(buffer.to_vec())
}

impl ClipboardBridge {
    pub fn install(ctx: egui::Context) -> Result<Self, String> {
        let bridge = Self { ctx, ..Default::default() };
        let listener_bridge = bridge.clone();
        let on_paste = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(move |event: web_sys::ClipboardEvent| {
            if focused_html_editor() {
                return;
            }
            let Some(data) = event.clipboard_data() else { return };
            let mut paste = PendingPaste {
                text: data.get_data("text/plain").unwrap_or_default(),
                html: data.get_data("text/html").ok().filter(|s| !s.is_empty()),
                rtf: data.get_data("text/rtf").ok().filter(|s| !s.is_empty()),
                images: Vec::new(),
            };
            if paste.enforce_caps() {
                listener_bridge.notice(Notice::TooLarge);
            }
            let mut formats = formats(data.types());
            let mut files = Vec::new();
            if let Some(list) = data.files() {
                let mut count = 0;
                if list.length() as usize > MAX_FORMATS {
                    listener_bridge.notice(Notice::TooLarge);
                }
                for i in 0..list.length().min(MAX_FORMATS as u32) {
                    let Some(file) = list.get(i) else { continue };
                    if !file.type_().starts_with("image/") {
                        continue;
                    }
                    let mime = file.type_();
                    if formats.len() < MAX_FORMATS && !formats.contains(&mime) {
                        formats.push(mime);
                    }
                    count += 1;
                    if count > MAX_IMAGES {
                        listener_bridge.notice(Notice::TooLarge);
                        break;
                    }
                    if file.size() > MAX_IMAGE_BYTES as f64 {
                        listener_bridge.notice(Notice::TooLarge);
                        continue;
                    }
                    files.push(file);
                }
            }
            event.prevent_default();
            event.stop_immediate_propagation();
            if files.is_empty() {
                listener_bridge.deliver(paste, formats, None);
            } else {
                let bridge = listener_bridge.clone();
                spawn_local(async move {
                    for file in files {
                        match bytes(file.as_ref()).await {
                            Ok(bytes) => paste.images.push((file.type_(), bytes)),
                            Err(_) => bridge.notice(Notice::ReadFailed),
                        }
                    }
                    bridge.deliver(paste, formats, None);
                });
            }
        });
        let window = web_sys::window().ok_or("no window")?;
        window
            .add_event_listener_with_callback_and_bool("paste", on_paste.as_ref().unchecked_ref(), true)
            .map_err(|_| "could not install clipboard listener")?;
        // The web runner owns this listener for the lifetime of the page.
        on_paste.forget();
        Ok(bridge)
    }

    pub fn write(&self, html: &str, text: &str) -> bool {
        let Ok(constructor) = property(&js_sys::global(), "ClipboardItem").and_then(|v| v.dyn_into::<Function>()) else {
            return false;
        };
        if html.len() > MAX_TEXT_BYTES || text.len() > MAX_TEXT_BYTES {
            self.notice(Notice::TooLarge);
            return false;
        }
        let start = || -> Result<Promise, JsValue> {
            let parts = js_sys::Object::new();
            for (mime, content) in [("text/html", html), ("text/plain", text)] {
                let options = web_sys::BlobPropertyBag::new();
                options.set_type(mime);
                let strings = Array::new();
                strings.push(&JsValue::from_str(content));
                let blob = web_sys::Blob::new_with_str_sequence_and_options(&strings, &options)?;
                Reflect::set(&parts, &JsValue::from_str(mime), blob.as_ref())?;
            }
            let args = Array::new();
            args.push(&parts);
            let item = Reflect::construct(&constructor, &args)?;
            let items = Array::new();
            items.push(&item);
            let args = Array::new();
            args.push(&items);
            promise(&clipboard()?, "write", &args)
        };
        match start() {
            Ok(write) => {
                let bridge = self.clone();
                spawn_local(async move {
                    if JsFuture::from(write).await.is_err() {
                        bridge.notice(Notice::WriteFailed);
                    }
                });
                true
            }
            Err(_) => {
                self.notice(Notice::WriteFailed);
                false
            }
        }
    }

    pub fn request_read(&self, command: &str) {
        let read = clipboard().and_then(|c| promise(&c, "read", &Array::new()));
        let Ok(read) = read else {
            self.notice(Notice::ReadFailed);
            return;
        };
        let bridge = self.clone();
        let command = command.to_string();
        spawn_local(async move {
            match read_items(read, &bridge).await {
                Ok((paste, formats)) => bridge.deliver(paste, formats, Some(command)),
                Err(_) => bridge.notice(Notice::ReadFailed),
            }
        });
    }
}

async fn read_items(read: Promise, bridge: &ClipboardBridge) -> Result<(PendingPaste, Vec<String>), JsValue> {
    let value = JsFuture::from(read).await?;
    let items = value.dyn_into::<Array>()?;
    let mut paste = PendingPaste::default();
    let mut names = Vec::new();
    let mut image_count = 0;
    if items.length() as usize > MAX_IMAGES {
        bridge.notice(Notice::TooLarge);
    }
    for item in items.iter().take(MAX_IMAGES) {
        let types = property(&item, "types")?.dyn_into::<Array>()?;
        for mime in formats(types) {
            if names.len() < MAX_FORMATS && !names.contains(&mime) {
                names.push(mime.clone());
            }
            let cap = match mime.as_str() {
                "text/plain" | "text/html" | "text/rtf" => MAX_TEXT_BYTES,
                _ if mime.starts_with("image/") && image_count < MAX_IMAGES => {
                    image_count += 1;
                    MAX_IMAGE_BYTES
                }
                _ => continue,
            };
            let args = Array::new();
            args.push(&JsValue::from_str(&mime));
            let blob = match promise(&item, "getType", &args) {
                Ok(read) => JsFuture::from(read).await.and_then(|v| v.dyn_into::<web_sys::Blob>()),
                Err(e) => Err(e),
            };
            let Ok(blob) = blob else {
                bridge.notice(Notice::ReadFailed);
                continue;
            };
            if blob.size() > cap as f64 {
                bridge.notice(Notice::TooLarge);
                continue;
            }
            if mime.starts_with("image/") {
                match bytes(&blob).await {
                    Ok(bytes) => paste.images.push((mime, bytes)),
                    Err(_) => bridge.notice(Notice::ReadFailed),
                }
            } else {
                let Some(text) = JsFuture::from(blob.text()).await.ok().and_then(|v| v.as_string()) else {
                    bridge.notice(Notice::ReadFailed);
                    continue;
                };
                if text.len() > MAX_TEXT_BYTES {
                    bridge.notice(Notice::TooLarge);
                    continue;
                }
                match mime.as_str() {
                    "text/plain" if paste.text.is_empty() => paste.text = text,
                    "text/html" if paste.html.is_none() => paste.html = Some(text),
                    "text/rtf" if paste.rtf.is_none() => paste.rtf = Some(text),
                    _ => {}
                }
            }
        }
    }
    Ok((paste, names))
}
