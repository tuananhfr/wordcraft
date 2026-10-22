//! Paste Special: paste the clipboard in a chosen format (formatted, RTF, HTML, picture,
//! unformatted), and the formatted content Paste and Merge Formatting take from other programs.
//!
//! The clipboard can hold several representations of the same content. WordCraft's own copy keeps
//! a rich fragment; the system clipboard gives text, and callers (agents, the CLI, a front end
//! with a richer clipboard) may pass `html`, `rtf` or a `picture` too. Text that is itself RTF or
//! HTML source can also be pasted as that format. Paste keeps the source formatting of another
//! program's HTML or RTF; Merge Formatting keeps only its emphasis (#687).

use std::sync::Arc;

use serde_json::{Value, json};
use wordcraft_doc::edit::Fragment;
use wordcraft_doc::{Block, Document, Paragraph};

use super::{delete_selection, sel_result};
use crate::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

/// Largest clipboard payload Paste Special accepts, in bytes.
pub const MAX_PASTE_BYTES: usize = 32 << 20;
/// Largest clipboard picture Paste Special accepts (base64), in bytes.
pub const MAX_PICTURE_BYTES: usize = 96 << 20;
pub const CLIPBOARD_TOO_LARGE: &str = "The clipboard content is too large to paste.";
pub const CLIPBOARD_UNUSABLE: &str = "The clipboard content cannot be pasted here.";
/// Nesting depth we follow when adjusting pasted tables.
const MAX_DEPTH: usize = 64;

/// The formats, in the order the dialog lists them: (id, label).
pub const FORMATS: [(&str, &str); 5] = [
    ("formatted", "Formatted Text"),
    ("rtf", "Rich Text Format (RTF)"),
    ("html", "HTML Format"),
    ("picture", "Picture"),
    ("text", "Unformatted Text"),
];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("edit.pasteSpecial", "Paste Special", "Home › Clipboard › Paste", paste_special)
            .key("Mod+Alt+V")
            .params(r#"{"as"?: "formatted"|"rtf"|"html"|"picture"|"text", "text"?: string, "html"?: string, "rtf"?: string, "picture"?: base64, "keepSource"?: bool}"#),
    ]
}

/// Clipboard text from the params (system clipboard), else WordCraft's own copy.
fn clip_text(s: &Session, v: &Value) -> Option<String> {
    match p::str(v, "text") {
        Some(t) => Some(t.replace("\r\n", "\n")),
        None => s.clipboard.as_ref().map(Fragment::plain_text),
    }
}

fn looks_like_rtf(t: &str) -> bool {
    t.trim_start().starts_with("{\\rtf")
}

fn looks_like_html(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with('<') && (t.contains("</") || t.contains("/>"))
}

/// The source for a format, if the clipboard has it.
fn source(s: &Session, v: &Value, fmt: &str) -> Option<String> {
    let non_empty = |t: String| if t.is_empty() { None } else { Some(t) };
    match fmt {
        "formatted" => {
            if let Some(html) = p::str(v, "html") {
                return super::clipboard_fragment::decode(html).map(|_| html.to_string());
            }
            if p::str(v, "rtf").is_some() {
                return None;
            }
            let f = s.clipboard.as_ref()?;
            // System clipboard text that isn't our own copy means someone else copied since.
            match p::str(v, "text") {
                Some(t) if f.clipboard_text() != t.replace("\r\n", "\n") => None,
                _ => Some(String::new()),
            }
        }
        "rtf" => p::str(v, "rtf").map(str::to_string).or_else(|| clip_text(s, v).filter(|t| looks_like_rtf(t))).and_then(non_empty),
        "html" => p::str(v, "html").map(str::to_string).or_else(|| clip_text(s, v).filter(|t| looks_like_html(t))).and_then(non_empty),
        "picture" => p::str(v, "picture").map(str::to_string).and_then(non_empty),
        "text" => clip_text(s, v).and_then(non_empty),
        _ => None,
    }
}

/// Formats the clipboard can be pasted as, best first.
pub fn available(s: &Session, v: &Value) -> Vec<&'static str> {
    FORMATS.iter().map(|(id, _)| *id).filter(|id| source(s, v, id).is_some()).collect()
}

fn formats_json(ids: &[&str]) -> Value {
    json!(ids.iter().map(|id| json!({"id": id, "label": FORMATS.iter().find(|(f, _)| f == id).map(|(_, l)| *l).unwrap_or(*id)})).collect::<Vec<_>>())
}

