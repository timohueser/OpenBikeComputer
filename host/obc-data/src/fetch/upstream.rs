//! The newest version upstream has: one HEAD or index request per source that can be checked. The
//! store keeps each answer, and each failure, for an hour.

use serde::{Deserialize, Serialize};

use super::http::Http;
use crate::date;
use crate::sources::{FetchKind, Source, VersionScheme};
use crate::store::{self, Store};

/// How long a check stays valid, in seconds.
pub const CACHE: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upstream {
    Newest(String),
    /// No request can tell, for example for a URL with `{area}`.
    CannotCheck,
    Failed(String),
}

impl Upstream {
    pub fn version(&self) -> Option<&str> {
        match self {
            Upstream::Newest(version) => Some(version),
            _ => None,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Cached {
    checked: u64,
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// The one request that finds the newest version.
enum Check<'a> {
    /// `planet-latest.osm.pbf` redirects to `planet-YYMMDD.osm.pbf`, named by the day of its data.
    PlanetRedirect(&'a str),
    LastModified(&'a str),
    /// The newest commit of the default branch, from the API URL of the repository.
    GithubCommit(String),
    /// The newest release that has the asset, whose name may hold `{version}`.
    GithubRelease(String, &'a str),
}

/// The newest upstream version of `source`, from a check at most `max_age` seconds old.
pub fn newest(store: &Store, http: &Http, source: &Source, max_age: u64) -> Upstream {
    let check = match plan(source) {
        Ok(check) => check,
        Err(answer) => return answer,
    };
    let path = store.root().join("upstream").join(format!("{}.json", source.id));
    let now = date::now();
    let cached = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Cached>(&text).ok());
    let answer = match cached.filter(|cached| now.saturating_sub(cached.checked) < max_age) {
        Some(cached) => cached.version.ok_or(cached.error.unwrap_or_default()),
        None => {
            let answer = run(http, &check);
            let (version, error) = (answer.clone().ok(), answer.clone().err());
            if let Ok(text) = serde_json::to_string(&Cached { checked: now, version, error }) {
                let _ = store::write_atomic(&path, text.as_bytes());
            }
            answer
        }
    };
    answer.map_or_else(Upstream::Failed, Upstream::Newest)
}

/// The request that checks `source`, or the answer when no request is needed.
fn plan(source: &Source) -> Result<Check<'_>, Upstream> {
    let Some(url) = source.fetch.url.as_deref() else { return Err(Upstream::CannotCheck) };
    match source.fetch.kind {
        FetchKind::Osm => Ok(Check::PlanetRedirect(url)),
        // A query service answers with today's data.
        FetchKind::Capture => Err(Upstream::Newest(date::format(date::today()))),
        FetchKind::Github => {
            let path = url.split_once("://").map_or("", |(_, rest)| rest);
            let mut parts = path.split('/').skip(1);
            let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else { return Err(Upstream::CannotCheck) };
            let api = format!("https://api.github.com/repos/{owner}/{repo}");
            match source.version {
                VersionScheme::Commit => Ok(Check::GithubCommit(api)),
                VersionScheme::Release if url.contains("/releases/download/") => {
                    Ok(Check::GithubRelease(api, url.rsplit('/').next().unwrap_or_default()))
                }
                _ => Err(Upstream::CannotCheck),
            }
        }
        FetchKind::Http | FetchKind::Geofabrik | FetchKind::Glo30
            if source.version == VersionScheme::Date && !url.contains('{') =>
        {
            Ok(Check::LastModified(url))
        }
        _ => Err(Upstream::CannotCheck),
    }
}

fn run(http: &Http, check: &Check) -> Result<String, String> {
    match check {
        Check::PlanetRedirect(url) => {
            let target = http.location(url)?;
            let digits = target.rsplit_once("planet-").and_then(|(_, rest)| rest.get(..6));
            let digits = digits.filter(|d| d.bytes().all(|b| b.is_ascii_digit()));
            let day = digits.map(|d| format!("20{}-{}-{}", &d[..2], &d[2..4], &d[4..]));
            day.filter(|day| date::parse(day).is_some()).ok_or_else(|| format!("{target} names no day"))
        }
        Check::LastModified(url) => http.modified(url)?.ok_or_else(|| format!("{url} has no Last-Modified")),
        Check::GithubCommit(api) => {
            let sha = http.text(&format!("{api}/commits/HEAD"), "application/vnd.github.sha")?;
            Ok(sha.trim().to_string())
        }
        Check::GithubRelease(api, asset) => {
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
            releases
                .into_iter()
                .find(|r| r.assets.iter().any(|a| a.name == asset.replace("{version}", &r.tag_name)))
                .map(|r| r.tag_name)
                .ok_or_else(|| format!("no release of {api} has {asset}"))
        }
    }
}
