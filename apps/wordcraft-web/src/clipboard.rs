//! Frame-scoped clipboard data shared by browser events and asynchronous menu reads.

use std::cell::RefCell;
use std::rc::Rc;

use wordcraft_ui_egui::{Inbox, Services, paste_picture::ClipboardPicture};

const MAX_TEXT_BYTES: usize = 8 << 20;
const MAX_IMAGE_BYTES: usize = 20 << 20;
const MAX_IMAGES: usize = 20;
const MAX_FORMATS: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingPaste {
    pub text: String,
    pub html: Option<String>,
    pub rtf: Option<String>,
    pub images: Vec<(String, Vec<u8>)>,
}

impl PendingPaste {
    fn enforce_caps(&mut self) -> bool {
        let mut dropped = false;
        for part in [&mut self.html, &mut self.rtf] {
            if part.as_ref().is_some_and(|s| s.len() > MAX_TEXT_BYTES) {
                *part = None;
                dropped = true;
            }
        }
        if self.text.len() > MAX_TEXT_BYTES {
            let mut end = MAX_TEXT_BYTES;
            while !self.text.is_char_boundary(end) {
                end = end.saturating_sub(1);
            }
            self.text.truncate(end);
            dropped = true;
        }
        let mut kept = 0;
        self.images.retain(|(mime, bytes)| {
            let fits = mime.starts_with("image/") && bytes.len() <= MAX_IMAGE_BYTES && kept < MAX_IMAGES;
            if fits {
                kept += 1;
            } else {
                dropped = true;
            }
            fits
        });
        if self.text.is_empty()
            && let Some(html) = &self.html
        {
            self.text = html_to_plain(html);
        }
        dropped
    }
}

pub fn paste_events(pending: &PendingPaste) -> Vec<egui::Event> {
    if pending.text.is_empty() { Vec::new() } else { vec![egui::Event::Paste(pending.text.clone())] }
}

