//! v2 fields read from the host's values (`doc.mode`, `doc.canDownload`, `setUser`), kept apart
//! from the JS calls so they can be checked without a browser.

use wordcraft_ui_egui::host_docs::HostMode;

/// The `erpHost.version` values this app takes; anything else leaves `erpHost` unused.
pub(crate) fn supported(version: Option<f64>) -> Option<u8> {
    if version == Some(1.0) {
        Some(1)
    } else if version == Some(2.0) {
        Some(2)
    } else {
        None
    }
}

/// `doc.mode` / `setMode`'s mode: absent is `edit`; a value outside the contract can't be trusted
/// to allow editing, so it views.
pub(crate) fn mode(value: Option<&str>, present: bool) -> HostMode {
    match value {
        Some("edit") => HostMode::Edit,
        Some("comment") => HostMode::Comment,
        Some("view") => HostMode::View,
        None if !present => HostMode::Edit,
        _ => HostMode::View,
    }
}

/// `doc.canDownload` / `setMode`'s flag: absent is `true`; anything but a boolean refuses.
pub(crate) fn can_download(value: Option<bool>, present: bool) -> bool {
    value.unwrap_or(!present)
}

/// `setUser(user)`'s name: `null` (an anonymous link viewer) or no usable name keeps the
/// preferences' name.
pub(crate) fn user_name(name: Option<String>) -> Option<String> {
    name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_one_and_two_are_taken() {
        assert_eq!(supported(Some(1.0)), Some(1));
        assert_eq!(supported(Some(2.0)), Some(2));
        for v in [None, Some(0.0), Some(1.5), Some(3.0), Some(f64::NAN)] {
            assert_eq!(supported(v), None, "{v:?}");
        }
    }

    #[test]
    fn modes_follow_the_contract_and_fail_closed() {
        assert_eq!(mode(None, false), HostMode::Edit, "absent is edit");
        assert_eq!(mode(Some("edit"), true), HostMode::Edit);
        assert_eq!(mode(Some("comment"), true), HostMode::Comment);
        assert_eq!(mode(Some("view"), true), HostMode::View);
        assert_eq!(mode(Some("owner"), true), HostMode::View);
        assert_eq!(mode(None, true), HostMode::View, "present but not a string");
        assert!(HostMode::Comment.read_only() && HostMode::View.read_only() && !HostMode::Edit.read_only());
    }

    #[test]
    fn can_download_is_true_only_when_absent_or_true() {
        assert!(can_download(None, false));
        assert!(can_download(Some(true), true));
        assert!(!can_download(Some(false), true));
        assert!(!can_download(None, true), "present but not a boolean");
    }

    #[test]
    fn a_blank_or_missing_user_name_keeps_the_preferences() {
        assert_eq!(user_name(Some(" Nguyễn An ".into())), Some("Nguyễn An".into()));
        assert_eq!(user_name(Some("  ".into())), None);
        assert_eq!(user_name(None), None);
    }
}
