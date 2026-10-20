//! Selection and document scope for Vietnamese legacy encoding conversion.

use serde_json::{Value, json};
use wordcraft_doc::para::Run;
use wordcraft_doc::props::CharProps;
use wordcraft_doc::{Paragraph, Path, Pos, StoryRef};

use crate::vn_legacy::{tcvn3_to_unicode, tcvn3_uppercase_to_unicode, vni_to_unicode};
use crate::{CmdError, CmdResult, CommandSpec, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tools.convert_vietnamese_legacy", "Convert TCVN3/VNI → Unicode", "Review › Language", automatic),
        CommandSpec::new("tools.convert_vietnamese_legacy_tcvn3", "Convert TCVN3 → Unicode", "Review › Language", tcvn3).when(selection_required),
        CommandSpec::new("tools.convert_vietnamese_legacy_vni", "Convert VNI → Unicode", "Review › Language", vni).when(selection_required),
    ]
}

fn selection_required(s: &Session) -> Option<&'static str> {
    s.sel.is_collapsed().then_some("Select text to choose a legacy encoding.")
}

#[derive(Clone, Copy)]
enum Encoding {
    Tcvn3,
    Tcvn3Upper,
    Vni,
}

impl Encoding {
    fn convert(self, text: &str) -> String {
        match self {
            Self::Tcvn3 => tcvn3_to_unicode(text),
            Self::Tcvn3Upper => tcvn3_uppercase_to_unicode(text),
            Self::Vni => vni_to_unicode(text),
        }
    }
}

fn encoding(font: &str) -> Option<Encoding> {
    let font = font.trim().to_ascii_lowercase();
    if font.starts_with("vni-") || font.starts_with("vni ") {
        Some(Encoding::Vni)
    } else if font.starts_with(".vn") && font.ends_with('h') {
        Some(Encoding::Tcvn3Upper)
    } else if font.starts_with(".vn") || font.starts_with("tcvn3") || font == "abc" || font.starts_with("abc-") || font.starts_with("abc ") {
        Some(Encoding::Tcvn3)
    } else {
        None
    }
}

fn automatic(s: &mut Session, _: &Value) -> CmdResult {
    convert(s, None)
}

fn tcvn3(s: &mut Session, _: &Value) -> CmdResult {
    convert(s, Some(Encoding::Tcvn3))
}

fn vni(s: &mut Session, _: &Value) -> CmdResult {
    convert(s, Some(Encoding::Vni))
}

fn body_font(s: &Session) -> String {
    let mut props = s.doc.styles.default_chr.clone();
    for style in s.doc.styles.chain("Normal") {
        props.overlay(&style.chr);
    }
    props.font.filter(|f| !f.trim().is_empty() && encoding(f).is_none()).unwrap_or_else(|| "Times New Roman".into())
}

struct Change {
    start: usize,
    end: usize,
    source: String,
    text: String,
    encoding: Encoding,
}

impl Change {
    fn map(&self, offset: usize) -> usize {
        if offset <= self.start {
            offset
        } else if offset >= self.end {
            offset.saturating_sub(self.end - self.start).saturating_add(self.text.len())
        } else {
            let prefix = self.source.get(..offset - self.start).unwrap_or("");
            let mut mapped = self.encoding.convert(prefix).len().min(self.text.len());
            // A caret between a VNI base and mark belongs after the resulting Unicode letter.
            while !self.text.is_char_boundary(mapped) && mapped < self.text.len() {
                mapped = mapped.saturating_add(1);
            }
            self.start.saturating_add(mapped)
        }
    }
}

fn remap(pos: &mut Pos, story: StoryRef, path: &Path, changes: &[Change]) {
    if pos.story == story && pos.path == *path {
        for change in changes.iter().rev() {
            pos.off = change.map(pos.off);
        }
    }
}

