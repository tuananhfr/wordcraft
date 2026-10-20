use std::sync::Arc;

use serde_json::json;
use wordcraft_doc::para::{InlineObject, Run};
use wordcraft_doc::props::{CharProps, Rgb, TextColor, Underline};
use wordcraft_doc::{Block, Document, Paragraph, PartKind, Path, Pos, StoryRef, Table, para_block};

use crate::{ColumnBlock, Selection, Session};

const AUTO: &str = "tools.convert_vietnamese_legacy";
const TCVN3: &str = "tools.convert_vietnamese_legacy_tcvn3";
const VNI: &str = "tools.convert_vietnamese_legacy_vni";

fn props(font: &str) -> CharProps {
    CharProps { font: Some(font.into()), ..Default::default() }
}

fn para(text: &str, font: &str) -> Paragraph {
    Paragraph::with_text(text, props(font))
}

fn session(text: &str, font: &str) -> Session {
    let mut doc = Document::new();
    doc.body = vec![para_block(para(text, font))];
    Session::new(doc)
}

fn text(s: &Session) -> &str {
    &s.doc.para(StoryRef::Body, &Path::top(0)).unwrap().text
}

#[test]
fn known_fonts_are_detected_case_insensitively() {
    for font in [".VnTime", ".VnArial", ".vNaRiAl", "TCVN3", "ABC", "ABC Times"] {
        let mut s = session("TiÕng ViÖt", font);
        s.run(AUTO, &json!({})).unwrap();
        assert_eq!(text(&s), "Tiếng Việt", "{font}");
    }
    for font in ["VNI-Times", "vni-times", "VnI Times"] {
        let mut s = session("Tieáng Vieät", font);
        s.run(AUTO, &json!({})).unwrap();
        assert_eq!(text(&s), "Tiếng Việt", "{font}");
    }
}

#[test]
fn capital_tcvn3_fonts_recover_uppercase_tones() {
    for font in [".VnTimeH", ".vNaRiAlH"] {
        let mut s = session("aµ¸¶·¹ ¨»¾¼½Æ ©ÇÊÈÉË eÌÐÎÏÑ ªÒÕÓÔÖ i×ÝØÜÞ oßãáâä «åèæçé ¬êíëìî uïóñòô \u{ad}õøö÷ù yúýûüþ ®", font);
        s.run(AUTO, &json!({})).unwrap();
        assert_eq!(text(&s), "AÀÁẢÃẠ ĂẰẮẲẴẶ ÂẦẤẨẪẬ EÈÉẺẼẸ ÊỀẾỂỄỆ IÌÍỈĨỊ OÒÓỎÕỌ ÔỒỐỔỖỘ ƠỜỚỞỠỢ UÙÚỦŨỤ ƯỪỨỬỮỰ YỲÝỶỸỴ Đ");
    }
}

#[test]
fn nonlegacy_runs_are_skipped_and_conversion_is_idempotent() {
    let mut s = session("Tiếng Việt: ô ó ò ñ ® Õ", "Arial");
    let original = s.doc.clone();
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(s.doc, original);

    let mut s = session("« ã ß ®", ".VnTime");
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(text(&s), "ô ó ò đ");
    let converted = s.doc.clone();
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(s.doc, converted);

    let mut s = session("oâ où oø ñ", "VNI-Times");
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(text(&s), "ô ó ò đ");
    let converted = s.doc.clone();
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(s.doc, converted);
}

#[test]
fn selected_text_only_and_reversed_selection_are_preserved() {
    let mut s = session("Õ TiÕng ViÖt Õ", ".VnTime");
    let start = "Õ ".len();
    let end = "Õ TiÕng ViÖt".len();
    s.sel = Selection { anchor: Pos::body(0, end), focus: Pos::body(0, start) };
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(text(&s), "Õ Tiếng Việt Õ");
    assert_eq!(s.sel.focus, Pos::body(0, start));
    assert_eq!(s.sel.anchor, Pos::body(0, "Õ Tiếng Việt".len()));
    let p = s.doc.para_at(&s.sel.focus).unwrap();
    assert_eq!(p.runs.len(), 3);
    assert_eq!(p.runs[0].props.font.as_deref(), Some(".VnTime"));
    assert_eq!(p.runs[1].props.font, s.doc.styles.default_chr.font);
    assert_eq!(p.runs[2].props.font.as_deref(), Some(".VnTime"));
}

