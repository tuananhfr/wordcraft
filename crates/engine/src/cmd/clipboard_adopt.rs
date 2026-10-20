//! Import resource tables without changing definitions already used by the destination.

use std::collections::BTreeMap;
use std::sync::Arc;

use wordcraft_doc::edit::Fragment;
use wordcraft_doc::numbering::Level;
use wordcraft_doc::props::{CharProps, ParaProps};
use wordcraft_doc::{Block, Document, InlineObject};

use crate::{CmdError, Session};

type StyleMap = BTreeMap<String, String>;
type NumMap = BTreeMap<u32, u32>;

fn next_ids(ids: impl Iterator<Item = u32>, count: usize, first: u32) -> Result<u32, CmdError> {
    if count == 0 {
        return Ok(first);
    }
    let next = ids.max().map_or(Some(first), |n| n.checked_add(1));
    next.filter(|n| u32::try_from(count).ok().and_then(|len| n.checked_add(len)).is_some())
        .ok_or_else(|| CmdError::Failed(super::paste::CLIPBOARD_UNUSABLE.into()))
}

fn style(id: &mut Option<String>, map: &StyleMap) {
    if let Some(to) = id.as_ref().and_then(|id| map.get(id)) {
        *id = Some(to.clone());
    }
}

fn chr(props: &mut CharProps, styles: &StyleMap) {
    style(&mut props.style, styles);
}

fn para(props: &mut ParaProps, styles: &StyleMap, nums: &NumMap) {
    style(&mut props.style, styles);
    if let Some(n) = &mut props.numbering
        && n.num != 0
    {
        n.num = nums.get(&n.num).copied().unwrap_or(0);
    }
}

fn level(l: &mut Level, styles: &StyleMap) {
    style(&mut l.style, styles);
    chr(&mut l.chr, styles);
}

fn object(o: &mut InlineObject, media: &BTreeMap<String, String>, depth: usize) {
    match o {
        InlineObject::Image { media: key, .. } => {
            if let Some(to) = media.get(key) {
                *key = to.clone();
            }
        }
        InlineObject::Group { children, .. } if depth < 16 => {
            for child in children {
                object(&mut child.obj, media, depth + 1);
            }
        }
        InlineObject::Graphic { graphic, .. } => {
            for item in &mut Arc::make_mut(graphic).items {
                if let wordcraft_doc::graphic::GraphicItem::Image { media: key, .. } = item
                    && let Some(to) = media.get(key)
                {
                    *key = to.clone();
                }
            }
        }
        _ => {}
    }
}

fn block(b: &mut Block, styles: &StyleMap, nums: &NumMap, media: &BTreeMap<String, String>, depth: usize) {
    match b {
        Block::Para(p) => {
            para(&mut p.props, styles, nums);
            chr(&mut p.mark, styles);
            for r in &mut p.runs {
                chr(&mut r.props, styles);
            }
            for o in &mut p.objects {
                object(o, media, 0);
            }
            p.touch();
        }
        Block::Table(t) if depth < 16 => {
            style(&mut t.props.style, styles);
            for b in t.rows.iter_mut().flat_map(|r| &mut r.cells).flat_map(|c| &mut c.blocks) {
                block(Arc::make_mut(b), styles, nums, media, depth + 1);
            }
        }
        _ => {}
    }
}

