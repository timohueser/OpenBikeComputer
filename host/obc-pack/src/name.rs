//! Map-sourced names in the glyph repertoire the device font draws.
//!
//! The face carries ASCII, Latin-1 Supplement and Latin Extended-A, and every other character
//! draws as `?`. [`obc_render::glyph_supported`] reads that repertoire off the real font strip, so
//! it is the one definition and the bake cannot drift from the device. Names are normalized here,
//! once, rather than at each place that reads an OSM tag.

use obc_render::glyph_supported;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// Cyrillic а to я (U+0430 to U+044F) in code order. `ъ` and `ь` carry no sound and spell empty.
const CYRILLIC: [&str; 32] = [
    "a", "b", "v", "g", "d", "e", "zh", "z", "i", "y", "k", "l", "m", "n", "o", "p", "r", "s", "t", "u", "f", "kh",
    "ts", "ch", "sh", "shch", "", "y", "", "e", "yu", "ya",
];

/// Greek α to ω (U+03B1 to U+03C9) in code order, including final sigma at index 17.
const GREEK: [&str; 25] = [
    "a", "v", "g", "d", "e", "z", "i", "th", "i", "k", "l", "m", "n", "x", "o", "p", "r", "s", "s", "t", "y", "f",
    "ch", "ps", "o",
];

/// One character to its ASCII spelling, for what decomposition does not reach: the typographic
/// punctuation, the letters with no decomposition, and the German digraphs decomposition would
/// flatten to a bare vowel.
///
/// Letters are listed in lowercase. An uppercase letter is looked up in lowercase and capitalized
/// again afterwards, so a digraph keeps its second letter small: `Жуков` is `Zhukov`. The two
/// uppercase ligatures below are the exception, spelled in full capitals as they always were.
fn spelling(c: char) -> Option<&'static str> {
    Some(match c {
        'Æ' => "AE",
        'Œ' => "OE",
        '\u{2010}'..='\u{2015}' | '\u{2212}' => "-",
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{2032}' => "'",
        '\u{201c}' | '\u{201d}' | '\u{201e}' => "\"",
        'ä' => "ae",
        'ö' => "oe",
        'ü' => "ue",
        'ß' => "ss",
        'æ' => "ae",
        'œ' => "oe",
        'ø' => "o",
        'ð' | 'đ' => "d",
        'þ' => "th",
        'ł' => "l",
        'ħ' => "h",
        'ŧ' => "t",
        'ŋ' => "n",
        'ı' => "i",
        'ĸ' => "k",
        'ơ' => "o",
        'ư' => "u",
        'ə' => "e",
        'а'..='я' => CYRILLIC[c as usize - 'а' as usize],
        'ё' => "yo",
        'ґ' => "g",
        'є' => "ye",
        'і' => "i",
        'ј' => "j",
        'ђ' => "dj",
        'ћ' => "c",
        'љ' => "lj",
        'њ' => "nj",
        'џ' | 'ѕ' => "dz",
        'α'..='ω' => GREEK[c as usize - 'α' as usize],
        _ => return None,
    })
}

/// `c` spelled in printable ASCII: itself when it already is one, else the table spelling, else
/// its compatibility decomposition without the combining marks.
///
/// A decomposition piece nothing reaches is dropped rather than failing the character, because a
/// letter beside it still carries the name: `ŀ` decomposes to `l` and a middle dot.
///
/// `None` for whitespace, for a control, and for every script nothing reaches, such as CJK.
fn spell_ascii(c: char) -> Option<String> {
    if c.is_ascii_graphic() {
        return Some(c.to_string());
    }
    if c.is_whitespace() || c.is_control() {
        return None;
    }
    if let Some(spelled) = spelling(c) {
        return Some(spelled.to_string());
    }
    if c.is_uppercase() {
        if let Some(spelled) = c.to_lowercase().next().and_then(spelling) {
            return Some(capitalized(spelled));
        }
    }
    let base: String = c.nfkd().filter(|piece| !is_combining_mark(*piece)).collect();
    // Decomposition is idempotent, so a character that decomposes to itself has nothing more to
    // give, and the recursion below is therefore one level deep.
    if base.chars().eq(core::iter::once(c)) {
        return None;
    }
    let spelled: String = base.chars().filter_map(spell_ascii).collect();
    (!spelled.is_empty()).then_some(spelled)
}

/// [`spell_ascii`] for one character of a name. `upper_word` comes from [`upper_word_mask`] and
/// puts a multi-letter spelling in full capitals, so `ЖУК` is `ZHUK` where `Жуков` is `Zhukov`.
///
/// Callers turn `None` into a word break or drop the character.
pub fn to_ascii(c: char, upper_word: bool) -> Option<String> {
    let spelled = spell_ascii(c)?;
    Some(if upper_word { spelled.to_ascii_uppercase() } else { spelled })
}