#[test]
fn unknown_fonts_require_explicit_selection_encoding() {
    for (command, source) in [(TCVN3, "TiÕng ViÖt"), (VNI, "Tieáng Vieät")] {
        let mut s = session(source, "Unknown Font");
        assert!(s.run(command, &json!({})).is_err());
        s.sel = Selection { anchor: Pos::body(0, 0), focus: Pos::body(0, source.len()) };
        s.run(AUTO, &json!({})).unwrap();
        assert_eq!(text(&s), source);
        s.run(command, &json!({})).unwrap();
        assert_eq!(text(&s), "Tiếng Việt");
        assert_eq!(s.sel.focus.off, "Tiếng Việt".len());
    }
}

#[test]
fn mixed_runs_keep_formatting_and_use_the_normal_body_font() {
    let mut s = session("", "Arial");
    s.doc.styles.get_mut("Normal").unwrap().chr.font = Some("Noto Serif".into());
    let rich = CharProps {
        font: Some("VNI-Times".into()),
        size: Some(17.5),
        bold: Some(true),
        italic: Some(true),
        color: Some(TextColor::Rgb(Rgb(12, 34, 56))),
        underline: Some(Underline::Double),
        ..Default::default()
    };
    let mut p = Paragraph::with_text("Tieáng Vieät", rich.clone());
    p.insert_text(p.len(), " | TiÕng ViÖt", &props(".VnArial")).unwrap();
    p.insert_text(p.len(), " | ô ñ ®", &props("Arial")).unwrap();
    s.doc.body = vec![para_block(p)];
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(text(&s), "Tiếng Việt | Tiếng Việt | ô ñ ®");
    let p = s.doc.para(StoryRef::Body, &Path::top(0)).unwrap();
    assert_eq!(p.runs[0].props, CharProps { font: Some("Noto Serif".into()), ..rich });
    assert_eq!(p.runs[1].props.font.as_deref(), Some("Noto Serif"));
    assert_eq!(p.runs[2].props.font.as_deref(), Some("Arial"));
}

#[test]
fn inherited_legacy_font_and_missing_or_legacy_body_font() {
    let mut s = session("TiÕng ViÖt", "Arial");
    s.doc.styles.get_mut("Normal").unwrap().chr.font = Some(".VnTime".into());
    s.doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap().runs[0].props.font = None;
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(text(&s), "Tiếng Việt");
    assert_eq!(s.doc.para_at(&s.sel.focus).unwrap().runs[0].props.font.as_deref(), Some("Times New Roman"));

    let mut s = session("Tieáng Vieät", "VNI-Times");
    s.doc.styles.default_chr.font = None;
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(s.doc.para_at(&s.sel.focus).unwrap().runs[0].props.font.as_deref(), Some("Times New Roman"));
}

#[test]
fn whole_document_includes_nested_tables_headers_footers_and_one_undo() {
    let mut s = session("TiÕng ViÖt", ".VnTime");
    let mut table = Table::new(1, 1, 200.0);
    let mut nested = Table::new(1, 1, 150.0);
    nested.rows[0].cells[0].blocks = vec![para_block(para("Tieáng Vieät", "VNI-Times"))];
    table.rows[0].cells[0].blocks = vec![Arc::new(Block::Table(nested)), para_block(para("®", ".VnArial"))];
    s.doc.body.push(Arc::new(Block::Table(table)));
    s.doc.body.push(para_block(Paragraph::new()));
    let header = s.doc.add_part(PartKind::Header, vec![para_block(para("Tieáng Vieät", "VNI-Times"))]);
    let footer = s.doc.add_part(PartKind::Footer, vec![para_block(para("TiÕng ViÖt", ".VnTime"))]);
    s.doc.last_section.headers.default = Some(header);
    s.doc.last_section.footers.default = Some(footer);
    let original = s.doc.clone();
    let selection = s.sel.clone();
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(s.undo_depth(), 1);
    for story in [StoryRef::Body, StoryRef::Part(header), StoryRef::Part(footer)] {
        for path in s.doc.para_paths(story) {
            let p = s.doc.para(story, &path).unwrap();
            if !p.is_empty() {
                assert!(p.text == "Tiếng Việt" || p.text == "đ", "{}", p.text);
                assert!(p.runs.iter().all(|r| r.props.font.as_deref() == Some("Aptos")));
            }
        }
    }
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc, original);
    assert_eq!(s.sel, selection);
    assert_eq!(s.undo_depth(), 0);
}

