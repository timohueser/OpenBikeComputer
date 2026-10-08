//! The Sources credit a map carries for one work: source, title, creator and licence. It is derived
//! from the retained source notices, so the compiler and the map serializer cannot disagree.
use super::{text, Attribution};
use obc_formats::obcm::landmarks::{CREDIT_FIELDS, MAX_ATTRIBUTION_BYTES};
use scraper::{ElementRef, Html, Node, Selector};
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
    let mut creator = Some(plain("Attribution")).filter(|value| !value.is_empty()).unwrap_or_else(|| plain("Artist"));
    let licence = photo_licence(&attribution.license_url, &metadata)?;
    if creator.is_empty() && !licence.starts_with("CC0") && !licence.starts_with("Public domain") {
        return Err("photo_creator_missing");
    }
    let residual = if metadata["Permission"]["format"] == "plain" {
        metadata["Permission"]["value"].as_str().unwrap_or_default().to_owned()
    } else {
        permission(
            metadata["Permission"]["value"].as_str().unwrap_or_default(),
            &attribution.license_url,
            &attribution.source_url,
        )?
    };
    if !residual.is_empty() {
        if !creator.is_empty() {
            creator.push_str("; ");
        }
        creator.push_str(&residual);
    }
    let title = Some(plain("ObjectName")).filter(|value| !value.is_empty()).unwrap_or_else(|| file.replace('_', " "));
    checked([attribution.source_url.clone(), title, creator, format!("{licence}; resized/dithered")]).map_err(
        |reason| {
            if !residual.is_empty() && matches!(reason, "attribution_bytes" | "attribution_glyph") {
                "photo_permission_unsupported"
            } else {
                reason
            }
        },
    )
}

/// Commons identifies a single licence block and its requested attribution with documented
/// classes. Only a matching supported block is redundant with the encoded licence URI.
/// Text and links outside it, and every requested-attribution node, remain uninterpreted.
pub(super) fn permission(markup: &str, license_url: &str, source_url: &str) -> Result<String, &'static str> {
    let document = Html::parse_fragment(markup);
    let blocks = Selector::parse(".licensetpl").expect("fixed selector");
    let links = Selector::parse(".licensetpl_link").expect("fixed selector");
    let nonfree = Selector::parse(".licensetpl_nonfree").expect("fixed selector");
    let matching: Vec<_> = document
        .select(&blocks)
        .filter_map(|block| {
            let names: Vec<_> =
                block.select(&links).map(|node| text::normalize(&node.text().collect::<String>())).collect();
            let matches =
                names.len() == 1 && licence(license_url).is_ok_and(|expected| licence(&names[0]) == Ok(expected));
            (matches && !block.select(&nonfree).any(|node| text::normalize(&node.text().collect::<String>()) == "true"))
                .then_some(block.id())
        })
        .collect();
    let omitted = |element: Option<ElementRef<'_>>| {
        let Some(element) = element else {
            return false;
        };
        let node = *element;
        let ancestors: Vec<_> = std::iter::once(element).chain(node.ancestors().filter_map(ElementRef::wrap)).collect();
        ancestors.iter().any(|element| matching.contains(&element.id()))
            && !ancestors.iter().any(|element| element.value().classes().any(|class| class == "licensetpl_attr"))
    };
    let mut parts = Vec::new();
    for node in document.tree.root().descendants() {
        if let Node::Text(value) = node.value() {
            if !omitted(node.parent().and_then(ElementRef::wrap)) {
                parts.push(value.to_string());
            }
        }
    }
    let base = url::Url::parse(source_url).map_err(|_| "photo_source_url")?;
    for link in document.select(&Selector::parse("a[href]").expect("fixed selector")) {
        if !omitted(Some(link)) {
            let href = link.value().attr("href").unwrap_or_default();
            let resolved = base.join(href).map_err(|_| "photo_permission_unsupported")?.to_string();
            if !parts.contains(&resolved) {
                parts.push(resolved);
            }
        }
    }
    Ok(text::normalize(&parts.join(" ")))
}

