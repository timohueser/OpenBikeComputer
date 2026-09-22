use obc_render::text::Font;
use scraper::{ElementRef, Html, Node, Selector};

pub const PAGE_WIDTH: u32 = 216;
pub const PAGE_HEIGHT: u32 = 240;
pub const MAX_TEXT_PAGES: usize = 4;
pub const MAX_CREDIT_BYTES: usize = 8192;
pub const MAX_SOURCE_PAGES: usize = 256;

/// Typography substitutions do not transliterate names or replace unsupported letters.
pub fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201a}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' => '"',
            '\u{2010}'..='\u{2014}' | '\u{2212}' => '-',
            '\u{a0}' | '\u{202f}' => ' ',
            _ => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every character has a glyph in the device font. `glyph_supported` reads the real strip, and
/// it answers true for DEL, which the strip maps but does not draw, so controls are excluded here.
pub fn supported(text: &str) -> bool {
    text.chars().all(|c| matches!(c, '\n' | '\t') || (!c.is_control() && obc_render::glyph_supported(c)))
}

fn excluded(element: ElementRef<'_>) -> bool {
    matches!(element.value().name(), "table" | "sup" | "script" | "style" | "figure" | "audio")
        || element.value().classes().any(|class| {
            matches!(
                class,
                "hatnote"
                    | "infobox"
                    | "reference"
                    | "mw-ref"
                    | "IPA"
                    | "IPA-label"
                    | "pronunciation"
                    | "noprint"
                    | "mw-editsection"
                    | "shortdescription"
            )
        })
}

pub fn plain_markup(markup: &str) -> String {
    let document = Html::parse_fragment(markup);
    let mut text = String::new();
    for node in document.tree.root().descendants() {
        if let Node::Text(value) = node.value() {
            if !node.ancestors().filter_map(ElementRef::wrap).any(excluded) {
                text.push_str(value);
            }
        }
    }
    normalize(&text)
}

pub fn lead(markup: &str) -> String {
    let document = Html::parse_document(markup);
    let selector = Selector::parse("p, h2").expect("fixed selector");
    let mut paragraphs = Vec::new();
    for element in document.select(&selector) {
        if element.ancestors().filter_map(ElementRef::wrap).any(excluded) {
            continue;
        }
        if element.value().name() == "h2" {
            break;
        }
        let text = plain_markup(&element.inner_html());
        if !text.is_empty() {
            paragraphs.push(text);
        }
    }
    paragraphs.join(" ")
}

/// Keep complete sentences in source order. Parentheses, initials and common abbreviations
/// do not terminate a sentence; decimals are never split.
pub fn sentences(text: &str) -> Vec<&str> {
    let mut output = Vec::new();
    let mut start = 0;
    let mut depth = 0u32;
    for (at, ch) in text.char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth != 0 || !matches!(ch, '.' | '!' | '?') {
            continue;
        }
        let end = at + ch.len_utf8();
        if !text[end..].is_empty() && !text[end..].starts_with(char::is_whitespace) {
            continue;
        }
        let word = text[..end].split_whitespace().next_back().unwrap_or("");
        let stem = word.trim_end_matches('.');
        let next = text[end..].split_whitespace().next().unwrap_or("");
        let ordinal = !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit());
        let roman = !stem.is_empty() && stem.chars().all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'));
        let continues = next.starts_with(char::is_lowercase) || next.starts_with('(');
        let date_or_century = matches!(
            next.trim_end_matches(['.', ',', ':', ';', '!', '?']).to_ascii_lowercase().as_str(),
            "januar"
                | "februar"
                | "märz"
                | "april"
                | "mai"
                | "juni"
                | "juli"
                | "august"
                | "september"
                | "oktober"
                | "november"
                | "dezember"
                | "jahrhundert"
                | "jahrhunderts"
        );
        if ch == '.'
            && ((ordinal && (continues || date_or_century))
                || (roman && continues)
                || matches!(
                    word.to_ascii_lowercase().as_str(),
                    "mr."
                        | "mrs."
                        | "ms."
                        | "dr."
                        | "st."
                        | "mt."
                        | "prof."
                        | "e.g."
                        | "i.e."
                        | "ca."
                        | "c."
                        | "no."
                        | "etc."
                )
                || (word.chars().count() == 2
                    && word.chars().next().is_some_and(char::is_alphabetic)
                    && (!roman || continues)))
        {
            continue;
        }
        output.push(text[start..end].trim());
        start = end;
    }
    output
}