pub(crate) fn adopt(s: &mut Session, mut src: Document, keep_source: bool, fit_pictures: bool) -> Result<Fragment, CmdError> {
    let next_abs = next_ids(s.doc.numbering.abstracts.iter().map(|a| a.id), src.numbering.abstracts.len(), 0)?;
    let next_num = next_ids(s.doc.numbering.nums.iter().map(|n| n.id), src.numbering.nums.len(), 1)?;
    let abs: NumMap = src.numbering.abstracts.iter().enumerate().map(|(i, a)| (a.id, next_abs + i as u32)).collect();
    let nums: NumMap = src.numbering.nums.iter().enumerate().map(|(i, n)| (n.id, next_num + i as u32)).collect();
    let mut styles = StyleMap::new();
    for st in &src.styles.styles {
        let existing =
            s.doc.styles.get(&st.id).or_else(|| s.doc.styles.styles.iter().find(|d| d.kind == st.kind && d.name.eq_ignore_ascii_case(&st.name)));
        let id = match existing {
            Some(d) if !keep_source || d == st => d.id.clone(),
            Some(_) => {
                let mut index = 1usize;
                loop {
                    let candidate = format!("{}Clipboard{index}", st.id);
                    if s.doc.styles.get(&candidate).is_none()
                        && !src.styles.styles.iter().any(|st| st.id == candidate)
                        && !styles.values().any(|id| *id == candidate)
                    {
                        break candidate;
                    }
                    index = index.saturating_add(1);
                }
            }
            None => st.id.clone(),
        };
        styles.insert(st.id.clone(), id);
    }
    for mut st in src.styles.styles {
        let Some(id) = styles.get(&st.id) else { continue };
        if s.doc.styles.get(id).is_some() {
            continue;
        }
        st.id = id.clone();
        style(&mut st.based_on, &styles);
        style(&mut st.next, &styles);
        style(&mut st.linked, &styles);
        para(&mut st.para, &styles, &nums);
        chr(&mut st.chr, &styles);
        if let Some(table) = &mut st.table {
            for props in [
                &mut table.header_chr,
                &mut table.band_chr,
                &mut table.first_col_chr,
                &mut table.total_chr,
                &mut table.last_col_chr,
                &mut table.col_band_chr,
            ] {
                chr(props, &styles);
            }
        }
        s.doc.styles.upsert(st);
    }
    for mut a in src.numbering.abstracts {
        if let Some(id) = abs.get(&a.id) {
            a.id = *id;
        }
        for l in &mut a.levels {
            level(l, &styles);
        }
        s.doc.numbering.abstracts.push(a);
    }
    for mut n in src.numbering.nums {
        if let Some(id) = nums.get(&n.id) {
            n.id = *id;
        }
        n.abstract_id = abs.get(&n.abstract_id).copied().unwrap_or(0);
        for (_, l) in &mut n.level_overrides {
            level(l, &styles);
        }
        s.doc.numbering.nums.push(n);
    }
    let mut media = BTreeMap::new();
    for (key, data) in src.media {
        let ext = key
            .rsplit_once('.')
            .map(|(_, e)| e)
            .filter(|e| e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
            .unwrap_or("png")
            .to_string();
        let new_key = if fit_pictures {
            s.doc.add_media(data.as_ref().clone(), &ext)
        } else {
            let mut index = s.doc.media.len();
            loop {
                let candidate = format!("clipboardImage{index}.{ext}");
                if !s.doc.media.contains_key(&candidate) {
                    s.doc.media.insert(candidate.clone(), data.clone());
                    break candidate;
                }
                index = index.checked_add(1).ok_or_else(|| CmdError::Failed(super::paste::CLIPBOARD_UNUSABLE.into()))?;
            }
        };
        media.insert(key, new_key);
    }
    let mut blocks: Vec<Block> = src.body.iter().map(|b| (**b).clone()).collect();
    if fit_pictures {
        let max_w = crate::io::text_width(&s.doc);
        for b in &mut blocks {
            crate::io::fit_block_pictures(b, max_w);
        }
    }
    for b in &mut blocks {
        block(b, &styles, &nums, &media, 0);
    }
    for part in src.parts.values_mut() {
        for b in &mut part.blocks {
            block(Arc::make_mut(b), &styles, &nums, &media, 0);
        }
    }
    let parts = src.parts.into_iter().filter(|(_, p)| p.kind == wordcraft_doc::PartKind::TextBox).collect();
    Ok(Fragment { blocks, parts })
}
