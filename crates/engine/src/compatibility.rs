//! Live document facts shared by compatibility reports and save warnings.

use serde::{Deserialize, Serialize};

use crate::Session;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocFacts {
    #[serde(default)]
    pub alt_chunks: usize,
    pub comments: usize,
    pub tracked_changes: usize,
    pub tracked_formatting: usize,
    pub vba: bool,
    pub signed: bool,
}

impl DocFacts {
    pub fn of(s: &Session) -> Self {
        let (tracked_changes, tracked_formatting) = crate::cmd::review::tracked_change_counts(s);
        Self {
            alt_chunks: s.doc.preserved_alt_chunk_count(),
            comments: s.doc.comments.len(),
            tracked_changes,
            tracked_formatting,
            vba: s.doc.passthrough.contains_key("word/vbaProject.bin"),
            signed: s.doc.passthrough.contains_key("wordcraft:package.signed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn report_uses_live_comments_and_unresolved_changes() {
        let mut s = Session::new(wordcraft_doc::Document::from_text("Draft"));
        s.run("review.newComment", &json!({"text": "Check"})).unwrap();
        s.run("review.trackChanges", &json!({"value": true})).unwrap();
        s.run("text.insert", &json!({"text": "New "})).unwrap();
        assert_eq!(DocFacts::of(&s).comments, 1);
        assert!(DocFacts::of(&s).tracked_changes > 0);
        s.run("review.acceptAll", &json!({})).unwrap();
        assert_eq!(DocFacts::of(&s).tracked_changes, 0);
        let report = s.run("file.compatibility", &json!({})).unwrap();
        assert_eq!(report["facts"], serde_json::to_value(DocFacts::of(&s)).unwrap());
    }

    #[test]
    fn revisions_in_secondary_stories_count_but_unused_revision_records_do_not() {
        use wordcraft_doc::{Paragraph, PartKind, Revision, RevisionKind, para_block};
        use wordcraft_doc::props::CharProps;

        let mut doc = wordcraft_doc::Document::new();
        doc.revisions.push(Revision::new(RevisionKind::Insert, "Reviewer", "2026-10-21T00:00:00Z"));
        doc.revisions.push(Revision::new(RevisionKind::Delete, "Reviewer", "2026-10-21T00:00:00Z"));
        let para = Paragraph::with_text("Header edit", CharProps { ins: Some(0), ..Default::default() });
        doc.add_part(PartKind::Header, vec![para_block(para)]);
        let facts = DocFacts::of(&Session::new(doc));
        assert_eq!(facts.tracked_changes, 1);
        assert_eq!(facts.tracked_formatting, 0);
    }

    #[test]
    fn revisions_count_once_per_id_within_and_across_stories() {
        use wordcraft_doc::props::{CharProps, PropChange};
        use wordcraft_doc::{Paragraph, PartKind, Revision, RevisionKind, para_block};

        for kind in [PartKind::Header, PartKind::Footer, PartKind::Footnote, PartKind::Endnote, PartKind::Comment, PartKind::TextBox] {
            let mut doc = wordcraft_doc::Document::new();
            for revision_kind in [RevisionKind::Insert, RevisionKind::Delete, RevisionKind::Format, RevisionKind::Format] {
                doc.revisions.push(Revision::new(revision_kind, "Reviewer", "2026-10-21T00:00:00Z"));
            }
            let inserted = Paragraph::with_text("Inserted", CharProps { ins: Some(0), ..Default::default() });
            let deleted = Paragraph::with_text("Deleted", CharProps { del: Some(1), ..Default::default() });
            let formatted = Paragraph::with_text(
                "Formatted",
                CharProps { bold: Some(true), fmt_change: PropChange::boxed(2, CharProps::default()), ..Default::default() },
            );
            let blocks = vec![para_block(inserted), para_block(deleted), para_block(formatted)];
            doc.add_part(kind, blocks.clone());
            let mut s = Session::new(doc);
            let facts = DocFacts::of(&s);
            assert_eq!(facts.tracked_changes, 3, "{kind:?}");
            assert_eq!(facts.tracked_formatting, 1, "{kind:?}");

            s.doc.body = blocks.clone();
            s.doc.add_part(PartKind::Header, blocks);
            assert_eq!(DocFacts::of(&s), facts, "{kind:?}");
        }
    }

    #[test]
    fn json_copy_does_not_claim_to_keep_the_original_signature() {
        let mut doc = wordcraft_doc::Document::new();
        doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
        let bytes = crate::io::save_bytes("copy.json", &doc).unwrap();
        let copy = crate::io::open_bytes("copy.json", &bytes).unwrap();
        assert!(doc.passthrough.contains_key("wordcraft:package.signed"));
        assert!(!DocFacts::of(&Session::new(copy)).signed);
    }
}
