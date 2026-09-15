use super::*;
use scraper::{Html, Selector};

fn supported_license(url: &str) -> bool {
    let suffix = url
        .strip_prefix("https://creativecommons.org/")
        .or_else(|| url.strip_prefix("http://creativecommons.org/"))
        .or_else(|| url.strip_prefix("//creativecommons.org/"));
    let suffix = suffix.map(|value| {
        let value = value.trim_end_matches('/');
        value
            .rsplit_once('/')
            .filter(|(_, tail)| tail.starts_with("deed.") || tail.starts_with("legalcode"))
            .map_or(value, |(base, _)| base)
    });
    matches!(
        suffix,
        Some(
            "publicdomain/zero/1.0"
                | "licenses/by/1.0"
                | "licenses/by/2.0"
                | "licenses/by/2.5"
                | "licenses/by/3.0"
                | "licenses/by/4.0"
                | "licenses/by-sa/1.0"
                | "licenses/by-sa/2.0"
                | "licenses/by-sa/2.5"
                | "licenses/by-sa/3.0"
                | "licenses/by-sa/4.0"
        )
    )
}
fn display_url(url: &str) -> String {
    url.bytes().map(|b| if b.is_ascii_graphic() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
fn credits(markup: &str, origin: &str) -> String {
    let document = Html::parse_fragment(markup);
    let mut parts = vec![text::normalize(&document.root_element().text().collect::<String>())];
    let selector = Selector::parse("a[href]").expect("fixed selector");
    let mut urls = BTreeSet::new();
    for link in document.select(&selector) {
        if let Some(url) = link.value().attr("href") {
            let url = if url.starts_with("//") {
                format!("https:{url}")
            } else if url.starts_with('/') {
                format!("{origin}{url}")
            } else {
                url.to_owned()
            };
            if (url.starts_with("https://") || url.starts_with("http://")) && urls.insert(url.clone()) {
                parts.push(display_url(&url));
            }
        }
    }
    parts.join("\n")
}
fn attribution(
    source_url: String,
    revision: String,
    license_url: String,
    original_notices: String,
    display: String,
) -> Result<Attribution, String> {
    if !supported_license(&license_url) {
        return Err("unsupported_license".into());
    }
    if original_notices.len() > text::MAX_CREDIT_BYTES {
        return Err("attribution_bytes".into());
    }
    let display_pages = text::credit_pages(&display).map_err(str::to_owned)?;
    Ok(Attribution { source_url, revision, license_url, original_notices, display_pages })
}

pub(super) fn article(root: &Path, sources: &[Source], entity: &Value, capture: &Value) -> Result<Article, String> {
    let language = string(capture, "language")?;
    let title = string(capture, "title")?;
    if entity["sitelinks"][format!("{language}wiki")]["title"].as_str() != Some(title) {
        return Err("article_identity_mismatch".into());
    }
    let raw = json_pinned(root, sources, string(capture, "path")?)?;
    let page =
        raw["query"]["pages"].as_object().and_then(|pages| pages.values().next()).ok_or("missing_article_page")?;
    let revision = capture["revision"].as_u64().ok_or("missing_article_revision")?;
    if page["revisions"][0]["revid"].as_u64() != Some(revision) || page["title"] != title {
        return Err("article_revision_mismatch".into());
    }
    if page["pageprops"]["wikibase_item"].as_str().is_some_and(|id| entity["id"] != id) {
        return Err("article_identity_mismatch".into());
    }
    let markup = String::from_utf8(read_pinned(root, sources, string(capture, "html_path")?, 4 * 1024 * 1024)?)
        .map_err(|_| "article_utf8")?;
    let stamp = markup
        .split_once("\"wgRevisionId\":")
        .and_then(|(_, tail)| tail.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|number| number.parse::<u64>().ok());
    if stamp != Some(revision) {
        return Err("rendered_revision_mismatch".into());
    }
    let document = Html::parse_document(&markup);
    let container = Selector::parse("#mw-content-text .mw-parser-output").expect("fixed selector");
    let body = document.select(&container).next().ok_or("missing_rendered_body")?.inner_html();
    let pages = text::article_pages(&body).map_err(str::to_owned)?;
    let license_selector = Selector::parse("#footer-info-copyright a[href]").expect("fixed selector");
    let mut licenses: Vec<_> = document
        .select(&license_selector)
        .filter_map(|a| a.value().attr("href"))
        .map(|url| {
            if url == "/wiki/Wikipedia:Text_of_the_Creative_Commons_Attribution-ShareAlike_4.0_International_License" {
                "https://creativecommons.org/licenses/by-sa/4.0/"
            } else {
                url
            }
        })
        .filter(|url| supported_license(url))
        .collect();
    licenses.sort();
    licenses.dedup();
    let license = licenses.first().ok_or("article_license_missing")?.to_string();
    let license = if license.starts_with("//") { format!("https:{license}") } else { license };
    let notices_selector = Selector::parse("#footer-info-copyright, .mw-parser-output .licensetpl, .mw-parser-output .attribution, .mw-parser-output .source-attribution").expect("fixed selector");
    let notices = document.select(&notices_selector).map(|item| item.inner_html()).collect::<Vec<_>>().join("\n");
    let url = string(capture, "url")?.to_owned();
    if !sources.iter().any(|source| source.path == capture["html_path"] && source.url == url) {
        return Err("article_source_mismatch".into());
    }
    let display = format!("{title}\n{language}\nWikipedia contributors\n{}\nRevision {revision}\n{}\n{}\nExcerpt; typography and page layout changed.", display_url(&url), display_url(&license), credits(&notices, &format!("https://{language}.wikipedia.org")));
    let attribution = attribution(url, revision.to_string(), license, notices, display)?;
    let body = Html::parse_fragment(&body);
    let lead_image = commons_lead_image(&body);
    Ok(Article { language: language.to_owned(), pages, attribution, lead_image })
}

fn commons_lead_image(body: &Html) -> Option<String> {
    body.select(&Selector::parse("a.mw-file-description[href], h2").expect("fixed selector"))
        .next()
        .filter(|element| {
            element
                .select(&Selector::parse("img[src]").expect("fixed selector"))
                .next()
                .and_then(|image| image.value().attr("src"))
                .is_some_and(|url| {
                    url.starts_with("https://upload.wikimedia.org/wikipedia/commons/")
                        || url.starts_with("//upload.wikimedia.org/wikipedia/commons/")
                })
        })
        .and_then(|element| element.value().attr("href"))
        .and_then(|href| href.split_once("/wiki/File:"))
        .and_then(|(_, file)| percent_encoding::percent_decode_str(file).decode_utf8().ok())
        .map(|file| file.replace('_', " "))
}

pub(super) struct Article {
    pub(super) language: String,
    pub(super) pages: Vec<String>,
    pub(super) attribution: Attribution,
    pub(super) lead_image: Option<String>,
}

pub(super) fn photo(
    root: &Path,
    sources: &[Source],
    capture: &Value,
    allowed: &BTreeSet<String>,
    qid: &str,
) -> Result<(Photo, Vec<u8>), String> {
    let metadata = json_pinned(root, sources, string(capture, "metadata_path")?)?;
    let page = metadata["query"]["pages"]
        .as_object()
        .and_then(|pages| pages.values().next())
        .ok_or("photo_metadata_missing")?;
    let filename = string(page, "title")?.strip_prefix("File:").ok_or("photo_identity_mismatch")?.replace('_', " ");
    if !allowed.contains(&filename) {
        return Err("photo_identity_mismatch".into());
    }
    let info = &page["imageinfo"][0];
    let ext = &info["extmetadata"];
    let license = ext["LicenseUrl"]["value"].as_str().ok_or("photo_license_missing")?.to_owned();
    let original = serde_json::to_string(ext).map_err(|e| e.to_string())?;
    let mut display = Vec::new();
    for key in
        ["ObjectName", "Artist", "Credit", "Copyright", "Attribution", "Permission", "LicenseShortName", "LicenseUrl"]
    {
        if let Some(value) = ext[key]["value"].as_str() {
            display.push(format!("{key}: {}", credits(value, "https://commons.wikimedia.org")));
        }
    }
    let source_url = string(info, "descriptionurl")?.to_owned();
    display.push(display_url(&source_url));
    display.push("Resized, white padded and ordered dithered to RGB222.".into());
    let attribution =
        attribution(source_url, string(info, "timestamp")?.to_owned(), license, original, display.join("\n"))?;
    let input_path = string(capture, "path")?;
    let source = sources.iter().find(|source| source.path == input_path).ok_or("photo_source_missing")?;
    if source.url != string(info, "url")? {
        return Err("photo_identity_mismatch".into());
    }
    let bytes = read_pinned(root, sources, input_path, photo::MAX_SOURCE_BYTES as u64)?;
    let sha1 =
        <sha1::Sha1 as sha1::Digest>::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    if info["sha1"].as_str() != Some(&sha1) {
        return Err("photo_revision_mismatch".into());
    }
    let pixels = photo::prepare(&bytes).map_err(str::to_owned)?;
    let path = format!("{qid}.rgb222");
    Ok((Photo { path, sha256: hash(&pixels), bytes: pixels.len(), attribution }, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_wikipedia_lead_cannot_alias_a_commons_filename() {
        for (repository, expected) in [("commons", Some("Example.jpg")), ("en", None)] {
            let body = Html::parse_fragment(&format!("<a class='mw-file-description' href='/wiki/File:Example.jpg'><img src='https://upload.wikimedia.org/wikipedia/{repository}/a/ab/Example.jpg'></a><a class='mw-file-description' href='/wiki/File:Later.jpg'><img src='//upload.wikimedia.org/wikipedia/commons/b/bb/Later.jpg'></a>"));
            assert_eq!(commons_lead_image(&body).as_deref(), expected);
        }
    }
}
