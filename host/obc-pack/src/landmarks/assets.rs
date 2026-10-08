use super::*;
use scraper::{Html, Selector};

fn supported_license(url: &str) -> bool {
    credit::licence(url).is_ok()
}
fn attribution(
    source_url: String,
    revision: String,
    license_url: String,
    original_notices: String,
) -> Result<Attribution, String> {
    if !license_url.is_empty() && !supported_license(&license_url) {
        return Err("unsupported_license".into());
    }
    if original_notices.len() > text::MAX_NOTICE_BYTES {
        return Err("attribution_bytes".into());
    }
    Ok(Attribution { source_url, revision, license_url, original_notices })
}

pub(super) fn article(root: &Path, sources: &[Source], entity: &Value, capture: &Value) -> Result<Article, String> {
    if capture["compact"] == true {
        let raw = json_pinned(root, sources, string(capture, "path")?)?;
        let language = string(&raw, "language")?;
        let title = string(&raw, "title")?;
        if raw["kind"] != "article"
            || raw["status"] != "present"
            || raw["revision"] != capture["revision"]
            || raw["qid"].as_str().is_some_and(|id| entity["id"] != id)
            || entity["sitelinks"][format!("{language}wiki")]["title"]
                != raw["key"]
                    .as_str()
                    .and_then(|key| key.split_once(':'))
                    .map(|(_, title)| Value::String(title.into()))
                    .unwrap_or(Value::Null)
        {
            return Err("article_identity_mismatch".into());
        }
        let markup = string(&raw, "lead_html")?;
        let pages = text::article_pages(markup).map_err(str::to_owned)?;
        let attribution = attribution(
            string(&raw, "url")?.into(),
            raw["revision"].to_string(),
            raw["license"].as_str().or_else(|| raw["license"]["url"].as_str()).ok_or("article_license_missing")?.into(),
            string(&raw, "original_notices")?.into(),
        )?;
        if credit::article(&attribution)?[1] != text::normalize(title) {
            return Err("article_identity_mismatch".into());
        }
        return Ok(Article {
            language: language.into(),
            pages,
            attribution,
            lead_image: commons_lead_image(&Html::parse_fragment(markup)),
        });
    }
    let language = string(capture, "language")?;
    let title = string(capture, "title")?;
    let expected =
        entity["sitelinks"][format!("{language}wiki")]["title"].as_str().ok_or("article_identity_mismatch")?;
    let raw = json_pinned(root, sources, string(capture, "path")?)?;
    let page = resolved_page(&raw, expected)?;
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
            if matches!(url,
                "/wiki/Wikipedia:Text_of_the_Creative_Commons_Attribution-ShareAlike_4.0_International_License"
                | "/wiki/Wikipedia:Texto_de_la_Licencia_Creative_Commons_Atribuci%C3%B3n-CompartirIgual_4.0_Internacional") {
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
    let attribution = attribution(url, revision.to_string(), license, notices)?;
    if credit::article(&attribution)?[1] != text::normalize(title) {
        return Err("article_identity_mismatch".into());
    }
    let body = Html::parse_fragment(&body);
    let lead_image = commons_lead_image(&body);
    Ok(Article { language: language.to_owned(), pages, attribution, lead_image })
}

pub(super) fn commons_lead_image(body: &Html) -> Option<String> {
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
        .and_then(|href| href.split_once("/wiki/File:").map(|(_, file)| file).or_else(|| href.strip_prefix("./File:")))
        .and_then(|file| percent_encoding::percent_decode_str(file).decode_utf8().ok())
        .map(|file| file.replace('_', " "))
}

pub(super) struct Article {
    pub(super) language: String,
    pub(super) pages: Vec<String>,
    pub(super) attribution: Attribution,
    pub(super) lead_image: Option<String>,
}

/// The ranking evidence of one captured photo. Every field comes from pinned bytes, so the policy
/// digest covers the order the compiler picks in.
pub(super) struct Signals {
    pub(super) filename: String,
    pub(super) categories: BTreeSet<String>,
    pub(super) camera: Option<(f64, f64)>,
    pub(super) depicts: BTreeSet<String>,
}

impl Signals {
    /// Commons keeps views taken from a place, and views of it, in `<prefix><category>`
    /// subcategories of the place's own Commons category. Those categories are the entity's own
    /// P373 claims, so no other place can match. A qualifier follows the name over any
    /// non-alphanumeric boundary, which is a space, a comma or a bracket on Commons.
    pub(super) fn views(&self, categories: &[String], prefix: &str) -> bool {
        let named = |rest: &str| {
            categories.iter().filter_map(|category| category.strip_prefix("Category:")).any(|name| {
                rest.strip_prefix(name).is_some_and(|tail| tail.chars().next().is_none_or(|c| !c.is_alphanumeric()))
            })
        };
        self.categories.iter().any(|title| {
            title
                .strip_prefix("Category:")
                .and_then(|title| title.strip_prefix(prefix))
                .map(|rest| rest.strip_prefix("the ").unwrap_or(rest))
                .is_some_and(named)
        })
    }
}

/// Commons states a signed decimal, as a number or as a string.
fn degrees(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.trim().parse().ok()).filter(|value: &f64| value.is_finite())
}

