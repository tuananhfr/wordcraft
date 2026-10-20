use std::io::{Cursor, Write};

#[test]
fn signature_parts_remain_detectable_after_opening() {
    for name in ["_xmlsignatures/sig1.xml", "signatures/custom.xml"] {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p/></w:body></w:document>"#).unwrap();
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"<Signature/>").unwrap();
        zip.start_file("[Content_Types].xml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(format!(r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/{name}" ContentType="application/vnd.openxmlformats-package.digital-signature-xmlsignature+xml"/></Types>"#).as_bytes()).unwrap();
        let doc = wordcraft_docx::read(&zip.finish().unwrap().into_inner()).unwrap();
        assert!(doc.passthrough.contains_key("wordcraft:package.signed"));
    }
    let bytes = wordcraft_docx::write(&wordcraft_doc::Document::new()).unwrap();
    assert!(!wordcraft_docx::read(&bytes).unwrap().passthrough.contains_key("wordcraft:package.signed"));
}
