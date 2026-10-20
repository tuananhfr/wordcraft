//! Bounded CSS declarations and cascade for HTML paste.

use super::*;

pub(super) fn css_len_pt(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let num = |s: &str| s.trim().parse::<f32>().ok().filter(|x| x.is_finite());
    if let Some(x) = v.strip_suffix("pt") {
        num(x)
    } else if let Some(x) = v.strip_suffix("px") {
        num(x).map(|p| p * 0.75)
    } else if let Some(x) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        num(x).map(|p| p * 12.0)
    } else if let Some(x) = v.strip_suffix("in") {
        num(x).map(|p| p * 72.0)
    } else if let Some(x) = v.strip_suffix("cm") {
        num(x).map(|p| p * 72.0 / 2.54)
    } else if let Some(x) = v.strip_suffix("mm") {
        num(x).map(|p| p * 72.0 / 25.4)
    } else if let Some(x) = v.strip_suffix('%') {
        num(x).map(|p| p * 0.12)
    } else {
        match v.as_str() {
            "xx-small" => Some(7.0),
            "x-small" => Some(7.5),
            "small" => Some(10.0),
            "medium" => Some(12.0),
            "large" => Some(13.5),
            "x-large" => Some(18.0),
            "xx-large" => Some(24.0),
            _ => num(&v).map(|p| p * 0.75),
        }
    }
}

/// Apply a `style` attribute to formatting / block properties.
pub(super) fn apply_style(css: &str, f: &mut Fmt, align: &mut Option<Align>, preserve: &mut bool, page_break: &mut bool) {
    for decl in css.split(';') {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().trim_end_matches("!important").trim();
        let lv = v.to_ascii_lowercase();
        match k.as_str() {
            "font-weight" => f.bold = lv.starts_with("bold") || lv.parse::<u32>().is_ok_and(|w| w >= 600),
            "font-style" => f.italic = lv == "italic" || lv == "oblique",
            "text-decoration" | "text-decoration-line" => {
                if lv.contains("none") {
                    f.underline = false;
                    f.strike = false;
                }
                if lv.contains("underline") {
                    f.underline = true;
                }
                if lv.contains("line-through") {
                    f.strike = true;
                }
            }
            "color" => {
                if let Some(c) = parse_color(v) {
                    f.color = Some(c);
                }
            }
            "background-color" | "background" => {
                if let Some(c) = parse_color(v).or_else(|| v.split_whitespace().find_map(parse_color)) {
                    f.background = Some(c);
                }
            }
            "font-size" => {
                if let Some(p) = css_len_pt(v).filter(|p| *p >= 1.0 && *p <= 1638.0) {
                    f.size = Some(p);
                }
            }
            "font-family" => {
                let fam = v.split(',').next().unwrap_or("").trim().trim_matches(['"', '\'']).to_string();
                if !fam.is_empty() && !matches!(fam.to_ascii_lowercase().as_str(), "serif" | "sans-serif" | "inherit" | "initial") {
                    f.code = false;
                    f.font = Some(fam);
                }
            }
            "vertical-align" => {
                f.sup = lv == "super";
                f.sub = lv == "sub";
            }
            "text-align" => {
                *align = match lv.as_str() {
                    "center" => Some(Align::Center),
                    "right" | "end" => Some(Align::Right),
                    "justify" => Some(Align::Justify),
                    "left" | "start" => Some(Align::Left),
                    _ => *align,
                }
            }
            "white-space" => *preserve = lv.starts_with("pre"),
            "page-break-before" | "break-before" => *page_break = lv == "always" || lv == "page",
            _ => {}
        }
    }
}

pub(super) const MAX_CSS: usize = 256 * 1024;
const MAX_RULES: usize = 2048;
pub(super) const MAX_STYLE: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct Sheet {
    rules: Vec<Rule>,
    bytes: usize,
    rule_bytes: usize,
}

struct Rule {
    tag: String,
    class: Option<String>,
    declarations: String,
}

