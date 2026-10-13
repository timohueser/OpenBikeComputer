//! Discover missing archives from upstream metadata. A reviewed input keeps its URL, size and
//! object identity; Start downloads it without changing that decision.

use super::{dumps_dir, read_json, regional_osm, stat, Dump};
use obc_data::fetch::http::{Expect, Http};
use obc_data::prepared::{Archive, Input, Planning, Scope};
use obc_data::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const DUMPS: &str = "https://dumps.wikimedia.org";
const ENTERPRISE: &str = "https://api.enterprise.wikimedia.com";

pub(super) fn plan(context: &Planning, scope: Option<&Scope>, languages: &[String]) -> Result<Vec<Input>, String> {
    let directory = dumps_dir().unwrap_or_else(|| context.store.root().join("wiki-dumps"));
    plan_in(&directory, scope, languages, |key, prefix, suffix, area| discover(context, key, prefix, suffix, area))
}

fn plan_in(
    directory: &Path,
    scope: Option<&Scope>,
    languages: &[String],
    mut remote: impl FnMut(&str, &str, &str, Option<&str>) -> Result<Remote, String>,
) -> Result<Vec<Input>, String> {
    let mut inputs = Vec::new();
    for (key, source, prefix, suffix, area) in kinds(scope, languages)? {
        let prefix_path = Path::new(&prefix);
        let dir = directory.join(prefix_path.parent().unwrap_or(Path::new("")));
        let prefix = prefix_path.file_name().and_then(|v| v.to_str()).ok_or("invalid archive prefix")?;
        let names = match fs::read_dir(&dir) {
            Ok(entries) => entries.filter_map(|v| v.ok()?.file_name().into_string().ok()).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        let cached = newest(&names, prefix, &suffix);
        let (day, name, url, size, etag) = if let Some((day, name)) = cached {
            let size = stat(&dir.join(&name))?.0;
            (day, name, None, size, None)
        } else {
            let (day, name, url, size, etag) = remote(&key, prefix, &suffix, area.as_deref())?;
            (day, name, Some(url), size, etag)
        };
        let path = dir.join(&name);
        let dump =
            Dump { key: key.clone(), source, day, path: path.clone(), size, modified: 0, sha256: String::new(), area };
        inputs.push(Input {
            wanted: dump.wanted(),
            bytes: Some(size),
            archive: Some(Archive { key, path, url, etag }),
        });
    }
    Ok(inputs)
}

type Kind = (String, String, String, String, Option<String>);
fn kinds(scope: Option<&Scope>, languages: &[String]) -> Result<Vec<Kind>, String> {
    let area = regional_osm(scope)?;
    let mut kinds = vec![
        ("wikidata".into(), "wikidata".into(), "wikidata-".into(), "-all.json.bz2".into(), None),
        ("wikidata_pages".into(), "wikidata".into(), "wikidatawiki-".into(), "-page.sql.gz".into(), None),
        ("wikidata_redirects".into(), "wikidata".into(), "wikidatawiki-".into(), "-redirect.sql.gz".into(), None),
        (
            "osm".into(),
            if area.is_some() { "geofabrik-extracts" } else { "osm-planet" }.into(),
            area.as_ref().map_or("planet-".into(), |v| format!("{v}-")),
            ".osm.pbf".into(),
            area,
        ),
    ];
    for language in languages {
        kinds.push((
            format!("langlinks:{language}"),
            "wikipedia".into(),
            format!("{language}wiki-"),
            "-langlinks.sql.gz".into(),
            None,
        ));
        kinds.push((
            format!("wikipedia:{language}"),
            "wikipedia".into(),
            format!("{language}wiki-NS0-"),
            "-ENTERPRISE-HTML.json.tar.gz".into(),
            None,
        ));
    }
    Ok(kinds)
}

fn links(html: &str) -> Vec<String> {
    html.split("href=\"").skip(1).filter_map(|part| part.split_once('"').map(|(href, _)| href.to_string())).collect()
}

fn day(digits: &str) -> Option<String> {
    let digits = if digits.len() == 6 { format!("20{digits}") } else { digits.to_string() };
    if digits.len() != 8 || !digits.bytes().all(|v| v.is_ascii_digit()) {
        return None;
    }
    let day = format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..]);
    obc_data::date::parse(&day).map(|_| day)
}

fn newest(names: &[String], prefix: &str, suffix: &str) -> Option<(String, String)> {
    names.iter().filter_map(|name| Some((day(name.strip_prefix(prefix)?.strip_suffix(suffix)?)?, name.clone()))).max()
}