/// The short name and canonical URI of a supported licence. Older CC licences and FAL require
/// a licence reference with every copy.
pub fn licence(url: &str) -> Result<String, &'static str> {
    if matches!(url.trim_end_matches('/'), "http://artlibre.org/licence/lal/en" | "https://artlibre.org/licence/lal/en")
    {
        return Ok("FAL 1.3 artlibre.org/licence/lal/en/".into());
    }
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
    if path == "publicdomain/zero/1.0" {
        return Ok(format!("CC0 1.0 creativecommons.org/{path}/"));
    }
    let parts: Vec<_> = path.split('/').collect();
    match parts.as_slice() {
        ["licenses", kind @ ("by" | "by-sa"), version @ ("1.0" | "2.0" | "2.5" | "3.0" | "4.0")]
        | ["licenses", kind @ ("by" | "by-sa"), version @ ("2.0" | "3.0"), "de"] => {
            let port = if parts.len() == 4 { " DE" } else { "" };
            Ok(format!("CC {} {version}{port} creativecommons.org/{path}/", kind.to_ascii_uppercase()))
        }
        _ => Err("unsupported_license"),
    }
}

/// Empty licence URLs occur on Commons public-domain templates. The basis must be explicit,
/// and the machine-readable notices must agree. A public-domain label alone is insufficient.
fn photo_licence(url: &str, metadata: &Value) -> Result<String, &'static str> {
    if !url.is_empty() {
        return licence(url);
    }
    if metadata["License"]["value"] != "pd"
        || metadata["Copyrighted"]["value"] != "False"
        || metadata["AttributionRequired"]["value"] != "false"
    {
        return Err("unsupported_license");
    }
    let categories = metadata["Categories"]["value"].as_str().unwrap_or_default();
    for (category, basis) in
        [("PD-self", "author release"), ("PD-user", "author release"), ("PD-old-100-expired", "expired copyright")]
    {
        if categories.split('|').any(|value| value == category) {
            return Ok(format!("Public domain ({basis})"));
        }
    }
    Err("unsupported_public_domain_basis")
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
            [url, "Fuorcla Radönt 2.jpg", "Ab C", "CC BY 3.0 creativecommons.org/licenses/by/3.0/; resized/dithered"]
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
            ["", "CC0 1.0 creativecommons.org/publicdomain/zero/1.0/; resized/dithered"]
        );
        let long = format!(r#"{{"Artist":{{"value":"{}"}}}}"#, "a ".repeat(600));
        assert_eq!(photo(&attribution(url, "t", by, &long)), Err("attribution_bytes"));
        assert_eq!(photo(&attribution(url, "t", by, r#"{"Artist":{"value":"作者"}}"#)), Err("attribution_glyph"));
        assert_eq!(licence("https://creativecommons.org/licenses/by-nc/4.0/"), Err("unsupported_license"));
    }
    #[test]
    fn photo_licences_keep_ports_and_require_a_public_domain_basis() {
        assert_eq!(
            licence("https://creativecommons.org/licenses/by-sa/3.0/de/deed.en").unwrap(),
            "CC BY-SA 3.0 DE creativecommons.org/licenses/by-sa/3.0/de/"
        );
        assert!(licence("https://creativecommons.org/licenses/by/4.0/de/").is_err());
        assert_eq!(licence("http://artlibre.org/licence/lal/en").unwrap(), "FAL 1.3 artlibre.org/licence/lal/en/");
        for basis in ["PD-self", "PD-user", "PD-old-100-expired"] {
            let mut metadata = serde_json::json!({"License":{"value":"pd"}, "Copyrighted":{"value":"False"},
                "AttributionRequired":{"value":"false"}, "Categories":{"value":basis}});
            assert!(photo_licence("", &metadata).unwrap().starts_with("Public domain"));
            metadata["AttributionRequired"]["value"] = Value::String("true".into());
            assert_eq!(photo_licence("", &metadata), Err("unsupported_license"));
        }
        let metadata = serde_json::json!({"License":{"value":"pd"}, "Copyrighted":{"value":"False"},
            "AttributionRequired":{"value":"false"}, "Categories":{"value":"PD other reasons"}});
        assert_eq!(photo_licence("", &metadata), Err("unsupported_public_domain_basis"));
    }
    #[test]
    fn retained_custom_permissions_are_preserved_or_rejected_without_truncation() {
        for (url, license, notices) in [
            (
                "https://commons.wikimedia.org/wiki/File:Konstanz_Schloss_Seeheim_asv2022-10.jpg",
                "http://artlibre.org/licence/lal/en",
                r###"{"Artist":{"value":"<b><span class=\"plainlinks\"><a class=\"external text\" href=\"https://commons.wikimedia.org/wiki/User:A.Savin\">A.Savin</a></span></b>"},"Permission":{"value":"Free Art License. Correct attribution is «A.Savin, Wikipedia». Free usage of the photo, no need to ask for approval. Лицензия Свободное искусство. Корректное указание авторства - «А.Савин, Википедия». Использование фотографии бесплатно, согласование не требуется. Lizenz Freie Kunst. Korrekte Autorenkennzeichnung ist «A.Savin, Wikipedia». Nutzung des Fotos kostenlos, Anfrage nicht erforderlich."}}"###,
            ),
            (
                "https://commons.wikimedia.org/wiki/File:Weil_am_Rhein_-_Vitra_Slide_Tower16.jpg",
                "http://artlibre.org/licence/lal/en",
                r###"{"Artist":{"value":"<a href=\"//commons.wikimedia.org/wiki/User:Taxiarchos228\" title=\"User:Taxiarchos228\">Taxiarchos228</a>"},"Permission":{"value":"Copyleft: This work of art is free; you can redistribute it and/or modify it according to terms of the Free Art License. You will find a specimen of this license on the Copyleft Attitude site as well as on other sites. http://artlibre.org/licence/lal/enFALFree Art Licensefalsetrue Bilder des zentralen Medienarchivs Wikimedia Commons, unterstehen einer Freien Lizenz. Diese Freiheit bedeutet nicht, dass dadurch das Urheberrecht entfällt. Ganz im Gegenteil: Als Gegenleistung für die kostenlose (nichtgewerbliche) Nutzung muss der Weiternutzer nur die Lizenzbedingungen einhalten und den Fotografen (meinen vollständigen Klarnamen Wladyslaw Sojka sowie die verlinkte Website www.sojka.photo) als Urheber nennen. Juristisch ist die unterlassene Namensnennung eine Urheberrechtsverletzung bzw. eine Verletzung der Urheberpersönlichkeitsrechte. Als Urheber kann ich mich gegen Bilderklau wehren, in dem ich einen Strafantrag stelle oder unmittelbar einen Rechtsanwalt beauftrage. Die Folge ist eine Abmahnung und die Aufforderung, eine strafbewehrte Unterlassungserklärung abzugeben. So weit sollte es nicht kommen. Bitte achten Sie auf diese Regeln oder kontaktieren Sie mich im Zweifelsfall. Sollten Abweichungen von den hier angegebenen Regeln erwünscht sein, dann bedarf es auf jeden Fall einer ausdrücklichen Genehmigung von mir als Urheber des jeweiligen Bildes. Kontaktmöglichkeiten: über Wikimedia Commons oder über www.sojka.photo."}}"###,
            ),
        ] {
            let source = attribution(url, "t", license, notices);
            assert_eq!(photo(&source), Err("photo_permission_unsupported"));
        }
        let source = attribution(
            "https://commons.wikimedia.org/wiki/File:Example.jpg",
            "t",
            "http://artlibre.org/licence/lal/en",
            r#"{"Artist":{"value":"A.Savin"},"Permission":{"value":"Correct attribution is A.Savin, Wikipedia."}}"#,
        );
        assert_eq!(photo(&source).unwrap()[2], "A.Savin; Correct attribution is A.Savin, Wikipedia.");
    }

    #[test]
    fn matching_licence_blocks_keep_requested_credit_and_every_residual_link() {
        let source = "https://commons.wikimedia.org/wiki/File:Example.jpg";
        let license = "https://creativecommons.org/licenses/by/4.0/";
        let markup = r#"<div class="licensetpl">Standard terms
            <span class="licensetpl_link">https://creativecommons.org/licenses/by/4.0/</span>
            <span class="licensetpl_attr">Credit Example and <a href="https://example.test/author">website</a></span></div>
            <p>Also credit Publisher <a href="/wiki/User:Publisher">profile</a>.</p>"#;
        let notice = permission(markup, license, source).unwrap();
        assert!(!notice.contains("Standard terms"));
        assert!(notice.contains("Credit Example and website"));
        assert!(notice.contains("Also credit Publisher profile"));
        assert!(notice.contains("https://example.test/author"));
        assert!(notice.contains("https://commons.wikimedia.org/wiki/User:Publisher"));
        assert!(permission(markup, "https://creativecommons.org/licenses/by-sa/4.0/", source)
            .unwrap()
            .contains("Standard terms"));
    }
}
