//! The Sources credit a map carries for one work: source, title, creator and licence. It is derived
//! from the retained source notices, so the compiler and the map serializer cannot disagree.
use super::{text, Attribution};
use obc_formats::obcm::landmarks::{CREDIT_FIELDS, MAX_ATTRIBUTION_BYTES};
use scraper::Html;
use serde_json::Value;

pub type Credit = [String; CREDIT_FIELDS as usize];

/// The source is the shortest URL that opens the exact revision. Wikipedia's terms of use accept a
/// URL to the page as the credit of its authors.
pub fn article(attribution: &Attribution) -> Result<Credit, &'static str> {
    let url = url::Url::parse(&attribution.source_url).map_err(|_| "article_source_url")?;
    let host = url.host_str().filter(|host| host.ends_with(".wikipedia.org")).ok_or("article_source_url")?;
    let title = url.query_pairs().find(|(key, _)| key == "title").ok_or("article_source_url")?.1.replace('_', " ");
    let revision: u64 = attribution.revision.parse().map_err(|_| "missing_article_revision")?;
    checked([
        format!("{host}/?oldid={revision}"),
        title,
        "Wikipedia contributors".into(),
        licence(&attribution.license_url)?,
    ])
}

/// On Commons the file name is the file page, so it names the photo and locates it. A licensor's
/// requested attribution replaces the plain author.
pub fn photo(attribution: &Attribution) -> Result<Credit, &'static str> {
    let file =
        attribution.source_url.strip_prefix("https://commons.wikimedia.org/wiki/File:").ok_or("photo_source_url")?;
    let file = percent_encoding::percent_decode_str(file).decode_utf8().map_err(|_| "photo_source_url")?;
    let metadata: Value = serde_json::from_str(&attribution.original_notices).map_err(|_| "photo_notices")?;
    let plain = |key: &str| {
        let markup = metadata[key]["value"].as_str().unwrap_or_default();
        text::normalize(&Html::parse_fragment(markup).root_element().text().collect::<String>())
    };
    let creator = Some(plain("Attribution")).filter(|value| !value.is_empty()).unwrap_or_else(|| plain("Artist"));
    let licence = licence(&attribution.license_url)?;
    if creator.is_empty() && !licence.starts_with("CC0") {
        return Err("photo_creator_missing");
    }
    checked(["Wikimedia Commons".into(), file.replace('_', " "), creator, licence])
}

/// The short name and the canonical URI of a supported Creative Commons licence. The 1.0 to 3.0
/// licences require the URI with every copy, and one form for all keeps the rule simple.
pub fn licence(url: &str) -> Result<String, &'static str> {
    let path = url
        .strip_prefix("https://creativecommons.org/")
        .or_else(|| url.strip_prefix("http://creativecommons.org/"))
        .or_else(|| url.strip_prefix("//creativecommons.org/"))
        .ok_or("unsupported_license")?
        .trim_end_matches('/');
    let path = path
        .rsplit_once('/')
        .filter(|(_, tail)| tail.starts_with("deed.") || tail.starts_with("legalcode"))
        .map_or(path, |(base, _)| base);
    match path {
        "publicdomain/zero/1.0" => Ok(format!("CC0 1.0 creativecommons.org/{path}/")),
        _ => match path.strip_prefix("licenses/").and_then(|rest| rest.split_once('/')) {
            Some((kind @ ("by" | "by-sa"), version @ ("1.0" | "2.0" | "2.5" | "3.0" | "4.0"))) => {
                Ok(format!("CC {} {version} creativecommons.org/{path}/", kind.to_ascii_uppercase()))
            }
            _ => Err("unsupported_license"),
        },
    }
}

fn checked(fields: Credit) -> Result<Credit, &'static str> {
    let fields = fields.map(|field| text::normalize(&field));
    if [0, 1, 3].iter().any(|&i| fields[i].is_empty()) {
        return Err("attribution_missing");
    }
    if !fields.iter().all(|field| text::supported(field)) {
        return Err("attribution_glyph");
    }
    let bytes = 2 + (fields.len() + 1) * 4 + fields.iter().map(String::len).sum::<usize>();
    if bytes > MAX_ATTRIBUTION_BYTES as usize {
        return Err("attribution_bytes");
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribution(source_url: &str, revision: &str, license_url: &str, original_notices: &str) -> Attribution {
        Attribution {
            source_url: source_url.into(),
            revision: revision.into(),
            license_url: license_url.into(),
            original_notices: original_notices.into(),
        }
    }

    #[test]
    fn article_credit_drops_the_site_notices_and_keeps_the_revision_link() {
        let source = attribution(
            "https://en.wikipedia.org/w/index.php?title=Dunlough+Castle&oldid=1322295338",
            "1322295338",
            "https://creativecommons.org/licenses/by-sa/4.0/deed.de",
            "By using this site, you agree to the Terms of Use.",
        );
        assert_eq!(
            article(&source).unwrap(),
            [
                "en.wikipedia.org/?oldid=1322295338",
                "Dunlough Castle",
                "Wikipedia contributors",
                "CC BY-SA 4.0 creativecommons.org/licenses/by-sa/4.0/"
            ]
        );
    }

    #[test]
    fn photo_credit_names_the_file_and_the_requested_attribution() {
        let url = "https://commons.wikimedia.org/wiki/File:Fuorcla_Rad%C3%B6nt_2.jpg";
        let artist = r#"{"Artist":{"value":"<a href=\"//commons.wikimedia.org/wiki/User:Ab\">Ab  C</a>"}}"#;
        let by = "https://creativecommons.org/licenses/by/3.0";
        assert_eq!(
            photo(&attribution(url, "t", by, artist)).unwrap(),
            ["Wikimedia Commons", "Fuorcla Radönt 2.jpg", "Ab C", "CC BY 3.0 creativecommons.org/licenses/by/3.0/"]
        );
        let requested = r#"{"Artist":{"value":"Ab"},"Attribution":{"value":"© Ab / Wikimedia Commons"}}"#;
        assert_eq!(photo(&attribution(url, "t", by, requested)).unwrap()[2], "© Ab / Wikimedia Commons");
        assert_eq!(
            photo(&attribution(url, "t", by, r#"{"Credit":{"value":"Own work"}}"#)),
            Err("photo_creator_missing")
        );
        let zero = "https://creativecommons.org/publicdomain/zero/1.0/";
        assert_eq!(
            photo(&attribution(url, "t", zero, "{}")).unwrap()[2..],
            ["", "CC0 1.0 creativecommons.org/publicdomain/zero/1.0/"]
        );
        let long = format!(r#"{{"Artist":{{"value":"{}"}}}}"#, "a ".repeat(600));
        assert_eq!(photo(&attribution(url, "t", by, &long)), Err("attribution_bytes"));
        assert_eq!(photo(&attribution(url, "t", by, r#"{"Artist":{"value":"作者"}}"#)), Err("attribution_glyph"));
        assert_eq!(licence("https://creativecommons.org/licenses/by-nc/4.0/"), Err("unsupported_license"));
    }
}
