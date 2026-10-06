//! Saved coverage definitions and read-only suggestions from the verified local area index.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;

use super::api::{confirm, print_json, Code, Error};
use crate::regions::{geofabrik, parse_region, Bbox, Region, Regions};
use crate::store::{hash_file, Store};

mod delete;
pub(super) use delete::{deletion, remove, Deletion};

#[derive(Subcommand)]
pub(super) enum Action {
    /// Every saved region.
    List,
    /// One saved region and its coverage.
    Show { id: String },
    /// Search names and paths in the cached public Geofabrik index. No download.
    Areas {
        #[arg(default_value = "")]
        query: String,
    },
    /// Save one region from selected source areas or a box. This does not commit data/.
    Create(Create),
    /// Review definition deletion. --apply requires the SHA from the reviewed preview.
    Delete {
        id: String,
        #[arg(long, requires = "expected")]
        apply: bool,
        #[arg(long, requires = "apply")]
        expected: Option<String>,
        #[arg(long, requires = "apply")]
        yes: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub(super) struct Create {
    pub(super) id: String,
    #[arg(long)]
    pub(super) name: String,
    /// Repeat for each source path. Selection is independent of the saved region id.
    #[arg(long = "area", required_unless_present = "bbox", conflicts_with = "bbox")]
    pub(super) areas: Vec<String>,
    /// West,south,east,north in degrees.
    #[arg(long = "box", required_unless_present = "areas", allow_hyphen_values = true)]
    pub(super) bbox: Option<String>,
    /// Required for boxes and areas without country metadata. Other areas derive their countries.
    #[arg(long = "country")]
    pub(super) countries: Vec<String>,
    /// An explicit IANA zone. The computer's local zone is never used.
    #[arg(long)]
    pub(super) time_zone: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct Suggestions {
    pub(super) version: String,
    pub(super) areas: Vec<Suggestion>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct Suggestion {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) parent: Option<String>,
    pub(super) countries: Vec<String>,
    pub(super) bounds: Bbox,
}

pub(super) fn run(root: &Path, action: Option<Action>, json: bool) -> Result<(), Error> {
    match action {
        None | Some(Action::List) => {
            super::print_regions(&Regions::load(root).map_err(|e| Code::InvalidData.error(e))?, json)
        }
        Some(Action::Show { id }) => {
            super::print_region(&Regions::load(root).map_err(|e| Code::InvalidData.error(e))?, &id, json)
        }
        Some(Action::Areas { query }) => {
            let suggestions = suggestions(&Store::open()?, &query)?;
            if json {
                print_json(&suggestions)
            } else {
                let mut rows = vec![super::cells(["AREA", "NAME", "COUNTRIES"])];
                rows.extend(
                    suggestions
                        .areas
                        .iter()
                        .map(|area| vec![area.id.clone(), area.name.clone(), area.countries.join(", ")]),
                );
                super::print_table(&rows);
                Ok(())
            }
        }
        Some(Action::Create(args)) => {
            let region = create(root, &Store::open()?, args)?;
            if json {
                print_json(&region)
            } else {
                eprintln!("Saved {}. Review and commit data/ before apply.", region.id);
                Ok(())
            }
        }
        Some(Action::Delete { id, apply, expected, yes }) => {
            let plan = delete::deletion(root, &id)?;
            if apply {
                if expected.as_deref() != Some(&plan.sha256) {
                    return Err(Code::PlanOutdated.error("the region definition changed; review deletion again"));
                }
                if !plan.used_by.is_empty() {
                    return Err(Code::Blocked.error(format!("region `{id}` is used by {}", plan.used_by.join(", "))));
                }
                confirm(&format!("Delete saved region `{id}`? Its data stays in the store."), yes)?;
                delete::remove(root, &plan)?;
            }
            if json {
                print_json(&plan)
            } else {
                eprintln!("{} {} ({})", if apply { "Deleted" } else { "Review deletion:" }, id, plan.sha256);
                for reference in plan.used_by {
                    eprintln!("  used by {reference}");
                }
                Ok(())
            }
        }
    }
}

pub(super) fn suggestions(store: &Store, query: &str) -> Result<Suggestions, Error> {
    let (version, index) = cached_index(store)?;
    let query = query.trim().to_lowercase();
    let areas = index
        .into_values()
        .filter(|area| area.id.contains(&query) || area.name.to_lowercase().contains(&query))
        .map(|area| Suggestion {
            id: area.id,
            name: area.name,
            parent: area.parent,
            countries: area.countries,
            bounds: area.bounds,
        })
        .collect();
    Ok(Suggestions { version, areas })
}

/// Explicit preparation of the small public index; opening or searching a view never fetches it.
pub(super) fn load_areas(root: &Path, store: &Store) -> Result<Suggestions, Error> {
    let registry = super::registry(root)?;
    let source = super::find(&registry, "geofabrik-index")?;
    let request = crate::fetch::Request { source, version: None, params: Vec::new() };
    super::fetched(source, crate::input_copy::fetch(store, &crate::fetch::http::Http::new(), None, &request, &[]))?;
    suggestions(store, "")
}

const INDEX_URL: &str = "https://download.geofabrik.de/index-v1.json";

fn cached_index(store: &Store) -> Result<(String, BTreeMap<String, geofabrik::Area>), Error> {
    let mut records = store.snapshots("geofabrik-index")?;
    records.sort_by(|a, b| b.version.cmp(&a.version));
    let (version, file) = records
        .into_iter()
        .find_map(|snapshot| {
            snapshot.files.into_iter().find(|file| file.url == INDEX_URL).map(|file| (snapshot.version, file))
        })
        .ok_or_else(|| {
            Code::Blocked
                .error("the Geofabrik area index is not in the local store")
                .fix("Prepare it with `obc data fetch geofabrik-index`, then search again.")
        })?;
    if file.sha256.len() != 64
        || !file.sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Code::InvalidData.error("the cached Geofabrik index has an invalid object digest"));
    }
    let path = store.object(&file.sha256);
    let (hash, size) = hash_file(&path)?;
    if hash != file.sha256 || size != file.size {
        return Err(Code::InvalidData.error("the cached Geofabrik index does not match its snapshot"));
    }
    let body = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok((version, geofabrik::parse(&body).map_err(|e| Code::InvalidData.error(e))?))
}

#[derive(Serialize)]
struct Definition<'a> {
    name: &'a str,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    areas: Option<&'a [String]>,
    #[serde(rename = "box", skip_serializing_if = "Option::is_none")]
    bbox: Option<[f64; 4]>,
    countries: Vec<String>,
    time_zone: &'a str,
}