fn paste_special(s: &mut Session, v: &Value) -> CmdResult {
    for k in ["text", "html", "rtf"] {
        if p::str(v, k).is_some_and(|t| t.len() > MAX_PASTE_BYTES) {
            return Err(CmdError::Failed(CLIPBOARD_TOO_LARGE.into()));
        }
    }
    if p::str(v, "picture").is_some_and(|t| t.len() > MAX_PICTURE_BYTES) {
        return Err(CmdError::Failed(CLIPBOARD_TOO_LARGE.into()));
    }
    let ids = available(s, v);
    let Some(fmt) = p::str(v, "as") else {
        // No format chosen: list them and ask a front end (if any) to show the dialog.
        let mut req = json!({"open": "pasteSpecial", "formats": ids});
        for k in ["text", "html", "rtf", "picture"] {
            if let Some(t) = p::str(v, k) {
                req[k] = json!(t);
            }
        }
        s.ui_requests.push(req);
        return Ok(json!({"formats": formats_json(&ids)}));
    };
    let fmt = match fmt.to_ascii_lowercase().as_str() {
        "formatted" | "keep" | "wordcraft" => "formatted",
        "rtf" => "rtf",
        "html" => "html",
        "picture" | "png" | "image" => "picture",
        "text" | "unformatted" | "plain" => "text",
        other => return Err(CmdError::Params(format!("unknown format `{other}` (formatted, rtf, html, picture or text)"))),
    };
    let Some(src) = source(s, v, fmt) else {
        return Err(CmdError::Failed(if ids.is_empty() { "the clipboard is empty".into() } else { CLIPBOARD_UNUSABLE.into() }));
    };
    let preserve = (fmt == "html" && foreign_marker(s, &src)) || (fmt == "formatted" && foreign_source(s, v));
    let frag = match fmt {
        "text" => {
            super::paste_text(s, &src.replace("\r\n", "\r").replace('\n', "\r"))?;
            let mut r = sel_result(s)?;
            r["pastedAs"] = json!(fmt);
            return Ok(r);
        }
        "picture" => {
            let mut r = super::insert::picture(s, &json!({"data": src}))?;
            r["pastedAs"] = json!(fmt);
            return Ok(r);
        }
        "formatted" => paste_fragment(s, v)?,
        "html" => from_html(s, &src, p::bool(v, "keepSource").unwrap_or(false))?,
        _ => adopt(s, wordcraft_formats::rtf::import(src.as_bytes()).map_err(|_| CmdError::Failed(CLIPBOARD_UNUSABLE.into()))?)?,
    };
    insert_prepared(s, frag, preserve)?;
    let mut r = sel_result(s)?;
    r["pastedAs"] = json!(fmt);
    Ok(r)
}

/// The HTML itself from a Windows "HTML Format" (CF_HTML) clipboard payload — a `Version:` header
/// giving byte offsets — or the text unchanged. Hostile offsets fall back to the text after the
/// header.
pub fn html_fragment(html: &str) -> &str {
    if !html.starts_with("Version:") {
        return html;
    }
    let offset = |key: &str| -> Option<usize> {
        let at = html.find(key)?.checked_add(key.len())?;
        let digits: &str = html.get(at..)?.split(['\r', '\n']).next()?;
        digits.trim().parse().ok()
    };
    if let (Some(a), Some(b)) = (offset("StartFragment:"), offset("EndFragment:"))
        && let Some(frag) = html.get(a..b)
    {
        return frag;
    }
    html.find('<').and_then(|i| html.get(i..)).unwrap_or("")
}

/// Whether an imported document has anything to paste: text, a table, or a picture it carries.
fn has_content(d: &Document) -> bool {
    d.body.iter().any(|b| match &**b {
        Block::Para(p) => !p.text.trim().is_empty() || (!p.objects.is_empty() && !d.media.is_empty()),
        Block::Table(_) => true,
    })
}

/// Another program's formatted copy, from the system clipboard's HTML, else its RTF (#687), as a
/// fragment of this document. `None` when neither is given, either is larger than
/// [`MAX_PASTE_BYTES`], or it holds nothing to paste (the caller pastes the text then).
pub fn rich_fragment(s: &mut Session, v: &Value) -> Option<Fragment> {
    let fits = |k: &str| p::str(v, k).filter(|t| !t.trim().is_empty() && t.len() <= MAX_PASTE_BYTES);
    if let Some(html) = fits("html")
        && super::clipboard_fragment::decode(html).is_some()
    {
        return from_html(s, html, p::bool(v, "keepSource").unwrap_or(false)).ok();
    }
    let doc = fits("html").map(|html| import_html(s, html_fragment(html)));
    let doc = match doc.filter(has_content) {
        Some(d) => d,
        None => {
            let rtf = fits("rtf").filter(|t| looks_like_rtf(t))?;
            wordcraft_formats::rtf::import(rtf.as_bytes()).ok().filter(has_content)?
        }
    };
    adopt(s, doc).ok()
}

