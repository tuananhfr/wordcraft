//! Bounded, self-contained rich clipboard data shared by native and web hosts.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use wordcraft_doc::edit::Fragment;
use wordcraft_doc::numbering::Numbering;
use wordcraft_doc::styles::StyleSheet;
use wordcraft_doc::{Block, Document, InlineObject};

use crate::Session;

/// The encoded attribute, including base64 expansion, must fit in 4 MiB.
pub(crate) const MAX_MARKER_BYTES: usize = 4 << 20;
const MAX_JSON_BYTES: usize = MAX_MARKER_BYTES / 4 * 3;
const SMALL_IMAGE_BYTES: usize = 256 << 10;
const MAX_CELLS: usize = 20_000;

#[derive(Serialize, Deserialize)]
pub(crate) struct ClipPayload {
    v: u8,
    source_instance: String,
    source_document: u64,
    pub(crate) fragment: Fragment,
    pub(crate) styles: StyleSheet,
    pub(crate) numbering: Numbering,
    pub(crate) media: BTreeMap<String, Arc<Vec<u8>>>,
}

impl ClipPayload {
    fn capture(s: &Session, fragment: &Fragment) -> Self {
        let mut styles = BTreeSet::from(["Normal".to_string()]);
        let mut nums = BTreeSet::new();
        let mut media = BTreeSet::new();
        let mut collect = |b: &Block| match b {
            Block::Para(p) => {
                styles.extend(p.props.style.iter().cloned());
                styles.extend(p.mark.style.iter().cloned());
                styles.extend(p.runs.iter().filter_map(|r| r.props.style.clone()));
                nums.extend(p.props.numbering.iter().map(|n| n.num));
                for o in &p.objects {
                    image_keys(o, 0, &mut media);
                }
            }
            Block::Table(t) => styles.extend(t.props.style.iter().cloned()),
        };
        for b in &fragment.blocks {
            visit(b, 0, &mut collect);
        }
        for part in fragment.parts.values() {
            for b in &part.blocks {
                visit(b, 0, &mut collect);
            }
        }
        let mut abstracts = BTreeSet::new();
        loop {
            let before = (styles.len(), nums.len(), abstracts.len());
            for st in &s.doc.styles.styles {
                if styles.contains(&st.id) {
                    styles.extend(st.based_on.iter().chain(&st.next).chain(&st.linked).cloned());
                    styles.extend(st.para.style.iter().chain(&st.chr.style).cloned());
                    nums.extend(st.para.numbering.iter().map(|n| n.num));
                    if let Some(table) = &st.table {
                        for props in
                            [&table.header_chr, &table.band_chr, &table.first_col_chr, &table.total_chr, &table.last_col_chr, &table.col_band_chr]
                        {
                            styles.extend(props.style.iter().cloned());
                        }
                    }
                }
            }
            for n in &s.doc.numbering.nums {
                if nums.contains(&n.id) {
                    abstracts.insert(n.abstract_id);
                    for (_, level) in &n.level_overrides {
                        styles.extend(level.style.iter().chain(&level.chr.style).cloned());
                    }
                }
            }
            for a in &s.doc.numbering.abstracts {
                if abstracts.contains(&a.id) {
                    for level in &a.levels {
                        styles.extend(level.style.iter().chain(&level.chr.style).cloned());
                    }
                }
            }
            if before == (styles.len(), nums.len(), abstracts.len()) {
                break;
            }
        }
        let mut sheet = s.doc.styles.clone();
        sheet.styles.retain(|st| styles.contains(&st.id));
        Self {
            v: 1,
            source_instance: instance_id().to_string(),
            source_document: s.document_id(),
            fragment: fragment.clone(),
            styles: sheet,
            numbering: Numbering {
                abstracts: s.doc.numbering.abstracts.iter().filter(|a| abstracts.contains(&a.id)).cloned().collect(),
                nums: s.doc.numbering.nums.iter().filter(|n| nums.contains(&n.id)).cloned().collect(),
            },
            media: s.doc.media.iter().filter(|(key, _)| media.contains(*key)).map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }

    pub(crate) fn into_document(self) -> Document {
        let mut doc = self.fragment.to_document();
        doc.parts = self.fragment.parts;
        doc.styles = self.styles;
        doc.numbering = self.numbering;
        doc.media = self.media;
        doc
    }

    pub(crate) fn is_local(&self, s: &Session) -> bool {
        self.source_instance == instance_id() && self.source_document == s.document_id()
    }
}

fn instance_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        // Session counters restart in each tab; the instance tag only distinguishes origins.
        #[cfg(target_arch = "wasm32")]
        {
            format!("{:x}-{:x}", js_sys::Date::now().to_bits(), js_sys::Math::random().to_bits())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos())
        }
    })
}

