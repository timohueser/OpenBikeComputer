//! Source-wide moves admit only versions known for every active acquisition request.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::Serialize;

use super::{Code, Error, SourceRow};
use crate::store::Store;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct Request {
    pub params: Vec<(String, String)>,
    pub live: Option<String>,
    pub stored: Vec<String>,
    pub upstream: Option<String>,
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct Versions {
    pub source: String,
    pub requests: Vec<Request>,
    /// Exact versions known for every active request.
    pub common: Vec<String>,
    pub newest: bool,
}

pub(super) fn read(store: &Store, row: &SourceRow) -> Result<Versions, Error> {
    let requests = row
        .requests
        .iter()
        .map(|request| {
            let stored = store.requests(&row.source.id, &request.params)?;
            let upstream = request.observation.result.version().map(str::to_string);
            Ok(Request {
                params: request.params.clone(),
                live: request.live.clone(),
                stored: stored.into_iter().map(|record| record.version).collect(),
                unavailable: upstream.is_none().then(|| {
                    request.reason.clone().unwrap_or_else(|| {
                        "No newest version is known. Check upstream, or prepare the missing request metadata.".into()
                    })
                }),
                upstream,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(assemble(row.source.id.clone(), requests))
}

fn assemble(source: String, requests: Vec<Request>) -> Versions {
    let known = |request: &Request| {
        request.stored.iter().chain(&request.live).chain(&request.upstream).cloned().collect::<BTreeSet<_>>()
    };
    let mut common = requests.first().map(known).unwrap_or_default();
    for request in &requests {
        common.retain(|version| known(request).contains(version));
    }
    let newest = !requests.is_empty() && requests.iter().all(|request| request.upstream.is_some());
    Versions { source, requests, common: common.into_iter().rev().collect(), newest }
}

impl Versions {
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("{}: ALL active requests", self.source),
            format!("Common exact versions: {}", self.common.join(", ")),
        ];
        for request in &self.requests {
            let params =
                request.params.iter().map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join(" ");
            lines.push(format!(
                "{params}: live {}, upstream {}, stored records {}",
                request.live.as_deref().unwrap_or("unknown"),
                request.upstream.as_deref().unwrap_or("unavailable"),
                request.stored.join(", ")
            ));
            if let Some(reason) = &request.unavailable {
                lines.push(reason.clone());
            }
        }
        lines
    }

    pub fn check(&self, version: Option<&str>) -> Result<(), Error> {
        if version.map_or(self.newest, |version| self.common.iter().any(|known| known == version)) {
            return Ok(());
        }
        Err(Code::Blocked
            .error("The move is not known for every active request of this source.")
            .fix("Check upstream and prepare missing request metadata. The request details show unavailable versions."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_wide_versions_preserve_separate_area_availability_and_unknown_requests() {
        let request = |area: &str, stored: &[&str], upstream: Option<&str>| Request {
            params: vec![("area".into(), area.into())],
            live: Some("1".into()),
            stored: stored.iter().map(|v| (*v).into()).collect(),
            upstream: upstream.map(str::to_string),
            unavailable: upstream.is_none().then(|| "Upstream failed for this area".into()),
        };
        let mut requests = vec![request("east", &["1", "2"], Some("3")), request("west", &["1"], None)];
        let versions = assemble("extracts".into(), requests.clone());
        assert_eq!(versions.common, ["1"]);
        assert!(versions.check(Some("2")).is_err());
        assert!(versions.check(None).is_err());
        assert_eq!(versions.requests[1].params[0].1, "west");
        assert!(versions.requests[1].unavailable.as_ref().unwrap().contains("Upstream failed"));
        requests[1].upstream = Some("4".into());
        let versions = assemble("extracts".into(), requests);
        versions.check(None).unwrap();
        assert_eq!(versions.common, ["1"], "newest is resolved per request, not one shared date");
        assert!(assemble("held".into(), Vec::new()).check(None).is_err());
    }
}
