use egui_kittest::kittest::Queryable;
use serde_json::json;
use wordcraft_doc::{CharProps, Document, Paragraph, PartKind, para_block};

use super::*;

#[test]
fn substitution_message_formats_zero_one_three_and_five_fonts() {
    let vi = i18n::Lang::from_pref("vi");
    let names: Vec<String> = ["A", "B", "C", "D", "E"].map(str::to_string).into();
    assert_eq!(message(vi, &[]), None);
    for (count, listed) in [(1, "A"), (3, "A, B, C"), (5, "A, B, C và 2 khác")] {
        assert_eq!(message(vi, &names[..count]), Some(format!("Các phông {listed} chưa có trên máy này nên được hiển thị bằng phông tương tự.")));
    }
    assert_eq!(message(i18n::Lang::EN, &names), Some("The fonts A, B, C and 2 others are not available on this device, so similar fonts are used to display them.".into()));
}

fn missing_font_doc() -> Document {
    let mut doc = Document::from_text("Visible text");
    doc.styles.default_chr.font = Some("WordCraft Missing Test Font W4".into());
    doc
}

#[test]
fn collects_resolved_styles_and_secondary_stories_without_duplicates() {
    let mut doc = missing_font_doc();
    doc.add_part(PartKind::Header, vec![para_block(Paragraph::with_text(
        "Header", CharProps { font: Some("WordCraft Missing Header Font W4".into()), ..Default::default() },
    ))]);
    let fonts = substituted_fonts(&doc);
    assert_eq!(fonts, ["WordCraft Missing Header Font W4", "WordCraft Missing Test Font W4"]);
}

#[test]
fn available_fonts_do_not_create_a_notice() {
    let mut doc = Document::from_text("Visible text");
    doc.styles.default_chr.font = Some(wordcraft_fonts::FALLBACK_FAMILY.into());
    assert!(substituted_fonts(&doc).is_empty());
}

#[test]
fn unused_asian_and_complex_fonts_do_not_create_a_notice() {
    let mut doc = Document::from_text("Latin text");
    doc.styles.default_chr.font = Some(wordcraft_fonts::FALLBACK_FAMILY.into());
    doc.styles.default_chr.font_ea = Some("WordCraft Missing Asian Font W4".into());
    doc.styles.default_chr.font_cs = Some("WordCraft Missing Complex Font W4".into());
    assert!(substituted_fonts(&doc).is_empty());
}

#[test]
fn used_asian_and_complex_fonts_are_reported() {
    for (text, asian, complex) in [("文字", true, false), ("نص", false, true)] {
        let mut doc = Document::from_text(text);
        doc.styles.default_chr.font = Some(wordcraft_fonts::FALLBACK_FAMILY.into());
        let missing = "WordCraft Missing Script Font W4";
        if asian {
            doc.styles.default_chr.font_ea = Some(missing.into());
        }
        if complex {
            doc.styles.default_chr.font_cs = Some(missing.into());
        }
        assert_eq!(substituted_fonts(&doc), [missing]);
    }
}

#[test]
fn list_label_fonts_are_reported_without_unused_styles() {
    let mut doc = Document::from_text("List item");
    doc.styles.default_chr.font = Some(wordcraft_fonts::FALLBACK_FAMILY.into());
    let id = doc.numbering.add_list(wordcraft_doc::numbering::ListKind::Bullet);
    let missing = "WordCraft Missing Bullet Font W4";
    let abstract_id = doc.numbering.nums.iter().find(|num| num.id == id).unwrap().abstract_id;
    doc.numbering.abstracts.iter_mut().find(|num| num.id == abstract_id).unwrap().levels[0].chr.font = Some(missing.into());
    let p = doc.para_mut(wordcraft_doc::StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap();
    p.props.numbering = Some(wordcraft_doc::props::NumRef { num: id, level: 0 });
    assert_eq!(substituted_fonts(&doc), [missing]);
}

#[test]
fn malformed_run_lengths_are_ignored_without_panicking() {
    let mut doc = missing_font_doc();
    let p = doc.para_mut(wordcraft_doc::StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap();
    p.runs[0].len = usize::MAX;
    assert!(substituted_fonts(&doc).is_empty());
}

#[test]
fn conditional_table_fonts_are_reported_only_in_the_regions_using_them() {
    use std::sync::Arc;

    let mut doc = Document::from_text("Body");
    doc.styles.default_chr.font = Some(wordcraft_fonts::FALLBACK_FAMILY.into());
    let missing = "WordCraft Missing Header Row Font W4";
    doc.styles.get_mut("TableGrid").unwrap().table.as_mut().unwrap().header_chr.font = Some(missing.into());
    let mut table = wordcraft_doc::Table::new(1, 1, 200.0);
    table.rows[0].cells[0].blocks = vec![para_block(Paragraph::with_text("Header", Default::default()))];
    doc.body.push(Arc::new(Block::Table(table.clone())));
    assert_eq!(substituted_fonts(&doc), [missing]);
    table.props.look.header_row = false;
    doc.body.pop();
    doc.body.push(Arc::new(Block::Table(table)));
    assert!(substituted_fonts(&doc).is_empty());
}

#[test]
fn notice_dismissal_survives_edits_and_document_switches() {
    for width in [390.0, 1366.0] {
        let mut app = WordApp::new(wordcraft_engine::Session::new(Document::new()), Default::default());
        app.ui.language = "vi".into();
        let bytes = wordcraft_engine::io::save_bytes("fonts.json", &missing_font_doc()).unwrap();
        let params = json!({"path": "fonts.json", "data": wordcraft_engine::cmd::insert::base64_encode(&bytes)});
        app.run("file.open", params.clone()).unwrap();
        let document = app.session.document_id();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(width, 844.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            app,
        );
        h.run_steps(6);
        let text = h.get_by_label("Các phông WordCraft Missing Test Font W4 chưa có trên máy này nên được hiển thị bằng phông tương tự.").rect();
        assert!(text.left() >= 0.0 && text.right() <= width && text.bottom() <= 844.0, "{text:?}");
        h.get_by_label("Đóng").click();
        h.run_steps(4);
        assert!(!h.state().font_notice.notices.contains_key(&document));
        h.state_mut().run("text.insert", json!({"text": "Edit"})).unwrap();
        h.state_mut().run("file.new", json!({})).unwrap();
        h.state_mut().activate_document(0).unwrap();
        h.run_steps(4);
        assert!(!h.state().font_notice.notices.contains_key(&document));
        h.state_mut().run("file.open", params).unwrap();
        let reopened = h.state().session.document_id();
        assert!(h.state().font_notice.notices.contains_key(&reopened));
    }
}