fn allowed(name: &str) -> bool {
    matches!(name,
        "color" | "background" | "background-color" | "font-weight" | "font-style" |
        "font-size" | "font-family" | "text-decoration" | "text-decoration-line" |
        "vertical-align" | "text-align" | "white-space" | "page-break-before" | "break-before" |
        "margin-left" | "margin-right" | "margin-top" | "margin-bottom" | "text-indent" |
        "line-height" | "mso-list" | "list-style-type" | "width" | "height" |
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left")
}

pub(super) fn declarations(css: &str) -> String {
    let mut out = String::new();
    for decl in css.split(';') {
        let Some((key, value)) = decl.split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        let lower = value.to_ascii_lowercase();
        let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        if !allowed(&key) || compact.contains("url(") || compact.contains("expression(") || lower.contains('@') || lower.contains('\\') {
            continue;
        }
        let value = if lower.ends_with("!important") { value.get(..value.len().saturating_sub(10)).unwrap_or("").trim() } else { value };
        out.push_str(&key);
        out.push(':');
        out.push_str(value);
        out.push(';');
    }
    out
}

fn identifier(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

impl Sheet {
    pub(super) fn account(&mut self, bytes: usize) -> Result<(), ()> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_CSS {
            return Err(());
        }
        Ok(())
    }

    pub(super) fn add(&mut self, css: &str) -> Result<(), ()> {
        self.account(css.len())?;
        let mut clean = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(at) = rest.find("/*") {
            clean.push_str(rest.get(..at).unwrap_or(""));
            clean.push(' ');
            rest = rest.get(at + 2..).unwrap_or("");
            let Some(end) = rest.find("*/") else { rest = ""; break };
            rest = rest.get(end + 2..).unwrap_or("");
        }
        clean.push_str(rest);
        rest = &clean;
        while let Some(open) = rest.find('{') {
            let selector = rest.get(..open).unwrap_or("").rsplit(';').next().unwrap_or("").trim();
            let body = rest.get(open + 1..).unwrap_or("");
            let mut depth = 1usize;
            let mut end = None;
            for (at, c) in body.char_indices() {
                match c {
                    '{' => depth = depth.saturating_add(1),
                    '}' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 { end = Some(at); break; }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { break };
            let block = body.get(..end).unwrap_or("");
            rest = body.get(end + 1..).unwrap_or("");
            if selector.contains('@') || block.contains('{') {
                continue;
            }
            let declarations = declarations(block);
            if declarations.is_empty() { continue; }
            for selector in selector.split(',') {
                let selector = selector.trim();
                let (tag, class) = match selector.split_once('.') {
                    Some((tag, class)) if (tag.is_empty() || identifier(tag)) && identifier(class) => (tag, Some(class.to_string())),
                    None if identifier(selector) => (selector, None),
                    _ => continue,
                };
                self.rule_bytes = self.rule_bytes.saturating_add(declarations.len());
                if self.rules.len() >= MAX_RULES || self.rule_bytes > MAX_CSS { return Err(()); }
                self.rules.push(Rule { tag: tag.to_ascii_lowercase(), class, declarations: declarations.clone() });
            }
        }
        Ok(())
    }

    pub(super) fn style(&self, name: &str, attrs: &[(String, String)]) -> String {
        let mut out = String::new();
        let classes = attr(attrs, "class").unwrap_or("");
        // Keep source order within each specificity; inline declarations always come last.
        for specificity in 0..3 {
            for rule in &self.rules {
                let weight = usize::from(rule.class.is_some()) + usize::from(rule.class.is_some() && !rule.tag.is_empty());
                if weight == specificity && (rule.tag.is_empty() || rule.tag == name) &&
                    rule.class.as_ref().is_none_or(|class| classes.split_ascii_whitespace().any(|c| c == class)) {
                    out.push_str(&rule.declarations);
                }
            }
        }
        if let Some(inline) = attr(attrs, "style") {
            out.push_str(&declarations(inline));
        }
        out
    }
}

pub(super) fn length(v: &str, em: f32) -> Option<f32> {
    let lower = v.trim().to_ascii_lowercase();
    if lower.ends_with('%') { return None; }
    let result = if let Some(n) = lower.strip_suffix("em").filter(|_| !lower.ends_with("rem")) {
        n.trim().parse::<f32>().ok().map(|n| n * em)
    } else {
        css_len_pt(&lower)
    };
    result.filter(|n| n.is_finite()).map(|n| n.clamp(-31680.0, 31680.0))
}

pub(super) fn spacing(css: &str, em: f32) -> model::Spacing {
    use wordcraft_doc::props::LineSpacing;
    let mut out = model::Spacing::default();
    for decl in css.split(';') {
        let Some((key, value)) = decl.split_once(':') else { continue };
        let value = value.trim();
        let n = length(value, em);
        match key.trim() {
            "margin-left" => out.left = n.or(out.left),
            "margin-right" => out.right = n.or(out.right),
            "text-indent" => out.first = n.or(out.first),
            "margin-top" => out.before = n.map(|n| n.max(0.0)).or(out.before),
            "margin-bottom" => out.after = n.map(|n| n.max(0.0)).or(out.after),
            "line-height" => {
                let multiple = value.strip_suffix('%').and_then(|v| v.trim().parse::<f32>().ok()).map(|v| v / 100.0)
                    .or_else(|| value.parse::<f32>().ok());
                out.line = if let Some(n) = multiple.filter(|n| n.is_finite() && *n > 0.0) {
                    Some(LineSpacing::Multiple(n.clamp(0.1, 100.0)))
                } else {
                    n.filter(|n| *n > 0.0).map(LineSpacing::Exactly)
                }.or(out.line);
            }
            _ => {}
        }
    }
    out
}

pub(super) fn list(css: &str) -> Option<ListInfo> {
    let mut out = None;
    for decl in css.split(';') {
        let Some((key, value)) = decl.split_once(':') else { continue };
        if key.trim() == "mso-list" {
            let level = value.split_whitespace().find_map(|word| word.strip_prefix("level").and_then(|v| v.parse::<u8>().ok()));
            out = level.map(|level| ListInfo { ordered: true, level: level.saturating_sub(1).min(8) });
        }
        if key.trim() == "list-style-type" && let Some(li) = &mut out {
            li.ordered = !matches!(value.trim(), "disc" | "circle" | "square");
        }
    }
    out
}

