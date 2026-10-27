//! Guard (ERPCons): log records never interpolate a document's name, path or URL. Such values go
//! through `wordcraft_fonts::log_safe` (extension and size; `~` for the home folder) instead, and
//! so do errors logged next to code that opens or saves a document, which quote its name.

use std::path::{Path, PathBuf};

/// Where the app's log records are written.
const SCANNED: &[&str] = &["apps", "crates/ui-egui/src", "crates/engine/src", "crates/fonts/src"];

/// Identifiers that hold, or are likely to hold, a document's name, path or URL, or the office
/// shell's id of a server document.
const SUSPECT: &[&str] = &["name", "path", "paths", "url", "title", "file", "file_name", "dir", "display", "p", "f", "req", "doc_id"];

/// Records that interpolate such an identifier but no user document: (file, text in the record).
const ALLOWED: &[(&str, &str)] = &[
    // The shell's account method, a Zotero command and a GPU adapter, not files.
    ("apps/wordcraft-web/src/account.rs", "erpAccount.{name} failed"),
    ("crates/ui-egui/src/zotero.rs", "Zotero: {name} failed"),
    ("crates/ui-egui/src/zotero.rs", "Zotero: {name} {} after"),
    ("crates/ui-egui/src/zotero.rs", "cmd.wire_name()"),
    ("apps/wordcraft/src/graphics.rs", "info.name"),
    // Font file names the user added, not documents.
    ("apps/wordcraft-web/src/web/font_store.rs", "the font {name} for the next visit"),
    ("apps/wordcraft-web/src/web/font_store.rs", "the stored font {name}"),
    ("apps/wordcraft-web/src/web/font_store.rs", "stored font {name} is too large"),
    ("apps/wordcraft-web/src/web/font_store.rs", "stored font {name} skipped"),
    ("crates/fonts/src/fontdb/user.rs", "couldn't store the font {name}"),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "target" | "tests" | "dist") {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && !name.starts_with("tests") {
            out.push(path);
        }
    }
}

/// Each `log::…!(…)` call in `source` (where its arguments start, their text), parentheses
/// balanced and strings skipped.
fn records(source: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for level in ["error", "warn", "info", "debug", "trace"] {
        let start = format!("log::{level}!(");
        let mut from = 0;
        while let Some(at) = source[from..].find(&start) {
            let open = from + at + start.len();
            let mut depth = 1;
            let mut in_string = false;
            let mut escaped = false;
            let mut end = source.len();
            for (i, c) in source[open..].char_indices() {
                match c {
                    _ if escaped => escaped = false,
                    '\\' if in_string => escaped = true,
                    '"' => in_string = !in_string,
                    '(' if !in_string => depth += 1,
                    ')' if !in_string => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            out.push((open, &source[open..end]));
            from = end;
        }
    }
    out
}

/// The identifiers a record interpolates: inline `{ident}` placeholders and the arguments after
/// the format string. Arguments that go through `log_safe` are fine.
fn interpolated(record: &str) -> Vec<String> {
    let mut idents = Vec::new();
    let (format, args) = match record.find('"').and_then(|q| string_end(record, q + 1)) {
        Some(close) => (&record[..close], &record[close + 1..]),
        None => (record, ""),
    };
    let mut rest = format;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        if rest.starts_with('{') {
            rest = &rest[1..];
            continue;
        }
        let inner: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !inner.is_empty() {
            idents.push(inner);
        }
    }
    for arg in top_level_args(args).into_iter().filter(|a| !a.contains("log_safe::")) {
        idents.extend(arg.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).map(str::to_string));
    }
    idents
}

/// The index of the `"` closing the string that starts at `from`.
fn string_end(text: &str, from: usize) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in text[from..].char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(from + i),
            _ => {}
        }
    }
    None
}

/// `args` split at the commas outside brackets and strings.
fn top_level_args(args: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start, mut in_string, mut escaped) = (Vec::new(), 0i32, 0, false, false);
    for (i, c) in args.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '(' | '[' | '{' if !in_string => depth += 1,
            ')' | ']' | '}' if !in_string => depth -= 1,
            ',' if !in_string && depth == 0 => {
                out.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&args[start..]);
    out
}

