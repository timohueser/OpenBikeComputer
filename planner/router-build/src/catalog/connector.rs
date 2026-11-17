//! Connector legs: the signed pieces of a footpath network between two signposts, which are not
//! routes of their own. `specs/route-catalog.md#selection` is the rule.
use crate::source::Tags;

/// The Swiss trail grades: yellow, white-red-white and white-blue-white.
const GRADE_SYMBOLS: [&str; 3] = ["yellow::yellow_diamond", "red:white:red_bar", "blue:white:blue_bar"];
/// A word with one of these endings names a theme trail, not a leg between two places.
const THEME_ENDINGS: [&str; 6] = ["weg", "wäg", "pfad", "trail", "loop", "tour"];

fn is_dash(c: char) -> bool {
    matches!(c, '-' | '–' | '—')
}

/// Whether a selected relation is a connector leg.
pub fn connector(tags: &Tags, has_child_route: bool) -> bool {
    let tag = |key| tags.get(key).unwrap_or("");
    let name = tag("name");
    let shape = match parts(name).as_slice() {
        _ if name.is_empty() => true,
        [a, _] => !a.is_empty(),
        [a, b, c] => !a.is_empty() && !b.is_empty() && !c.is_empty(),
        _ => false,
    };
    matches!(tag("route"), "hiking" | "foot")
        && (tag("osmc:symbol").is_empty() || GRADE_SYMBOLS.contains(&tag("osmc:symbol")))
        && tag("ref").is_empty()
        && tag("roundtrip") != "yes"
        && !has_child_route
        && shape
        && !theme(name)
}

/// The trimmed parts of a name: cut at each dash with a space before it and a space or the end
/// after it, when the name has one; else at each dash. So a dash inside a place name does not cut.
fn parts(name: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = name.char_indices().collect();
    let spaced =
        |k: usize| is_dash(chars[k].1) && k > 0 && chars[k - 1].1 == ' ' && chars.get(k + 1).is_none_or(|c| c.1 == ' ');
    let any_spaced = (0..chars.len()).any(spaced);
    let mut parts = Vec::new();
    let mut start = 0;
    for k in 0..chars.len() {
        if if any_spaced { spaced(k) } else { is_dash(chars[k].1) } {
            parts.push(name[start..chars[k].0].trim());
            start = chars[k].0 + chars[k].1.len_utf8();
        }
    }
    parts.push(name[start..].trim());
    parts
}

/// Whether a word outside round brackets ends in a theme ending, in any letter case.
fn theme(name: &str) -> bool {
    let mut depth = 0usize;
    let outside: String = name
        .chars()
        .map(|c| {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            if depth == 0 && c != ')' {
                c
            } else {
                ' '
            }
        })
        .collect();
    outside
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| THEME_ENDINGS.iter().any(|ending| word.ends_with(ending)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(name: &str, extra: &[(&str, &str)]) -> bool {
        let pairs = [&[("route", "hiking"), ("name", name), ("osmc:symbol", "yellow::yellow_diamond")], extra].concat();
        connector(&Tags::from_pairs(pairs), false)
    }

    #[test]
    fn parts_cut_at_spaced_dashes_first() {
        assert_eq!(parts("Unteriberg-Stöcken - Minsterbrücke"), ["Unteriberg-Stöcken", "Minsterbrücke"]);
        assert_eq!(parts("Emmenbrücke-Rotenburg"), ["Emmenbrücke", "Rotenburg"]);
        assert_eq!(parts("A – B — C"), ["A", "B", "C"]);
        assert_eq!(parts("A -"), ["A", ""]);
        assert_eq!(parts("Pilatus"), ["Pilatus"]);
    }

    #[test]
    fn name_shapes_and_theme_words_decide() {
        for name in ["Rotkreuz - Auleten", "Emmenbrücke-Rotenburg", "Unteriberg-Stöcken - Minsterbrücke", "A -"] {
            assert!(leg(name, &[]), "{name}");
        }
        assert!(leg("Wädenswil - Richterswil (Seeweg)", &[]), "a theme word in brackets does not count");
        assert!(leg("A - B - C", &[]));
        for name in ["Zwergenweg Käserstatt-Lischen", "Brünig - Käserstatt - Gibel - Lungern - Giswil", "Pilatus"] {
            assert!(!leg(name, &[]), "{name}");
        }
        assert!(!leg("A - SEEWÄG", &[]));
        assert!(!leg("- B", &[]));
        assert!(!leg("A - - C", &[]));
    }

    #[test]
    fn tags_and_child_routes_keep_a_route() {
        assert!(!leg("A - B", &[("ref", "7")]));
        assert!(!leg("A - B", &[("roundtrip", "yes")]));
        let pairs = |symbol| [("route", "foot"), ("name", "A - B"), ("osmc:symbol", symbol)];
        assert!(connector(&Tags::from_pairs(pairs("")), false));
        assert!(!connector(&Tags::from_pairs(pairs("red:white:red_bar")), true));
        assert!(!connector(&Tags::from_pairs(pairs("red")), false));
        assert!(!connector(&Tags::from_pairs([("route", "bicycle"), ("name", "A - B")]), false));
    }
}
