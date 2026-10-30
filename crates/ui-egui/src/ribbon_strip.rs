//! The ribbon's tab strip (it scrolls sideways when the tabs don't fit) with Share, Editing and
//! Comments at its right end, and the folded ribbon shown over the page for one command.

use egui::{Align2, Rect, Sense, Ui, pos2, vec2};
use serde_json::json;

use crate::theme::{Tokens, medium, regular, semibold};
use crate::{WordApp, icons};

/// The tab strip's contents: `tabs` are (name, custom) in order; `collapsed` is the ribbon's
/// folded state this frame.
pub fn show(app: &mut WordApp, ui: &mut Ui, tabs: &[(String, bool)], collapsed: bool, t: &Tokens) {
    let narrow = app.narrow.is_narrow();
    let full = ui.max_rect();
    ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
    let mut right_start = full.max.x;
    ui.horizontal_centered(|ui| {
        // As tall as a tab, so the buttons centre on the same row as before the tabs scrolled.
        ui.allocate_exact_size(vec2(0.0, 30.0), Sense::hover());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // No document open (#1129): nothing to share, edit or comment on.
            if !app.has_document() {
                ui.disable();
            }
            if narrow {
                more_menu(app, ui, t);
            } else {
                share_editing_comments(app, ui, t);
            }
            right_start = ui.min_rect().min.x;
        });
    });
    // The tabs take what is left of the strip and scroll within it.
    let rest = Rect::from_min_max(full.min, pos2((right_start - 4.0).max(full.min.x), full.max.y));
    ui.scope_builder(egui::UiBuilder::new().max_rect(rest).layout(egui::Layout::left_to_right(egui::Align::Min)), |ui| {
        tab_row(app, ui, tabs, collapsed, t)
    });
}

fn tab_row(app: &mut WordApp, ui: &mut Ui, tabs: &[(String, bool)], collapsed: bool, t: &Tokens) {
    let shown_id = ui.id().with("tab_scrolled_to");
    let scroll = egui::ScrollArea::horizontal()
        .id_salt("tab_strip")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                for (tab, custom) in tabs {
                    tab_button(app, ui, tab, *custom, collapsed, shown_id, t);
                }
            });
        });
    crate::ribbon::edge_cues(ui, &scroll, tl!("Scroll ribbon left"), tl!("Scroll ribbon right"), t.tab_strip, t);
}

fn tab_button(app: &mut WordApp, ui: &mut Ui, tab: &str, custom: bool, collapsed: bool, shown_id: egui::Id, t: &Tokens) {
    let contextual = !custom && wordcraft_engine::ribbon::CONTEXTUAL_TABS.contains(&tab);
    // A custom tab's name is the user's own text: shown as typed.
    let shown = if custom { tab } else { tl!(tab) };
    let w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(shown.to_string(), medium(12.5), t.text).size().x) + 18.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    if app.ui.keytips == crate::keytips::Phase::Tabs && app.ui.keytip_prefix.is_empty() {
        crate::keytips::record_tab(r, tab, &mut app.keytip_rects);
    }
    let active = app.ui.tab == tab && !app.ui.backstage;
    // A tab that becomes active (a contextual tab appearing, a keytip) is scrolled into view once;
    // after that the user's own scrolling stands.
    if active && ui.data(|d| d.get_temp::<String>(shown_id)).as_deref() != Some(tab) {
        resp.scroll_to_me(None);
        ui.data_mut(|d| d.insert_temp(shown_id, tab.to_string()));
    }
    if resp.hovered() && !active {
        ui.painter().rect_filled(r.shrink2(vec2(0.0, 4.0)), 4.0, t.hover);
    }
    let color = if contextual || active { t.accent_text } else { t.text };
    ui.painter().text(r.center(), Align2::CENTER_CENTER, shown, if active { semibold(12.5) } else { medium(12.5) }, color);
    if active {
        let u = Rect::from_center_size(pos2(r.center().x, r.max.y - 2.0), vec2(w - 16.0, 3.0));
        ui.painter().rect_filled(u, 2.0, t.accent);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, active, shown));
    if resp.clicked() {
        let again = active && app.ribbon_peek;
        let _ = app.run("ui.tab", json!({"tab": tab}));
        // Folded, a tab shows its commands over the page until one is used, as in Word; the
        // same tab again folds them away.
        app.ribbon_peek = collapsed && tab != "File" && !again;
    }
    if resp.double_clicked() && tab != "File" {
        let _ = app.run("ui.collapseRibbon", json!({}));
    }
    resp.context_menu(|ui| crate::widgets::customize_menu_items(ui, app));
}

