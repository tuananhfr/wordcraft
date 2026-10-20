//! Vietnamese legacy encoding tables, interpreted as Windows-1252 text.

use unicode_normalization::UnicodeNormalization;

// Published character mappings, not font outlines: https://vietunicode.sourceforge.net/charset/
const TCVN3_ROWS: &[(&str, &str)] = &[
    ("µ¸¶·¹", "àáảãạ"),
    ("¨»¾¼½Æ", "ăằắẳẵặ"),
    ("©ÇÊÈÉË", "âầấẩẫậ"),
    ("ÌÐÎÏÑ", "èéẻẽẹ"),
    ("ªÒÕÓÔÖ", "êềếểễệ"),
    ("×ÝØÜÞ", "ìíỉĩị"),
    ("ßãáâä", "òóỏõọ"),
    ("«åèæçé", "ôồốổỗộ"),
    ("¬êíëìî", "ơờớởỡợ"),
    ("ïóñòô", "ùúủũụ"),
    ("\u{ad}õøö÷ù", "ưừứửữự"),
    ("úýûüþ", "ỳýỷỹỵ"),
    ("¡¢£¤¥¦§®", "ĂÂÊÔƠƯĐđ"),
];

const VNI_PAIRS: &[(char, &str, &str)] = &[
    ('a', "øùûõïâàáåãäêèéúüë", "àáảãạâầấẩẫậăằắẳẵặ"),
    ('e', "øùûõïâàáåãä", "èéẻẽẹêềếểễệ"),
    ('o', "øùûõïâàáåãä", "òóỏõọôồốổỗộ"),
    ('u', "øùûõï", "ùúủũụ"),
    ('y', "øùûõ", "ỳýỷỹ"),
    ('ô', "øùûõï", "ờớởỡợ"),
    ('ö', "øùûõï", "ừứửữự"),
];

/// Decode TCVN3 (ABC) bytes represented by Windows-1252 characters to NFC.
pub fn tcvn3_to_unicode(text: &str) -> String {
    text.chars().map(tcvn3_char).nfc().collect()
}

pub(crate) fn tcvn3_uppercase_to_unicode(text: &str) -> String {
    // Capital TCVN3 fonts share the lowercase bytes, including ASCII letters.
    text.chars()
        .map(|c| {
            let decoded = tcvn3_char(c);
            if c.is_ascii_lowercase() || decoded != c { decoded.to_uppercase().next().unwrap_or(decoded) } else { decoded }
        })
        .nfc()
        .collect()
}

fn tcvn3_char(c: char) -> char {
    if !('\u{a1}'..='\u{fe}').contains(&c) {
        return c;
    }
    TCVN3_ROWS.iter().find_map(|(from, to)| from.chars().zip(to.chars()).find_map(|(a, b)| (a == c).then_some(b))).unwrap_or(c)
}

/// Decode VNI Windows pairs and single-byte letters to NFC.
pub fn vni_to_unicode(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut out = String::with_capacity(text.len());
    while let Some(c) = chars.next() {
        if let Some(converted) = chars.peek().and_then(|next| vni_pair(c, *next)) {
            let _ = chars.next();
            out.push(converted);
        } else {
            out.push(vni_single(c));
        }
    }
    out.nfc().collect()
}

fn vni_pair(base: char, mark: char) -> Option<char> {
    let upper = base.is_uppercase();
    let base = base.to_lowercase().next()?;
    VNI_PAIRS.iter().find(|(c, _, _)| *c == base).and_then(|(_, marks, letters)| {
        marks.chars().zip(letters.chars()).find_map(|(m, letter)| {
            let expected = if upper { m.to_uppercase().next()? } else { m };
            (expected == mark).then(|| if upper { letter.to_uppercase().next().unwrap_or(letter) } else { letter })
        })
    })
}

fn vni_single(c: char) -> char {
    match c {
        'ñ' => 'đ',
        'Ñ' => 'Đ',
        'ô' => 'ơ',
        'Ô' => 'Ơ',
        'ö' => 'ư',
        'Ö' => 'Ư',
        'æ' => 'ỉ',
        'Æ' => 'Ỉ',
        'ó' => 'ĩ',
        'Ó' => 'Ĩ',
        'ò' => 'ị',
        'Ò' => 'Ị',
        'î' => 'ỵ',
        'Î' => 'Ỵ',
        _ => c,
    }
}

#[cfg(test)]
#[path = "vn_legacy_tests.rs"]
mod tests;