/// Whether each character of `raw` sits in a word of two or more letters that are all uppercase.
///
/// A single capital is title case, not shouting, so it is not marked: the rule reads the word, and
/// one letter is not a word's worth of evidence.
fn upper_word_mask(raw: &str) -> Vec<bool> {
    let chars: Vec<char> = raw.chars().collect();
    let mut mask = vec![false; chars.len()];
    let mut at = 0;
    while at < chars.len() {
        if !chars[at].is_alphabetic() {
            at += 1;
            continue;
        }
        let end = chars[at..].iter().position(|c| !c.is_alphabetic()).map_or(chars.len(), |n| at + n);
        if end - at >= 2 && chars[at..end].iter().all(|c| c.is_uppercase()) {
            mask[at..end].fill(true);
        }
        at = end;
    }
    mask
}

/// Every character of `raw` spelled in printable ASCII, with a word break where nothing reaches
/// one. Whitespace collapses and the result is trimmed.
pub fn to_ascii_name(raw: &str) -> String {
    let spelled: String = raw
        .chars()
        .zip(upper_word_mask(raw))
        .map(|(c, upper)| to_ascii(c, upper).unwrap_or_else(|| " ".into()))
        .collect();
    collapse(&spelled)
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `raw` rewritten into the device repertoire: a character the font draws is kept as it is, one it
/// does not is spelled in ASCII, and one that nothing reaches stays put, so the result still fails
/// the drawable test. Runs of whitespace collapse to one space, and the result is trimmed.
pub fn to_repertoire(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .zip(upper_word_mask(raw))
        .map(|(c, upper)| {
            if c.is_whitespace() {
                " ".to_string()
            } else if glyph_supported(c) {
                c.to_string()
            } else {
                to_ascii(c, upper).unwrap_or_else(|| c.to_string())
            }
        })
        .collect();
    collapse(&mapped)
}

/// Every character of `s` has a glyph in the device font.
fn drawable(s: &str) -> bool {
    s.chars().all(glyph_supported)
}

/// The name the device stores: `source` folded into the repertoire, else the first `fallbacks`
/// entry that folds cleanly, else the fold with the unreachable characters dropped.
///
/// `None` when nothing readable is left. The caller then drops the name, or the record with it,
/// because a row of question marks is worse than no label.
pub fn device_name<'a>(source: &str, fallbacks: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let folded = to_repertoire(source);
    if let Some(name) = usable(&folded) {
        return Some(name);
    }
    for fallback in fallbacks {
        if let Some(name) = usable(&to_repertoire(fallback)) {
            return Some(name);
        }
    }
    // A dropped character is a word break, never nothing: `AB東CD` is two words, not `ABCD`.
    let broken: String = folded.chars().map(|c| if glyph_supported(c) { c } else { ' ' }).collect();
    usable(&collapse(&broken))
}

