//! Hostile packages (zip bombs, too many entries, bombs nested in other containers) are refused
//! with one message the interface translates and open nothing; large legitimate files open.
use std::io::{Cursor, Write};

use wordcraft_doc::inflate::{self, LIMITS, TOO_LARGE};
use wordcraft_engine::io::{open_bytes, open_bytes_with, too_large};

const CT: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
const RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

fn zip_with(parts: &[(&str, &[u8])], options: zip::write::SimpleFileOptions) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        z.start_file(*name, options).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn zip(parts: &[(&str, &[u8])]) -> Vec<u8> {
    zip_with(parts, zip::write::SimpleFileOptions::default())
}

/// A document whose body is `body`, plus `extra` parts.
fn docx(body: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let document = format!("<w:document {W}><w:body>{body}<w:p><w:r><w:t>Xin chào</w:t></w:r></w:p></w:body></w:document>");
    let mut parts: Vec<(&str, &[u8])> =
        vec![("[Content_Types].xml", CT.as_bytes()), ("_rels/.rels", RELS.as_bytes()), ("word/document.xml", document.as_bytes())];
    parts.extend_from_slice(extra);
    zip(&parts)
}

fn bomb() -> Vec<u8> {
    docx(&" ".repeat(64 << 20), &[])
}

fn refused<T>(r: Result<T, String>) -> String {
    match r {
        Ok(_) => panic!("the hostile file opened"),
        Err(e) => {
            assert!(too_large(&e), "not refused as too large: {e}");
            e
        }
    }
}

#[test]
fn a_zip_bomb_is_refused_with_the_translatable_message() {
    let bytes = bomb();
    assert!(bytes.len() < 1 << 20);
    let e = refused(open_bytes("bomb.docx", &bytes));
    assert!(e.contains("compressed more than 200:1"), "{e}");
    // Named otherwise, it is still read by what it holds.
    refused(open_bytes("bomb.doc", &bytes));
    refused(open_bytes("bomb", &bytes));
}

#[test]
fn a_bomb_in_an_optional_part_still_refuses_the_file() {
    let bytes = docx("", &[("customXml/item1.xml", " ".repeat(16 << 20).as_bytes()), ("word/media/image1.png", &vec![0u8; 32 << 20])]);
    refused(open_bytes("parts.docx", &bytes));
}

#[test]
fn too_many_zip_entries_are_refused() {
    let names: Vec<String> = (0..=LIMITS.entries).map(|i| format!("customXml/i{i}.xml")).collect();
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut parts: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    parts.push(("[Content_Types].xml", CT.as_bytes()));
    refused(open_bytes("many.docx", &zip_with(&parts, stored)));
}

#[test]
fn bombs_nested_in_other_containers_are_refused() {
    // Inside an encrypted package (a compound file holding the zip).
    let encrypted = wordcraft_docx::encrypt(&bomb(), "pw").unwrap();
    refused(open_bytes_with("secret.docx", &encrypted, Some("pw")));
    // Zip-in-zip with the inner bomb stored: the outer entry is what inflates.
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let inner = zip_with(&[("xl/worksheets/sheet1.xml", &vec![b' '; 32 << 20])], stored);
    refused(open_bytes("embedded.docx", &docx("", &[("word/embeddings/Microsoft_Excel_Worksheet.xlsx", &inner)])));
    // The inner bomb deflated: kept as opaque bytes, never inflated, so the document opens.
    let doc = open_bytes("embedded.docx", &docx("", &[("word/embeddings/Microsoft_Excel_Worksheet.xlsx", &bomb())])).unwrap();
    assert!(!doc.body.is_empty());
}

