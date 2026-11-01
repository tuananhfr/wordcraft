//! The ids this client makes while editing (`erpHost.setClientId`; scheme in `wordcraft_doc::ids`):
//! paragraph and row ids after every edit, and new list and comment ids.

use wordcraft_doc::Document;
use wordcraft_doc::ids::{ClientIds, IdKind};

use crate::{CmdError, Session};

const NO_FREE_ID: &str = "this document has no free id left";

/// A new list id (`numId` or `abstractNumId`) unused in `doc` and not in `also` (ids made for
/// the same change and not added yet).
pub(crate) fn new_list_id(ids: &ClientIds, doc: &Document, also: &[u32]) -> Result<u32, CmdError> {
    let taken = doc.list_ids();
    ids.mint(IdKind::List, |id| taken.contains(&id) || also.contains(&id)).ok_or_else(|| CmdError::Failed(NO_FREE_ID.into()))
}

impl Session {
    /// Makes the ids this client creates from now on carry `prefix` (6 lowercase hex digits);
    /// `false` and no change for anything else.
    pub fn set_client_prefix(&mut self, prefix: &str) -> bool {
        self.ids.set_prefix(prefix)
    }

    pub fn client_prefix(&self) -> String {
        self.ids.prefix()
    }

    pub(crate) fn client_ids(&self) -> &ClientIds {
        &self.ids
    }

    /// A new list's abstract list id and list id.
    pub(crate) fn new_list_ids(&self) -> Result<(u32, u32), CmdError> {
        let abstract_id = new_list_id(&self.ids, &self.doc, &[])?;
        Ok((abstract_id, new_list_id(&self.ids, &self.doc, &[abstract_id])?))
    }

    pub(crate) fn new_comment_id(&self) -> Result<u32, CmdError> {
        let taken = &self.doc.comments;
        self.ids.mint(IdKind::Comment, |id| taken.contains_key(&id)).ok_or_else(|| CmdError::Failed(NO_FREE_ID.into()))
    }

    /// Gives every paragraph and row an edit left without an id, or with a copy of another's,
    /// a new one (`before`: the document before the edit, to tell the original from its copy).
    pub(crate) fn ensure_block_ids(&mut self, before: Option<&Document>) {
        let ids = self.ids.clone();
        wordcraft_doc::ids::ensure_ids(&mut self.doc, before, &mut |taken| ids.mint(IdKind::Block, |id| taken.contains(&id)));
    }
}
