//! Connector legs: the signed pieces of a footpath network between two signposts, which are not
//! routes of their own. `specs/route-catalog.md#selection` is the rule.
use crate::source::Tags;

/// The Swiss trail grades: yellow, white-red-white and white-blue-white.
const GRADE_SYMBOLS: [&str; 3] = ["yellow::yellow_diamond", "red:white:red_bar", "blue:white:blue_bar"];
/// A word with one of these endings names a theme trail, not a leg between two places.
const THEME_ENDINGS: [&str; 6] = ["weg", "wäg", "pfad", "trail", "loop", "tour"];

/// Whether a selected relation is a connector leg.
pub fn connector(tags: &Tags, has_child_route: bool) -> bool {
    let tag = |key| tags.get(key).unwrap_or("");
    let name = tag("name");
    matches!(tag("route"), "hiking" | "foot")
        && (tag("osmc:symbol").is_empty() || GRADE_SYMBOLS.contains(&tag("osmc:symbol")))
        && tag("ref").is_empty()
        && tag("roundtrip") != "yes"
        && !has_child_route
        && (name.is_empty() || name.contains(['-', '–', '—']))
        && !theme(name)
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
    fn a_dash_and_no_theme_word_decide() {
        for name in [
            "Rotkreuz - Auleten",
            "Emmenbrücke-Rotenburg",
            "A – B — C",
            "- Niederrickenbach",
            "Sonnhalde - Linden - Meiestoss - Moos",
            "Wädenswil - Richterswil (Seeweg)",
        ] {
            assert!(leg(name, &[]), "{name}");
        }
        for name in ["Zwergenweg Käserstatt-Lischen", "A - SEEWÄG", "Pilatus"] {
            assert!(!leg(name, &[]), "{name}");
        }
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
