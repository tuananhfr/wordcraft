//! DOCX `altChunk`s holding HTML, MHTML, RTF or plain text become document content when the file
//! opens, as Word imports them; chunks of other types stay preserved for saving.

use std::sync::Arc;

use wordcraft_doc::para::PreservedAltChunk;
use wordcraft_doc::{Block, Document, Paragraph};
use wordcraft_formats::{html, mhtml, rtf, txt};

use crate::Session;

/// The depth [`Document::preserved_alt_chunk_count`] counts to.
const MAX_DEPTH: usize = 16;

type Imported = std::vec::IntoIter<Option<Vec<Block>>>;

pub(super) fn expand(doc: &mut Document) {
    if doc.preserved_alt_chunk_count() == 0 {
        return;
    }
    let mut sources = Vec::new();
    for blocks in std::iter::once(&doc.body).chain(doc.parts.values().map(|p| &p.blocks)) {
        each_chunk(blocks, 0, &mut |chunk| sources.push(convert(chunk)));
    }
    // Adopting a chunk's styles, lists and pictures leaves the stories alone, so the results
    // splice back in the order they were read.
    let mut s = Session::new(std::mem::take(doc));
    let mut imported: Imported = sources.into_iter().map(|src| adopt(&mut s, src?)).collect::<Vec<_>>().into_iter();
    splice(&mut s.doc.body, 0, &mut imported);
    for part in s.doc.parts.values_mut() {
        splice(&mut part.blocks, 0, &mut imported);
    }
    *doc = s.doc;
}

fn convert(chunk: &PreservedAltChunk) -> Option<Document> {
    let mime = chunk.content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let bytes = chunk.bytes.as_slice();
    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => Some(html::import(&html::decode(bytes))),
        "message/rfc822" | "multipart/related" | "application/x-mimearchive" => mhtml::to_html(bytes).map(|h| html::import(&h)),
        "application/rtf" | "text/rtf" => rtf::import(bytes).ok(),
        "text/plain" => Some(txt::import(bytes)),
        _ => None,
    }
}

fn adopt(s: &mut Session, src: Document) -> Option<Vec<Block>> {
    let mut blocks = crate::cmd::clipboard_adopt::adopt(s, src, false, true).ok()?.blocks;
    // The file keeps its own sections; an imported page setup would split them.
    for b in &mut blocks {
        crate::cmd::paste::each_para(b, 0, &mut |p| {
            if p.section.take().is_some() {
                p.touch();
            }
        });
    }
    Some(blocks)
}

fn each_chunk(blocks: &[Arc<Block>], depth: usize, f: &mut dyn FnMut(&PreservedAltChunk)) {
    for b in blocks {
        match &**b {
            Block::Para(p) => p.preserved_before.iter().for_each(&mut *f),
            Block::Table(t) if depth < MAX_DEPTH => {
                for cell in t.rows.iter().flat_map(|r| &r.cells) {
                    each_chunk(&cell.blocks, depth + 1, f);
                }
            }
            Block::Table(_) => {}
        }
    }
}

fn holds_chunks(block: &Arc<Block>, depth: usize) -> bool {
    let mut any = false;
    each_chunk(std::slice::from_ref(block), depth, &mut |_| any = true);
    any
}

fn splice(blocks: &mut Vec<Arc<Block>>, depth: usize, imported: &mut Imported) {
    let mut out = Vec::with_capacity(blocks.len());
    for mut block in std::mem::take(blocks) {
        if holds_chunks(&block, depth) {
            match Arc::make_mut(&mut block) {
                Block::Para(p) => {
                    let mut kept = Vec::new();
                    for chunk in std::mem::take(&mut p.preserved_before) {
                        match imported.next().flatten() {
                            Some(content) => push_content(&mut out, content, &mut kept),
                            None => kept.push(chunk),
                        }
                    }
                    p.preserved_before = kept;
                    p.touch();
                }
                Block::Table(t) => {
                    for cell in t.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
                        splice(&mut cell.blocks, depth + 1, imported);
                    }
                }
            }
        }
        out.push(block);
    }
    *blocks = out;
}

