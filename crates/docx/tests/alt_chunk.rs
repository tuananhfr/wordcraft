use std::io::{Cursor, Read, Write};

fn fixture(cell: bool) -> Vec<u8> {
    let blocks = r#"<w:p><w:r><w:t>A</w:t></w:r></w:p><w:altChunk r:id="chunk"/><w:p><w:r><w:t>C</w:t></w:r></w:p>"#;
    let body = if cell { format!("<w:tbl><w:tr><w:tc>{blocks}</w:tc></w:tr></w:tbl>") } else { blocks.to_string() };
    let document = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}</w:body></w:document>"#
    );
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("word/document.xml", document.as_bytes()),
        ("word/chunk.html", b"<html><body>B-CHUNK</body></html>".as_slice()),
        ("word/_rels/document.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="chunk" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/aFChunk" Target="chunk.html"/></Relationships>"#.as_slice()),
        ("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/chunk.html" ContentType="text/html"/></Types>"#.as_slice()),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn alt_chunk_round_trip_body_and_cell() {
    for cell in [false, true] {
        let doc = wordcraft_docx::read(&fixture(cell)).unwrap();
        let bytes = wordcraft_docx::write(&doc).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
        assert!(xml.contains("w:altChunk"), "altChunk was dropped: {xml}");
        assert!(xml.find(">A<").unwrap() < xml.find("w:altChunk").unwrap());
        assert!(xml.find("w:altChunk").unwrap() < xml.find(">C<").unwrap());
        let mut rels = String::new();
        zip.by_name("word/_rels/document.xml.rels").unwrap().read_to_string(&mut rels).unwrap();
        assert!(rels.contains("relationships/aFChunk"));
        assert!(rels.contains("Target=\"chunk.html\""));
        let id = xml.split("<w:altChunk r:id=\"").nth(1).unwrap().split('"').next().unwrap();
        assert!(rels.contains(&format!("Id=\"{id}\"")));
        let mut part = Vec::new();
        zip.by_name("word/chunk.html").unwrap().read_to_end(&mut part).unwrap();
        assert_eq!(part, b"<html><body>B-CHUNK</body></html>");
        let mut types = String::new();
        zip.by_name("[Content_Types].xml").unwrap().read_to_string(&mut types).unwrap();
        assert!(types.contains("PartName=\"/word/chunk.html\" ContentType=\"text/html\""));
    }
}

#[test]
fn alt_chunk_carrier_deletion_removes_part_and_relationship() {
    let mut doc = wordcraft_docx::read(&fixture(false)).unwrap();
    doc.body.pop();
    let bytes = wordcraft_docx::write(&doc).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert!(zip.by_name("word/chunk.html").is_err());
    let mut rels = String::new();
    zip.by_name("word/_rels/document.xml.rels").unwrap().read_to_string(&mut rels).unwrap();
    assert!(!rels.contains("aFChunk"));
}

#[test]
fn alt_chunk_survives_boundary_merge_and_trailing_container() {
    use wordcraft_doc::{Path, Pos, StoryRef};
    let mut doc = wordcraft_docx::read(&fixture(false)).unwrap();
    doc.delete_range(&Pos::new(StoryRef::Body, Path::top(0), 1), &Pos::new(StoryRef::Body, Path::top(1), 0)).unwrap();
    assert_eq!(doc.preserved_alt_chunk_count(), 1);
    let bytes = wordcraft_docx::write(&doc).unwrap();
    assert_eq!(wordcraft_docx::read(&bytes).unwrap().preserved_alt_chunk_count(), 1);
    for cell in [false, true] {
        let mut zip = zip::ZipArchive::new(Cursor::new(fixture(cell))).unwrap();
        let mut entries = Vec::new();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if entry.name() == "word/document.xml" {
                bytes = String::from_utf8(bytes).unwrap().replace("<w:p><w:r><w:t>C</w:t></w:r></w:p>", "").into_bytes();
            }
            entries.push((entry.name().to_string(), bytes));
        }
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(&bytes).unwrap();
        }
        let doc = wordcraft_docx::read(&zip.finish().unwrap().into_inner()).unwrap();
        let bytes = wordcraft_docx::write(&doc).unwrap();
        assert_eq!(wordcraft_docx::read(&bytes).unwrap().preserved_alt_chunk_count(), 1);
    }
}

#[test]
fn alt_chunk_selection_deletion_removes_entire_carrier() {
    use wordcraft_doc::{Paragraph, Pos, para_block};
    let mut doc = wordcraft_docx::read(&fixture(false)).unwrap();
    doc.body.push(para_block(Paragraph::with_text("D", Default::default())));
    doc.delete_range(&Pos::body(0, 1), &Pos::body(2, 0)).unwrap();
    assert_eq!(doc.preserved_alt_chunk_count(), 0);
    let bytes = wordcraft_docx::write(&doc).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert!(zip.by_name("word/chunk.html").is_err());
}

#[test]
fn alt_chunk_shared_references_write_one_part() {
    let mut doc = wordcraft_docx::read(&fixture(false)).unwrap();
    doc.body.push(doc.body.last().unwrap().clone());
    let bytes = wordcraft_docx::write(&doc).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert_eq!(zip.file_names().filter(|name| name.ends_with(".html")).count(), 1);
    let mut xml = String::new();
    zip.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
    assert_eq!(xml.matches("<w:altChunk ").count(), 2);
}