fn suspect(ident: &str) -> bool {
    SUSPECT.contains(&ident) || ["_name", "_path", "_url", "_title", "_doc_id"].iter().any(|s| ident.ends_with(s))
}

/// Errors are named like this; one from opening or saving a document starts with its name or path.
const ERRORS: &[&str] = &["e", "err", "error"];

/// Code that opens or saves a named document. An error logged within a few lines of it is
/// assumed to quote the document, so it has to go through `log_safe::scrub`.
const OPENS_OR_SAVES: &[&str] = &["\"file.open\"", "\"file.save\"", "save_bytes", "save_path", "open_bytes", "open_path"];

/// Whether the record at `offset` in `source` may name a document.
fn names_document(source: &str, offset: usize, record: &str) -> bool {
    let idents = interpolated(record);
    if idents.iter().any(|i| suspect(i)) {
        return true;
    }
    if !idents.iter().any(|i| ERRORS.contains(&i.as_str())) {
        return false;
    }
    let before = &source[..offset];
    let context = before.rfind('\n').map_or(before, |end| {
        let start = before[..end].char_indices().rev().filter(|(_, c)| *c == '\n').nth(9).map_or(0, |(i, _)| i);
        &before[start..]
    });
    OPENS_OR_SAVES.iter().any(|marker| context.contains(marker))
}

#[test]
fn log_records_never_name_documents() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for dir in SCANNED {
        sources(&root.join(dir), &mut files);
    }
    assert!(files.len() > 50, "found only {} sources", files.len());
    let mut used = vec![false; ALLOWED.len()];
    let mut found = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap_or(file).to_string_lossy().replace('\\', "/");
        let source = std::fs::read_to_string(file).unwrap_or_default();
        for (offset, record) in records(&source) {
            if !names_document(&source, offset, record) {
                continue;
            }
            match ALLOWED.iter().position(|(f, text)| rel == *f && record.contains(text)) {
                Some(i) => used[i] = true,
                None => found.push(format!("{rel}: log::…!({})", record.split_whitespace().collect::<Vec<_>>().join(" "))),
            }
        }
    }
    assert!(found.is_empty(), "log records naming a document (use wordcraft_fonts::log_safe):\n{}", found.join("\n"));
    let stale: Vec<_> = ALLOWED.iter().zip(&used).filter(|(_, u)| !**u).map(|(a, _)| a).collect();
    assert!(stale.is_empty(), "allowlist entries that match nothing: {stale:?}");
}

#[test]
fn the_scanner_sees_names_paths_urls_and_document_errors() {
    let source = r#"
        log::warn!("{p}: {e}");
        log::error!("couldn't write {name}: {e}");
        log::info!("saved {} ({} bytes)", s.name, n);
        log::debug!("loaded {}", path.display());
        log::warn!("open {file_name:?}");
        log::warn!("couldn't keep a {}: {}", log_safe::file_label(&name, None), log_safe::scrub(&e, &name));
        log::info!("{{literal}} and {count} of {total}");
        log::warn!("erpHost.save failed for {}", host.doc_id);
        log::error!("missing <canvas id=\"{CANVAS_ID}\">");
        let bytes = wordcraft_engine::io::save_bytes_with(&name, &doc, None, &opts);
        if let Err(e) = bytes {
            log::warn!("couldn't keep a draft: {e}");
        }
    "#;
    let flagged: Vec<bool> = records(source).iter().map(|(at, r)| names_document(source, *at, r)).collect();
    assert_eq!(flagged.len(), 10);
    let flagged_count = flagged.iter().filter(|f| **f).count();
    assert_eq!(flagged_count, 7, "{flagged:?}");
    let far = format!("save_bytes(x);{}log::warn!(\"clipboard: {{e}}\");", "\n".repeat(20));
    assert!(!records(&far).iter().any(|(at, r)| names_document(&far, *at, r)), "an error far from any open or save");
}