/// Whether clipboard HTML holds one picture and nothing else, a picture the HTML import cannot
/// keep (a browser's Copy Image: `<img src=URL>` beside the bitmap, which is then the picture).
pub fn single_lost_image(html: &str) -> bool {
    // Most pasted HTML is text: skip the parse unless there is exactly one `<img`.
    if html.as_bytes().windows(4).filter(|w| w.eq_ignore_ascii_case(b"<img")).take(2).count() != 1 {
        return false;
    }
    let (doc, report) = wordcraft_formats::html::import_report(html);
    report.count == 1 && report.only_images && doc.media.is_empty() && matches!(doc.body.as_slice(), [block] if matches!(&**block, Block::Para(_)))
}

fn import_html(s: &mut Session, html: &str) -> Document {
    let (doc, report) = wordcraft_formats::html::import_report(html);
    if report.external + report.too_large + report.unsupported > 0 {
        s.ui_requests.push(json!({"imagePaste": {"external": report.external, "tooLarge": report.too_large, "unsupported": report.unsupported}}));
    }
    doc
}

fn from_html(s: &mut Session, html: &str, keep_source: bool) -> Result<Fragment, CmdError> {
    if let Some(payload) = super::clipboard_fragment::decode(html) {
        if payload.is_local(s) {
            // The cached fragment also retains native-only data omitted by serde.
            if super::clipboard_fragment::is_cached(s, html)
                && let Some(f) = &s.clipboard
            {
                return Ok(f.clone());
            }
            return Ok(payload.fragment);
        }
        return super::clipboard_adopt::adopt(s, payload.into_document(), keep_source, false);
    }
    let doc = import_html(s, html_fragment(html));
    adopt(s, doc)
}

fn foreign_marker(s: &Session, html: &str) -> bool {
    super::clipboard_fragment::decode(html).is_some_and(|p| !p.is_local(s))
}

fn foreign_source(s: &Session, v: &Value) -> bool {
    if let Some(html) = p::str(v, "html") {
        return foreign_marker(s, html);
    }
    if p::str(v, "rtf").is_some() {
        return false;
    }
    let Some(f) = &s.clipboard else { return false };
    if p::str(v, "text").is_some_and(|t| f.clipboard_text() != t.replace("\r\n", "\n")) {
        return false;
    }
    super::clipboard_fragment::decode(&s.clipboard_html).is_some_and(|payload| payload.fragment == *f && !payload.is_local(s))
}

pub(crate) fn paste(s: &mut Session, v: &Value, merge: bool) -> Result<(), CmdError> {
    if let Some(picture) = p::str(v, "picture").filter(|_| p::str(v, "html").is_none() && p::str(v, "rtf").is_none()) {
        if picture.len() > MAX_PICTURE_BYTES {
            return Err(CmdError::Failed(CLIPBOARD_TOO_LARGE.into()));
        }
        return super::insert::picture(s, &json!({"data": picture})).map(|_| ());
    }
    let preserve = !merge && foreign_source(s, v);
    let mut frag = paste_fragment(s, v)?;
    if merge {
        merge_formatting(s, &mut frag);
    }
    insert_prepared(s, frag, preserve)
}

/// What Paste inserts: WordCraft's own copy while the system clipboard still holds it, else
/// another program's formatted copy (HTML, RTF), else the system clipboard's text.
pub fn paste_fragment(s: &mut Session, v: &Value) -> Result<Fragment, CmdError> {
    if let Some(html) = p::str(v, "html")
        && super::clipboard_fragment::decode(html).is_some()
    {
        return from_html(s, html, p::bool(v, "keepSource").unwrap_or(false));
    }
    let text = p::str(v, "text").map(|t| t.replace("\r\n", "\n"));
    if let Some(f) = &s.clipboard
        && p::str(v, "html").is_none()
        && p::str(v, "rtf").is_none()
        && text.as_ref().is_none_or(|t| f.clipboard_text() == *t)
    {
        if let Some(payload) = super::clipboard_fragment::decode(&s.clipboard_html)
            && payload.fragment == *f
            && !payload.is_local(s)
        {
            return super::clipboard_adopt::adopt(s, payload.into_document(), false, false);
        }
        return Ok(f.clone());
    }
    if let Some(f) = rich_fragment(s, v) {
        return Ok(f);
    }
    match text {
        Some(t) if t.len() > MAX_PASTE_BYTES => Err(CmdError::Failed(CLIPBOARD_TOO_LARGE.into())),
        Some(t) if !t.is_empty() => Ok(Fragment::from_text(&t)),
        _ if ["html", "rtf"].iter().any(|k| p::str(v, k).is_some_and(|t| t.len() > MAX_PASTE_BYTES)) => {
            Err(CmdError::Failed(CLIPBOARD_TOO_LARGE.into()))
        }
        _ if ["html", "rtf"].iter().any(|k| p::str(v, k).is_some_and(|t| !t.trim().is_empty())) => Err(CmdError::Failed(CLIPBOARD_UNUSABLE.into())),
        _ => Err(CmdError::Failed("the clipboard is empty".into())),
    }
}

