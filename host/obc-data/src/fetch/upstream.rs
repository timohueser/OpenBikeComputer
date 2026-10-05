//! The newest version upstream has: at most one HEAD or index request per source, cached in the
//! store for an hour.

use serde::{Deserialize, Serialize};

use super::http::Http;
use crate::date;
use crate::sources::{FetchKind, Source, VersionScheme};
use crate::store::{self, Store};

/// How long a check stays valid, in seconds.
pub const CACHE: u64 = 3600;

#[derive(Deserialize, Serialize)]
struct Cached {
    checked: u64,
    version: Option<String>,
}

/// The newest upstream version of `source`, from a check at most `max_age` seconds old. `None`
/// when the source cannot be checked or the check fails.
pub fn newest(store: &Store, http: &Http, source: &Source, max_age: u64) -> Option<String> {
    let path = store.root().join("upstream").join(format!("{}.json", source.id));
    let now = date::now();
    let cached = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Cached>(&text).ok());
    if let Some(cached) = cached.filter(|cached| now.saturating_sub(cached.checked) < max_age) {
        return cached.version;
    }
    let version = check(http, source).map_err(|why| eprintln!("obc data: upstream of `{}`: {why}", source.id)).ok()?;
    let text = serde_json::to_string(&Cached { checked: now, version: version.clone() }).ok()?;
    let _ = store::write_atomic(&path, text.as_bytes());
    version
}

fn check(http: &Http, source: &Source) -> Result<Option<String>, String> {
    let Some(url) = source.fetch.url.as_deref() else { return Ok(None) };
    match source.fetch.kind {
        // `planet-latest.osm.pbf` redirects to `planet-YYMMDD.osm.pbf`, named by the day of its data.
        FetchKind::Osm => {
            let target = http.head(url)?.url;
            let digits = target.rsplit_once("planet-").and_then(|(_, rest)| rest.get(..6));
            let digits = digits.filter(|d| d.bytes().all(|b| b.is_ascii_digit()));
            let day =
                digits.map(|d| format!("20{}-{}-{}", &d[..2], &d[2..4], &d[4..])).filter(|d| date::parse(d).is_some());
            day.map(Some).ok_or_else(|| format!("{target} names no day"))
        }
        // A query service answers with today's data.
        FetchKind::Capture => Ok(Some(date::format(date::today()))),
        FetchKind::Github => github(http, url, source.version),
        FetchKind::Http | FetchKind::Geofabrik | FetchKind::Glo30
            if source.version == VersionScheme::Date && !url.contains('{') =>
        {
            http.head(url)?.modified.map(Some).ok_or_else(|| format!("{url} has no Last-Modified"))
        }
        _ => Ok(None),
    }
}

/// The newest commit of the default branch, or the newest release that has the asset of `url`.
fn github(http: &Http, url: &str, scheme: VersionScheme) -> Result<Option<String>, String> {
    let path = url.split_once("://").map_or("", |(_, rest)| rest);
    let mut parts = path.split('/').skip(1);
    let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else { return Ok(None) };
    let api = format!("https://api.github.com/repos/{owner}/{repo}");
    match scheme {
        VersionScheme::Commit => {
            let sha = http.text(&format!("{api}/commits/HEAD"), "application/vnd.github.sha")?;
            Ok(Some(sha.trim().to_string()))
        }
        VersionScheme::Release => {
            let Some(asset) = url.rsplit('/').next().filter(|_| url.contains("/releases/download/")) else {
                return Ok(None);
            };
            #[derive(Deserialize)]
            struct Release {
                tag_name: String,
                assets: Vec<Asset>,
            }
            #[derive(Deserialize)]
            struct Asset {
                name: String,
            }
            let text = http.text(&format!("{api}/releases?per_page=100"), "application/vnd.github+json")?;
            let releases: Vec<Release> = serde_json::from_str(&text).map_err(|e| format!("{api}/releases: {e}"))?;
            Ok(releases
                .into_iter()
                .find(|r| r.assets.iter().any(|a| a.name == asset.replace("{version}", &r.tag_name)))
                .map(|r| r.tag_name))
        }
        _ => Ok(None),
    }
}
