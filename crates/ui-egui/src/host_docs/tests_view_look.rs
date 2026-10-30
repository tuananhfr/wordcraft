//! `erp-host.md` v2 view and comment modes as they look on screen: no caret in the page, the
//! mode button says "Viewing", the shell's Share button is the only one, and comment mode's notice.

use egui::epaint::Shape;
use egui::{Event, Pos2};
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use wordcraft_doc::Pos;

use super::tests::{Page, docx, web_app};
use super::tests_access::{READ_ONLY, opened};
use super::tests_offered::click;
use super::{HostMode, HostOpen};
use crate::WordApp;
use crate::table_resize::tests::{H, steps};

const COMMENT_NOTICE: &str = "Nhận xét trên web sẽ có sau, hiện chỉ xem được";
const VIEW_NOTICE: &str = "Tài liệu này chỉ được xem, không sửa được.";

/// The web app in a `width`-wide window with "Xin chào" open as `d1` in `mode`.
fn hosted(mode: HostMode, width: f32) -> (H, Page) {
    let (app, page) = web_app();
    page.link.open(HostOpen {
        doc_id: "d1".into(),
        version: 1,
        title: "Báo cáo".into(),
        format: "docx".into(),
        bytes: docx("Xin chào"),
        mode,
        can_download: true,
        ..Default::default()
    });
    let mut h: H = egui_kittest::Harness::builder().with_size(egui::vec2(width, 900.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h, 6);
    assert_eq!(h.state().read_only(), mode.read_only(), "{mode:?}: the document is open");
    (h, page)
}

/// Where the caret at `pos` stands on screen: its top and bottom.
fn caret_at(h: &mut H, pos: &Pos) -> (Pos2, Pos2) {
    let app = h.state_mut();
    let hint = app.session.page_hint;
    let c = app.session.layout().caret_on(pos, hint).expect("the caret is laid out");
    let top = crate::canvas::page_to_screen(app, c.page, c.x, c.top).unwrap();
    let bottom = crate::canvas::page_to_screen(app, c.page, c.x, c.top + c.height).unwrap();
    (top, bottom)
}

/// Whether the last frame drew a caret bar (1.5 wide, upright) over the line from `top` to `bottom`.
fn caret_drawn(h: &H, (top, bottom): (Pos2, Pos2)) -> bool {
    fn bar(s: &Shape, top: Pos2, bottom: Pos2) -> bool {
        match s {
            Shape::Vec(v) => v.iter().any(|s| bar(s, top, bottom)),
            Shape::LineSegment { points: [a, b], stroke } => {
                stroke.width == 1.5
                    && (a.x - b.x).abs() < 0.5
                    && (a.x - top.x).abs() < 3.0
                    && a.y.min(b.y) >= top.y - 3.0
                    && a.y.max(b.y) <= bottom.y + 3.0
            }
            _ => false,
        }
    }
    h.output().shapes.iter().any(|c| bar(&c.shape, top, bottom))
}

/// A click into the text of a read-only document places the selection there but draws no
/// blinking caret; selecting and copying (when `canDownload` allows) still work. In edit mode the
/// same click shows the caret.
#[test]
fn no_caret_blinks_in_view_or_comment_mode() {
    for mode in [HostMode::Edit, HostMode::View, HostMode::Comment] {
        let (mut h, _page) = hosted(mode, 1440.0);
        let (top, bottom) = caret_at(&mut h, &Pos::body(0, 3));
        click(&mut h, top + (bottom - top) / 2.0);
        let sel = h.state().session.sel.clone();
        assert!(sel.is_collapsed() && sel.focus.off > 0, "{mode:?}: the click placed the selection: {sel:?}");
        let at = caret_at(&mut h, &sel.focus);
        // The blink runs on the wall clock: watch longer than one on-and-off period (2 × 530 ms).
        let mut drawn = false;
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            h.step();
            drawn |= caret_drawn(&h, at);
        }
        assert_eq!(drawn, !mode.read_only(), "{mode:?}: caret drawn");
        if mode.read_only() {
            assert!(h.output().platform_output.ime.is_none(), "{mode:?}: no input method asked for");
            h.state_mut().run("select.all", json!({})).unwrap();
            assert!(!h.state().session.sel.is_collapsed(), "{mode:?}: selecting works");
            h.state_mut().run("edit.copy", json!({})).unwrap();
        }
    }
}

