//! Region ids of a shared document (`erpHost` v3): a top-level paragraph `para:<paraId>`, a
//! top-level table `table:<paraId of its first row>`, or the whole document `*`. Ids are the
//! eight upper-case hex digits of `w14:paraId` (`wordcraft_doc::ids`).

use wordcraft_doc::Block;
use wordcraft_doc::ids::{format_para_id, parse_para_id};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Region {
    Para(u32),
    Table(u32),
    Doc,
}

impl Region {
    /// The region of a top-level block; `None` for one without an id yet.
    pub fn of(b: &Block) -> Option<Region> {
        match b {
            Block::Para(p) => p.id.map(Region::Para),
            Block::Table(t) => t.rows.first().and_then(|r| r.id).map(Region::Table),
        }
    }

    pub fn id(&self) -> String {
        match self {
            Region::Para(i) => format!("para:{}", format_para_id(*i)),
            Region::Table(i) => format!("table:{}", format_para_id(*i)),
            Region::Doc => "*".into(),
        }
    }

    pub fn parse(s: &str) -> Option<Region> {
        if s == "*" {
            return Some(Region::Doc);
        }
        let (kind, id) = s.split_once(':')?;
        let id = parse_para_id(id)?;
        match kind {
            "para" => Some(Region::Para(id)),
            "table" => Some(Region::Table(id)),
            _ => None,
        }
    }

    /// The top-level block id the region stands for (none for `*`).
    pub fn block(&self) -> Option<u32> {
        match self {
            Region::Para(i) | Region::Table(i) => Some(*i),
            Region::Doc => None,
        }
    }

    /// Whether two members can't hold both at once (the server's rule: `*` clashes with all).
    pub fn clashes(&self, other: &Region) -> bool {
        *self == Region::Doc || *other == Region::Doc || self.block() == other.block()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse_back_and_clash_like_the_server() {
        for r in [Region::Para(0x1A2B), Region::Table(0x7FFF_FFFF), Region::Doc] {
            assert_eq!(Region::parse(&r.id()), Some(r));
        }
        assert_eq!(Region::Para(0x1A2B).id(), "para:00001A2B");
        assert_eq!(Region::parse("para:0"), None);
        assert_eq!(Region::parse("cell:f1:A1"), None);
        assert!(Region::Doc.clashes(&Region::Para(1)));
        assert!(Region::Para(1).clashes(&Region::Para(1)));
        assert!(!Region::Para(1).clashes(&Region::Table(2)));
    }
}