pub(crate) fn is_cached(s: &Session, html: &str) -> bool {
    marker(html).is_some_and(|m| marker(&s.clipboard_html) == Some(m))
}

fn visit(b: &Block, depth: usize, f: &mut dyn FnMut(&Block)) {
    f(b);
    if depth < 16
        && let Block::Table(t) = b
    {
        for b in t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks) {
            visit(b, depth + 1, f);
        }
    }
}

fn image_keys(o: &InlineObject, depth: usize, keys: &mut BTreeSet<String>) {
    match o {
        InlineObject::Image { media, .. } => {
            keys.insert(media.clone());
        }
        InlineObject::Group { children, .. } if depth < 16 => {
            for child in children {
                image_keys(&child.obj, depth + 1, keys);
            }
        }
        InlineObject::Graphic { graphic, .. } => {
            for item in &graphic.items {
                if let wordcraft_doc::graphic::GraphicItem::Image { media, .. } = item {
                    keys.insert(media.clone());
                }
            }
        }
        _ => {}
    }
}

struct CappedJson(Vec<u8>);

impl Write for CappedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_JSON_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("clipboard marker size limit"));
        }
        self.0.try_reserve(bytes.len()).map_err(io::Error::other)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn export(s: &Session, fragment: &Fragment) -> String {
    let payload = ClipPayload::capture(s, fragment);
    let mut bytes = CappedJson(Vec::new());
    let encoded = serde_json::to_writer(&mut bytes, &payload).ok().map(|()| super::insert::base64_encode(&bytes.0));
    let mut doc = payload.into_document();
    // Large pictures remain in the internal marker; HTML stays useful to external programs.
    doc.media.retain(|_, data| data.len() <= SMALL_IMAGE_BYTES);
    let mut html = wordcraft_formats::html::export_fragment(&doc);
    if let Some(encoded) = encoded.filter(|s| s.len() <= MAX_MARKER_BYTES) {
        html.push_str("<meta name=\"erpcons-clip\" content=\"");
        html.push_str(&encoded);
        html.push_str("\">");
    }
    html
}

fn marker(html: &str) -> Option<&str> {
    // Only the exact inert tag our exporter writes is accepted; other HTML is imported normally.
    let prefix = "<meta name=\"erpcons-clip\" content=\"";
    let rest = html.get(html.find(prefix)?.checked_add(prefix.len())?..)?;
    let end = rest.find("\">")?;
    (end <= MAX_MARKER_BYTES).then(|| rest.get(..end)).flatten()
}

pub(crate) fn decode(html: &str) -> Option<ClipPayload> {
    if html.len() > super::paste::MAX_PASTE_BYTES {
        return None;
    }
    let encoded = marker(html)?;
    let bytes = super::insert::base64_decode(encoded)?;
    if bytes.len() > MAX_JSON_BYTES || super::insert::base64_encode(&bytes) != encoded {
        return None;
    }
    // serde_json keeps its recursion limit enabled before model validation runs.
    let payload: ClipPayload = serde_json::from_slice(&bytes).ok()?;
    let mut cells = 0;
    if payload.v != 1
        || payload.fragment.blocks.is_empty()
        || !valid_resources(&payload)
        || !payload.fragment.blocks.iter().all(|b| valid_block(b, 0, &mut cells))
        || !payload
            .fragment
            .parts
            .values()
            .all(|p| p.kind == wordcraft_doc::PartKind::TextBox && p.blocks.iter().all(|b| valid_block(b, 0, &mut cells)))
    {
        return None;
    }
    Some(payload)
}