/// Share, Editing mode and Comments, at the right end of a wide window's tab strip.
fn share_editing_comments(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    if !app.shell_shares() {
        share_button(app, ui, t);
        ui.add_space(6.0);
    }
    // A read-only document says so and offers no switch to editing.
    let read_only = app.read_only();
    let track = app.session.doc.settings.track_changes;
    let (icon, label) = match (read_only, track) {
        (true, _) => ("readMode", tl!("Viewing")),
        (false, true) => ("pencil", tl!("Reviewing")),
        (false, false) => ("pencil", tl!("Editing")),
    };
    let mode = ui.add_enabled_ui(!read_only, |ui| crate::widgets::icon_text_menu_button(ui, icon, label, regular(12.0))).inner;
    mode.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !read_only, label));
    if let Some(lock) = app.edit_lock().filter(|_| read_only) {
        mode.on_disabled_hover_text(crate::i18n::t(lock.reason()));
    } else if !read_only {
        egui::Popup::menu(&mode).show(|ui| editing_choices(app, ui));
    }
    ui.add_space(4.0);
    if crate::widgets::icon_text_button(ui, "comment", tl!("Comments"), regular(12.0)).clicked() {
        let _ = app.run("view.commentsPane", json!({}));
    }
}

fn share_button(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    let share = tl!("Share");
    let share_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(share.to_string(), medium(12.0), t.text).size().x);
    let (r, resp) = ui.allocate_exact_size(vec2((share_w + 36.0).max(74.0), 24.0), Sense::click());
    ui.painter().rect_filled(r, 4.0, if resp.hovered() { t.accent_text } else { t.accent });
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 14.0, r.center().y), vec2(14.0, 14.0)), "share", t.on_accent, t.on_accent);
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, share, medium(12.0), t.on_accent);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, share));
    let resp = resp.on_hover_text(tl!("Export a copy to share (PDF, Word document)"));
    if resp.clicked() {
        share_copy(app, crate::share::pointer_click(&resp));
    }
}

/// A narrow window: the same three behind one "⋯" button, so the tabs keep the width.
fn more_menu(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    let label = tl!("More Commands…");
    let (r, resp) = ui.allocate_exact_size(vec2(30.0, 24.0), Sense::click());
    if resp.hovered() || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp)) {
        ui.painter().rect_filled(r, 4.0, t.hover);
    }
    // Drawn, not a "⋯" character: the interface fonts may lack it.
    let ink = if ui.is_enabled() { t.text } else { t.text_dim };
    for dx in [-6.0, 0.0, 6.0] {
        ui.painter().circle_filled(pos2(r.center().x + dx, r.center().y), 1.8, ink);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let resp = resp.on_hover_text(label);
    egui::Popup::menu(&resp).show(|ui| {
        if !app.shell_shares() {
            let share = ui.button(tl!("Share")).on_hover_text(tl!("Export a copy to share (PDF, Word document)"));
            if share.clicked() {
                share_copy(app, crate::share::pointer_click(&share));
                ui.close();
            }
            ui.separator();
        }
        editing_choices(app, ui);
        ui.separator();
        if ui.button(tl!("Comments")).clicked() {
            let _ = app.run("view.commentsPane", json!({}));
            ui.close();
        }
    });
}

