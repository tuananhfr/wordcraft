//! `w14:paraId` on paragraphs and rows, and the list and comment ids, survive a save and an open
//! unchanged: they are what live editing knows blocks by.

use std::io::{Cursor, Read};
use std::sync::Arc;

use wordcraft_doc::numbering::ListKind;
use wordcraft_doc::props::CharProps;
use wordcraft_doc::table::Table;
use wordcraft_doc::{Block, Comment, Document, InlineObject, Paragraph, PartKind, para_block};

fn rt(doc: &Document) -> Document {
    wordcraft_docx::read(&wordcraft_docx::write(doc).expect("write")).expect("read")
}

fn document_xml(doc: &Document) -> String {
    let bytes = wordcraft_docx::write(doc).expect("write");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut s = String::new();
    zip.by_name("word/document.xml").expect("document.xml").read_to_string(&mut s).expect("utf-8");
    s
}

fn para(text: &str, id: Option<u32>) -> Paragraph {
    let mut p = Paragraph::with_text(text, CharProps::default());
    p.id = id;
    p
}

fn body_para_ids(doc: &Document) -> Vec<Option<u32>> {
    doc.body.iter().filter_map(|b| b.as_para()).map(|p| p.id).collect()
}

#[test]
fn paragraph_and_row_ids_survive_save_and_open() {
    let mut d = Document::new();
    let mut t = Table::new(2, 2, 200.0);
    t.rows[0].id = Some(0x0000_0A01);
    t.rows[1].id = Some(0x7FFF_FFFF);
    if let Some(Block::Para(p)) = t.rows[0].cells[0].blocks.first_mut().map(Arc::make_mut) {
        p.id = Some(0x55AA_0101);
    }
    d.body = vec![para_block(para("first", Some(0x1234_5678))), Arc::new(Block::Table(t)), para_block(para("last", Some(0x2A)))];
    let xml = document_xml(&d);
    assert!(xml.contains(r#"<w:p w14:paraId="12345678""#), "{xml}");
    assert!(xml.contains(r#"<w:tr w14:paraId="7FFFFFFF""#), "{xml}");
    let r = rt(&d);
    assert_eq!(body_para_ids(&r), [Some(0x1234_5678), Some(0x2A)]);
    let Some(Block::Table(t)) = r.body.get(1).map(|b| &**b) else { panic!("table lost") };
    assert_eq!(t.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [Some(0x0A01), Some(0x7FFF_FFFF)]);
    assert_eq!(t.rows[0].cells[0].blocks.first().and_then(|b| b.as_para()).and_then(|p| p.id), Some(0x55AA_0101));
    assert_eq!(wordcraft_doc::ids::block_id(&r.body[1]), Some(0x0A01), "a table is known by its first row");
    // A second round trip changes nothing.
    assert_eq!(body_para_ids(&rt(&r)), body_para_ids(&r));
}

#[test]
fn a_repeated_id_is_written_once_and_a_missing_one_not_at_all() {
    let mut d = Document::new();
    d.body = vec![para_block(para("a", Some(9))), para_block(para("copy of a", Some(9))), para_block(para("none", None))];
    let xml = document_xml(&d);
    assert_eq!(xml.matches(r#"w14:paraId="00000009""#).count(), 1, "{xml}");
    assert_eq!(body_para_ids(&rt(&d)), [Some(9), None, None]);
}

#[test]
fn list_and_comment_ids_survive_save_and_open() {
    let mut d = Document::new();
    let (aid, nid) = (0x0055_5501, 0x0055_5502);
    d.numbering.add_list_as(ListKind::Numbered, aid, nid);
    let mut item = para("item", Some(0x30));
    item.props.numbering = Some(wordcraft_doc::props::NumRef { num: nid, level: 0 });
    let cid = 0x00AB_CD81;
    let note = para("note", Some(0x40));
    let part = d.add_part(PartKind::Comment, vec![para_block(note)]);
    d.comments.insert(cid, Comment { author: "A".into(), part, ..Default::default() });
    let mut commented = para("commented", Some(0x31));
    commented.insert_object(0, InlineObject::CommentStart { id: cid }, &CharProps::default()).unwrap();
    let end = commented.len();
    commented.insert_object(end, InlineObject::CommentEnd { id: cid }, &CharProps::default()).unwrap();
    d.body = vec![para_block(item), para_block(commented)];
    let r = rt(&d);
    assert!(r.numbering.nums.iter().any(|n| n.id == nid && n.abstract_id == aid));
    assert_eq!(r.body[0].as_para().and_then(|p| p.props.numbering).map(|n| n.num), Some(nid));
    assert_eq!(r.comments.keys().copied().collect::<Vec<_>>(), [cid]);
    let c = &r.comments[&cid];
    let note = r.parts[&c.part].blocks.iter().find_map(|b| b.as_para()).unwrap();
    assert_eq!(note.id, Some(0x40), "the comment's paragraph keeps its own id");
    assert_eq!(body_para_ids(&r), [Some(0x30), Some(0x31)]);
}