fn valid_block(b: &Block, depth: usize, cells: &mut usize) -> bool {
    if depth > 16 {
        return false;
    }
    match b {
        Block::Para(p) => {
            let mut end = 0usize;
            for run in &p.runs {
                let Some(next) = end.checked_add(run.len) else { return false };
                if next > p.text.len() || !p.text.is_char_boundary(next) || !valid_chr(&run.props) {
                    return false;
                }
                end = next;
            }
            end == p.text.len()
                && p.text.matches(wordcraft_doc::para::OBJ).count() == p.objects.len()
                && valid_chr(&p.mark)
                && p.objects.iter().all(|o| valid_object(o, 0))
                && p.props.numbering.as_ref().is_none_or(|n| n.level < 9)
        }
        Block::Table(t) => {
            for cell in t.rows.iter().flat_map(|r| &r.cells) {
                *cells = cells.saturating_add(1);
                if *cells > MAX_CELLS
                    || cell.blocks.is_empty()
                    || cell.props.span > wordcraft_doc::table::MAX_COLS as u32
                    || !cell.blocks.iter().all(|b| valid_block(b, depth + 1, cells))
                {
                    return false;
                }
            }
            !t.rows.is_empty() && t.rows.iter().all(|r| !r.cells.is_empty()) && t.grid.len() <= wordcraft_doc::table::MAX_COLS
        }
    }
}

fn valid_chr(props: &wordcraft_doc::props::CharProps) -> bool {
    props.link.as_deref().is_none_or(wordcraft_doc::link::is_safe_href)
}

fn valid_object(o: &InlineObject, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    match o {
        InlineObject::Image { w, h, .. } | InlineObject::Graphic { w, h, .. } | InlineObject::Shape { w, h, .. } => {
            w.is_finite() && h.is_finite() && (0.0..=1_000_000.0).contains(w) && (0.0..=1_000_000.0).contains(h)
        }
        InlineObject::Group { children, .. } => {
            children.len() <= wordcraft_doc::para::MAX_GROUP_CHILDREN && children.iter().all(|c| valid_object(&c.obj, depth + 1))
        }
        _ => true,
    }
}

