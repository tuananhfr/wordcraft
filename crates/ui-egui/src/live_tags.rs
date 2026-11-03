//! Name labels of co-editing on the page: which member names to draw and how. A name shows once
//! per member and place, and shrinks to an initial when the page is too far zoomed out for a
//! name to fit over a line.

use egui::{Color32, FontId, Painter, Pos2, Rect, Vec2, vec2};
use wordcraft_engine::live::{Lock, Region};

/// Below this zoom (a phone fitting the page width shows it at ~36%) an 11 pt name covers lines.
const COMPACT_BELOW: f32 = 0.6;

/// How labels look at the page's zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TagLook {
    pub compact: bool,
}

impl TagLook {
    pub(crate) fn at_zoom(zoom: f32) -> TagLook {
        TagLook { compact: zoom < COMPACT_BELOW }
    }

    pub(crate) fn text(self, name: &str) -> String {
        if self.compact { initial(name) } else { name.to_string() }
    }

    fn font(self) -> f32 {
        if self.compact { 9.0 } else { 11.0 }
    }
}

/// The letter the shell's avatars show: Vietnamese names end with the given name, the one
/// people are called by.
pub(crate) fn initial(name: &str) -> String {
    name.split_whitespace().last().and_then(|w| w.chars().next()).map(|c| c.to_uppercase().collect()).unwrap_or_default()
}

/// Whether `author`'s caret, in top-level region `at`, sits where their own lock already names them.
pub(crate) fn named_by_own_lock(author: &str, at: Option<Region>, locks: &[Lock]) -> bool {
    locks.iter().filter(|l| l.holder == author).filter_map(|l| Region::parse(&l.region)).any(|r| match at {
        Some(at) => r.clashes(&at),
        None => r == Region::Doc,
    })
}

/// Paints `name` (as `look` shows it) on a chip whose top-left corner `place` picks from its size.
pub(crate) fn tag(p: &Painter, name: &str, fill: Color32, look: TagLook, place: impl FnOnce(Vec2) -> Pos2) {
    let text = look.text(name);
    if text.is_empty() {
        return;
    }
    let galley = p.layout_no_wrap(text, FontId::proportional(look.font()), Color32::WHITE);
    let size = galley.size() + vec2(6.0, 2.0);
    let rect = Rect::from_min_size(place(size), size);
    p.rect_filled(rect, 2.0, fill);
    p.galley(rect.min + vec2(3.0, 1.0), galley, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(region: Region, holder: &str) -> Lock {
        Lock { region: region.id(), holder: holder.into(), name: "An".into(), colour: "#cc3300".into() }
    }

    #[test]
    fn initials_are_the_given_names_first_letter() {
        assert_eq!(initial("Lê Thứ Cá Nhân"), "N");
        assert_eq!(initial("  đức "), "Đ");
        assert_eq!(initial("an"), "A");
        assert_eq!(initial("   "), "");
    }

    #[test]
    fn names_shrink_to_an_initial_only_when_zoomed_far_out() {
        assert_eq!(TagLook::at_zoom(1.0).text("Lê Thứ Cá Nhân"), "Lê Thứ Cá Nhân");
        assert_eq!(TagLook::at_zoom(0.6).text("Lê Thứ Cá Nhân"), "Lê Thứ Cá Nhân");
        assert_eq!(TagLook::at_zoom(0.36).text("Lê Thứ Cá Nhân"), "N");
        assert!(TagLook::at_zoom(f32::NAN).text("An") == "An", "a broken zoom keeps the full name");
    }

    #[test]
    fn a_caret_is_named_by_its_owners_lock_only() {
        let locks = [lock(Region::Para(7), "m1"), lock(Region::Table(9), "m2")];
        assert!(named_by_own_lock("m1", Some(Region::Para(7)), &locks));
        assert!(!named_by_own_lock("m2", Some(Region::Para(7)), &locks), "another member's lock");
        assert!(!named_by_own_lock("m1", Some(Region::Para(8)), &locks), "outside the lock");
        assert!(named_by_own_lock("m2", Some(Region::Table(9)), &locks));
        assert!(!named_by_own_lock("m1", None, &locks), "a block without an id");
        let whole = [lock(Region::Doc, "m1")];
        assert!(named_by_own_lock("m1", Some(Region::Para(8)), &whole) && named_by_own_lock("m1", None, &whole));
        assert!(!named_by_own_lock("m2", Some(Region::Para(8)), &whole));
    }
}
