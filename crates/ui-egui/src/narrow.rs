//! Phone-width layout: below [`NARROW_WIDTH`] the ribbon folds away and side panes float over the
//! page instead of squeezing it.

/// Window content width (points) below which the interface compacts. A tablet in portrait
/// (768+ points) stays wide; a phone (360–430) is narrow.
pub const NARROW_WIDTH: f32 = 600.0;

/// Whether the window is narrow, and the ribbon's folded state while it is.
///
/// The persisted [`crate::UiState::ribbon_collapsed`] stays the user's wide-window choice: while
/// narrow the ribbon follows [`Narrow::ribbon_collapsed`] instead, so going back to a wide window
/// restores that choice and a phone session never overwrites it.
#[derive(Clone, Debug, Default)]
pub struct Narrow {
    /// `None` until the first width is seen, so a window that starts narrow compacts too.
    narrow: Option<bool>,
    ribbon_collapsed: bool,
}

impl Narrow {
    /// Follow the window's content width. Only a crossing of the breakpoint changes anything,
    /// so a ribbon the user expanded while narrow stays expanded.
    pub fn update(&mut self, width: f32) {
        // Junk widths (a window not yet sized) count as wide: nothing compacts on them.
        let now = width.is_finite() && width > 0.0 && width < NARROW_WIDTH;
        if self.narrow == Some(now) {
            return;
        }
        if now {
            self.ribbon_collapsed = true;
        }
        self.narrow = Some(now);
    }

    pub fn is_narrow(&self) -> bool {
        self.narrow == Some(true)
    }

    /// The ribbon's folded state shown now, given the user's wide-window choice.
    pub fn ribbon_collapsed(&self, wide_choice: bool) -> bool {
        if self.is_narrow() { self.ribbon_collapsed } else { wide_choice }
    }

    /// Fold or unfold the ribbon (`ui.collapseRibbon`); returns the new shown state.
    pub fn toggle_ribbon(&mut self, wide_choice: &mut bool) -> bool {
        if self.is_narrow() {
            self.ribbon_collapsed = !self.ribbon_collapsed;
            self.ribbon_collapsed
        } else {
            *wide_choice = !*wide_choice;
            *wide_choice
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: f32 = 390.0;
    const TABLET: f32 = 768.0;
    const DESKTOP: f32 = 1366.0;

    #[test]
    fn crossing_into_a_narrow_window_folds_the_ribbon_once() {
        let mut n = Narrow::default();
        let mut wide = false;
        n.update(DESKTOP);
        assert!(!n.is_narrow());
        assert!(!n.ribbon_collapsed(wide));
        n.update(PHONE);
        assert!(n.is_narrow());
        assert!(n.ribbon_collapsed(wide), "wide -> narrow folds the ribbon");
        // Staying narrow (another phone width) doesn't fold it again after the user unfolds it.
        assert!(!n.toggle_ribbon(&mut wide), "the user unfolds it");
        for w in [PHONE, 420.0, 599.0, PHONE] {
            n.update(w);
            assert!(!n.ribbon_collapsed(wide), "a manual expand sticks while narrow ({w})");
        }
        assert!(!wide, "the wide choice is untouched while narrow");
    }

    #[test]
    fn going_back_to_a_wide_window_restores_the_users_wide_choice() {
        for wide_choice in [false, true] {
            let mut n = Narrow::default();
            let mut wide = wide_choice;
            n.update(DESKTOP);
            n.update(PHONE);
            assert!(n.ribbon_collapsed(wide));
            n.toggle_ribbon(&mut wide);
            n.update(TABLET);
            assert!(!n.is_narrow(), "a tablet in portrait is wide");
            assert_eq!(n.ribbon_collapsed(wide), wide_choice, "narrow -> wide restores the wide choice");
            // And crossing again folds it again, whatever was chosen last time while narrow.
            n.update(PHONE);
            assert!(n.ribbon_collapsed(wide), "the next crossing compacts again");
        }
    }

    #[test]
    fn a_window_that_starts_narrow_compacts_and_junk_widths_count_as_wide() {
        let mut n = Narrow::default();
        n.update(PHONE);
        assert!(n.is_narrow() && n.ribbon_collapsed(false), "the first frame already narrow compacts");
        let mut j = Narrow::default();
        for w in [f32::NAN, f32::INFINITY, 0.0, -5.0] {
            j.update(w);
            assert!(!j.is_narrow(), "{w} is not a narrow window");
        }
        assert!(!Narrow::default().is_narrow(), "nothing seen yet: wide");
        let mut edge = Narrow::default();
        edge.update(NARROW_WIDTH);
        assert!(!edge.is_narrow(), "the breakpoint itself is wide");
    }

    #[test]
    fn toggling_while_wide_changes_only_the_wide_choice() {
        let mut n = Narrow::default();
        let mut wide = false;
        n.update(DESKTOP);
        assert!(n.toggle_ribbon(&mut wide));
        assert!(wide);
        assert!(n.ribbon_collapsed(wide));
        assert!(!n.toggle_ribbon(&mut wide));
        assert!(!wide);
    }
}
