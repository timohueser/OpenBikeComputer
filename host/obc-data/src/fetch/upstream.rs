//! Request-specific upstream observations. Check age and data age are separate.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::http::Http;
use crate::date;
use crate::sources::{FetchKind, Source, VersionScheme};
use crate::store::{self, Store};

/// How long a check stays valid, in seconds.
pub const CACHE: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum Upstream {
    Newest(String),
    /// The service captures current data on demand; no network probe establishes a version.
    Capture,
    /// The source has no cheap upstream probe.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Success {
    pub checked_at: u64,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Observation {
    /// None for policy-derived capture results and sources without a probe.
    pub checked_at: Option<u64>,
    pub result: Upstream,
    pub last_success: Option<Success>,
}

#[derive(Deserialize, Serialize)]
struct Cached {
    fetch: crate::sources::Fetch,
    version: VersionScheme,
    checks: Vec<Check>,
    observation: Observation,
}

/// The one request that finds the newest version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Check {
    /// The URL with `latest` for `{yymmdd}` redirects to the file of the newest day, such as
    /// `planet-latest.osm.pbf` to `planet-YYMMDD.osm.pbf`.
    LatestRedirect(String),
    /// The `state.txt` of a replication directory names the day of its newest diff.
    ReplicationState(String),
    LastModified(String),
    /// The newest commit of the default branch, from the API URL of the repository.
    GithubCommit(String),
    /// The newest release that has the asset, whose name may hold `{version}`.
    GithubRelease(String, String),
}

/// The newest upstream version of `source`, from a check at most `max_age` seconds old.
pub fn newest(store: &Store, http: &Http, source: &Source, max_age: u64) -> Upstream {
    observe(store, http, source, &[], max_age, date::now()).result
}

#[cfg(test)]
pub(crate) fn seed(store: &Store, source: &Source, params: &[(String, String)], version: &str) {
    let mut checks = plan(source, params).unwrap();
    checks.sort_by_cached_key(|check| serde_json::to_string(check).unwrap());
    checks.dedup();
    let checked_at = date::now();
    let observation = Observation {
        checked_at: Some(checked_at),
        result: Upstream::Newest(version.into()),
        last_success: Some(Success { checked_at, version: version.into() }),
    };
    store::write_atomic(
        &path(store, &source.id, params),
        &serde_json::to_vec(&Cached { fetch: source.fetch.clone(), version: source.version, checks, observation })
            .unwrap(),
    )
    .unwrap();
}

pub fn path(store: &Store, source: &str, params: &[(String, String)]) -> std::path::PathBuf {
    let params = serde_json::to_vec(&store::sorted(params)).expect("strings serialize");
    store.root().join("upstream").join(source).join(format!("{}.json", store::sha256_hex(&params)))
}

/// Observe one normalized acquisition request. Failures retain the previous successful probe.
pub fn observe(
    store: &Store,
    http: &Http,
    source: &Source,
    params: &[(String, String)],
    max_age: u64,
    now: u64,
) -> Observation {
    let immediate = |result| Observation { checked_at: None, result, last_success: None };
    let mut checks = match plan(source, params) {
        Ok(checks) => checks,
        Err(answer) => return immediate(answer),
    };
    checks.sort_by_cached_key(|check| serde_json::to_string(check).expect("probe serializes"));
    checks.dedup();
    let path = path(store, &source.id, params);
    let lock = store.lock(&format!("upstream-{}", store::sha256_hex(path.to_string_lossy().as_bytes())));
    let _lock = match lock {
        Ok(lock) => lock,
        Err(error) => return immediate(Upstream::Failed(error)),
    };
    let cached = std::fs::read(&path).ok().and_then(|text| serde_json::from_slice::<Cached>(&text).ok());
    let cached = cached
        .filter(|cached| cached.fetch == source.fetch && cached.version == source.version && cached.checks == checks);
    if let Some(cached) = cached
        .as_ref()
        .filter(|cached| cached.observation.checked_at.is_some_and(|checked| now.saturating_sub(checked) < max_age))
    {
        return cached.observation.clone();
    }
    let answer = checks.iter().map(|check| run(http, check)).collect::<Result<Vec<_>, _>>();
    let result = answer.map_or_else(Upstream::Failed, |versions| {
        versions.into_iter().max().map_or(Upstream::CannotCheck, Upstream::Newest)
    });
    let last_success = match &result {
        Upstream::Newest(version) => Some(Success { checked_at: now, version: version.clone() }),
        _ => cached.and_then(|cached| cached.observation.last_success),
    };
    let observation = Observation { checked_at: Some(now), result, last_success };
    if let Ok(text) = serde_json::to_vec(&Cached {
        fetch: source.fetch.clone(),
        version: source.version,
        checks,
        observation: observation.clone(),
    }) {
        let _ = store::write_atomic(&path, &text);
    }
    observation
}

/// The request that checks `source`, or the answer when no request is needed.
fn plan(source: &Source, params: &[(String, String)]) -> Result<Vec<Check>, Upstream> {
    let Some(url) = source.fetch.url.as_deref() else { return Err(Upstream::CannotCheck) };
    let check = match source.fetch.kind {
        FetchKind::Osm => Ok(Check::ReplicationState(format!("{url}state.txt"))),
        FetchKind::Http | FetchKind::Geofabrik if url.contains("{yymmdd}") => {
            return super::expand(&url.replace("{yymmdd}", "latest"), None, params)
                .map(|urls| urls.into_iter().map(Check::LatestRedirect).collect())
                .map_err(Upstream::Failed);
        }
        // A query service answers with today's data.
        FetchKind::Capture => Err(Upstream::Capture),
        FetchKind::Github => {
            let path = url.split_once("://").map_or("", |(_, rest)| rest);
            let mut parts = path.split('/').skip(1);
            let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else { return Err(Upstream::CannotCheck) };
            let api = format!("https://api.github.com/repos/{owner}/{repo}");
            match source.version {
                VersionScheme::Commit => Ok(Check::GithubCommit(api)),
                VersionScheme::Release if url.contains("/releases/download/") => {
                    Ok(Check::GithubRelease(api, url.rsplit('/').next().unwrap_or_default().into()))
                }
                _ => Err(Upstream::CannotCheck),
            }
        }
        FetchKind::Http | FetchKind::Geofabrik | FetchKind::Glo30
            if source.version == VersionScheme::Date && !super::names_version(url) =>
        {
            return super::expand(url, None, params)
                .map(|urls| urls.into_iter().map(Check::LastModified).collect())
                .map_err(Upstream::Failed);
        }
        _ => Err(Upstream::CannotCheck),
    }?;
    Ok(vec![check])
}

fn run(http: &Http, check: &Check) -> Result<String, String> {
    match check {
        Check::LatestRedirect(url) => {
            let target = http.location(url)?;
            let name = url.rsplit('/').next().unwrap_or_default();
            let (prefix, suffix) = name.rsplit_once("latest").unwrap_or_default();
            let got = target.rsplit('/').next().unwrap_or_default();
            let digits = got.strip_prefix(prefix).and_then(|rest| rest.strip_suffix(suffix));
            let digits = digits.filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_digit()));
            let day = digits.map(|d| format!("20{}-{}-{}", &d[..2], &d[2..4], &d[4..]));
            day.filter(|day| date::parse(day).is_some()).ok_or_else(|| format!("{target} names no day"))
        }
        Check::ReplicationState(url) => Ok(date::format(super::osm::state(http, url)?.1)),
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