pub fn pages(text: &str, maximum: usize) -> Result<Vec<String>, &'static str> {
    if !supported(text) {
        return Err("unsupported_glyph");
    }
    let columns = (PAGE_WIDTH / Font::Label.char_width()) as usize;
    let rows = (PAGE_HEIGHT / Font::Label.line_height()) as usize;
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let count = word.chars().count();
        if count > columns {
            return Err("unbreakable_word");
        }
        if !line.is_empty() && line.chars().count() + 1 + count > columns {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > maximum * rows {
        return Err("page_budget");
    }
    Ok(lines.chunks(rows).map(|part| part.join("\n")).collect())
}

pub fn article_pages(markup: &str) -> Result<Vec<String>, &'static str> {
    let text = lead(markup);
    let sentences = sentences(&text);
    let first = sentences.first().ok_or("no_complete_sentence")?;
    let mut result = pages(first, MAX_TEXT_PAGES)?;
    if let Some(second) = sentences.get(1) {
        if let Ok(two) = pages(&format!("{first} {second}"), MAX_TEXT_PAGES) {
            result = two;
        }
    }
    Ok(result)
}

/// URLs may wrap at any character; their percent-encoded bytes remain reconstructible.
pub fn credit_pages(text: &str) -> Result<Vec<String>, &'static str> {
    if text.len() > MAX_CREDIT_BYTES {
        return Err("attribution_bytes");
    }
    if !supported(text) {
        return Err("attribution_glyph");
    }
    let columns = (PAGE_WIDTH / Font::Label.char_width()) as usize;
    let rows = (PAGE_HEIGHT / Font::Label.line_height()) as usize;
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let chars: Vec<_> = paragraph.chars().collect();
        lines.extend(chars.chunks(columns).map(|line| line.iter().collect::<String>()));
    }
    if lines.len() > MAX_SOURCE_PAGES * rows {
        return Err("attribution_pages");
    }
    Ok(lines.chunks(rows).map(|part| part.join("\n")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structural_lead_and_complete_sentences() {
        let html = "<table class=infobox><tr><td><p>Wrong.</p></td></tr></table><p><b>St. Mary's</b> is 2.5 km long (measured by Dr. May).<sup>[1]</sup> It is old.</p><h2>History</h2><p>Later.</p>";
        let text = lead(html);
        assert_eq!(sentences(&text), ["St. Mary's is 2.5 km long (measured by Dr. May).", "It is old."]);
        assert!(article_pages(html).unwrap().join(" ").contains("Mary's"));
        assert_eq!(
            sentences("Die Burg wurde am 1. Mai 1280 von Heinrich IV. gegründet. Sie steht im 13. Jahrhundert."),
            ["Die Burg wurde am 1. Mai 1280 von Heinrich IV. gegründet.", "Sie steht im 13. Jahrhundert."]
        );
        assert_eq!(
            sentences("It belonged to Henry IV. It burned in 1940. Then it was rebuilt."),
            ["It belonged to Henry IV.", "It burned in 1940.", "Then it was rebuilt."]
        );
    }
    #[test]
    fn unsupported_and_oversized_first_sentence_are_not_replaced() {
        assert_eq!(article_pages("<p>城 is old. A better story.</p>"), Err("unsupported_glyph"));
        let long = format!("<p>{}. Short.</p>", "word ".repeat(200));
        assert_eq!(article_pages(&long), Err("page_budget"));
        assert_eq!(pages("abcdefghijklmnopqrs", 4), Err("unbreakable_word"));
    }
    #[test]
    fn credits_have_an_independent_budget_and_preserve_characters() {
        let source = format!("Creator Áine\nhttps://example.org/{}", "a".repeat(400));
        assert_eq!(credit_pages(&source).unwrap().join("").replace('\n', ""), source.replace('\n', ""));
        assert_eq!(credit_pages(&"x".repeat(8193)), Err("attribution_bytes"));
        assert_eq!(credit_pages("作者"), Err("attribution_glyph"));
    }
}
