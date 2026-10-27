//! MHTML (RFC 2557 `multipart/related`): web archives and the `.mht` parts DOCX `altChunk`s
//! point to.

use std::collections::HashMap;

use crate::model::{base64_decode, base64_encode};

const MAX_PARTS: usize = 1024;
const MAX_HEADERS: usize = 256;
const MAX_BOUNDARY: usize = 200;
/// `data:` URI text inlined in all: what the HTML importer takes in pictures, base64-encoded. One
/// picture referenced many times would otherwise multiply without bound.
const MAX_INLINED: usize = crate::html::MAX_INLINE_TOTAL_BYTES / 3 * 4 + 1024;

struct Entity<'a> {
    headers: Vec<(String, String)>,
    body: &'a [u8],
}

impl Entity<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Media type without parameters, lowercase.
    fn mime(&self) -> String {
        self.header("content-type").and_then(|v| v.split(';').next()).unwrap_or("").trim().to_ascii_lowercase()
    }

    fn content_id(&self) -> Option<&str> {
        self.header("content-id").map(|v| v.trim().trim_start_matches('<').trim_end_matches('>')).filter(|v| !v.is_empty())
    }

    fn location(&self) -> Option<&str> {
        self.header("content-location").map(str::trim).filter(|v| !v.is_empty())
    }

    fn decoded(&self) -> Vec<u8> {
        let encoding = self.header("content-transfer-encoding").unwrap_or("").trim().to_ascii_lowercase();
        match encoding.as_str() {
            "base64" => base64_decode(&String::from_utf8_lossy(self.body)).unwrap_or_default(),
            "quoted-printable" => quoted_printable(self.body),
            _ => self.body.to_vec(),
        }
    }
}

/// The archive's root HTML page with the pictures it embeds inlined as `data:` URIs, so the HTML
/// importer's own picture limits apply. `None` when the bytes are not a MIME archive holding HTML.
pub fn to_html(bytes: &[u8]) -> Option<String> {
    let top = entity(bytes)?;
    top.header("content-type")?;
    if !top.mime().starts_with("multipart/") {
        return (top.mime() == "text/html").then(|| crate::html::decode(&top.decoded()));
    }
    let content_type = top.header("content-type").unwrap_or("");
    let boundary = param(content_type, "boundary").filter(|b| !b.is_empty() && b.len() <= MAX_BOUNDARY)?;
    let parts = split(top.body, &boundary);
    let start = param(content_type, "start").map(|s| s.trim().trim_start_matches('<').trim_end_matches('>').to_string());
    let root = start
        .and_then(|id| parts.iter().position(|p| p.content_id() == Some(id.as_str())))
        .or_else(|| parts.iter().position(|p| matches!(p.mime().as_str(), "text/html" | "application/xhtml+xml")))?;
    let page = parts.get(root)?;
    let html = crate::html::decode(&page.decoded());
    let base = page.location().and_then(|l| l.rfind('/').and_then(|i| l.get(..=i)));
    let mut pictures: HashMap<String, String> = HashMap::new();
    for (i, part) in parts.iter().enumerate() {
        let mime = part.mime();
        if i == root || !mime.starts_with("image/") {
            continue;
        }
        let data = part.decoded();
        // The importer drops a larger picture anyway.
        if data.len() > crate::html::MAX_INLINE_IMAGE_BYTES {
            continue;
        }
        let uri = format!("data:{mime};base64,{}", base64_encode(&data));
        if let Some(id) = part.content_id() {
            pictures.insert(format!("cid:{id}"), uri.clone());
        }
        if let Some(location) = part.location() {
            if let Some(relative) = base.and_then(|b| location.strip_prefix(b)) {
                pictures.insert(relative.to_string(), uri.clone());
            }
            pictures.insert(location.to_string(), uri);
        }
    }
    Some(if pictures.is_empty() { html } else { inline_pictures(&html, &pictures) })
}

