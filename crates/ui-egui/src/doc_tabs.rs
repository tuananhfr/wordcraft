//! The document tab strip under the title bar: one tab per open document, shown once two are
//! open, so a second file never hides the first. Click to switch, × or a middle-click to close
//! (through "Save changes?" when it has unsaved changes); its context menu also closes the others
//! or all ([`crate::close_many`]) and reopens the last closed document.

use egui::{CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;

use crate::WordApp;
use crate::open_documents::DocumentTab;
use crate::theme::{Tokens, regular, semibold};

pub const STRIP_H: f32 = 28.0;
const TAB_MIN_W: f32 = 96.0;
const TAB_MAX_W: f32 = 220.0;
const CLOSE_W: f32 = 18.0;
const DOT_W: f32 = 12.0;
const PAD: f32 = 8.0;

/// The tabs to show: none with a single document, so the window looks as it always did.
pub fn tabs(app: &WordApp) -> Vec<DocumentTab> {
    if app.document_count() < 2 { Vec::new() } else { app.document_tabs() }
}

enum Click {
    Activate,
    Close,
    CloseOthers,
    CloseAll,
    Reopen,
}

pub fn show(app: &mut WordApp, ui: &mut Ui) {
    let tabs = tabs(app);
    if tabs.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    // A dialog or file dialog belongs to the shown document: no switching under it.
    let enabled = app.dialog.is_none() && app.colors.is_none() && !app.file_dialog_open();
    let reopen = app.can_reopen_document().then(|| app.closed.last_title().unwrap_or_default().to_string());
    let mut clicked = None;
    egui::Panel::top("document_tabs").exact_size(STRIP_H).frame(egui::Frame::NONE.fill(t.title_bar)).show(ui, |ui| {
        let rect = ui.max_rect();
        ui.painter().line_segment([pos2(rect.left(), rect.bottom() - 0.5), pos2(rect.right(), rect.bottom() - 0.5)], Stroke::new(1.0, t.border));
        let width = ((rect.width() - 2.0 * PAD) / tabs.len() as f32).clamp(TAB_MIN_W, TAB_MAX_W);
        // Too many tabs for a narrow window scroll: bring the shown one into view when it or the
        // layout changes, but let the user browse the others without snapping back.
        let active = tabs.iter().find(|tab| tab.active).map(|tab| tab.document);
        let reveal_key = egui::Id::new((active, tabs.len(), width.to_bits()));
        let reveal_id = ui.id().with("document_tabs_reveal");
        let reveal = ui.ctx().data_mut(|data| {
            let changed = data.get_temp::<egui::Id>(reveal_id) != Some(reveal_key);
            data.insert_temp(reveal_id, reveal_key);
            changed
        });
        egui::ScrollArea::horizontal()
            .id_salt("document_tabs_scroll")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    ui.add_space(PAD);
                    for tab in &tabs {
                        if let Some(c) = tab_button(ui, &t, tab, width, reveal, enabled, reopen.as_deref()) {
                            clicked = Some((tab.document, c));
                        }
                    }
                });
            });
    });
    match clicked {
        Some((document, Click::Close)) => {
            let _ = app.run("ui.closeDocument", json!({"document": document}));
        }
        Some((document, Click::Activate)) => {
            let _ = app.run("ui.activateDocument", json!({"document": document}));
        }
        Some((document, Click::CloseOthers)) => {
            let _ = app.run("ui.closeOthers", json!({"document": document}));
        }
        Some((_, Click::CloseAll)) => {
            let _ = app.run("ui.closeAll", json!({}));
        }
        Some((_, Click::Reopen)) => {
            let _ = app.run("ui.reopenDocument", json!({}));
        }
        None => {}
    }
}