#[test]
fn odt_and_recipient_spreadsheet_bombs_are_refused() {
    let content = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"><office:body><office:text><text:p>Xin chào<draw:frame svg:width="2cm" svg:height="2cm"><draw:image xlink:href="Pictures/a.png"/></draw:frame></text:p></office:text></office:body></office:document-content>"#;
    let mimetype = b"application/vnd.oasis.opendocument.text";
    let odt = zip(&[("mimetype", mimetype), ("content.xml", content.as_bytes()), ("Pictures/a.png", &vec![0u8; 32 << 20])]);
    refused(open_bytes("picture.odt", &odt));
    let odt = zip(&[("mimetype", mimetype), ("content.xml", format!("{content}{}", " ".repeat(32 << 20)).as_bytes())]);
    refused(open_bytes("content.odt", &odt));
    let odt = zip(&[("mimetype", mimetype), ("content.xml", content.as_bytes())]);
    assert!(open_bytes("fine.odt", &odt).is_ok());

    let ods = zip(&[("mimetype", b"application/vnd.oasis.opendocument.spreadsheet"), ("content.xml", " ".repeat(48 << 20).as_bytes())]);
    let e = wordcraft_formats::sheet::read_rows(&ods, wordcraft_formats::sheet::Pick::Index(0)).unwrap_err();
    assert!(e.contains(TOO_LARGE), "{e}");
}

/// Rewrites the inflated size `name` declares, in its local header and the central directory.
fn declare_size(zip: &mut [u8], name: &str, size: u32) {
    for i in 0..zip.len().saturating_sub(4) {
        let (name_at, size_at) = match zip[i..i + 4] {
            [0x50, 0x4b, 0x03, 0x04] => (30, 22),
            [0x50, 0x4b, 0x01, 0x02] => (46, 24),
            _ => continue,
        };
        if zip.get(i + name_at..i + name_at + name.len()) == Some(name.as_bytes()) {
            zip[i + size_at..i + size_at + 4].copy_from_slice(&size.to_le_bytes());
        }
    }
}

#[test]
fn a_required_part_declared_over_the_cap_is_refused_as_too_large() {
    // Honest headers on a part too large for this build: not inflated, and not reported as a
    // corrupt file for the missing part either.
    let over = u32::try_from(LIMITS.entry + 1).unwrap();
    let mut bytes = docx("", &[]);
    declare_size(&mut bytes, "word/document.xml", over);
    let e = refused(open_bytes("bomb.docx", &bytes));
    assert!(e.contains("declares"), "{e}");
    let content = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"><office:body><office:text/></office:body></office:document-content>"#;
    let mut odt = zip(&[("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", content.as_bytes())]);
    declare_size(&mut odt, "content.xml", over);
    refused(open_bytes("bomb.odt", &odt));
    let mut ct = docx("", &[]);
    declare_size(&mut ct, "[Content_Types].xml", over);
    refused(open_bytes("types.docx", &ct));
}

#[test]
fn a_large_legitimate_document_still_opens() {
    // About 300 pages: mail-merge-like tables, one page each, deflating ~55:1 past the ratio floor.
    let names = ["Nguyễn Văn An", "Trần Thị Bình", "Lê Văn Cường", "Phạm Thị Dung"];
    let mut body = String::new();
    for page in 0..300 {
        body.push_str("<w:tbl>");
        for row in 0..20 {
            body.push_str("<w:tr>");
            for col in 0..4 {
                let who = names[(page * 7 + row * 3 + col) % 4];
                body.push_str(&format!(r#"<w:tc><w:tcPr><w:tcW w:w="2394" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>{who} {page}-{row}-{col}</w:t></w:r></w:p></w:tc>"#));
            }
            body.push_str("</w:tr>");
        }
        body.push_str(r#"</w:tbl><w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
    }
    let bytes = docx(&body, &[]);
    assert!(body.len() as u64 > LIMITS.ratio_floor, "past the ratio floor");
    let doc = open_bytes("merge.docx", &bytes).unwrap();
    assert!(doc.body.len() >= 600);
}

#[test]
fn the_web_build_selects_the_smaller_limits() {
    assert_eq!(inflate::for_target(true), inflate::WEB);
    assert_eq!(inflate::for_target(false), inflate::NATIVE);
    assert_eq!(LIMITS, inflate::for_target(cfg!(target_arch = "wasm32")));
    assert_eq!((inflate::WEB.entry, inflate::WEB.total), (128 << 20, 256 << 20));
    assert_eq!((inflate::NATIVE.entry, inflate::NATIVE.total), (256 << 20, 512 << 20));
}
