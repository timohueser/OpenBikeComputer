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
/// Only lowercase letters are listed. An uppercase letter is looked up in lowercase and
/// capitalized again afterwards, so a digraph keeps its second letter small: `Жуков` is `Zhukov`.
fn spelling(c: char) -> Option<&'static str> {
    Some(match c {
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
/// `None` for whitespace, for a control, and for every script the table does not reach, such as
/// CJK. Callers turn that into a word break or drop the character.
pub fn to_ascii(c: char) -> Option<String> {
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
    base.chars().try_fold(String::new(), |mut out, piece| {
        out.push_str(&to_ascii(piece)?);
        Some(out)
    })
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `raw` rewritten into the device repertoire: a character the font draws is kept as it is, one it
/// does not is spelled in ASCII, and one that nothing reaches stays put so that [`drawable`] still
/// rejects the result. Runs of whitespace collapse to one space, and the result is trimmed.
pub fn to_repertoire(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|c| {
            if c.is_whitespace() {
                " ".to_string()
            } else if glyph_supported(c) {
                c.to_string()
            } else {
                to_ascii(c).unwrap_or_else(|| c.to_string())
            }
        })
        .collect();
    collapse(&mapped)
}

/// Every character of `s` has a glyph in the device font.
pub fn drawable(s: &str) -> bool {
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
    usable(&collapse(&folded.chars().filter(|c| glyph_supported(*c)).collect::<String>()))
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
        assert_eq!(to_repertoire("Жуков"), "Zhukov", "an uppercase digraph keeps its second letter small");
        assert_eq!(to_repertoire("Zürich"), "Zürich", "Latin-1 is in the font and is left alone");
        assert_eq!(to_repertoire("Grüßau"), "Grüßau", "so is Latin Extended-A");
        assert_eq!(to_repertoire("Đà Nẵng"), "Đà Nang", "Đ is in the font; the stacked Vietnamese marks are not");
    }

    #[test]
    fn a_name_the_fold_cannot_reach_walks_the_fall_backs() {
        assert_eq!(device_name("東京", ["Tokyo"]).as_deref(), Some("Tokyo"));
        assert_eq!(device_name("東京", []), None, "nothing readable is left");
        assert_eq!(device_name("Café 東京", []).as_deref(), Some("Café"), "the last rung drops what is left");
        assert_eq!(device_name("Αθήνα", ["Athens"]).as_deref(), Some("Athina"), "a fold beats a fall-back");
        assert_eq!(device_name("  ", ["Tokyo"]).as_deref(), Some("Tokyo"), "an empty name is not a name");
    }

    #[test]
    fn the_ascii_fold_spells_out_what_the_font_would_still_draw() {
        let ascii = |s: &str| s.chars().filter_map(to_ascii).collect::<String>();
        assert_eq!(ascii("Bäckerei Müller"), "BaeckereiMueller", "the caller owns the word breaks");
        assert_eq!(ascii("Straße"), "Strasse");
        assert_eq!(ascii("Ærøskøbing"), "Aeroskobing");
        assert_eq!(to_ascii('東'), None, "an unreachable script has no spelling");
    }
}