/// `reopen`: the title Reopen Closed Document would bring back, when there is one.
fn tab_button(ui: &mut Ui, t: &Tokens, tab: &DocumentTab, width: f32, reveal: bool, enabled: bool, reopen: Option<&str>) -> Option<Click> {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, STRIP_H), sense);
    if tab.active && reveal {
        resp.scroll_to_me(None);
    }
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + 4.0), rect.max);
    let top = CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 };
    let painter = ui.painter();
    if tab.active {
        // Covers the strip's bottom line, so the shown tab joins the ribbon's tabs below it.
        painter.rect_filled(body, top, t.tab_strip);
        let edge = Stroke::new(1.0, t.border);
        painter.line_segment([body.left_bottom(), body.left_top() + vec2(0.0, 2.0)], edge);
        painter.line_segment([body.right_bottom(), body.right_top() + vec2(0.0, 2.0)], edge);
        painter.rect_filled(Rect::from_min_size(body.min, vec2(body.width(), 2.0)), top, t.accent);
    } else if resp.hovered() && enabled {
        painter.rect_filled(body.shrink2(vec2(0.0, 1.0)), top, t.hover);
    }
    let close = Rect::from_center_size(pos2(body.right() - 6.0 - CLOSE_W / 2.0, body.center().y), vec2(CLOSE_W, CLOSE_W));
    let text_left = body.left() + 10.0;
    let dot = if tab.dirty { DOT_W } else { 0.0 };
    let color = if tab.active { t.text } else { t.text_dim };
    let font = if tab.active { semibold(12.5) } else { regular(12.5) };
    let galley = painter.layout_job(elided(&tab.title, font, color, close.left() - 4.0 - dot - text_left));
    painter.galley(pos2(text_left, body.center().y - galley.size().y / 2.0), galley, color);
    if tab.dirty {
        painter.circle_filled(pos2(close.left() - 2.0 - DOT_W / 2.0, body.center().y), 3.5, if tab.active { t.accent } else { t.text_dim });
    }
    let close_resp = ui.interact(close, resp.id.with("close"), sense);
    if close_resp.hovered() && enabled {
        ui.painter().rect_filled(close, 3.0, t.pressed);
    }
    crate::icons::paint(ui.painter(), close.shrink(3.0), "close", if close_resp.hovered() { t.text } else { t.text_dim }, t.accent);
    let close_label = crate::i18n::fmt(tl!("Close {name}"), &[("name", &tab.title)]);
    close_resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &close_label));
    let close_resp = close_resp.on_hover_text(tl!("Close"));
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, tab.active, &tab.title));
    let tip = if tab.dirty { format!("{}\n{}", tab.title, tl!("Unsaved changes")) } else { tab.title.clone() };
    let resp = resp.on_hover_text(tip);
    let mut menu = None;
    if enabled {
        resp.context_menu(|ui| menu = tab_menu(ui, reopen));
    }
    if close_resp.clicked() || resp.middle_clicked() {
        Some(Click::Close)
    } else if resp.clicked() && !tab.active {
        Some(Click::Activate)
    } else {
        menu
    }
}

fn tab_menu(ui: &mut Ui, reopen: Option<&str>) -> Option<Click> {
    let mut picked = None;
    if ui.button(tl!("Close")).clicked() {
        picked = Some(Click::Close);
    }
    if ui.button(tl!("Close Others")).clicked() {
        picked = Some(Click::CloseOthers);
    }
    if ui.button(tl!("Close All")).clicked() {
        picked = Some(Click::CloseAll);
    }
    ui.separator();
    let item = ui.add_enabled(reopen.is_some(), egui::Button::new(tl!("Reopen Closed Document")));
    let item = match reopen {
        Some(title) => item.on_hover_text(crate::i18n::fmt(tl!("Reopen {name}"), &[("name", title)])),
        None => item,
    };
    if item.clicked() {
        picked = Some(Click::Reopen);
    }
    if picked.is_some() {
        ui.close();
    }
    picked
}

/// `text` on one line, cut with "…" where it would pass `max_width`.
pub(crate) fn elided(text: &str, font: egui::FontId, color: egui::Color32, max_width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_width.max(0.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    job.halign = egui::Align::LEFT;
    job
}