fn insert_prepared(s: &mut Session, frag: Fragment, preserve: bool) -> Result<(), CmdError> {
    let (a, b) = s.sel.ordered();
    let props = if preserve && frag.blocks.len() == 1 {
        match (frag.blocks.first(), s.doc.para_at(&a)) {
            (Some(Block::Para(src)), Some(dst)) if a.path == b.path && b.off.saturating_sub(a.off) == dst.len() => Some(src.props.clone()),
            _ => None,
        }
    } else {
        None
    };
    insert(s, frag)?;
    // A foreign whole paragraph in an empty target must retain its list and paragraph style.
    if let Some(props) = props {
        let target = s.doc.para_mut(a.story, &a.path)?;
        target.props = props;
        target.touch();
    }
    Ok(())
}

/// Merge Formatting (#687): the pasted text takes the formatting where it lands — the paragraph's
/// and the caret's — keeping only its emphasis (bold, italic, underline, strikethrough).
pub fn merge_formatting(s: &Session, frag: &mut Fragment) {
    let mut base = s.typing_props();
    base.ins = None;
    base.del = None;
    base.fmt_change = None;
    let (start, _) = s.sel.ordered();
    let para = s.doc.para_at(&start).map(|p| p.props.clone()).unwrap_or_default();
    for b in &mut frag.blocks {
        each_para(b, 0, &mut |p| {
            p.props = para.clone();
            for run in &mut p.runs {
                let src = std::mem::replace(&mut run.props, base.clone());
                if src.bold == Some(true) {
                    run.props.bold = Some(true);
                }
                if src.italic == Some(true) {
                    run.props.italic = Some(true);
                }
                if src.underline.is_some_and(|u| u != wordcraft_doc::props::Underline::None) {
                    run.props.underline = src.underline;
                }
                if src.strike == Some(true) {
                    run.props.strike = Some(true);
                }
                run.props.link = src.link;
            }
            p.touch();
        });
    }
}

/// Replace the selection with `frag`, marking it as an insertion under Track Changes.
pub fn insert(s: &mut Session, mut frag: Fragment) -> Result<(), CmdError> {
    super::controls::prepare_edit(s)?;
    let at = delete_selection(s)?;
    let track = s.doc.settings.track_changes;
    let mut rid = None;
    if track {
        // What the last command cut, pasted again: the two ends of a move.
        let r = match super::moves::take_paste(s, &frag) {
            Some(r) => r,
            None => super::new_revision(s, wordcraft_doc::RevisionKind::Insert),
        };
        rid = Some(r);
        for b in &mut frag.blocks {
            each_para(b, 0, &mut |p| {
                for run in &mut p.runs {
                    run.props.ins = Some(r);
                    run.props.del = None;
                    run.props.fmt_change = None;
                }
                p.mark.ins = None;
                p.mark.del = None;
            });
        }
    }
    let end = s.doc.insert_fragment(&at, &frag)?;
    // The paragraph marks the paste added are inserted too, so rejecting it joins the
    // paragraphs again. The last paragraph ends with the original mark and keeps it.
    if let Some(r) = rid
        && at.story == end.story
        && at.path.parent() == end.path.parent()
    {
        for i in at.path.last()..end.path.last() {
            if let Ok(p) = s.doc.para_mut(at.story, &at.path.with_last(i)) {
                p.mark.ins = Some(r);
                p.mark.del = None;
                p.touch();
            }
        }
    }
    s.sel = Selection::caret(end);
    Ok(())
}

/// Every paragraph in a block, tables included (bounded depth).
pub(crate) fn each_para(b: &mut Block, depth: usize, f: &mut dyn FnMut(&mut Paragraph)) {
    if depth > MAX_DEPTH {
        return;
    }
    match b {
        Block::Para(p) => f(p),
        Block::Table(t) => {
            for row in &mut t.rows {
                for cell in &mut row.cells {
                    for cb in &mut cell.blocks {
                        each_para(Arc::make_mut(cb), depth + 1, f);
                    }
                }
            }
        }
    }
}

/// Adopt external HTML and RTF while retaining their existing picture fitting behavior.
fn adopt(s: &mut Session, src: Document) -> Result<Fragment, CmdError> {
    super::clipboard_adopt::adopt(s, src, false, true)
}