/// The mode button's nodes on the tab strip's row (the one `anchor` sits on) labelled `label`.
fn on_row(h: &H, anchor: egui::Rect, label: &str) -> usize {
    h.query_all_by_label(label).filter(|n| (n.rect().center().y - anchor.center().y).abs() < 12.0).count()
}

/// The mode button says "Chỉ xem" in view and comment mode, not "Chỉnh sửa", and offers no switch
/// to editing; in edit mode it says "Chỉnh sửa" and opens its menu.
#[test]
fn the_mode_button_says_viewing_and_cannot_switch_to_editing() {
    for mode in READ_ONLY {
        let (mut h, _page) = hosted(mode, 1600.0);
        let (r, disabled) = {
            let b = h.get_by_label("Chỉ xem");
            (b.rect(), b.accesskit_node().is_disabled())
        };
        assert!(disabled, "{mode:?}: the mode button is greyed");
        assert_eq!(on_row(&h, r, "Chỉnh sửa"), 0, "{mode:?}: the button says Editing");
        click(&mut h, r.center());
        assert!(!egui::Popup::is_any_open(&h.ctx), "{mode:?}: the mode menu opened");
        assert!(!h.state().session.doc.settings.track_changes);
        assert!(h.state().read_only(), "{mode:?}: still read-only");
    }
    let (mut h, _page) = hosted(HostMode::Edit, 1600.0);
    let r = h.get_by_label("Chỉnh sửa").rect();
    assert_eq!(on_row(&h, r, "Chỉ xem"), 0);
    click(&mut h, r.center());
    assert!(egui::Popup::is_any_open(&h.ctx), "the mode menu opens in edit mode");
}

/// In the narrow window's "⋯" menu, view mode shows "Chỉ xem" greyed in place of the choices.
#[test]
fn the_narrow_menu_offers_no_editing_choice_in_view_mode() {
    let (mut h, _page) = hosted(HostMode::View, 560.0);
    let r = h.get_by_label("Lệnh Khác…").rect();
    click(&mut h, r.center());
    assert!(h.get_by_label("Chỉ xem").accesskit_node().is_disabled());
    assert_eq!(h.query_all_by_label("Chỉnh sửa — sửa trực tiếp trên tài liệu").count(), 0);
}

fn shares(h: &H) -> usize {
    h.query_all_by_label("Chia sẻ").count()
}

/// With a v2 shell (`set_shell_shares`) the ribbon's own Share button is gone, wide and narrow;
/// a v1 host and an app without a host (desktop, the free web build) keep it.
#[test]
fn the_ribbon_has_no_share_button_when_the_shell_shares() {
    let (mut h, page) = hosted(HostMode::Edit, 1600.0);
    assert_eq!(shares(&h), 1, "a v1 host: the ribbon shares");
    page.link.set_shell_shares();
    steps(&mut h, 2);
    assert_eq!(shares(&h), 0, "the shell's Share button is the only one");

    let (mut h, page) = hosted(HostMode::Edit, 560.0);
    let more = h.get_by_label("Lệnh Khác…").rect();
    click(&mut h, more.center());
    assert_eq!(shares(&h), 1, "a v1 host: the narrow menu shares");
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    steps(&mut h, 2);
    page.link.set_shell_shares();
    click(&mut h, more.center());
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert_eq!(shares(&h), 0, "the narrow menu leaves sharing to the shell");

    let app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Xin chào")), Default::default());
    let mut h: H = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 900.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h, 6);
    assert_eq!(h.query_all_by_label(crate::tl!("Share")).count(), 1, "no host: the ribbon shares");
}

/// Comment mode is view-only until comments come to the web, and says exactly that; view mode's
/// notice is unchanged. Both from a command and from a key typed at the page.
#[test]
fn comment_mode_says_commenting_comes_later() {
    for (mode, notice) in [(HostMode::Comment, COMMENT_NOTICE), (HostMode::View, VIEW_NOTICE)] {
        let (mut app, _page) = opened(mode, true);
        assert_eq!(app.run("text.insert", json!({"text": "x"})).unwrap_err(), notice, "{mode:?}");

        let (mut h, _page) = hosted(mode, 1440.0);
        let (top, bottom) = caret_at(&mut h, &Pos::body(0, 3));
        click(&mut h, top + (bottom - top) / 2.0);
        h.state_mut().status_msg = None;
        h.event(Event::Text("x".into()));
        steps(&mut h, 2);
        assert_eq!(h.state().status_msg.as_ref().map(|(s, _)| s.as_str()), Some(notice), "{mode:?}: typing says why");
        assert_eq!(super::tests::body(h.state()), "Xin chào");
    }
}