/// Straight to the share sheet where the host can share files (`pointer`: see
/// [`WordApp::share_document`]), otherwise File › Export.
fn share_copy(app: &mut WordApp, pointer: bool) {
    if app.can_share() {
        if let Err(error) = app.share_document(pointer) {
            app.status(error);
        }
        return;
    }
    let _ = app.run("ui.backstage", json!({"value": true, "page": "export"}));
}

fn editing_choices(app: &mut WordApp, ui: &mut Ui) {
    if app.read_only() {
        ui.add_enabled(false, egui::Button::new(tl!("Viewing")).selected(true));
        return;
    }
    let track = app.session.doc.settings.track_changes;
    if ui.selectable_label(!track, tl!("Editing — edit the document directly")).clicked() {
        let _ = app.run("review.trackChanges", json!({"value": false}));
        ui.close();
    }
    if ui.selectable_label(track, tl!("Reviewing — edits become suggestions")).clicked() {
        let _ = app.run("review.trackChanges", json!({"value": true}));
        ui.close();
    }
}

/// The folded ribbon, shown under the tab strip over the page. It folds away again after a
/// command, a click outside it, or Esc; a menu or a text field opened from it keeps it up.
pub fn peek(app: &mut WordApp, ui: &mut Ui, strip: Rect, frame: egui::Frame, height: f32, body: impl FnOnce(&mut WordApp, &mut Ui)) {
    let ctx = ui.ctx().clone();
    let margin = frame.total_margin();
    let inner = vec2((strip.width() - margin.sum().x).max(0.0), (height - margin.sum().y).max(0.0));
    let area =
        egui::Area::new(egui::Id::new("ribbon_peek")).order(egui::Order::Middle).fixed_pos(strip.left_bottom()).constrain(false).show(&ctx, |ui| {
            frame.shadow(ui.style().visuals.popup_shadow).show(ui, |ui| {
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                body(app, ui);
            });
        });
    let rect = area.response.rect;
    if peek_done(&ctx, rect, strip, area.response.layer_id) {
        app.ribbon_peek = false;
    }
}

/// Whether this frame's input ends the peek over `rect` (on layer `own`).
fn peek_done(ctx: &egui::Context, rect: Rect, strip: Rect, own: egui::LayerId) -> bool {
    let (esc, press, click) = ctx.input(|i| {
        let at = i.pointer.interact_pos();
        (i.key_pressed(egui::Key::Escape), at.filter(|_| i.pointer.primary_pressed()), at.filter(|_| i.pointer.primary_clicked()))
    });
    let on_popup = |p| ctx.layer_id_at(p).is_some_and(|l| l != own && matches!(l.order, egui::Order::Foreground | egui::Order::Tooltip));
    // A press on the page, a pane or the status bar; the tab strip's own clicks decide there.
    let outside = press.is_some_and(|p| !rect.contains(p) && !strip.contains(p) && !on_popup(p));
    // A click on a command (in the ribbon, or in a menu it opened) that left nothing open.
    let used = click.is_some_and(|p| rect.contains(p) || on_popup(p)) && !egui::Popup::is_any_open(ctx) && !ctx.text_edit_focused();
    esc || outside || used
}