fn usable(folded: &str) -> Option<String> {
    (!folded.is_empty() && drawable(folded)).then(|| folded.to_string())
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 45-line letter table this module replaced, kept verbatim as the oracle below.
    #[rustfmt::skip]
    fn deleted_table(c: char) -> Option<&'static str> {
        Some(match c {
            'Ä' => "Ae",
            'ä' => "ae",
            'Ö' => "Oe",
            'ö' => "oe",
            'Ü' => "Ue",
            'ü' => "ue",
            'ß' | 'ſ' => "ss",
            'Æ' => "AE",
            'æ' => "ae",
            'Œ' => "OE",
            'œ' => "oe",
            'Ĳ' => "IJ",
            'ĳ' => "ij",
            'Þ' => "Th",
            'þ' => "th",
            'Ð' | 'Đ' | 'Ď' => "D",
            'ð' | 'đ' | 'ď' => "d",
            'À'..='Å' | 'Ā' | 'Ă' | 'Ą' => "A",
            'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
            'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => "C",
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
            'È'..='Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => "E",
            'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => "G",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'Ĥ' | 'Ħ' => "H",
            'ĥ' | 'ħ' => "h",
            'Ì'..='Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => "I",
            'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
            'Ĵ' => "J",
            'ĵ' => "j",
            'Ķ' => "K",
            'ķ' | 'ĸ' => "k",
            'Ĺ' | 'Ļ' | 'Ľ' | 'Ŀ' | 'Ł' => "L",
            'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
            'Ñ' | 'Ń' | 'Ņ' | 'Ň' | 'Ŋ' => "N",
            'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' | 'ŋ' => "n",
            'Ò'..='Õ' | 'Ø' | 'Ō' | 'Ŏ' | 'Ő' => "O",
            'ò'..='õ' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'Ŕ' | 'Ŗ' | 'Ř' => "R",
            'ŕ' | 'ŗ' | 'ř' => "r",
            'Ś' | 'Ŝ' | 'Ş' | 'Š' => "S",
            'ś' | 'ŝ' | 'ş' | 'š' => "s",
            'Ţ' | 'Ť' | 'Ŧ' => "T",
            'ţ' | 'ť' | 'ŧ' => "t",
            'Ù'..='Û' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "U",
            'ù'..='û' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'Ŵ' => "W",
            'ŵ' => "w",
            'Ý' | 'Ŷ' | 'Ÿ' => "Y",
            'ý' | 'ÿ' | 'ŷ' => "y",
            'Ź' | 'Ż' | 'Ž' => "Z",
            'ź' | 'ż' | 'ž' => "z",
            _ => return None,
        })
    }

    /// The one character whose spelling deliberately differs from the table above: long s is one
    /// `s`, the letter it stands for, not the `ss` the table gave it.
    const IMPROVED: [(char, &str); 1] = [('\u{17f}', "s")];

    /// Every character the deleted table spelled is spelled the same way now, so replacing it with
    /// decomposition lost no letter. It reaches past Latin Extended-B into the punctuation block.
    #[test]
    fn the_deleted_letter_table_is_reproduced() {
        for code in 0..=0x2FFF {
            let Some(c) = char::from_u32(code) else { continue };
            let Some(want) = deleted_table(c) else { continue };
            let want = IMPROVED.iter().find(|(ch, _)| *ch == c).map_or(want, |(_, s)| s);
            assert_eq!(to_ascii(c, false).as_deref(), Some(want), "{c:?} (U+{code:04X})");
        }
    }

    /// The fold may never touch a character the device can already draw, so the packer's table can
    /// not disagree with the font about the repertoire.
    #[test]
    fn every_drawable_character_survives_the_fold() {
        for code in 0..=char::MAX as u32 {
            let Some(c) = char::from_u32(code) else { continue };
            // A drawable space still collapses, and a control is dropped by the record writer.
            if !glyph_supported(c) || c.is_whitespace() || c.is_control() {
                continue;
            }
            assert_eq!(to_repertoire(&c.to_string()), c.to_string(), "{c:?} (U+{code:04X}) is drawable");
        }
    }

    #[test]
    fn unsupported_letters_take_their_closest_latin_spelling() {
        assert_eq!(to_repertoire("Ploiești"), "Ploiesti", "Romanian comma-below decomposes");
        assert_eq!(to_repertoire("Αθήνα"), "Athina", "Greek transliterates, accents included");
        assert_eq!(to_repertoire("Москва"), "Moskva");
        assert_eq!(to_repertoire("Zürich"), "Zürich", "Latin-1 is in the font and is left alone");
        assert_eq!(to_repertoire("Grüßau"), "Grüßau", "so is Latin Extended-A");
        assert_eq!(to_repertoire("Đà Nẵng"), "Đà Nang", "Đ is in the font; the stacked Vietnamese marks are not");
    }

    /// A shouted word shouts its digraphs too; one capital is a name, not shouting.
    #[test]
    fn a_word_in_capitals_keeps_its_spelling_in_capitals() {
        assert_eq!(to_repertoire("ЖУК"), "ZHUK");
        assert_eq!(to_repertoire("Жуков"), "Zhukov");
        assert_eq!(to_repertoire("ΑΘΗΝΑ"), "ATHINA");
        assert_eq!(to_repertoire("Αθήνα"), "Athina");
        assert_eq!(to_repertoire("ЩУКА Жуков"), "SHCHUKA Zhukov", "the rule reads one word at a time");
    }

    #[test]
    fn a_name_the_fold_cannot_reach_walks_the_fall_backs() {
        assert_eq!(device_name("東京", ["Tokyo"]).as_deref(), Some("Tokyo"));
        assert_eq!(device_name("東京", []), None, "nothing readable is left");
        assert_eq!(device_name("Café 東京", []).as_deref(), Some("Café"), "the last rung drops what is left");
        assert_eq!(device_name("AB東CD", []).as_deref(), Some("AB CD"), "and breaks the word rather than glue it");
        assert_eq!(device_name("Αθήνα", ["Athens"]).as_deref(), Some("Athina"), "a fold beats a fall-back");
        assert_eq!(device_name("  ", ["Tokyo"]).as_deref(), Some("Tokyo"), "an empty name is not a name");
    }

    #[test]
    fn the_ascii_fold_spells_out_what_the_font_would_still_draw() {
        assert_eq!(to_ascii_name("Bäckerei Müller"), "Baeckerei Mueller");
        assert_eq!(to_ascii_name("Straße"), "Strasse");
        assert_eq!(to_ascii_name("Ærøskøbing"), "AEroskobing", "the ligature keeps its capitals");
        assert_eq!(to_ascii_name("Paral·lel"), "Paral lel", "a middle dot of its own is a word break");
        assert_eq!(to_ascii_name("Paraŀlel"), "Parallel", "but the ligature keeps the letter beside it");
        assert_eq!(to_ascii_name("北京烤鸭"), "", "an unreachable script leaves nothing");
    }
}