/// Headers (unfolded, names lowercase) and body of a MIME entity.
fn entity(bytes: &[u8]) -> Option<Entity<'_>> {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut pos = 0usize;
    loop {
        let end = bytes.get(pos..)?.iter().position(|&b| b == b'\n').map_or(bytes.len(), |n| pos.saturating_add(n));
        let line = bytes.get(pos..end)?;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let next = end.saturating_add(1).min(bytes.len());
        if line.is_empty() {
            return Some(Entity { headers, body: bytes.get(next..).unwrap_or_default() });
        }
        let text = String::from_utf8_lossy(line);
        if text.starts_with([' ', '\t']) {
            // A folded header continues the previous one.
            if let Some((_, value)) = headers.last_mut()
                && value.len() < 4096
            {
                value.push(' ');
                value.push_str(text.trim());
            }
        } else if let Some((name, value)) = text.split_once(':') {
            if headers.len() >= MAX_HEADERS {
                return None;
            }
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        } else {
            return None;
        }
        if end >= bytes.len() {
            return Some(Entity { headers, body: &[] });
        }
        pos = next;
    }
}

/// A `name=value` parameter of a header value (`;`-separated, value optionally quoted).
fn param(value: &str, name: &str) -> Option<String> {
    let mut rest = value;
    while let Some(i) = rest.find(';') {
        rest = rest.get(i + 1..)?.trim_start();
        let (key, tail) = rest.split_once('=')?;
        let tail = tail.trim_start();
        let (v, after) = match tail.strip_prefix('"') {
            Some(q) => {
                let close = q.find('"').unwrap_or(q.len());
                (q.get(..close)?, q.get(close..).unwrap_or(""))
            }
            None => {
                let end = tail.find(';').unwrap_or(tail.len());
                (tail.get(..end)?.trim_end(), tail.get(end..).unwrap_or(""))
            }
        };
        if key.trim().eq_ignore_ascii_case(name) {
            return Some(v.to_string());
        }
        rest = after;
    }
    None
}

/// The body parts between `--boundary` delimiter lines.
fn split<'a>(body: &'a [u8], boundary: &str) -> Vec<Entity<'a>> {
    let delimiter = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut at = find_delimiter(body, &delimiter, 0);
    while let Some(pos) = at {
        let after = pos.saturating_add(delimiter.len());
        if body.get(after..after.saturating_add(2)) == Some(b"--") || parts.len() >= MAX_PARTS {
            break;
        }
        let Some(start) = body.get(after..).and_then(|r| r.iter().position(|&b| b == b'\n')).map(|n| after + n + 1) else { break };
        at = find_delimiter(body, &delimiter, start);
        let mut end = at.unwrap_or(body.len());
        if at.is_some() {
            // The line break before a delimiter belongs to the delimiter.
            end = end.saturating_sub(1).max(start);
            if end > start && body.get(end - 1) == Some(&b'\r') {
                end -= 1;
            }
        }
        if let Some(part) = body.get(start..end).and_then(entity) {
            parts.push(part);
        }
    }
    parts
}

/// The next delimiter at the start of a line, from `from`.
fn find_delimiter(body: &[u8], delimiter: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while let Some(n) = body.get(i..)?.windows(delimiter.len()).position(|w| w == delimiter) {
        let pos = i.saturating_add(n);
        if pos == 0 || body.get(pos - 1) == Some(&b'\n') {
            return Some(pos);
        }
        i = pos.saturating_add(1);
    }
    None
}

fn quoted_printable(body: &[u8]) -> Vec<u8> {
    let hex = |b: u8| (b as char).to_digit(16).and_then(|d| u8::try_from(d).ok());
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0usize;
    while let Some(&b) = body.get(i) {
        if b != b'=' {
            out.push(b);
            i += 1;
            continue;
        }
        let (first, second) = (body.get(i + 1).copied(), body.get(i + 2).copied());
        match (first, second) {
            // Soft line break.
            (Some(b'\n'), _) => i += 2,
            (Some(b'\r'), Some(b'\n')) => i += 3,
            _ => match (first.and_then(hex), second.and_then(hex)) {
                (Some(h), Some(l)) => {
                    out.push((h << 4) | l);
                    i += 3;
                }
                _ => {
                    out.push(b);
                    i += 1;
                }
            },
        }
    }
    out
}

