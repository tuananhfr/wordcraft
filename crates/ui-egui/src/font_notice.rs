//! One dismissible substitution notice for each opened document.

use std::collections::BTreeMap;

use wordcraft_doc::resolve::ResolvedChar;
use wordcraft_doc::{Block, Blocks, Document};
use wordcraft_layout::para::CellText;

use crate::{WordApp, i18n};

#[derive(Default)]
pub(crate) struct State {
    notices: BTreeMap<u64, Vec<String>>,
}

pub(crate) fn opened(app: &mut WordApp) {
    let tabs = app.document_tabs();
    app.font_notice.notices.retain(|id, _| tabs.iter().any(|tab| tab.document == *id));
    let fonts = substituted_fonts(&app.session.doc);
    if !fonts.is_empty() {
        app.font_notice.notices.insert(app.session.document_id(), fonts);
    }
}

fn substituted_fonts(doc: &Document) -> Vec<String> {
    let mut fonts = BTreeMap::new();
    for blocks in std::iter::once(&doc.body).chain(doc.parts.values().map(|p| &p.blocks)) {
        collect_fonts(doc, blocks, None, 0, &mut fonts);
    }
    fonts.into_values().collect()
}

fn collect_fonts(doc: &Document, blocks: &Blocks, table: Option<&CellText>, depth: usize, fonts: &mut BTreeMap<String, String>) {
    if depth > 16 {
        return;
    }
    for block in blocks {
        match block.as_ref() {
            Block::Para(p) => {
                let mut offset = 0usize;
                for run in &p.runs {
                    let Some(end) = offset.checked_add(run.len) else { break };
                    let Some(text) = p.text.get(offset..end) else { break };
                    offset = end;
                    if text.is_empty() {
                        continue;
                    }
                    let rc = doc.styles.resolve_char_in(p.props.style.as_deref(), table.map(|s| &s.chr), &run.props);
                    collect_run(&rc, text, fonts);
                }
                if let Some(num) = doc.styles.resolve_para(&p.props).numbering
                    && doc.numbering.shows_label(num.num, num.level)
                    && let Some(level) = doc.numbering.level(num.num, num.level.min(8))
                {
                    let props = p.mark.clone().overlaid(&level.chr);
                    let rc = doc.styles.resolve_char_in(p.props.style.as_deref(), table.map(|s| &s.chr), &props);
                    collect_run(&rc, &level.text, fonts);
                }
            }
            Block::Table(t) => {
                let style = t.props.style.as_deref().and_then(|id| doc.styles.table_style(id));
                let columns = t.cols().max(1);
                for (row_index, row) in t.rows.iter().enumerate() {
                    let mut column = 0usize;
                    for cell in &row.cells {
                        let text = style.as_ref().map(|style| wordcraft_layout::cell_text_style(t, style, row_index, column, cell.span(), columns));
                        if cell.props.vmerge != wordcraft_doc::props::VMerge::Continue {
                            collect_fonts(doc, &cell.blocks, text.as_ref(), depth + 1, fonts);
                        }
                        column = column.saturating_add(cell.span());
                    }
                }
            }
        }
    }
}

fn collect_run(rc: &ResolvedChar, text: &str, fonts: &mut BTreeMap<String, String>) {
    let mut add = |name: &str, bold, italic| {
        if wordcraft_fonts::word::resolve(name, bold, italic).substituted {
            fonts.entry(name.to_lowercase()).or_insert_with(|| name.to_string());
        }
    };
    if text.chars().any(|c| !rc.uses_complex(c) && !(wordcraft_layout::para::is_east_asian(c) && rc.font_ea.is_some())) {
        add(&rc.font, rc.bold, rc.italic);
    }
    if text.chars().any(|c| rc.uses_complex(c)) {
        add(&rc.font_cs, rc.bold_cs, rc.italic_cs);
    }
    if text.chars().any(|c| !rc.uses_complex(c) && wordcraft_layout::para::is_east_asian(c))
        && let Some(font) = &rc.font_ea
    {
        add(font, rc.bold, rc.italic);
    }
}

fn message(lang: i18n::Lang, fonts: &[String]) -> Option<String> {
    if fonts.is_empty() {
        return None;
    }
    let mut names = fonts.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if fonts.len() > 3 {
        names = i18n::fmt(i18n::tr(lang, "{fonts} and {count} others"), &[("fonts", &names), ("count", &fonts.len().saturating_sub(3).to_string())]);
    }
    Some(i18n::fmt(
        i18n::tr(lang, "The fonts {fonts} are not available on this device, so similar fonts are used to display them."),
        &[("fonts", &names)],
    ))
}

pub(crate) fn show(app: &mut WordApp, ui: &mut egui::Ui) {
    let document = app.session.document_id();
    let Some(text) = app.font_notice.notices.get(&document).and_then(|fonts| message(i18n::current(), fonts)) else { return };
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut close = false;
    egui::Frame::new().fill(t.panel).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                close = ui.small_button(tl!("Close")).clicked();
                ui.add(egui::Label::new(text).wrap());
            });
        });
    });
    if close {
        app.font_notice.notices.remove(&document);
    }
}

#[cfg(test)]
mod tests;
