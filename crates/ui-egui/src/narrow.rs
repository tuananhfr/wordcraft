//! Phone-width layout: below [`NARROW_WIDTH`] the ribbon folds away, the page fits the width
//! without rulers, and side panes float over the page instead of squeezing it.

use serde_json::json;
use wordcraft_engine::ViewState;

use crate::WordApp;

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
    /// The documents open while narrow, with the zoom and rulers each had in the wide window.
    documents: Vec<Fitted>,
    /// The document shown in the last narrow frame.
    shown: Option<u64>,
}

#[derive(Clone, Debug)]
struct Fitted {
    document: u64,
    wide: WideView,
    fitted: bool,
}

/// What fitting a document to a narrow window changes, kept to put back when the window widens.
#[derive(Clone, Debug, PartialEq)]
struct WideView {
    zoom: f32,
    fit: String,
    multi_page: bool,
    single_column: bool,
    ruler: bool,
}

impl WideView {
    fn of(v: &ViewState) -> WideView {
        WideView { zoom: v.zoom, fit: v.fit.clone(), multi_page: v.multi_page, single_column: v.single_column, ruler: v.ruler }
    }

    fn put_back(self, v: &mut ViewState) {
        (v.zoom, v.fit, v.multi_page, v.single_column, v.ruler) = (self.zoom, self.fit, self.multi_page, self.single_column, self.ruler);
    }
}

/// Follow the window width: fit each document to a narrow window once, the first time it shows
/// there, and give every document its wide zoom and rulers back when the window widens.
pub(crate) fn follow_width(app: &mut WordApp, width: f32) {
    let was_narrow = app.narrow.is_narrow();
    app.narrow.update(width);
    if app.narrow.is_narrow() {
        if !was_narrow {
            let parked = app.docs.parked().map(|(_, p)| (p.session.document_id(), &p.session.view));
            app.narrow.begin(std::iter::once((app.session.document_id(), &app.session.view)).chain(parked));
        }
        if app.narrow.show(app.session.document_id(), &app.session.view) {
            // View › Page Width, so the zoom then behaves as if the user had picked it.
            let _ = app.session.run("view.pageWidth", &json!({}));
            app.session.view.ruler = false;
        }
    } else if was_narrow {
        app.narrow.restore(app.session.document_id(), &mut app.session.view);
        for s in app.docs.parked_sessions_mut() {
            app.narrow.restore(s.document_id(), &mut s.view);
        }
        app.narrow.documents.clear();
        app.narrow.shown = None;
    }
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

    /// The window just narrowed: note each open document's wide view, to fit it when shown.
    fn begin<'a>(&mut self, documents: impl Iterator<Item = (u64, &'a ViewState)>) {
        self.documents = documents.map(|(document, v)| Fitted { document, wide: WideView::of(v), fitted: false }).collect();
        self.shown = None;
    }

    /// A narrow frame shows `document`; true when it should be fitted now, the first time it
    /// shows. Later zooms and ruler changes are the user's and stand.
    fn show(&mut self, document: u64, view: &ViewState) -> bool {
        let fit = if let Some(d) = self.documents.iter_mut().find(|d| d.document == document) {
            !std::mem::replace(&mut d.fitted, true)
        } else {
            // Opened or created while narrow: its view is a copy of the one shown before (see
            // `Session::sibling`), and so is the wide view it should get back.
            let before = self.shown.and_then(|s| self.documents.iter().find(|d| d.document == s)).map(|d| d.wide.clone());
            let fit = before.is_none();
            self.documents.push(Fitted { document, wide: before.unwrap_or_else(|| WideView::of(view)), fitted: true });
            fit
        };
        self.shown = Some(document);
        fit
    }

    /// Put back the wide view of a document fitted while narrow.
    fn restore(&mut self, document: u64, view: &mut ViewState) {
        if let Some(i) = self.documents.iter().position(|d| d.document == document) {
            self.documents.swap_remove(i).wide.put_back(view);
        }
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
    fn each_document_is_fitted_once_per_narrow_spell_and_gets_its_own_view_back() {
        let mut n = Narrow::default();
        let (mut a, mut b) = (ViewState::default(), ViewState::default());
        b.zoom = 1.5;
        b.ruler = false;
        let (wide_a, wide_b) = (WideView::of(&a), WideView::of(&b));
        n.update(PHONE);
        n.begin([(1, &a), (2, &b)].into_iter());
        assert!(n.show(1, &a), "the shown document is fitted");
        assert!(!n.show(1, &a), "once");
        a.zoom = 0.4;
        a.ruler = false;
        // A document opened now copies the shown one's (narrow) view: it isn't fitted, and the
        // wide view it gets back is the one it was copied from.
        let mut c = a.clone();
        assert!(!n.show(3, &c));
        assert!(n.show(2, &b), "a parked document is fitted when first shown");
        b.zoom = 0.7;
        n.restore(2, &mut b);
        n.restore(1, &mut a);
        n.restore(3, &mut c);
        assert_eq!((WideView::of(&a), WideView::of(&b), WideView::of(&c)), (wide_a.clone(), wide_b, wide_a));
        let before = WideView::of(&a);
        n.restore(1, &mut a);
        assert_eq!(WideView::of(&a), before, "a second restore changes nothing");
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