/// Chunks kept ahead of imported content ride on a paragraph in front of it.
fn push_content(out: &mut Vec<Arc<Block>>, content: Vec<Block>, kept: &mut Vec<PreservedAltChunk>) {
    let mut content = content.into_iter().peekable();
    if !kept.is_empty() && content.peek().is_some() {
        let mut carrier = match content.next_if(|b| matches!(b, Block::Para(_))) {
            Some(Block::Para(p)) => p,
            _ => Paragraph::new(),
        };
        carrier.preserved_before.splice(0..0, kept.drain(..));
        carrier.touch();
        out.push(Arc::new(Block::Para(carrier)));
    }
    out.extend(content.map(Arc::new));
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use wordcraft_doc::{Block, Document, StoryRef};

    const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    const SECT: &str = r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="850" w:right="1134" w:bottom="850" w:left="1417" w:header="0" w:footer="397" w:gutter="0"/></w:sectPr>"#;
    const PNG_1PX: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF,
        0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60,
        0x82,
    ];

    struct Chunk<'a> {
        id: &'a str,
        target: &'a str,
        content_type: &'a str,
        bytes: Vec<u8>,
    }

    fn chunk<'a>(id: &'a str, target: &'a str, content_type: &'a str, bytes: impl Into<Vec<u8>>) -> Chunk<'a> {
        Chunk { id, target, content_type, bytes: bytes.into() }
    }

    fn package(body: &str, chunks: &[Chunk]) -> Vec<u8> {
        let document = format!(r#"<w:document {NS}><w:body>{body}</w:body></w:document>"#);
        let rels: String = chunks
            .iter()
            .map(|c| {
                format!(
                    r#"<Relationship Id="{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/aFChunk" Target="{}"/>"#,
                    c.id, c.target
                )
            })
            .collect();
        let rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#);
        let types: String = chunks.iter().map(|c| format!(r#"<Override PartName="/word/{}" ContentType="{}"/>"#, c.target, c.content_type)).collect();
        let types = format!(
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>{types}</Types>"#
        );
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let mut files = vec![
            ("word/document.xml".to_string(), document.into_bytes()),
            ("word/_rels/document.xml.rels".to_string(), rels.into_bytes()),
            ("[Content_Types].xml".to_string(), types.into_bytes()),
        ];
        files.extend(chunks.iter().map(|c| (format!("word/{}", c.target), c.bytes.clone())));
        for (name, bytes) in files {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn open(body: &str, chunks: &[Chunk]) -> Document {
        crate::io::open_bytes("chunk.docx", &package(body, chunks)).unwrap()
    }

    fn para(text: &str) -> String {
        format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
    }

    fn mht(html: &str, image: Option<&[u8]>) -> Vec<u8> {
        let b64 = wordcraft_formats::model::base64_encode;
        let mut out = format!(
            "MIME-Version: 1.0\r\nContent-Type: multipart/related; type=\"text/html\"; boundary=\"----=part\"\r\n\r\n\
             ------=part\r\nContent-Type: text/html; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\nContent-Location: file:///C:/erp/document.html\r\n\r\n{}\r\n",
            b64(html.as_bytes())
        );
        if let Some(png) = image {
            out.push_str(&format!(
                "------=part\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: base64\r\nContent-Location: file:///C:/erp/image1.png\r\n\r\n{}\r\n",
                b64(png)
            ));
        }
        out.push_str("------=part--\r\n");
        out.into_bytes()
    }

    #[test]
    fn mhtml_chunk_alone_in_the_body_becomes_its_content() {
        let html = r#"<html><body><p><img src="file:///C:/erp/image1.png" width="40" height="40"></p><p>PHIẾU GIAO HÀNG</p><table><tr><td>STT</td><td>Nội dung hàng</td></tr><tr><td>1</td><td>Hàng mẫu</td></tr></table></body></html>"#;
        let doc = open(
            &format!(r#"<w:altChunk r:id="htmlChunk"/>{SECT}"#),
            &[chunk("htmlChunk", "afchunk.mht", "message/rfc822", mht(html, Some(PNG_1PX)))],
        );
        let text = doc.plain_text(StoryRef::Body);
        assert!(text.contains("PHIẾU GIAO HÀNG"), "{text}");
        assert!(text.contains("Nội dung hàng") && text.contains("Hàng mẫu"), "{text}");
        assert!(doc.body.iter().any(|b| matches!(&**b, Block::Table(_))));
        assert_eq!(doc.media.len(), 1, "the archive's picture is imported");
        assert_eq!(doc.preserved_alt_chunk_count(), 0);
        assert_eq!(doc.sections().len(), 1);
        let sect = doc.sections()[0].1;
        assert!((sect.page_w - 595.3).abs() < 1.0, "the file's page setup is kept: {}", sect.page_w);
    }

    #[test]
    fn html_chunk_lands_between_its_neighbours() {
        let doc = open(
            &format!(r#"{}<w:altChunk r:id="c"/>{}{SECT}"#, para("A"), para("C")),
            &[chunk("c", "chunk.html", "text/html", "<html><body><p>B-CHUNK</p></body></html>")],
        );
        let text = doc.plain_text(StoryRef::Body);
        let (a, b, c) = (text.find('A').unwrap(), text.find("B-CHUNK").unwrap(), text.find('C').unwrap());
        assert!(a < b && b < c, "{text:?}");
        assert_eq!(doc.preserved_alt_chunk_count(), 0);
        assert_eq!(doc.sections().len(), 1, "imported blocks add no section break");
    }

    #[test]
    fn chunk_in_a_table_cell() {
        let body = format!(r#"<w:tbl><w:tr><w:tc>{}<w:altChunk r:id="c"/>{}</w:tc></w:tr></w:tbl>{}{SECT}"#, para("A"), para("C"), para("after"));
        let doc = open(&body, &[chunk("c", "chunk.html", "text/html", "<p>IN-CELL</p>")]);
        let Some(Block::Table(t)) = doc.body.first().map(|b| &**b) else { panic!("table first") };
        let cell: String = t.rows[0].cells[0].blocks.iter().filter_map(|b| b.as_para()).map(|p| p.text.clone()).collect::<Vec<_>>().join("|");
        assert_eq!(cell, "A|IN-CELL|C");
        assert_eq!(doc.preserved_alt_chunk_count(), 0);
    }

    #[test]
    fn rtf_and_plain_text_chunks() {
        let body = format!(r#"<w:altChunk r:id="r"/><w:altChunk r:id="t"/>{}{SECT}"#, para("end"));
        let doc = open(
            &body,
            &[chunk("r", "chunk.rtf", "application/rtf", r"{\rtf1\ansi RTF-CHUNK\par}"), chunk("t", "chunk.txt", "text/plain", "TEXT-CHUNK")],
        );
        let text = doc.plain_text(StoryRef::Body);
        let (r, t, e) = (text.find("RTF-CHUNK").unwrap(), text.find("TEXT-CHUNK").unwrap(), text.find("end").unwrap());
        assert!(r < t && t < e, "{text:?}");
        assert_eq!(doc.preserved_alt_chunk_count(), 0);
    }

    #[test]
    fn unsupported_chunks_stay_preserved_in_order() {
        let body = format!(r#"<w:altChunk r:id="x"/><w:altChunk r:id="h"/>{}{SECT}"#, para("end"));
        let doc = open(&body, &[chunk("x", "chunk.bin", "application/x-unknown", "opaque"), chunk("h", "chunk.html", "text/html", "<p>B-CHUNK</p>")]);
        assert_eq!(doc.preserved_alt_chunk_count(), 1);
        let first = doc.body.first().and_then(|b| b.as_para()).unwrap();
        assert_eq!(first.text, "B-CHUNK", "the kept chunk rides on the next paragraph");
        assert_eq!(first.preserved_before.len(), 1);
        assert_eq!(first.preserved_before[0].content_type, "application/x-unknown");
    }

    #[test]
    fn saving_writes_the_imported_content_not_the_chunk() {
        let doc = open(&format!(r#"{}<w:altChunk r:id="c"/>{SECT}"#, para("A")), &[chunk("c", "chunk.html", "text/html", "<p>B-CHUNK</p>")]);
        let bytes = crate::io::save_bytes("out.docx", &doc).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
        assert!(!xml.contains("altChunk"), "{xml}");
        assert!(xml.contains("B-CHUNK"), "{xml}");
        assert!(zip.by_name("word/chunk.html").is_err());
    }

    #[test]
    fn hostile_chunks_open_without_panicking() {
        let garbage: Vec<u8> = (0..=255u8).cycle().take(50_000).collect();
        for content_type in ["text/html", "message/rfc822", "multipart/related", "application/rtf", "text/plain", "application/xhtml+xml"] {
            for bytes in [garbage.clone(), Vec::new(), b"Content-Type: multipart/related; boundary=b\n\n--b".to_vec(), b"{\\rtf1".to_vec()] {
                let doc = open(&format!(r#"<w:altChunk r:id="c"/>{SECT}"#), &[chunk("c", "chunk.bin", content_type, bytes)]);
                assert!(!doc.body.is_empty());
            }
        }
    }
}
