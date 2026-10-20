//! Cell formatting and column width hints from allow-listed HTML.

use super::*;
use wordcraft_doc::props::{Border, BorderStyle, Borders, VAlign};

fn border(value: &str) -> Option<Border> {
    let value = value.to_ascii_lowercase();
    let mut out = Border { style: BorderStyle::Single, width: 0.75, color: None, space: 0.0 };
    let mut known = false;
    if let Some(start) = value.find("rgb") && let Some(end) = value.get(start..).and_then(|v| v.find(')')) {
        out.color = value.get(start..start + end + 1).and_then(parse_color);
        known = out.color.is_some();
    }
    for token in value.split_whitespace() {
        match token {
            "none" | "hidden" => return Some(Border { style: BorderStyle::None, ..out }),
            "solid" => { out.style = BorderStyle::Single; known = true; }
            "double" => { out.style = BorderStyle::Double; known = true; }
            "dotted" => { out.style = BorderStyle::Dotted; known = true; }
            "dashed" => { out.style = BorderStyle::Dashed; known = true; }
            "thin" => { out.width = 0.75; known = true; }
            "medium" => { out.width = 2.25; known = true; }
            "thick" => { out.width = 3.75; known = true; }
            _ => {
                if let Some(color) = parse_color(token) {
                    out.color = Some(color);
                    known = true;
                } else if let Some(width) = css::length(token, 12.0) {
                    out.width = width.clamp(0.0, 96.0);
                    known = true;
                }
            }
        }
    }
    if out.width == 0.0 { out.style = BorderStyle::None; }
    known.then_some(out)
}

pub(super) fn borders(css: &str) -> Option<Borders> {
    let mut out = Borders::default();
    let mut known = false;
    for decl in css.split(';') {
        let Some((key, value)) = decl.split_once(':') else { continue };
        if !matches!(key.trim(), "border" | "border-top" | "border-right" | "border-bottom" | "border-left") { continue; }
        let Some(border) = border(value.trim()) else { continue };
        known = true;
        match key.trim() {
            "border" => { out.top = Some(border); out.right = Some(border); out.bottom = Some(border); out.left = Some(border); }
            "border-top" => out.top = Some(border),
            "border-right" => out.right = Some(border),
            "border-bottom" => out.bottom = Some(border),
            "border-left" => out.left = Some(border),
            _ => {}
        }
    }
    known.then_some(out)
}

pub(super) fn valign(css: &str, attrs: &[(String, String)]) -> VAlign {
    let mut value = attr(attrs, "valign").unwrap_or("top").to_ascii_lowercase();
    for decl in css.split(';') {
        if let Some((key, v)) = decl.split_once(':') && key.trim() == "vertical-align" {
            value = v.trim().to_ascii_lowercase();
        }
    }
    match value.as_str() {
        "middle" | "center" => VAlign::Center,
        "bottom" => VAlign::Bottom,
        _ => VAlign::Top,
    }
}

pub(super) fn width(css: &str, attrs: &[(String, String)]) -> Option<f32> {
    let mut out = attr(attrs, "width").and_then(|v| css::length(v, 12.0));
    for decl in css.split(';') {
        if let Some((key, value)) = decl.split_once(':') && key.trim() == "width" {
            out = css::length(value, 12.0).or(out);
        }
    }
    out.filter(|w| *w > 0.0).map(|w| w.clamp(6.0, 1584.0))
}

pub(super) fn set_width(table: &mut TableB, column: usize, span: usize, width: Option<f32>, replace: bool) {
    let end = column.saturating_add(span).min(63);
    if table.widths.len() < end { table.widths.resize(end, None); }
    for column in column..end {
        if let Some(slot) = table.widths.get_mut(column) && (replace || slot.is_none()) {
            *slot = width.map(|w| w / span.max(1) as f32);
        }
    }
}

pub(super) fn finish(mut table: TableB) -> FTable {
    let mut out = FTable { rows: table.rows.into_iter().filter(|r| !r.is_empty()).collect(), widths: Vec::new(), borderless: !table.bordered };
    out.insert_covered();
    let cols = out.cols().min(63);
    table.widths.resize(cols, None);
    for row in &out.rows {
        let mut column = 0usize;
        for cell in row {
            let span = cell.colspan.clamp(1, 63) as usize;
            if let Some(width) = cell.width {
                for at in column..column.saturating_add(span).min(cols) {
                    if let Some(slot) = table.widths.get_mut(at) && slot.is_none() {
                        *slot = Some(width / span as f32);
                    }
                }
            }
            column = column.saturating_add(span);
        }
    }
    if table.widths.iter().any(Option::is_some) {
        out.widths = (0..cols).map(|i| table.widths.get(i).copied().flatten().unwrap_or(72.0)).collect();
    }
    out
}