#[test]
fn selection_in_a_header_does_not_convert_body_or_other_stories() {
    let mut s = session("TiÕng ViÖt", ".VnTime");
    let header = s.doc.add_part(PartKind::Header, vec![para_block(para("Tieáng Vieät", "VNI-Times"))]);
    let story = StoryRef::Part(header);
    s.sel = Selection { anchor: s.doc.start_of(story), focus: s.doc.end_of(story) };
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(text(&s), "TiÕng ViÖt");
    assert_eq!(s.doc.plain_text(story), "Tiếng Việt");
}

#[test]
fn inline_objects_survive_conversion_and_selection_offsets_stay_valid() {
    let mut s = session("Tieáng Vieät", "VNI-Times");
    let p = s.doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
    p.insert_object("Tieáng".len(), InlineObject::BookmarkStart { name: "anchor".into() }, &props("VNI-Times")).unwrap();
    let objects = p.objects.clone();
    s.sel = Selection::caret(Pos::body(0, 3));
    s.run(AUTO, &json!({})).unwrap();
    let p = s.doc.para_at(&s.sel.focus).unwrap();
    assert_eq!(p.text, "Tiếng\u{fffc} Việt");
    assert_eq!(p.objects, objects);
    assert!(p.text.is_char_boundary(s.sel.focus.off));
}

#[test]
fn column_selection_converts_only_its_segments() {
    let mut s = session("TiÕng | TiÕng", ".VnTime");
    s.doc.body.push(para_block(para("ViÖt | ViÖt", ".VnArial")));
    let a = Pos::body(0, 0);
    let b = Pos::body(1, "ViÖt".len());
    s.sel = Selection { anchor: a.clone(), focus: b.clone() };
    s.column = Some(ColumnBlock {
        sel: s.sel.clone(),
        anchor_x: 0.0,
        focus_x: 40.0,
        segments: vec![(a, Pos::body(0, "TiÕng".len())), (Pos::body(1, 0), b)],
        cells: None,
    });
    s.run(AUTO, &json!({})).unwrap();
    assert_eq!(text(&s), "Tiếng | TiÕng");
    assert_eq!(s.doc.para(StoryRef::Body, &Path::top(1)).unwrap().text, "Việt | ViÖt");
    assert_eq!(s.column_segments().unwrap()[0].1.off, "Tiếng".len());
    assert_eq!(s.undo_depth(), 1);
}

#[test]
fn invalid_run_boundaries_return_an_error_without_changing_the_document() {
    let mut s = session("Õ", ".VnTime");
    s.doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap().runs = vec![Run { len: 1, props: props(".VnTime") }];
    let original = s.doc.clone();
    assert!(s.run(AUTO, &json!({})).is_err());
    assert_eq!(s.doc, original);
    assert_eq!(s.undo_depth(), 0);
}

#[test]
fn conversion_commands_are_discoverable_without_new_shortcuts() {
    let registry = crate::cmd::registry();
    for id in [AUTO, TCVN3, VNI] {
        let spec = registry.get(id).unwrap();
        assert_eq!(spec.location, "Review › Language");
        assert!(spec.shortcut.is_empty());
        assert!(spec.mutates);
    }
}
