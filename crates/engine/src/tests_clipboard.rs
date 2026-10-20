use std::sync::Arc;

use serde_json::{Value, json};
use wordcraft_doc::numbering::{AbstractNum, Level, Num};
use wordcraft_doc::props::{CharProps, NumRef, Underline};
use wordcraft_doc::styles::{Style, StyleKind};
use wordcraft_doc::{Block, Document, InlineObject, Pos, StoryRef};

use crate::{Selection, Session};

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn source() -> Session {
    let mut doc = Document::from_text("Item");
    doc.styles.upsert(Style {
        id: "SourceList".into(),
        name: "Source List".into(),
        based_on: Some("Normal".into()),
        chr: CharProps { italic: Some(true), ..Default::default() },
        ..Default::default()
    });
    doc.styles.upsert(Style {
        id: "SourceAccent".into(),
        name: "Source Accent".into(),
        kind: StyleKind::Character,
        chr: CharProps { bold: Some(true), ..Default::default() },
        ..Default::default()
    });
    doc.numbering.abstracts.push(AbstractNum { id: 7, levels: vec![Level::default()], ..Default::default() });
    doc.numbering.nums.push(Num { id: 42, abstract_id: 7, ..Default::default() });
    let props = CharProps { style: Some("SourceAccent".into()), underline: Some(Underline::Single), ..Default::default() };
    doc.format_range(&Pos::body(0, 0), &Pos::body(0, 4), &|c| *c = props.clone()).unwrap();
    let p = doc.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap();
    p.props.style = Some("SourceList".into());
    p.props.numbering = Some(NumRef { num: 42, level: 0 });
    let key = doc.add_media(vec![137, 80, 78, 71, 13, 10, 26, 10], "png");
    doc.insert_object(
        &Pos::body(0, 4),
        InlineObject::Image { media: key, w: 24.0, h: 18.0, alt: "Picture".into(), float: Default::default(), crop: [0.0; 4], ole: None },
        &props,
    )
    .unwrap();
    Session::new(doc)
}

fn copy(s: &mut Session) -> Value {
    run(s, "select.all", json!({}));
    run(s, "edit.copy", json!({}))
}

