//! When AutoSave sends a host document: a pause after the last edit, or a ceiling while edits
//! keep coming (`erp-host.md` rule 3).

/// Quiet time after the last edit before AutoSave sends it.
pub(crate) const PAUSE_MS: f64 = 3_000.0;

/// Longest an edit waits while editing goes on without a pause.
pub(crate) const CEILING_MS: f64 = 30_000.0;

/// The edits a document has had since its last save was sent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Clock {
    /// The first edit not in a sent save.
    since: Option<f64>,
    /// The latest edit.
    last: Option<f64>,
}

impl Clock {
    /// The document changed at `now`.
    pub(crate) fn edited(&mut self, now: f64) {
        self.since.get_or_insert(now);
        self.last = Some(now);
    }

    /// A save of every edit so far went out.
    pub(crate) fn sent(&mut self) {
        *self = Clock::default();
    }

    /// When AutoSave is due: the pause or the ceiling, whichever comes first. `None` without edits.
    pub(crate) fn due_at(&self) -> Option<f64> {
        let (since, last) = (self.since?, self.last?);
        Some((last + PAUSE_MS).min(since + CEILING_MS))
    }

    pub(crate) fn due(&self, now: f64) -> bool {
        self.due_at().is_some_and(|at| now >= at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pause_of_three_seconds_sends_the_edit() {
        let mut c = Clock::default();
        assert!(!c.due(1e9), "nothing to send without an edit");
        c.edited(1_000.0);
        assert!(!c.due(3_999.0));
        assert!(c.due(4_000.0));
        c.sent();
        assert!(!c.due(1e9));
    }

    #[test]
    fn edits_that_keep_coming_are_sent_after_thirty_seconds() {
        let mut c = Clock::default();
        let mut t = 0.0;
        while t < 29_000.0 {
            c.edited(t);
            assert!(!c.due(t), "at {t}");
            t += 1_000.0;
        }
        c.edited(30_000.0);
        assert!(c.due(30_000.0), "the ceiling is counted from the first unsent edit");
        c.sent();
        c.edited(31_000.0);
        assert!(!c.due(33_999.0));
        assert!(c.due(34_000.0));
    }
}