/// The one Commons file a metadata response describes, and its normalized name.
pub(super) fn described(metadata: &Value) -> Result<(&Value, String), String> {
    let page = metadata["query"]["pages"]
        .as_object()
        .filter(|pages| pages.len() == 1)
        .and_then(|pages| pages.values().next())
        .ok_or("photo_metadata_missing")?;
    let filename = string(page, "title")?.strip_prefix("File:").ok_or("photo_identity_mismatch")?.replace('_', " ");
    Ok((page, filename))
}

/// The evidence is read apart from the photo itself: ranking has to order every candidate before
/// the compiler asks any one of them for pixels.
pub(super) fn signals(root: &Path, sources: &[Source], capture: &Value) -> Result<Signals, String> {
    let metadata = json_pinned(root, sources, string(capture, "metadata_path")?)?;
    let (page, filename) = described(&metadata)?;
    let categories = page["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|category| category["title"].as_str())
        .map(|title| title.replace('_', " "))
        .collect();
    let extended = &page["imageinfo"][0]["extmetadata"];
    let camera = degrees(&extended["GPSLatitude"]["value"])
        .zip(degrees(&extended["GPSLongitude"]["value"]))
        .filter(|(lat, lon)| (-90.0..=90.0).contains(lat) && (-180.0..=180.0).contains(lon));
    let mut depicts = BTreeSet::new();
    if let Some(path) = capture.get("depicts_path").and_then(Value::as_str) {
        let raw = json_pinned(root, sources, path)?;
        for media in raw["entities"].as_object().into_iter().flatten().map(|(_, media)| media) {
            for statement in media["statements"]["P180"].as_array().into_iter().flatten() {
                let id = statement["mainsnak"]["datavalue"]["value"]["id"].as_str();
                if let Some(id) = id.filter(|_| statement["rank"] != "deprecated") {
                    depicts.insert(id.to_owned());
                }
            }
        }
    }
    Ok(Signals { filename, categories, camera, depicts })
}

pub(super) fn photo(
    root: &Path,
    sources: &[Source],
    capture: &Value,
    allowed: &BTreeSet<String>,
    qid: &str,
) -> Result<(Photo, Vec<u8>), String> {
    let metadata = json_pinned(root, sources, string(capture, "metadata_path")?)?;
    let (page, filename) = described(&metadata)?;
    if !allowed.contains(&filename) {
        return Err("photo_identity_mismatch".into());
    }
    let info = &page["imageinfo"][0];
    let attribution = photo_attribution(info)?;
    let input_path = string(capture, "path")?;
    let source = sources.iter().find(|source| source.path == input_path).ok_or("photo_source_missing")?;
    let thumbnail = info.get("thumburl").and_then(Value::as_str).is_some_and(|url| source.url == url);
    if source.url != string(info, "url")? && !thumbnail {
        return Err("photo_identity_mismatch".into());
    }
    let bytes = read_pinned(root, sources, input_path, photo::MAX_SOURCE_BYTES as u64)?;
    let sha1 =
        <sha1::Sha1 as sha1::Digest>::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    if thumbnail {
        for field in ["revision_before_path", "revision_after_path"] {
            let path = capture[field].as_str().ok_or("photo_thumbnail_revision_unverified")?;
            let witness = json_pinned(root, sources, path)?;
            let (witness_page, witness_file) = described(&witness)?;
            let current = &witness_page["imageinfo"][0];
            if witness_file != filename || !revision_matches(info, current) {
                return Err("photo_revision_mismatch".into());
            }
        }
    }
    if !thumbnail && info["sha1"].as_str() != Some(&sha1) {
        return Err("photo_revision_mismatch".into());
    }
    let pixels = photo::prepare(&bytes).map_err(str::to_owned)?;
    let path = format!("{qid}.rgb222");
    Ok((
        Photo {
            credit: credit::online_photo(&attribution)?,
            file_identity: page["pageid"].as_u64().map(|page_id| PhotoIdentity { filename: filename.clone(), page_id }),
            page_revision: page["revisions"][0]["revid"].as_u64(),
            file_revision: Some(FileRevision {
                timestamp: string(info, "timestamp")?.into(),
                sha1: string(info, "sha1")?.into(),
            }),
            online_url: info["thumburl"].as_str().map(str::to_owned),
            path,
            sha256: hash(&pixels),
            bytes: pixels.len(),
            attribution,
        },
        pixels,
    ))
}

fn revision_matches(expected: &Value, current: &Value) -> bool {
    if !["timestamp", "sha1"].iter().all(|field| {
        expected[field].as_str().is_some_and(|value| !value.is_empty()) && expected[field] == current[field]
    }) {
        return false;
    }
    if let Some(revision) = expected["description_revision"].as_u64() {
        return current["description_revision"].as_u64() == Some(revision);
    }
    [
        "Artist",
        "Attribution",
        "Permission",
        "License",
        "LicenseUrl",
        "Copyrighted",
        "AttributionRequired",
        "Categories",
        "ObjectName",
        "Credit",
        "LicenseShortName",
        "UsageTerms",
    ]
    .iter()
    .all(|field| expected["extmetadata"][field]["value"] == current["extmetadata"][field]["value"])
}

pub(super) fn photo_attribution(info: &Value) -> Result<Attribution, String> {
    let ext = &info["extmetadata"];
    let license = ext["LicenseUrl"]["value"].as_str().unwrap_or_default().to_owned();
    let source_url = string(info, "descriptionurl")?.to_owned();
    // Metadata provenance and HTML decoration are retained in the pinned response. The credit
    // notices keep requested attribution and residual Permission text; licence boilerplate is
    // represented by its matching URI.
    let mut notices = serde_json::Map::new();
    for (key, field) in ext.as_object().ok_or("photo_notices")? {
        let value = if key == "Permission" {
            Value::String(credit::permission(field["value"].as_str().unwrap_or_default(), &license, &source_url)?)
        } else {
            field["value"].clone()
        };
        notices.insert(
            key.clone(),
            if key == "Permission" {
                serde_json::json!({"value":value,"format":"plain"})
            } else {
                serde_json::json!({"value":value})
            },
        );
    }
    let original = serde_json::to_string(&notices).map_err(|e| e.to_string())?;
    let attribution = attribution(source_url, string(info, "timestamp")?.to_owned(), license, original)?;
    credit::photo(&attribution)?;
    Ok(attribution)
}

/// Check the API normalization and redirect chain against the requested title.
pub(super) fn resolved_page<'a>(raw: &'a Value, title: &str) -> Result<&'a Value, String> {
    let mut title = title.to_owned();
    for key in ["normalized", "redirects"] {
        let entries = raw["query"][key].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let mut seen = BTreeSet::new();
        while let Some(entry) = entries.iter().find(|entry| entry["from"] == title) {
            if !seen.insert(title.clone()) {
                return Err("article_redirect_cycle".into());
            }
            title = string(entry, "to")?.to_owned();
        }
    }
    let pages = raw["query"]["pages"].as_object().ok_or("missing_article_page")?;
    if pages.len() != 1 {
        return Err("article_identity_mismatch".into());
    }
    let page = pages.values().next().unwrap();
    if page["title"] != title || page.get("missing").is_some() {
        return Err("article_identity_mismatch".into());
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_article_normalization_starts_with_the_exact_osm_title() {
        let raw = serde_json::json!({"query": {
            "normalized": [{"from": "mount_Everest", "to": "Mount Everest"}],
            "redirects": [{"from": "Mount Everest", "to": "Everest"}],
            "pages": {"1": {"pageid": 1, "title": "Everest"}}
        }});
        assert_eq!(resolved_page(&raw, "mount_Everest").unwrap()["title"], "Everest");
        assert!(resolved_page(&raw, "Different summit").is_err());
    }

    #[test]
    fn a_views_subcategory_belongs_to_one_place_only() {
        let signals = |title: &str| Signals {
            filename: "File.png".into(),
            categories: BTreeSet::from([title.to_owned()]),
            camera: None,
            depicts: BTreeSet::new(),
        };
        let claimed = ["Category:Alpspitz".to_owned(), "Category:Hochblassen".to_owned()];
        for (title, expected) in [
            ("Category:Views from Alpspitz", true),
            ("Category:Views from the Alpspitz in winter", true),
            ("Category:Views from Alpspitz, Bavaria", true),
            ("Category:Views from Alpspitz (winter)", true),
            ("Category:Views from Hochblassen", true),
            ("Category:Views from Alpspitzli", false),
            ("Category:Views of Alpspitz", false),
            ("Category:Alpspitz", false),
        ] {
            assert_eq!(signals(title).views(&claimed, "Views from "), expected, "{title}");
        }
    }

    #[test]
    fn a_local_wikipedia_lead_cannot_alias_a_commons_filename() {
        for (repository, expected) in [("commons", Some("Example.jpg")), ("en", None)] {
            let body = Html::parse_fragment(&format!("<a class='mw-file-description' href='/wiki/File:Example.jpg'><img src='https://upload.wikimedia.org/wikipedia/{repository}/a/ab/Example.jpg'></a><a class='mw-file-description' href='/wiki/File:Later.jpg'><img src='//upload.wikimedia.org/wikipedia/commons/b/bb/Later.jpg'></a>"));
            assert_eq!(commons_lead_image(&body).as_deref(), expected);
        }
    }
}