fn convert(s: &mut Session, forced: Option<Encoding>) -> CmdResult {
    let selected = !s.sel.is_collapsed() || s.column_segments().is_some();
    let font = body_font(s);
    let ranges = if let Some(segments) = s.column_segments() {
        segments.to_vec()
    } else if selected {
        vec![s.sel.ordered()]
    } else {
        std::iter::once(StoryRef::Body)
            .chain(s.doc.parts.keys().copied().map(StoryRef::Part))
            .map(|story| (s.doc.start_of(story), s.doc.end_of(story)))
            .collect()
    };
    let mut converted_runs = 0usize;
    for (a, b) in ranges.into_iter().rev() {
        let (a, b) = (s.doc.clamp(&a), s.doc.clamp(&b));
        if a.story != b.story {
            continue;
        }
        for path in s.doc.para_paths_in(&a, &b).into_iter().rev() {
            let Some(para) = s.doc.para(a.story, &path) else { continue };
            let from = if path == a.path { a.off } else { 0 };
            let to = if path == b.path { b.off } else { para.len() };
            let mut changes = Vec::new();
            let mut text = String::new();
            let mut runs = Vec::new();
            let mut start = 0usize;
            for run in &para.runs {
                let end = start.saturating_add(run.len).min(para.len());
                let Some(source) = para.text.get(start..end) else {
                    return Err(CmdError::Failed("The document contains invalid text formatting.".into()));
                };
                let resolved = s.doc.styles.resolve_char(para.props.style.as_deref(), &run.props);
                let detected = encoding(&resolved.font);
                let choice = detected.or(if selected { forced } else { None });
                let lo = from.max(start).min(end);
                let hi = to.min(end).max(lo);
                if let Some(choice) = choice.filter(|_| lo < hi) {
                    let before = source.get(..lo - start).ok_or_else(invalid_boundary)?;
                    let part = source.get(lo - start..hi - start).ok_or_else(invalid_boundary)?;
                    let after = source.get(hi - start..).ok_or_else(invalid_boundary)?;
                    let converted = choice.convert(part);
                    append(&mut text, &mut runs, before, &run.props);
                    let props = CharProps { font: Some(font.clone()), ..run.props.clone() };
                    append(&mut text, &mut runs, &converted, &props);
                    append(&mut text, &mut runs, after, &run.props);
                    changes.push(Change { start: lo, end: hi, source: part.into(), text: converted, encoding: choice });
                } else {
                    append(&mut text, &mut runs, source, &run.props);
                }
                start = end;
            }
            if changes.is_empty() {
                continue;
            }
            if start != para.len() {
                return Err(CmdError::Failed("The document contains invalid text formatting.".into()));
            }
            let mut replacement: Paragraph = para.clone();
            replacement.text = text;
            replacement.runs = runs;
            if to == para.len() && encoding(&s.doc.styles.resolve_char(para.props.style.as_deref(), &para.mark).font).is_some() {
                replacement.mark.font = Some(font.clone());
            }
            replacement.normalize();
            replacement.touch();
            *s.doc.para_mut(a.story, &path)? = replacement;
            converted_runs = converted_runs.saturating_add(changes.len());
            remap(&mut s.sel.anchor, a.story, &path, &changes);
            remap(&mut s.sel.focus, a.story, &path, &changes);
            if let Some(column) = s.column.as_mut() {
                remap(&mut column.sel.anchor, a.story, &path, &changes);
                remap(&mut column.sel.focus, a.story, &path, &changes);
                for (start, end) in &mut column.segments {
                    remap(start, a.story, &path, &changes);
                    remap(end, a.story, &path, &changes);
                }
            }
        }
    }
    s.pending = None;
    Ok(json!({"convertedRuns": converted_runs}))
}

fn invalid_boundary() -> CmdError {
    CmdError::Failed("The document contains invalid text formatting.".into())
}

fn append(text: &mut String, runs: &mut Vec<Run>, part: &str, props: &CharProps) {
    if !part.is_empty() {
        text.push_str(part);
        runs.push(Run { len: part.len(), props: props.clone() });
    }
}

#[cfg(test)]
#[path = "vn_legacy_tests.rs"]
mod tests;
