use super::{tcvn3_to_unicode, vni_to_unicode};

#[test]
fn tcvn3_all_vowels_and_tones() {
    let vectors = [
        ("aµ¸¶·¹", "aàáảãạ"),
        ("¨»¾¼½Æ", "ăằắẳẵặ"),
        ("©ÇÊÈÉË", "âầấẩẫậ"),
        ("eÌÐÎÏÑ", "eèéẻẽẹ"),
        ("ªÒÕÓÔÖ", "êềếểễệ"),
        ("oßãáâä", "oòóỏõọ"),
        ("«åèæçé", "ôồốổỗộ"),
        ("¬êíëìî", "ơờớởỡợ"),
        ("uïóñòô", "uùúủũụ"),
        ("\u{ad}õøö÷ù", "ưừứửữự"),
        ("yúýûüþ", "yỳýỷỹỵ"),
        ("i×ÝØÜÞ", "iìíỉĩị"),
        ("®§", "đĐ"),
        ("¡¢£¤¥¦", "ĂÂÊÔƠƯ"),
        ("Ti\u{d5}ng Vi\u{d6}t", "Tiếng Việt"),
    ];
    for (source, expected) in vectors {
        assert_eq!(tcvn3_to_unicode(source), expected, "{source:?}");
    }
}

#[test]
fn vni_all_vowels_and_tones_in_both_cases() {
    let vectors = [
        ("a aø aù aû aõ aï", "a à á ả ã ạ"),
        ("aê aè aé aú aü aë", "ă ằ ắ ẳ ẵ ặ"),
        ("aâ aà aá aå aã aä", "â ầ ấ ẩ ẫ ậ"),
        ("e eø eù eû eõ eï", "e è é ẻ ẽ ẹ"),
        ("eâ eà eá eå eã eä", "ê ề ế ể ễ ệ"),
        ("o oø où oû oõ oï", "o ò ó ỏ õ ọ"),
        ("oâ oà oá oå oã oä", "ô ồ ố ổ ỗ ộ"),
        ("ô ôø ôù ôû ôõ ôï", "ơ ờ ớ ở ỡ ợ"),
        ("u uø uù uû uõ uï", "u ù ú ủ ũ ụ"),
        ("ö öø öù öû öõ öï", "ư ừ ứ ử ữ ự"),
        ("y yø yù yû yõ î", "y ỳ ý ỷ ỹ ỵ"),
        ("i ì í æ ó ò", "i ì í ỉ ĩ ị"),
        ("ñ Ñ", "đ Đ"),
        ("Tieáng Vieät", "Tiếng Việt"),
    ];
    for (source, expected) in vectors {
        assert_eq!(vni_to_unicode(source), expected, "{source:?}");
        assert_eq!(vni_to_unicode(&source.to_uppercase()), expected.to_uppercase(), "uppercase {source:?}");
    }
}

#[test]
fn unmapped_characters_pass_through_and_output_is_nfc() {
    let unchanged = "ASCII 123\0\n\t日本語 😀 \u{fffc} ắ ệ Ữ";
    assert_eq!(tcvn3_to_unicode(unchanged), unchanged);
    assert_eq!(vni_to_unicode(unchanged), unchanged);
    for convert in [tcvn3_to_unicode, vni_to_unicode] {
        assert_eq!(convert("a\u{306}\u{301} e\u{302}\u{323}"), "ắ ệ");
        assert_eq!(convert("\u{301}😀\u{10ffff}"), "\u{301}😀\u{10ffff}");
    }
}

#[test]
fn dangling_and_invalid_vni_marks_are_not_consumed() {
    assert_eq!(vni_to_unicode("aøø bù a ø øa"), "àø bù a ø øa");
    assert_eq!(vni_to_unicode("AØ UÕ ÔÙ ÖÏ Î"), "À Ũ Ớ Ự Ỵ");
}