/// `html` with each `src` attribute that names one of `pictures` pointing at its `data:` URI.
fn inline_pictures(html: &str, pictures: &HashMap<String, String>) -> String {
    let lower = html.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut copied = 0usize;
    let mut from = 0usize;
    let mut inlined = 0usize;
    while let Some(n) = lower.get(from..).and_then(|r| r.find("src")) {
        let name = from + n;
        from = name + 3;
        if name == 0 || !bytes.get(name - 1).is_some_and(u8::is_ascii_whitespace) {
            continue;
        }
        let mut i = from;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            continue;
        }
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let (start, end, quoted) = match bytes.get(i) {
            Some(&q @ (b'"' | b'\'')) => match lower.get(i + 1..).and_then(|r| r.find(q as char)) {
                Some(len) => (i + 1, i + 1 + len, true),
                None => break,
            },
            Some(_) => {
                let len = bytes.get(i..).map_or(0, |r| r.iter().position(|b| b.is_ascii_whitespace() || *b == b'>').unwrap_or(r.len()));
                (i, i + len, false)
            }
            None => break,
        };
        let (Some(value), Some(before)) = (html.get(start..end), html.get(copied..start)) else { continue };
        let Some(uri) = pictures.get(value.trim()).or_else(|| pictures.get(&crate::html::unescape(value.trim()))) else { continue };
        inlined = inlined.saturating_add(uri.len());
        if inlined > MAX_INLINED {
            break;
        }
        out.push_str(before);
        if quoted {
            out.push_str(uri);
        } else {
            // `data:` URIs hold `;` and `/`, so an unquoted value gets quotes.
            out.push('"');
            out.push_str(uri);
            out.push('"');
        }
        copied = end;
        from = end;
    }
    out.push_str(html.get(copied..).unwrap_or(""));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

    fn b64(data: &[u8]) -> String {
        crate::model::base64_encode(data)
    }

    #[test]
    fn base64_root_and_picture_by_content_location() {
        let html = r#"<html><body><p>Phiếu</p><img src="file:///C:/erp/image1.png"></body></html>"#;
        let mht = format!(
            "MIME-Version: 1.0\r\nContent-Type: multipart/related; type=\"text/html\";\r\n boundary=\"----=part\"\r\n\r\n\
             ------=part\r\nContent-Type: text/html; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\nContent-Location: file:///C:/erp/document.html\r\n\r\n{}\r\n\
             ------=part\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: base64\r\nContent-Location: file:///C:/erp/image1.png\r\n\r\n{}\r\n\
             ------=part--\r\n",
            b64(html.as_bytes()),
            b64(PNG)
        );
        let out = to_html(mht.as_bytes()).unwrap();
        assert!(out.contains("Phiếu"), "{out}");
        assert!(out.contains(&format!("src=\"data:image/png;base64,{}\"", b64(PNG))), "{out}");
        assert!(!out.contains("file:///"), "{out}");
    }

    #[test]
    fn quoted_printable_with_soft_breaks_and_utf8() {
        let mht = "Content-Type: multipart/related; boundary=b\n\n--b\nContent-Type: text/html; charset=utf-8\nContent-Transfer-Encoding: quoted-printable\n\n<p class=3D\"x\">C=C3=A1 =\nphe</p>\n--b--\n";
        let out = to_html(mht.as_bytes()).unwrap();
        assert!(out.contains("<p class=\"x\">Cá phe</p>"), "{out}");
    }

    #[test]
    fn cid_and_relative_references_and_start_parameter() {
        let mht = format!(
            "Content-Type: multipart/related; boundary=\"b\"; start=\"<root@x>\"\r\n\r\n\
             --b\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: base64\r\nContent-ID: <pic@x>\r\n\r\n{0}\r\n\
             --b\r\nContent-Type: image/gif\r\nContent-Transfer-Encoding: base64\r\nContent-Location: file:///C:/x/doc_files/a.gif\r\n\r\n{0}\r\n\
             --b\r\nContent-Type: text/html\r\nContent-ID: <root@x>\r\nContent-Location: file:///C:/x/doc.htm\r\n\r\n\
             <img src='cid:pic@x'><IMG SRC=doc_files/a.gif><img src=\"missing.png\"><img data-src=\"cid:pic@x\">\r\n--b--",
            b64(PNG)
        );
        let out = to_html(mht.as_bytes()).unwrap();
        assert!(out.contains(&format!("src='data:image/png;base64,{}'", b64(PNG))), "{out}");
        assert!(out.contains(&format!("SRC=\"data:image/gif;base64,{}\"", b64(PNG))), "{out}");
        assert!(out.contains("src=\"missing.png\""), "{out}");
        assert!(out.contains("data-src=\"cid:pic@x\""), "{out}");
    }

    #[test]
    fn single_part_html_message() {
        let out = to_html(b"Content-Type: text/html\r\n\r\n<b>one</b>").unwrap();
        assert_eq!(out.trim(), "<b>one</b>");
    }

    #[test]
    fn not_an_archive_or_no_html() {
        assert!(to_html(b"<html><body>plain</body></html>").is_none());
        assert!(to_html(b"").is_none());
        assert!(to_html(b"Content-Type: multipart/related; boundary=b\n\n--b\nContent-Type: image/png\n\nxx\n--b--").is_none());
    }

    #[test]
    fn a_picture_referenced_many_times_stays_within_the_importer_budget() {
        let picture = vec![7u8; 100_000];
        let html = "<img src=\"cid:p\">".repeat(10_000);
        let mht = format!(
            "Content-Type: multipart/related; boundary=b\n\n--b\nContent-Type: text/html\n\n{html}\n--b\nContent-Type: image/png\nContent-Transfer-Encoding: base64\nContent-ID: <p>\n\n{}\n--b--",
            b64(&picture)
        );
        let out = to_html(mht.as_bytes()).unwrap();
        assert!(out.len() <= MAX_INLINED + html.len(), "{} bytes", out.len());
        assert!(out.contains("data:image/png;base64,") && out.ends_with("<img src=\"cid:p\">"));
    }

    #[test]
    fn a_picture_the_importer_would_drop_is_not_inlined() {
        let picture = vec![7u8; crate::html::MAX_INLINE_IMAGE_BYTES + 1];
        let mht = format!(
            "Content-Type: multipart/related; boundary=b\n\n--b\nContent-Type: text/html\n\n<img src=\"cid:p\">\n--b\nContent-Type: image/png\nContent-Transfer-Encoding: base64\nContent-ID: <p>\n\n{}\n--b--",
            b64(&picture)
        );
        assert_eq!(to_html(mht.as_bytes()).unwrap(), "<img src=\"cid:p\">");
    }

    #[test]
    fn hostile_input_never_panics() {
        let many = format!("Content-Type: multipart/related; boundary=b\n\n{}", "--b\nContent-Type: text/html\n\nx\n".repeat(5000));
        let long_header = format!("Content-Type: multipart/related; boundary={}\n\n", "b".repeat(100_000));
        let folded = format!("Content-Type: multipart/related;\n{}", " x\n".repeat(10_000));
        let inputs: Vec<Vec<u8>> = vec![
            b"Content-Type: multipart/related; boundary=".to_vec(),
            b"Content-Type: multipart/related; boundary=\"\"\n\n--\n--".to_vec(),
            b"Content-Type: multipart/related; boundary=b\n\n--b".to_vec(),
            b"Content-Type: multipart/related; boundary=b\n\n--b\nContent-Type: text/html\nContent-Transfer-Encoding: base64\n\n!!!\n--b--".to_vec(),
            b"Content-Type: text/html\nContent-Transfer-Encoding: quoted-printable\n\n=\n=Z=\xff=E".to_vec(),
            b"Content-Type: multipart/related; boundary=b\n\n--b\nContent-Type: text/html\n\n<img src=\"\xc3".to_vec(),
            b"Content-Type: multipart/related; boundary=b\n\n--b\nContent-Type: text/html\n\n<img src".to_vec(),
            b"Content-Type: multipart/related; boundary=b\n\n--b\nContent-Type: text/html\n\n<img src=".to_vec(),
            many.into_bytes(),
            long_header.into_bytes(),
            folded.into_bytes(),
            (0..=255u8).cycle().take(70_000).collect(),
        ];
        for input in inputs {
            let _ = to_html(&input);
        }
    }
}