pub(super) fn create(root: &Path, store: &Store, mut args: Create) -> Result<Region, Error> {
    let bbox = args
        .bbox
        .as_ref()
        .map(|text| {
            text.split(',')
                .map(str::parse::<f64>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| Code::Usage.error("--box is west,south,east,north"))?
                .try_into()
                .map_err(|_| Code::Usage.error("--box has four coordinates"))
        })
        .transpose()?;
    if bbox.is_none() {
        let (_, index) = cached_index(store)?;
        let supplied_countries = !args.countries.is_empty();
        for id in &args.areas {
            let area = index
                .get(id)
                .ok_or_else(|| Code::Usage.error(format!("no Geofabrik area `{id}` in the cached index")))?;
            if area.countries.is_empty() && !supplied_countries {
                return Err(Code::Blocked.error(format!("Geofabrik area `{id}` names no countries; supply --country")));
            }
            args.countries.extend(area.countries.clone());
        }
    }
    args.countries.sort();
    args.countries.dedup();
    if args.countries.is_empty() {
        return Err(Code::Usage.error("name the box's countries with --country"));
    }
    args.areas.sort();
    args.areas.dedup();
    let definition = Definition {
        name: &args.name,
        kind: if bbox.is_some() { "box" } else { "geofabrik" },
        areas: bbox.is_none().then_some(&args.areas),
        bbox,
        countries: args.countries,
        time_zone: &args.time_zone,
    };
    let text = toml::to_string(&definition).map_err(|e| e.to_string())?;
    let region = parse_region(&args.id, &text).map_err(|e| Code::Usage.error(e))?;
    time_zone(root, &args.time_zone)?;
    let path = delete::definition_path(root, &args.id)?;
    std::fs::create_dir_all(path.parent().expect("definition parent")).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| Code::Failed.error(format!("{}: {e}", path.display())))?;
    file.write_all(text.as_bytes()).and_then(|()| file.sync_all()).map_err(|e| e.to_string())?;
    Ok(region)
}

fn time_zone(root: &Path, name: &str) -> Result<(), Error> {
    let python = crate::engine::code::python_executable(root)?;
    let result = std::process::Command::new(python)
        .args(["-c", "import sys; from zoneinfo import ZoneInfo; ZoneInfo(sys.argv[1])", name])
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(Code::Usage
            .error(format!(
                "cannot validate IANA time zone `{name}`: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ))
            .fix("Choose a valid IANA zone and use an offline Python runtime with its zoneinfo database."));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
