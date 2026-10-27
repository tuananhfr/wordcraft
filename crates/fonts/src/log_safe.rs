//! Log text that never names the user's documents: a document shows as its extension and size,
//! a folder under the user's home starts with `~`. It lives here because every crate that logs
//! user files (the apps, the UI, the font database) already depends on this one.

use std::path::Path;

/// A document as logs may show it: `.docx (12345 bytes)`, or `file (…)` when its extension
/// isn't a plain one (which could be part of the name).
pub fn file_label(name: &str, bytes: Option<u64>) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let ext = file.rsplit_once('.').map(|(_, ext)| ext).filter(|ext| (1..=8).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    let kind = ext.map_or_else(|| "file".to_string(), |ext| format!(".{}", ext.to_ascii_lowercase()));
    match bytes {
        Some(n) => format!("{kind} ({n} bytes)"),
        None => kind,
    }
}

/// `text` (an error message) with every mention of the document `name` replaced by its
/// [`file_label`]; engine errors start with the path or name they were given.
pub fn scrub(text: &str, name: &str) -> String {
    if name.is_empty() {
        return text.to_string();
    }
    let label = file_label(name, None);
    let text = text.replace(name, &label);
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if file != name && file.contains('.') { text.replace(file, &label) } else { text }
}

/// A settings, font or log folder with the user's home folder shown as `~`, which names the account.
pub fn home_relative(path: &Path) -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).filter(|h| !h.is_empty());
    match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_show_as_extension_and_size() {
        assert_eq!(file_label("Báo cáo quý.DOCX", Some(12345)), ".docx (12345 bytes)");
        assert_eq!(file_label("C:\\Users\\an\\Hợp đồng.docx", None), ".docx");
        assert_eq!(file_label("/home/an/notes.md", Some(0)), ".md (0 bytes)");
        assert_eq!(file_label("no extension", Some(7)), "file (7 bytes)");
        assert_eq!(file_label("v1.secret report", None), "file", "an extension with spaces is part of the name");
        assert_eq!(file_label("", None), "file");
    }

    #[test]
    fn errors_lose_the_document_name() {
        let path = "/home/an/Hợp đồng.docx";
        assert_eq!(scrub(&format!("{path}: permission denied"), path), ".docx: permission denied");
        assert_eq!(scrub("Hợp đồng.docx: not a document", path), ".docx: not a document");
        assert_eq!(scrub("Lương.odt: unsupported", "Lương.odt"), ".odt: unsupported");
        assert_eq!(scrub("unchanged", ""), "unchanged");
    }

    #[test]
    fn the_home_folder_shows_as_a_tilde() {
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).filter(|h| !h.is_empty()) else { return };
        let home = std::path::PathBuf::from(home);
        let shown = home_relative(&home.join("fonts").join("A.ttf"));
        assert_eq!(shown, format!("~{0}fonts{0}A.ttf", std::path::MAIN_SEPARATOR));
        assert_eq!(home_relative(&home), "~");
        let outside = Path::new("/usr/share/fonts/x.ttf");
        if !outside.starts_with(&home) {
            assert_eq!(home_relative(outside), outside.display().to_string());
        }
    }
}