fn dates(http: &Http, base: &str) -> Result<Vec<String>, String> {
    let mut dates: Vec<_> = links(&http.text(base, "text/html")?)
        .into_iter()
        .filter_map(|name| name.strip_suffix('/').filter(|v| v.len() == 8 && day(v).is_some()).map(String::from))
        .collect();
    dates.sort();
    dates.reverse();
    Ok(dates)
}

type Remote = (String, String, String, u64, Option<String>);
fn discover(context: &Planning, key: &str, prefix: &str, suffix: &str, area: Option<&str>) -> Result<Remote, String> {
    let http = context.http;
    if let Some(language) = key.strip_prefix("wikipedia:") {
        let client = enterprise(context.store)?;
        let url = format!("{ENTERPRISE}/v2/snapshots/{language}wiki_namespace_0");
        let info: Value = serde_json::from_str(&client.text(&url, "application/json")?).map_err(|e| e.to_string())?;
        let date =
            info["date_modified"].as_str().and_then(|v| v.get(..10)).ok_or("snapshot has no modification date")?;
        let name = format!("{prefix}{}{suffix}", date.replace('-', ""));
        let url = format!("{url}/download");
        let (size, etag) = client.headers(&url)?;
        if etag.is_none() {
            return Err("the Enterprise snapshot has no strong ETag; cannot pin this download".into());
        }
        return Ok((date.into(), name, url, size, etag));
    }
    if key == "osm" {
        let base = area.map_or("https://planet.openstreetmap.org/pbf/".into(), |area| {
            format!("https://download.geofabrik.de/{}.html", area)
        });
        let names = links(&http.text(&base, "text/html")?);
        let completed: Vec<_> = names.iter().filter(|name| names.contains(&format!("{name}.md5"))).cloned().collect();
        let (date, name) =
            newest(&completed, prefix, suffix).ok_or_else(|| format!("{base}: no completed OSM archive"))?;
        let base = base.rsplit_once('/').unwrap().0;
        let url = format!("{base}/{name}");
        let (size, etag) = http.headers(&url)?;
        return Ok((date, name, url, size, etag));
    }
    let project = if let Some(language) = key.strip_prefix("langlinks:") {
        format!("{language}wiki")
    } else {
        "wikidatawiki".into()
    };
    let base = if key == "wikidata" { format!("{DUMPS}/{project}/entities/") } else { format!("{DUMPS}/{project}/") };
    for date in dates(http, &base)? {
        let name = format!("{prefix}{date}{suffix}");
        let url = format!("{base}{date}/{name}");
        if key == "wikidata" {
            // Checksums appear after the corresponding dump is complete.
            let sums = format!("{base}{date}/wikidata-{date}-sha1sums.txt");
            let text = match http.text(&sums, "text/plain") {
                Ok(text) => text,
                Err(e) if obc_data::fetch::http::not_found(&e) => continue,
                Err(e) => return Err(e),
            };
            if !text.lines().any(|line| line.split_whitespace().last() == Some(name.as_str())) {
                continue;
            }
            let (size, etag) = http.headers(&url)?;
            return Ok((day(&date).unwrap(), name, url, size, etag));
        }
        let status = format!("{base}{date}/dumpstatus.json");
        let text = match http.text(&status, "application/json") {
            Ok(text) => text,
            Err(e) if obc_data::fetch::http::not_found(&e) => continue,
            Err(e) => return Err(e),
        };
        let status: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let job = if key == "wikidata_pages" {
            "pagetable"
        } else if key == "wikidata_redirects" {
            "redirecttable"
        } else {
            "langlinkstable"
        };
        let job = &status["jobs"][job];
        if job["status"] != "done" {
            continue;
        }
        if let Some(size) = job["files"][&name]["size"].as_u64().filter(|n| *n > 0) {
            let (actual, etag) = http.headers(&url)?;
            if actual != size {
                return Err(format!("{url}: size differs from the completed dump metadata"));
            }
            return Ok((day(&date).unwrap(), name, url, size, etag));
        }
    }
    Err(format!("{base}: no completed {prefix}<day>{suffix}"))
}

