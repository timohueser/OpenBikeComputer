//! The dated Geofabrik extracts, named by the day of their data.

use super::http::Http;
use super::{expand, files, Request};
use crate::date;
use crate::store::{Snapshot, Store};

/// A dated Geofabrik extract. Without a version, the day of the data in the replication state of
/// the area names the newest extract.
pub fn extract(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let template = request.source.fetch.url.as_deref().unwrap_or_default();
    if !template.contains("-{yymmdd}.osm.pbf") {
        return files(store, http, request);
    }
    let day = match &request.version {
        Some(_) => None,
        None => {
            let states = template.replace("-{yymmdd}.osm.pbf", "-updates/state.txt");
            // Geofabrik makes every area each day, so the earliest of the newest days has every extract.
            let mut day = i64::MAX;
            for url in expand(&states, None, &request.params)? {
                day = day.min(state_day(http, &url).map_err(hint)?);
            }
            Some(day)
        }
    };
    let at = |version: String| Request {
        refresh: false,
        source: request.source,
        version: Some(version),
        params: request.params.clone(),
    };
    match day {
        None => files(store, http, &at(request.version.clone().unwrap_or_default())).map_err(hint),
        // The dated file of the newest day can come after its `state.txt`; the day before is there.
        Some(day) => match files(store, http, &at(date::format(day))) {
            Err(error) if super::http::not_found(&error) => {
                files(store, http, &at(date::format(day - 1))).map_err(hint)
            }
            result => result.map_err(hint),
        },
    }
}

fn hint(error: String) -> String {
    match error.ends_with("HTTP 404") {
        true => format!(
            "{error}: Geofabrik keeps the extracts of about the last month, and of the first of each month, \
             or the area does not exist"
        ),
        false => error,
    }
}

/// The day of the data in the Osmosis replication `state.txt` at `url`.
fn state_day(http: &Http, url: &str) -> Result<i64, String> {
    let text = http.text(url, "text/plain")?;
    let timestamp = text.lines().find_map(|line| line.strip_prefix("timestamp="));
    timestamp.and_then(|t| t.get(..10)).and_then(date::parse).ok_or_else(|| format!("{url}: no timestamp"))
}
