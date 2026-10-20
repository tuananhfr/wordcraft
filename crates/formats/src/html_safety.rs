//! Resource bounds and inert plain-text fallback for HTML import.

use super::*;

pub(super) const MAX_INPUT: usize = 8 * 1024 * 1024;
pub(super) const MAX_CELLS: usize = 20_000;

pub(super) fn local_image(src: &str) -> bool {
    let src = src.trim();
    !src.is_empty() && !src.starts_with(['/', '\\']) && !src.contains([':', '\\']) && !src.chars().any(char::is_control)
}

pub(super) fn sheet(s: &str) -> Result<css::Sheet, ()> {
    let mut out = css::Sheet::default();
    let mut lx = Lexer { s, pos: 0 };
    while let Some(token) = lx.next() {
        if let Tok::Start { name, attrs, self_close } = token {
            if let Some(style) = attr(&attrs, "style") {
                if style.len() > css::MAX_STYLE { return Err(()); }
                out.account(style.len())?;
            }
            if !self_close && matches!(name.as_str(), "script" | "noscript" | "template" | "textarea" | "o:p") {
                lx.skip_past(&format!("</{name}"));
                lx.skip_past(">");
            } else if !self_close && name == "style" {
                let start = lx.pos;
                let end = lx.skip_past("</style");
                out.add(s.get(start..end).unwrap_or(""))?;
                lx.skip_past(">");
            }
        }
    }
    Ok(out)
}

pub(super) fn skip_element(lx: &mut Lexer, name: &str) -> String {
    let start = lx.pos;
    let mut depth = 1usize;
    let mut end = start;
    while let Some(token) = lx.next() {
        match token {
            Tok::Start { name: n, self_close: false, .. } if n == name => depth = depth.saturating_add(1),
            Tok::End(n) if n == name => {
                depth = depth.saturating_sub(1);
                if depth == 0 { break; }
            }
            _ => {}
        }
        end = lx.pos;
    }
    lx.s.get(start..end).unwrap_or("").to_string()
}

pub(super) fn plain(s: &str) -> Flow {
    let mut end = s.len().min(MAX_INPUT);
    while !s.is_char_boundary(end) { end = end.saturating_sub(1); }
    let mut lx = Lexer { s: s.get(..end).unwrap_or(""), pos: 0 };
    let mut para = Para::default();
    while let Some(token) = lx.next() {
        match token {
            Tok::Text(text) => para.push_text(&text, &Fmt::default()),
            Tok::Start { name, self_close, .. } => {
                if !self_close && matches!(name.as_str(), "script" | "style" | "template" | "noscript" | "textarea" | "o:p") {
                    lx.skip_past(&format!("</{name}"));
                    lx.skip_past(">");
                } else if block_kind(&name).is_some() || name == "br" {
                    para.push_text("\n", &Fmt::default());
                }
            }
            _ => {}
        }
    }
    para.trim();
    Flow { blocks: vec![FBlock::Para(para)], ..Default::default() }
}