/// A private, short-lived token cache avoids spending a refresh on each review and detached run.
fn enterprise(store: &Store) -> Result<Http, String> {
    let _lock = store.lock("wiki-enterprise-auth")?;
    let path = store.root().join("wiki-enterprise-token.json");
    let now = obc_data::date::now();
    if let Ok(value) = read_json::<Value>(&path) {
        if value["expires"].as_u64().is_some_and(|at| at > now + 60) {
            if let Some(token) = value["token"].as_str() {
                return Ok(Http::new().with_bearer(ENTERPRISE, token.into()));
            }
        }
    }
    let get = |name| std::env::var(name).ok().filter(|v| !v.is_empty());
    let (token, lifetime) = if let Some((username, refresh)) =
        get("OBC_WIKIMEDIA_ENTERPRISE_USERNAME").zip(get("OBC_WIKIMEDIA_ENTERPRISE_REFRESH_TOKEN"))
    {
        let body = json!({"username": username, "refresh_token": refresh}).to_string();
        let bytes = Http::new().post_json("https://auth.enterprise.wikimedia.com/v1/token-refresh", body.as_bytes())?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid Enterprise token response")?;
        (
            value["access_token"].as_str().ok_or("Enterprise returned no access token")?.to_string(),
            value["expires_in"].as_u64().unwrap_or(3600).min(86400),
        )
    } else {
        (get("OBC_WIKIMEDIA_ENTERPRISE_TOKEN").ok_or("missing Wikipedia archives need Wikimedia Enterprise credentials: set OBC_WIKIMEDIA_ENTERPRISE_USERNAME and OBC_WIKIMEDIA_ENTERPRISE_REFRESH_TOKEN in tools/obc.local")?, 3600)
    };
    // Credentials must never enter manifests, reviews or run logs.
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path).map_err(|e| e.to_string())?;
    file.write_all(json!({"token": token, "expires": now + lifetime}).to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(Http::new().with_bearer(ENTERPRISE, token))
}

pub(super) fn acquire(context: &Planning, input: &Input) -> Result<Dump, String> {
    let archive = input.archive.as_ref().ok_or("wiki input has no archive")?;
    let expected = input.bytes.ok_or("wiki archive has no reviewed size")?;
    let sha256 = if archive.path.is_file() {
        String::new()
    } else {
        let url = archive.url.as_deref().ok_or_else(|| format!("cached archive {} is gone", archive.path.display()))?;
        eprintln!("Downloading {} ({} bytes); interrupted downloads resume", archive.path.display(), expected);
        let client;
        let http = if url.starts_with(&format!("{ENTERPRISE}/")) {
            client = enterprise(context.store)?;
            &client
        } else {
            context.http
        };
        http.download_to(
            &archive.path,
            url,
            &Expect { etag: archive.etag.as_deref(), bytes: Some(expected), ..Expect::default() },
        )?
        .sha256
    };
    let (size, modified) = stat(&archive.path)?;
    if size != expected {
        return Err(format!("{} changed size since review", archive.path.display()));
    }
    Ok(Dump {
        key: archive.key.clone(),
        source: input.wanted.source.clone(),
        day: input.wanted.version.clone().ok_or("archive has no day")?,
        path: archive.path.clone(),
        size,
        modified,
        sha256,
        area: input.wanted.params.iter().find(|(key, _)| key == "area").map(|(_, v)| v.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_archives_are_reused_and_only_missing_selected_inputs_are_discovered() {
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope {
            region: "europe/dach".into(),
            definitions: [(
                "europe/dach".into(),
                "name = \"DACH\"\nkind = \"geofabrik\"\nareas = [\"europe/dach\"]\n".into(),
            )]
            .into(),
        };
        let languages = vec!["en".into(), "de".into()];
        for (key, _, prefix, suffix, _) in kinds(Some(&scope), &languages).unwrap() {
            if key == "wikipedia:de" {
                continue;
            }
            let date = if key == "osm" { "261001" } else { "20261001" };
            let path = dir.path().join(format!("{prefix}{date}{suffix}"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"cached").unwrap();
        }
        let mut asked = Vec::new();
        let inputs = plan_in(dir.path(), Some(&scope), &languages, |key, prefix, suffix, _| {
            asked.push(key.to_string());
            Ok((
                "2026-10-01".into(),
                format!("{prefix}20261001{suffix}"),
                "https://example.org/de".into(),
                42,
                Some("\"v1\"".into()),
            ))
        })
        .unwrap();
        assert_eq!(asked, ["wikipedia:de"]);
        assert_eq!(inputs.len(), 8);
        assert_eq!(inputs.iter().filter(|i| i.archive.as_ref().unwrap().url.is_some()).count(), 1);
        assert_eq!(inputs.last().unwrap().bytes, Some(42));
        assert!(inputs.iter().all(|i| !i.wanted.params.iter().any(|(_, v)| v.starts_with("frwiki"))));
        let names = links(r#"<a href="20261001/">a</a><a href="20261301/">invalid</a><a href="20261012/">b</a>"#);
        assert_eq!(newest(&names, "", "/").unwrap().0, "2026-10-12");
    }
}