fn valid_resources(payload: &ClipPayload) -> bool {
    let styles: BTreeSet<_> = payload.styles.styles.iter().map(|st| &st.id).collect();
    let abstracts: BTreeSet<_> = payload.numbering.abstracts.iter().map(|a| a.id).collect();
    let nums: BTreeSet<_> = payload.numbering.nums.iter().map(|n| n.id).collect();
    if styles.len() != payload.styles.styles.len()
        || abstracts.len() != payload.numbering.abstracts.len()
        || nums.len() != payload.numbering.nums.len()
        || !payload.numbering.nums.iter().all(|n| n.id != 0 && abstracts.contains(&n.abstract_id))
        || !payload.numbering.abstracts.iter().all(|a| a.levels.len() <= 9)
        || !valid_chr(&payload.styles.default_chr)
        || !payload.styles.styles.iter().all(|st| {
            valid_chr(&st.chr)
                && st.para.numbering.as_ref().is_none_or(|n| n.num == 0 || nums.contains(&n.num))
                && st.table.as_ref().is_none_or(|t| {
                    [&t.header_chr, &t.band_chr, &t.first_col_chr, &t.total_chr, &t.last_col_chr, &t.col_band_chr].iter().all(|c| valid_chr(c))
                })
        })
        || !payload.numbering.abstracts.iter().flat_map(|a| &a.levels).all(|l| valid_chr(&l.chr))
        || !payload
            .numbering
            .nums
            .iter()
            .all(|n| n.level_overrides.iter().all(|(i, l)| *i < 9 && valid_chr(&l.chr)) && n.start_overrides.iter().all(|(i, _)| *i < 9))
        || payload.fragment.parts.len() > 4096
    {
        return false;
    }
    let mut valid = true;
    let mut check = |b: &Block| {
        if let Block::Para(p) = b {
            valid &= p.props.numbering.as_ref().is_none_or(|n| n.num == 0 || nums.contains(&n.num));
            let mut keys = BTreeSet::new();
            for o in &p.objects {
                image_keys(o, 0, &mut keys);
            }
            valid &= keys.iter().all(|k| payload.media.contains_key(k));
        }
    };
    for b in &payload.fragment.blocks {
        visit(b, 0, &mut check);
    }
    for part in payload.fragment.parts.values() {
        for b in &part.blocks {
            visit(b, 0, &mut check);
        }
    }
    valid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> ClipPayload {
        let session = Session::new(Document::from_text("hello"));
        ClipPayload::capture(&session, &session.doc.body_fragment())
    }

    fn html(payload: &ClipPayload) -> String {
        format!(
            "<meta name=\"erpcons-clip\" content=\"{}\"><p>fallback</p>",
            super::super::insert::base64_encode(&serde_json::to_vec(payload).unwrap())
        )
    }

    #[test]
    fn marker_version_and_base64_are_validated() {
        let mut p = payload();
        assert!(decode(&html(&p)).is_some());
        p.v = 2;
        assert!(decode(&html(&p)).is_none());
        assert!(decode("<meta name=\"erpcons-clip\" content=\"e30=garbage\">").is_none());
        assert!(decode(&format!("<meta name=\"erpcons-clip\" content=\"{}\">", "A".repeat(MAX_MARKER_BYTES + 1))).is_none());
    }

    #[test]
    fn hostile_run_lengths_and_utf8_boundaries_are_rejected() {
        for len in [1, usize::MAX] {
            let mut p = payload();
            let Block::Para(para) = p.fragment.blocks.first_mut().unwrap() else { panic!("missing paragraph") };
            para.text = "ệ".into();
            para.runs.first_mut().unwrap().len = len;
            assert!(decode(&html(&p)).is_none());
        }
    }

    #[test]
    fn malformed_objects_tables_and_resource_ids_are_rejected() {
        let mut p = payload();
        let Block::Para(para) = p.fragment.blocks.first_mut().unwrap() else { panic!("missing paragraph") };
        para.props.numbering = Some(wordcraft_doc::props::NumRef { num: 999, level: 0 });
        assert!(decode(&html(&p)).is_none());
        let mut p = payload();
        let Block::Para(para) = p.fragment.blocks.first_mut().unwrap() else { panic!("missing paragraph") };
        para.text.push(wordcraft_doc::para::OBJ);
        para.runs.first_mut().unwrap().len = para.text.len();
        assert!(decode(&html(&p)).is_none());
        let mut p = payload();
        let mut table = wordcraft_doc::Table::new(1, 1, 100.0);
        table.rows[0].cells[0].props.span = u32::MAX;
        p.fragment.blocks = vec![Block::Table(table)];
        assert!(decode(&html(&p)).is_none());
        let mut p = payload();
        p.styles.styles.push(p.styles.styles[0].clone());
        assert!(decode(&html(&p)).is_none());
    }

    #[test]
    fn active_link_schemes_in_markers_are_rejected() {
        for link in ["javascript:alert(1)", "DATA:text/html,script", "java\nscript:alert(1)"] {
            let mut p = payload();
            let Block::Para(para) = p.fragment.blocks.first_mut().unwrap() else { panic!("missing paragraph") };
            para.runs.first_mut().unwrap().props.link = Some(link.into());
            assert!(decode(&html(&p)).is_none());
        }
    }
}