fn entity(s: &str) -> Option<char> {
    match s {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "ndash" => Some('–'),
        "mdash" => Some('—'),
        "hellip" => Some('…'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        _ => {
            let code = if let Some(hex) = s.strip_prefix("#x").or_else(|| s.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                s.strip_prefix('#')?.parse().ok()?
            };
            char::from_u32(code).filter(|c| *c != '\0')
        }
    }
}

pub fn html_to_plain(html: &str) -> String {
    if html.len() > MAX_TEXT_BYTES {
        return String::new();
    }
    let mut out = String::new();
    let mut chars = html.chars().peekable();
    let mut hidden = None::<String>;
    while let Some(c) = chars.next() {
        if c == '<' {
            if chars.clone().take(3).eq("!--".chars()) {
                for _ in 0..3 {
                    chars.next();
                }
                let mut previous = ('\0', '\0');
                for c in chars.by_ref() {
                    if previous == ('-', '-') && c == '>' {
                        break;
                    }
                    previous = (previous.1, c);
                }
                continue;
            }
            let mut tag = String::new();
            let mut quote = None;
            for c in chars.by_ref() {
                if quote == Some(c) {
                    quote = None;
                } else if quote.is_none() {
                    if c == '>' {
                        break;
                    }
                    if matches!(c, '\'' | '"') {
                        quote = Some(c);
                    }
                }
                tag.push(c);
            }
            let tag = tag.trim();
            let closing = tag.starts_with('/');
            let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_ascii_lowercase();
            if let Some(h) = &hidden {
                if closing && *h == name {
                    hidden = None;
                }
                continue;
            }
            if !closing && matches!(name.as_str(), "script" | "style" | "head") {
                hidden = Some(name);
                continue;
            }
            if matches!(name.as_str(), "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
            } else if closing && matches!(name.as_str(), "td" | "th") {
                out.push('\t');
            }
        } else if hidden.is_none() {
            if c == '&' {
                let mut name = String::new();
                while let Some(&next) = chars.peek() {
                    if name.len() >= 32 || !(next.is_ascii_alphanumeric() || next == '#') {
                        break;
                    }
                    name.push(next);
                    chars.next();
                }
                if chars.peek() == Some(&';') {
                    chars.next();
                    if let Some(decoded) = entity(&name) {
                        out.push(decoded);
                    } else {
                        out.push('&');
                        out.push_str(&name);
                        out.push(';');
                    }
                } else {
                    out.push('&');
                    out.push_str(&name);
                }
            } else {
                out.push(c);
            }
        }
    }
    out.trim().to_string()
}

// eframe 0.36 gives its hidden IME input no id; the browser glue recognises it by position.
fn native_text_target(tag: &str, egui_text_agent: bool, editable: bool) -> bool {
    !egui_text_agent && (matches!(tag, "INPUT" | "TEXTAREA") || editable)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    ReadFailed,
    WriteFailed,
    TooLarge,
}

impl Notice {
    pub fn message(self) -> String {
        use wordcraft_ui_egui::tl;
        match self {
            Self::ReadFailed => tl!("The browser cannot read the clipboard — use Ctrl+V").to_string(),
            Self::WriteFailed => tl!("Could not write to the clipboard").to_string(),
            Self::TooLarge => tl!("Some clipboard content is too large and was skipped").to_string(),
        }
    }
}

#[derive(Clone, Default)]
pub struct ClipboardBridge {
    inbox: Rc<RefCell<Option<PendingPaste>>>,
    current: Rc<RefCell<Option<PendingPaste>>>,
    formats: Rc<RefCell<Vec<String>>>,
    current_formats: Rc<RefCell<Vec<String>>>,
    command: Rc<RefCell<Option<String>>>,
    notices: Rc<RefCell<Vec<Notice>>>,
    ctx: egui::Context,
}

impl ClipboardBridge {
    fn notice(&self, notice: Notice) {
        let mut notices = self.notices.borrow_mut();
        if !notices.contains(&notice) {
            notices.push(notice);
        }
        self.ctx.request_repaint();
    }

    fn deliver(&self, mut paste: PendingPaste, formats: Vec<String>, command: Option<String>) {
        if paste.enforce_caps() {
            self.notice(Notice::TooLarge);
        }
        *self.inbox.borrow_mut() = Some(paste);
        *self.formats.borrow_mut() = formats.into_iter().take(MAX_FORMATS).map(|s| s.chars().take(128).collect()).collect();
        *self.command.borrow_mut() = command;
        self.ctx.request_repaint();
    }

    pub fn take(&self) -> Option<PendingPaste> {
        self.inbox.borrow_mut().take()
    }

    pub fn peek<T>(&self, read: impl FnOnce(&PendingPaste) -> T) -> Option<T> {
        self.current.borrow().as_ref().map(read)
    }

    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.notices.borrow_mut())
    }

    pub fn begin_frame(&self, raw: &mut egui::RawInput, pictures: &Inbox) -> Option<String> {
        self.current.borrow_mut().take();
        self.current_formats.borrow_mut().clear();
        let paste = self.take()?;
        *self.current_formats.borrow_mut() = std::mem::take(&mut *self.formats.borrow_mut());
        let command = self.command.borrow_mut().take();
        let empty = paste.text.is_empty();
        let has_pictures = !paste.images.is_empty();
        if command.is_none() {
            raw.events.extend(paste_events(&paste));
        }
        // Image-only browser pastes retain the existing multi-picture inbox path.
        if empty && has_pictures && command.as_deref().is_none_or(|id| id == "edit.paste") {
            let mut inbox = pictures.lock().unwrap_or_else(|e| e.into_inner());
            for (mime, bytes) in &paste.images {
                let ext = match mime.as_str() {
                    "image/jpeg" => "jpg",
                    "image/gif" => "gif",
                    "image/webp" => "webp",
                    "image/bmp" => "bmp",
                    _ => "png",
                };
                inbox.push((format!("pasted.{ext}"), bytes.clone()));
            }
        }
        *self.current.borrow_mut() = Some(paste);
        if empty && has_pictures && command.as_deref().is_none_or(|id| id == "edit.paste") {
            None
        } else {
            command.or_else(|| empty.then(|| "edit.paste".into()))
        }
    }

    pub fn connect(&self, services: &mut Services) {
        let bridge = self.clone();
        services.clipboard_text = Some(Box::new(move || bridge.peek(|p| p.text.clone())));
        let bridge = self.clone();
        services.clipboard_html = Some(Box::new(move || bridge.peek(|p| p.html.clone()).flatten()));
        let bridge = self.clone();
        services.clipboard_rtf = Some(Box::new(move || bridge.peek(|p| p.rtf.clone()).flatten()));
        let bridge = self.clone();
        services.clipboard_picture = Some(Box::new(move || {
            bridge.peek(|p| p.images.first().map(|(_, bytes)| ClipboardPicture::Encoded(bytes.clone()))).flatten()
        }));
        let bridge = self.clone();
        services.clipboard_formats = Some(Box::new(move || bridge.current_formats.borrow().clone()));
        #[cfg(target_arch = "wasm32")]
        {
            let bridge = self.clone();
            services.set_clipboard_html = Some(Box::new(move |html, text| bridge.write(html, text)));
            let bridge = self.clone();
            services.request_clipboard = Some(Box::new(move |command| {
                if bridge.current.borrow().is_some() {
                    return false;
                }
                bridge.request_read(command);
                true
            }));
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[path = "clipboard/browser.rs"]
mod browser;

#[cfg(test)]
#[path = "clipboard/tests.rs"]
mod tests;