fn first(s: &Session) -> &wordcraft_doc::Paragraph {
    s.doc.para(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap()
}

#[test]
fn cross_document_marker_adopts_styles_numbering_media_and_one_undo() {
    let mut a = source();
    let clip = copy(&mut a);
    assert!(clip["html"].as_str().unwrap().contains("name=\"erpcons-clip\""));
    let mut b = Session::new(Document::new());
    let before = b.doc.clone();
    run(&mut b, "edit.paste", clip);
    let p = first(&b);
    assert_eq!(p.props.style.as_deref(), Some("SourceList"));
    assert_eq!(p.runs.first().unwrap().props.style.as_deref(), Some("SourceAccent"));
    assert_eq!(b.doc.styles.get("SourceList"), a.doc.styles.get("SourceList"));
    assert_eq!(b.doc.styles.get("SourceAccent"), a.doc.styles.get("SourceAccent"));
    let n = p.props.numbering.as_ref().unwrap();
    let num = b.doc.numbering.nums.iter().find(|v| v.id == n.num).unwrap();
    assert_ne!(num.id, 42);
    let abs = b.doc.numbering.abstracts.iter().find(|v| v.id == num.abstract_id).unwrap();
    assert_eq!(abs.levels, a.doc.numbering.abstracts[0].levels);
    let InlineObject::Image { media, w, h, .. } = p.objects.first().unwrap() else { panic!("missing picture") };
    assert_eq!((*w, *h), (24.0, 18.0));
    assert_eq!(b.doc.media.get(media).unwrap().as_slice(), a.doc.media.values().next().unwrap().as_slice());
    assert_eq!(b.undo_depth(), 1);
    run(&mut b, "edit.undo", json!({}));
    assert_eq!(b.doc, before);
}

#[test]
fn desktop_plain_text_fallback_adopts_the_cached_foreign_marker() {
    let mut a = source();
    copy(&mut a);
    let mut b = Session::new(Document::new());
    b.clipboard = a.clipboard.clone();
    b.clipboard_html = a.clipboard_html.clone();
    b.clipboard_text = a.clipboard_text.clone();
    let params = json!({"text": b.clipboard_text});
    run(&mut b, "edit.paste", params);
    assert_eq!(first(&b).props.style.as_deref(), Some("SourceList"));
    assert!(first(&b).props.numbering.is_some());
    assert_eq!(b.doc.media.len(), 1);
}

#[test]
fn garbage_marker_falls_back_to_html_even_when_plain_text_matches_local_copy() {
    let mut s = Session::new(Document::from_text("same"));
    copy(&mut s);
    s.sel = Selection::caret(s.doc.end_of(StoryRef::Body));
    run(&mut s, "edit.paste", json!({"text": "same", "html": "<meta name=\"erpcons-clip\" content=\"not-base64!?\"><p><b>HTML</b></p>"}));
    assert_eq!(s.doc.plain_text(StoryRef::Body), "sameHTML");
    assert_eq!(first(&s).props_of_char(4).bold, Some(true));
}

#[test]
fn same_document_marker_does_not_duplicate_resource_tables() {
    let mut s = source();
    let clip = copy(&mut s);
    let styles = s.doc.styles.clone();
    let numbering = s.doc.numbering.clone();
    let media = s.doc.media.clone();
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", clip);
    assert_eq!(s.doc.styles, styles);
    assert_eq!(s.doc.numbering, numbering);
    assert_eq!(s.doc.media, media);
    assert_eq!(first(&s).objects.len(), 2);
}

#[test]
fn same_document_marker_keeps_style_edits_made_after_copy() {
    let mut s = source();
    let clip = copy(&mut s);
    let mut changed = s.doc.styles.get("SourceList").unwrap().clone();
    changed.chr.italic = Some(false);
    s.doc.styles.upsert(changed.clone());
    let numbering = s.doc.numbering.clone();
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", clip);
    assert_eq!(s.doc.numbering, numbering);
    assert_eq!(s.doc.styles.get("SourceList"), Some(&changed));
}

#[test]
fn same_document_marker_keeps_native_object_metadata() {
    let mut s = source();
    let p = s.doc.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap();
    let InlineObject::Image { ole, .. } = p.objects.first_mut().unwrap() else { panic!("missing picture") };
    *ole = Some(Arc::new(wordcraft_doc::graphic::Embedded { xml: "<object/>".into(), refs: Vec::new() }));
    let original = p.objects.first().unwrap().clone();
    let clip = copy(&mut s);
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", clip);
    assert_eq!(first(&s).objects.last(), Some(&original));
}

#[test]
fn older_same_document_marker_reuses_resources_after_a_new_copy() {
    let mut s = source();
    let older = copy(&mut s);
    run(&mut s, "select.range", json!({"anchor": {"block": 0, "off": 0}, "focus": {"block": 0, "off": 2}}));
    run(&mut s, "edit.copy", json!({}));
    let numbering = s.doc.numbering.clone();
    let media = s.doc.media.clone();
    run(&mut s, "caret.docEnd", json!({}));
    run(&mut s, "edit.paste", older);
    assert_eq!(s.doc.numbering, numbering);
    assert_eq!(s.doc.media, media);
    assert_eq!(first(&s).objects.len(), 2);
}

#[test]
fn cross_document_marker_adopts_graphic_picture_media() {
    let mut a = source();
    let key = a.doc.media.keys().next().unwrap().clone();
    let at = a.doc.end_of(StoryRef::Body);
    a.doc
        .insert_object(
            &at,
            InlineObject::Graphic {
                w: 24.0,
                h: 18.0,
                alt: "Graphic".into(),
                float: Default::default(),
                graphic: Arc::new(wordcraft_doc::graphic::Graphic {
                    items: vec![wordcraft_doc::graphic::GraphicItem::Image { rect: [0.0, 0.0, 24.0, 18.0], media: key.clone() }],
                    ..Default::default()
                }),
            },
            &CharProps::default(),
        )
        .unwrap();
    let clip = copy(&mut a);
    let mut b = Session::new(Document::new());
    run(&mut b, "edit.paste", clip);
    let InlineObject::Graphic { graphic, .. } = first(&b).objects.last().unwrap() else { panic!("missing graphic") };
    let wordcraft_doc::graphic::GraphicItem::Image { media, .. } = graphic.items.first().unwrap() else { panic!("missing image") };
    assert_ne!(media, &key);
    assert_eq!(b.doc.media.get(media), a.doc.media.get(&key));
}

#[test]
fn oversized_copy_omits_marker_but_keeps_html() {
    let mut s = Session::new(Document::from_text(&"x".repeat(4 << 20)));
    let clip = copy(&mut s);
    let html = clip["html"].as_str().unwrap();
    assert!(html.contains("<p"));
    assert!(!html.contains("erpcons-clip"));
    assert_eq!(clip["text"].as_str().unwrap().len(), 4 << 20);
}

#[test]
fn cross_document_marker_keeps_destination_style_by_name_and_can_keep_source() {
    let mut a = source();
    let clip = copy(&mut a);
    for keep in [false, true] {
        let mut b = Session::new(Document::new());
        let destination = Style {
            id: "LocalList".into(),
            name: "Source List".into(),
            chr: CharProps { italic: Some(false), ..Default::default() },
            ..Default::default()
        };
        b.doc.styles.upsert(destination.clone());
        let mut params = clip.clone();
        params["keepSource"] = json!(keep);
        run(&mut b, "edit.paste", params);
        assert_eq!(b.doc.styles.get("LocalList"), Some(&destination));
        let id = first(&b).props.style.as_deref().unwrap();
        if keep {
            assert_ne!(id, "LocalList");
            assert_eq!(b.doc.styles.get(id).unwrap().chr.italic, Some(true));
        } else {
            assert_eq!(id, "LocalList");
            assert!(b.doc.styles.get("SourceList").is_none());
        }
    }
}

#[test]
fn cross_document_marker_remaps_colliding_numbering_and_media() {
    let mut a = source();
    let clip = copy(&mut a);
    let mut b = Session::new(Document::new());
    b.doc.numbering = a.doc.numbering.clone();
    let old_media = b.doc.add_media(vec![0, 1, 2], "png");
    run(&mut b, "edit.paste", clip);
    let num_id = first(&b).props.numbering.as_ref().unwrap().num;
    assert_ne!(num_id, 42);
    let n = b.doc.numbering.nums.iter().find(|n| n.id == num_id).unwrap();
    assert_ne!(n.abstract_id, 7);
    let InlineObject::Image { media, .. } = first(&b).objects.first().unwrap() else { panic!("missing picture") };
    assert_ne!(media, &old_media);
    assert_eq!(b.doc.media.get(&old_media).unwrap().as_slice(), &[0, 1, 2]);
}

#[test]
fn cut_exports_html_and_marker_including_small_images() {
    let mut s = source();
    run(&mut s, "select.all", json!({}));
    let result = run(&mut s, "edit.cut", json!({}));
    let html = result["html"].as_str().unwrap();
    assert!(html.contains("data:image/png;base64,"));
    assert!(html.contains("erpcons-clip"));
    assert!(s.doc.plain_text(StoryRef::Body).is_empty());
}

#[test]
fn large_images_are_only_in_marker_and_unreferenced_media_is_excluded() {
    let mut s = source();
    let key = s.doc.media.keys().next().unwrap().clone();
    s.doc.media.insert(key, Arc::new(vec![1; 300 << 10]));
    s.doc.add_media(vec![2; 4 << 20], "png");
    let clip = copy(&mut s);
    let html = clip["html"].as_str().unwrap();
    assert!(html.contains("erpcons-clip"));
    assert!(!html.contains("data:image"));
    let mut dst = Session::new(Document::new());
    run(&mut dst, "edit.paste", clip);
    assert_eq!(dst.doc.media.len(), 1);
    assert_eq!(dst.doc.media.values().next().unwrap().len(), 300 << 10);
}

#[test]
fn paste_special_accepts_marker_without_a_local_clipboard() {
    let clip = copy(&mut source());
    for fmt in ["html", "formatted"] {
        let mut s = Session::new(Document::new());
        let mut params = clip.clone();
        params["as"] = json!(fmt);
        run(&mut s, "edit.pasteSpecial", params);
        assert_eq!(first(&s).props.style.as_deref(), Some("SourceList"));
        assert!(first(&s).props.numbering.is_some());
        assert_eq!(s.doc.media.len(), 1);
    }
}

#[test]
fn plain_copy_always_exports_html() {
    let mut s = Session::new(Document::from_text("plain"));
    assert!(copy(&mut s)["html"].as_str().unwrap().contains("plain"));
}

#[test]
fn selection_html_exports_run_formatting_links_and_lists() {
    let mut s = source();
    let style = CharProps {
        bold: Some(true),
        italic: Some(true),
        underline: Some(Underline::Single),
        strike: Some(true),
        color: Some(wordcraft_doc::props::TextColor::Rgb(wordcraft_doc::props::Rgb(0x12, 0x34, 0x56))),
        size: Some(18.0),
        font: Some("Example Font".into()),
        link: Some("https://example.test/".into()),
        ..Default::default()
    };
    s.doc.format_range(&Pos::body(0, 0), &Pos::body(0, 4), &|props| *props = style.clone()).unwrap();
    let clip = copy(&mut s);
    let html = clip["html"].as_str().unwrap();
    for part in ["<strong>", "<em>", "<u>", "<s>", "color:#123456", "font-size:18pt", "Example Font", "href=\"https://example.test/\"", "<ol", "<li"]
    {
        assert!(html.contains(part), "missing {part} in {html}");
    }
}

#[test]
fn numbering_id_exhaustion_fails_without_leaving_partial_resources() {
    let clip = copy(&mut source());
    let mut dst = Session::new(Document::new());
    dst.doc.numbering.nums.push(Num { id: u32::MAX, abstract_id: 0, ..Default::default() });
    let before = dst.doc.clone();
    assert!(dst.run("edit.paste", &clip).is_err());
    assert_eq!(dst.doc, before);
    assert_eq!(dst.undo_depth(), 0);
}

#[test]
fn copied_table_exports_and_pastes_through_marker() {
    let mut s = Session::new(Document::new());
    s.doc.body.push(Arc::new(Block::Table(wordcraft_doc::Table::new(2, 2, 200.0))));
    s.doc.body.push(wordcraft_doc::para_block(wordcraft_doc::Paragraph::new()));
    let clip = copy(&mut s);
    assert!(clip["html"].as_str().unwrap().contains("<table"));
    let mut b = Session::new(Document::new());
    run(&mut b, "edit.paste", clip);
    assert!(b.doc.body.iter().any(|b| matches!(&**b, Block::Table(t) if t.rows.len() == 2)));
}
