//! Legacy Word input classification and open metadata shared by desktop and web.

use wordcraft_doc::Document;

const OLE2: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const REFUSED: &str = "wordcraft:legacy-doc-refused:";

pub struct OpenedDocument {
    pub doc: Document,
    pub encrypted: bool,
    pub legacy: bool,
}

/// Recognize an OLE2 container independently of its file name.
pub fn has_ole2_header(bytes: &[u8]) -> bool {
    bytes.starts_with(OLE2)
}

/// Encrypted OOXML also uses OLE2; it must retain its password flow.
pub fn is_legacy_doc(name: &str, bytes: &[u8]) -> bool {
    if wordcraft_docx::is_encrypted(bytes) {
        return false;
    }
    has_ole2_header(bytes)
        || (matches!(super::ext_of(name).as_str(), "doc" | "dot") && !bytes.starts_with(b"PK\x03\x04") && !bytes.starts_with(b"{\\rtf"))
}

/// Whether an open error came from an unsupported or unreadable legacy document.
pub fn refused(err: &str) -> bool {
    err.contains(REFUSED)
}

/// Propose a DOCX destination distinct from the binary input.
pub fn save_path(name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(name);
    let proposed = path.with_extension("docx");
    if super::ext_of(name) == "docx" {
        path.with_file_name(format!("{}.docx", path.file_name().unwrap_or_default().to_string_lossy()))
    } else {
        proposed
    }
}

pub(super) fn opened(name: &str, bytes: &[u8], result: Result<(Document, bool), String>) -> Result<OpenedDocument, String> {
    let legacy = is_legacy_doc(name, bytes);
    match result {
        Ok((doc, encrypted)) => Ok(OpenedDocument { doc, encrypted, legacy }),
        Err(err) if legacy && !super::too_large(&err) => Err(format!("{REFUSED} {err}")),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ole2_is_legacy_regardless_of_extension() {
        for name in ["old.doc", "old.DOC", "old.docx", "old.txt", "old.html", "old", "old.bin"] {
            assert!(is_legacy_doc(name, OLE2), "{name}");
        }
        assert!(is_legacy_doc("old.doc", &[]));
    }

    #[test]
    fn zip_and_plain_text_are_not_ole2() {
        for bytes in [b"PK\x03\x04".as_slice(), b"Plain text", b"", &OLE2[..7]] {
            assert!(!has_ole2_header(bytes));
            assert!(!is_legacy_doc("document.txt", bytes));
        }
        assert!(!is_legacy_doc("renamed.doc", b"PK\x03\x04"));
        assert!(!is_legacy_doc("renamed.doc", br"{\rtf1 text}"));
    }

    #[test]
    fn ole2_never_falls_through_to_a_text_importer() {
        for name in ["old.doc", "old.txt", "old.html", "old.xml", "old.json", "old.md", "old.bin"] {
            let err = super::super::open_bytes(name, OLE2).unwrap_err();
            assert!(refused(&err), "{name}: {err}");
        }
    }

    #[test]
    fn successful_legacy_reads_carry_conversion_metadata() {
        let result = opened("old.doc", OLE2, Ok((Document::from_text("Kept"), false))).unwrap();
        assert!(result.legacy);
        assert!(!result.encrypted);
    }

    #[test]
    fn legacy_save_names_always_propose_a_separate_docx() {
        for (source, proposed) in [
            ("old.doc", "old.docx"),
            ("old.DOC", "old.docx"),
            ("old.txt", "old.docx"),
            ("old.docx", "old.docx.docx"),
            ("old.DOCX", "old.DOCX.docx"),
        ] {
            assert_eq!(save_path(source), std::path::Path::new(proposed));
            assert_ne!(save_path(source), std::path::Path::new(source));
        }
    }
}