#[cfg(test)]
mod tests {
    use egui::{Event, PointerButton, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::WordApp;

    fn harness(w: f32) -> Harness<'static, WordApp> {
        let a = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Hello WordCraft on a phone")), Default::default());
        let mut h = Harness::builder().with_size(vec2(w, 844.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
        h.run_steps(6);
        h
    }

    fn tap(h: &mut Harness<'_, WordApp>, at: egui::Pos2) {
        h.hover_at(at);
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() });
        }
        h.run_steps(3);
    }

    #[test]
    fn a_folded_ribbon_shows_a_tapped_tab_over_the_page_until_a_command_or_a_tap_outside() {
        let mut h = harness(390.0);
        assert!(h.state().ribbon_collapsed(), "a phone folds the ribbon");
        assert!(h.query_by_label("Copy").is_none(), "no commands showing");
        let page_top = h.state().canvas.canvas_rect.expect("the page is shown").min.y;
        h.get_by_label("Home").click();
        h.run_steps(3);
        assert!(h.state().ribbon_peek, "a tap on a tab shows its commands");
        let copy = h.get_by_label("Copy").rect();
        assert!(copy.min.y < page_top + 120.0, "over the page, under the tabs");
        assert_eq!(h.state().canvas.canvas_rect.map(|r| r.min.y), Some(page_top), "the page doesn't move down");
        h.get_by_label("Copy").click();
        h.run_steps(3);
        assert!(!h.state().ribbon_peek, "a command folds it away");
        assert!(h.query_by_label("Copy").is_none());

        h.get_by_label("Insert").click();
        h.run_steps(3);
        assert!(h.state().ribbon_peek && h.state().ui.tab == "Insert");
        let below = h.state().canvas.canvas_rect.unwrap().center_bottom() - vec2(0.0, 60.0);
        tap(&mut h, below);
        assert!(!h.state().ribbon_peek, "a tap on the page folds it away");

        h.get_by_label("Insert").click();
        h.run_steps(3);
        assert!(h.state().ribbon_peek);
        h.get_by_label("Insert").click();
        h.run_steps(3);
        assert!(!h.state().ribbon_peek, "the same tab again folds it");
        h.get_by_label("Insert").click();
        h.run_steps(3);
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(!h.state().ribbon_peek, "Esc folds it");
    }

    #[test]
    fn the_tab_strip_scrolls_and_keeps_share_editing_and_comments_reachable_when_narrow() {
        let mut h = harness(390.0);
        let strip_right = 390.0;
        let help = h.get_by_label("Help").rect();
        assert!(help.min.x > strip_right, "the last tabs are past the edge, not squeezed: {help:?}");
        assert!(h.query_by_label("Share").is_none(), "Share is behind the menu");
        h.get_by_label("More Commands…").click();
        h.run_steps(2);
        assert!(h.query_by_label("Share").is_some() && h.query_by_label("Comments").is_some(), "the menu holds them");
        h.get_by_label("Comments").click();
        h.run_steps(3);
        assert!(h.state().session.view.comments_pane, "Comments opens its pane from the menu");
        // Dragging the strip with a finger scrolls it: only on a touch screen (egui's default).
        h.event(Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::Start,
            pos: help.center(),
            force: None,
        });
        h.event(Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::End,
            pos: help.center(),
            force: None,
        });
        h.run_steps(2);
        let home = h.get_by_label("Home").rect();
        let from = egui::pos2(300.0, home.center().y);
        h.event(Event::PointerMoved(from));
        h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
        for k in 1..=20 {
            h.event(Event::PointerMoved(from - vec2(15.0 * k as f32, 0.0)));
        }
        h.event(Event::PointerButton { pos: from - vec2(300.0, 0.0), button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(4);
        let moved = h.get_by_label("Help").rect();
        assert!(moved.min.x < help.min.x - 100.0, "the strip scrolled: {help:?} -> {moved:?}");
    }

    #[test]
    fn a_desktop_window_keeps_the_ribbon_and_the_share_button() {
        let mut h = harness(1366.0);
        assert!(!h.state().ribbon_collapsed());
        assert!(h.query_by_label("Copy").is_some(), "the Home commands show");
        assert!(h.query_by_label("More Commands…").is_none(), "Share, Editing and Comments stay on the strip");
        // Folded by hand on a desktop, a tab click shows the commands over the page too.
        h.state_mut().run("ui.collapseRibbon", serde_json::json!({})).unwrap();
        h.run_steps(3);
        assert!(h.query_by_label("Copy").is_none());
        h.get_by_label("Home").click();
        h.run_steps(3);
        assert!(h.state().ribbon_peek && h.query_by_label("Copy").is_some());
        assert!(h.state().ui.ribbon_collapsed, "still folded: the peek doesn't unfold it");
    }
}
