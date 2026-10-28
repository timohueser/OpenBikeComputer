//! The source overview of `obc data sources`: how this machine holds the copy of each source that
//! a bake reads, by source, by layer and for a region. It reads only the archive status, the homes
//! of the prepared sources, the records of the store and the cached Geofabrik index.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use super::{bytes, overview, table, Code, Error, Stored, StoredRequest};
use crate::datasets::{ArchiveDataset, DatasetState};
use crate::prepared::{Prepared, Scope, Version};
use crate::regions::{geofabrik, Area, Bbox, Regions};
use crate::settings::Settings;
use crate::sources::{FetchKind, Keep, Kind, Refresh, Registry, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Rule {
    /// `keep = "archive"`.
    Keep,
    /// `keep = "prepared"`.
    Prepare,
    /// `keep = "store"`.
    Store,
    /// Not prepared, with a `refresh` in days: the OSM sources.
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    /// The dataset of the source is complete in the archive.
    Archived,
    /// A prepared version holds it, or R2 keeps our own asset.
    Prepared,
    /// A bake reads it upstream when it is due.
    Live,
    /// The archive or the store holds some of it.
    Partial,
    /// Nothing is held.
    Missing,
}

/// How this machine holds a source.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Held {
    pub rule: Rule,
    /// Where the source has data, in words.
    pub covers: String,
    pub status: SourceStatus,
    /// The archive root of its dataset, the home of the prepared source that holds it, `store`, or
    /// the host of an asset that R2 keeps; `null` when nothing is held.
    #[serde(rename = "where")]
    pub place: Option<String>,
    /// The dataset of `data/archive/` of the source.
    pub dataset: Option<String>,
    /// The prepared source whose versions hold it: itself, or the one that reads it.
    pub prepared_by: Option<String>,
    /// The bytes held; `null` when the source holds none of its own.
    pub bytes: Option<u64>,
    /// What an `archive get` still takes; `null` when unknown or nothing is to get.
    pub download_bytes: Option<u64>,
    /// `YYYY-MM-DD`: the last `archive get`, the newest version that holds it, or the newest file
    /// in the store.
    pub date: Option<String>,
    /// How much a partial source holds, or why a missing one has no copy.
    pub note: Option<String>,
}

/// What the store holds of a source.
#[derive(Default)]
pub(super) struct InStore {
    /// Its versions, the one fetched last first.
    pub snapshots: Vec<Stored>,
    pub requests: Vec<StoredRequest>,
    /// The newest `retrieved` of its files.
    pub retrieved: Option<String>,
}

/// The versions of a prepared source in its home, in the format of its producer.
struct Home {
    folder: PathBuf,
    newest: Version,
    /// The scopes of the newest version: those that it extends, then its own.
    scopes: Vec<Option<Scope>>,
    /// Each source that a version reads, with the newest version that reads it.
    inputs: BTreeMap<String, String>,
}

/// What this machine holds, read once.
pub(super) struct Holdings {
    datasets: Vec<ArchiveDataset>,
    homes: BTreeMap<String, Result<Home, String>>,
    pub stored: BTreeMap<String, InStore>,
    index: Option<BTreeMap<String, geofabrik::Area>>,
}

impl Holdings {
    pub fn read(
        (root, roots): (&Path, &[PathBuf]),
        registry: &Registry,
        producers: &[&dyn Prepared],
        stored: BTreeMap<String, InStore>,
        index: Option<BTreeMap<String, geofabrik::Area>>,
    ) -> Result<Self, Error> {
        let datasets = crate::datasets::load(root, registry).map_err(|e| Code::InvalidData.error(e))?;
        let datasets = crate::datasets::status(&datasets, registry, roots).datasets;
        let mut homes = BTreeMap::new();
        for source in registry.sources.iter().filter(|source| source.fetch.kind == FetchKind::Prepared) {
            let format = crate::prepared::producer(producers, &source.id).map(|producer| producer.format());
            if let Some(home) = home(roots, &source.id, format).transpose() {
                homes.insert(source.id.clone(), home);
            }
        }
        Ok(Holdings { datasets, homes, stored, index })
    }

    pub fn held(&self, source: &Source) -> Held {
        self.by_prepare(source).unwrap_or_else(|| self.kept(source))
    }

    /// A prepared source, a source that a prepared version reads, or an asset that R2 keeps.
    fn by_prepare(&self, source: &Source) -> Option<Held> {
        let mut held = self.empty(source);
        if source.fetch.kind == FetchKind::Prepared {
            match self.homes.get(&source.id) {
                Some(Ok(home)) => {
                    held.status = SourceStatus::Prepared;
                    held.place = Some(home.folder.display().to_string());
                    held.prepared_by = Some(source.id.clone());
                    held.bytes = Some(home.newest.bytes);
                    held.date = Some(crate::prepared::day(&home.newest.version).into());
                }
                Some(Err(error)) => held.note = Some(error.clone()),
                None => held.note = Some(format!("no version: `obc data source prepare {}` makes one", source.id)),
            }
            return Some(held);
        }
        if let Some(&(reader, version, home)) = self.readers(source).last() {
            held.status = SourceStatus::Prepared;
            held.place = Some(home.folder.display().to_string());
            held.prepared_by = Some(reader.into());
            held.date = Some(crate::prepared::day(version).into());
            return Some(held);
        }
        if source.keep == Keep::Prepared && source.kind == Kind::Asset {
            held.status = SourceStatus::Prepared;
            held.place = source.fetch.url.as_deref().and_then(host).map(|host| format!("R2 {host}"));
            return Some(held);
        }
        None
    }

    /// A source that the archive or the store keeps, or that a bake reads live.
    fn kept(&self, source: &Source) -> Held {
        let mut held = self.empty(source);
        let stored = self.stored.get(&source.id).filter(|stored| !stored.snapshots.is_empty());
        let in_store = |held: &mut Held| {
            if let Some(stored) = stored {
                held.place = Some("store".into());
                held.bytes = Some(match stored.requests.is_empty() {
                    true => stored.snapshots[0].bytes,
                    false => stored.requests.iter().map(|request| request.bytes).sum(),
                });
                held.date = stored.retrieved.as_ref().and_then(|time| time.get(..10)).map(str::to_string);
            }
        };
        if held.rule == Rule::Live {
            held.status = SourceStatus::Live;
            in_store(&mut held);
            return held;
        }
        let dataset = self.datasets.iter().find(|dataset| dataset.sources.contains(&source.id));
        if let Some(dataset) = dataset {
            held.dataset = Some(dataset.dataset.clone());
            held.place = dataset.root.clone();
            held.date = dataset.got.clone();
            if dataset.state == DatasetState::Complete {
                held.status = SourceStatus::Archived;
                held.bytes = Some(dataset.bytes);
                return held;
            }
            held.download_bytes = match dataset.listed {
                0 => dataset.bytes_estimate,
                _ => Some(dataset.listed_bytes.saturating_sub(dataset.bytes)),
            };
            if dataset.verified > 0 {
                held.status = SourceStatus::Partial;
                held.bytes = Some(dataset.bytes);
                held.note = Some(format!("{} of {} files in the archive", dataset.verified, dataset.listed));
                return held;
            }
            (held.place, held.date) = (None, None);
        }
        if let Some(stored) = stored {
            held.status = SourceStatus::Partial;
            in_store(&mut held);
            held.note = Some(match stored.requests.len() {
                0 => count(stored.snapshots.len(), "version", "in the store"),
                requests => count(requests, "request", "in the store"),
            });
            return held;
        }
        held.note = if source.id.starts_with("dtm-") {
            Some("gap: GLO-30 only".into())
        } else if source.keep == Keep::Archive && dataset.is_none() {
            Some("no dataset in data/archive/".into())
        } else if let Some(dataset) = dataset.filter(|dataset| dataset.state == DatasetState::Unreadable) {
            dataset.error.clone()
        } else {
            None
        };
        held
    }

    fn empty(&self, source: &Source) -> Held {
        Held {
            rule: rule(source),
            covers: self.covers(source),
            status: SourceStatus::Missing,
            place: None,
            dataset: None,
            prepared_by: None,
            bytes: None,
            download_bytes: None,
            date: None,
            note: None,
        }
    }

    /// The prepared sources whose versions read `source` when only a prepare reads it, with the
    /// newest version that reads it, in the order of their ids.
    fn readers(&self, source: &Source) -> Vec<(&str, &str, &Home)> {
        if !prepare_input(source) {
            return Vec::new();
        }
        let homes = self.homes.iter().filter_map(|(reader, home)| Some((reader.as_str(), home.as_ref().ok()?)));
        homes.filter_map(|(reader, home)| Some((reader, home.inputs.get(&source.id)?.as_str(), home))).collect()
    }

    fn covers(&self, source: &Source) -> String {
        if let Some(Ok(home)) = self.homes.get(&source.id) {
            let regions: Option<Vec<&str>> =
                home.scopes.iter().map(|scope| scope.as_ref().map(|scope| scope.region.as_str())).collect();
            return regions.map_or("World".into(), |regions| regions.join(" + "));
        }
        if let Some(dataset) = self.datasets.iter().find(|dataset| dataset.sources.contains(&source.id)) {
            return dataset.coverage.clone();
        }
        if !source.areas.is_empty() {
            let mut names: Vec<String> = Vec::new();
            for area in &source.areas {
                let codes = self.index.as_ref().and_then(|index| index.get(area)).map(|area| area.countries.clone());
                for name in codes.filter(|codes| !codes.is_empty()).unwrap_or_else(|| vec![area.clone()]) {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            return names.join(", ");
        }
        match source.extent {
            Some([west, south, east, north])
                if !(west <= -180.0 && east >= 180.0 && south <= -85.0 && north >= 85.0) =>
            {
                format!("box {west},{south},{east},{north}")
            }
            _ => "World".into(),
        }
    }

    /// Whether a scope of `home` covers each of `boxes`: a `null` scope, or a box of a scope
    /// that holds it.
    fn covers_boxes(&self, home: &Home, boxes: &[Bbox]) -> bool {
        let mut scopes = Vec::new();
        for scope in &home.scopes {
            let Some(scope) = scope else { return true };
            let settings = Settings {
                region: scope.region.clone(),
                definitions: scope.definitions.clone(),
                ..Settings::default()
            };
            let Ok(regions) = settings.regions() else { continue };
            scopes.extend(overview::area(&regions, &scope.region, &|| Ok(self.index.as_ref())).map(|area| area.boxes));
        }
        let inside =
            |b: &Bbox, s: &Bbox| s.west <= b.west && s.south <= b.south && s.east >= b.east && s.north >= b.north;
        boxes.iter().all(|b| scopes.iter().flatten().any(|s| inside(b, s)))
    }
}

/// The versions of the prepared source `id` in the format `format`, or else of the newest
/// version; `None` while no root has a home with such a version.
fn home(roots: &[PathBuf], id: &str, format: Option<u32>) -> Result<Option<Home>, String> {
    let Some(root) = crate::prepared::home(roots, id) else { return Ok(None) };
    let folder = crate::prepared::folder(root, id);
    let mut versions = crate::prepared::versions(&folder, id)?;
    let Some(format) = format.or(versions.last().map(|version| version.format)) else { return Ok(None) };
    versions.retain(|version| version.format == format);
    let Some(newest) = versions.last().cloned() else { return Ok(None) };
    let (mut inputs, mut scopes) = (BTreeMap::new(), Vec::new());
    for version in &versions {
        let manifest = crate::prepared::manifest(&folder, id, &version.version)?
            .ok_or_else(|| format!("{id}/{}: no manifest", version.version))?;
        for read in manifest.inputs {
            inputs.insert(read.source, version.version.clone());
        }
        scopes = manifest.extends.into_iter().chain([manifest.scope]).collect();
    }
    Ok(Some(Home { folder: crate::prepared::path(root, id), newest, scopes, inputs }))
}

fn rule(source: &Source) -> Rule {
    match source.keep {
        Keep::Archive => Rule::Keep,
        Keep::Prepared => Rule::Prepare,
        Keep::Store if matches!(source.refresh, Refresh::Days(_)) => Rule::Live,
        Keep::Store => Rule::Store,
    }
}

fn host(url: &str) -> Option<&str> {
    url.strip_prefix("https://")?.split('/').next()
}

fn count(n: usize, what: &str, place: &str) -> String {
    format!("{n} {what}{} {place}", if n == 1 { "" } else { "s" })
}

/// A source that only a prepare reads: the prepared source that reads it stands for it in a bake.
fn prepare_input(source: &Source) -> bool {
    source.kind == Kind::Data && source.keep == Keep::Prepared && source.fetch.kind != FetchKind::Prepared
}

/// The JSON word of an enum.
fn word(value: impl Serialize) -> String {
    serde_json::to_value(value).ok().and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default()
}

/// The status of a source, with its note and what a get still takes.
fn status_text(held: &Held) -> String {
    let mut text = word(held.status);
    if let Some(note) = &held.note {
        text += &format!(", {note}");
    }
    if let Some(download) = held.download_bytes.filter(|bytes| *bytes > 0) {
        text += &format!(", {} to get", bytes(download));
    }
    text
}

/// RULE, COVERS, STATUS, WHERE, SIZE and DATE.
pub(super) fn held_cells(held: &Held) -> Vec<String> {
    let place = match (&held.place, &held.dataset, held.status) {
        (Some(place), Some(dataset), SourceStatus::Archived | SourceStatus::Partial) if place != "store" => {
            format!("{place} · {dataset}")
        }
        (Some(place), _, _) => place.clone(),
        (None, _, _) => "—".into(),
    };
    vec![
        word(held.rule),
        held.covers.clone(),
        status_text(held),
        place,
        held.bytes.map_or("—".into(), bytes),
        held.date.clone().unwrap_or_else(|| "—".into()),
    ]
}

/// What `sources --layer` writes.
#[derive(Serialize, JsonSchema)]
pub struct SourceLayers {
    pub layers: Vec<LayerSources>,
}

/// A name of `used_by` and the sources that it reads.
#[derive(Serialize, JsonSchema)]
pub struct LayerSources {
    pub layer: String,
    /// Each source is `archived`, `prepared` or `live`: a bake on this machine reads only OSM
    /// upstream.
    pub ready: bool,
    /// The `data` sources and assets whose `used_by` has the layer, but no source that only a
    /// prepare reads.
    pub sources: Vec<HeldSource>,
}

#[derive(Serialize, JsonSchema)]
pub struct HeldSource {
    pub source: String,
    pub held: Held,
}

pub(super) fn layers(holdings: &Holdings, sources: &[Source], only: Option<&str>) -> Result<SourceLayers, Error> {
    let names: BTreeSet<&str> = sources.iter().flat_map(|source| source.used_by.iter().map(String::as_str)).collect();
    if let Some(only) = only.filter(|only| !names.contains(only)) {
        let known: Vec<&str> = names.into_iter().collect();
        return Err(Code::Usage.error(format!("no layer `{only}`: the layers are {}", known.join(", "))));
    }
    let layers = names.into_iter().filter(|name| only.is_none_or(|only| only == *name)).map(|layer| {
        let reads = sources.iter().filter(|source| {
            source.kind != Kind::Tool && !prepare_input(source) && source.used_by.iter().any(|used| used == layer)
        });
        let sources: Vec<HeldSource> =
            reads.map(|source| HeldSource { source: source.id.clone(), held: holdings.held(source) }).collect();
        let ready = sources
            .iter()
            .all(|s| matches!(s.held.status, SourceStatus::Archived | SourceStatus::Prepared | SourceStatus::Live));
        LayerSources { layer: layer.into(), ready, sources }
    });
    Ok(SourceLayers { layers: layers.collect() })
}

pub(super) fn print_layers(layers: &SourceLayers) {
    let mut rows = vec![super::cells(["  SOURCE", "RULE", "COVERS", "STATUS", "WHERE", "SIZE", "DATE"])];
    for layer in &layers.layers {
        let waiting =
            layer.sources.iter().filter(|s| matches!(s.held.status, SourceStatus::Partial | SourceStatus::Missing));
        let state = match waiting.count() {
            0 => "ready".to_string(),
            n => format!("not ready, {} of {} sources are partial or missing", n, layer.sources.len()),
        };
        rows.push(vec![format!("{}: {state}", layer.layer)]);
        for source in &layer.sources {
            rows.push([vec![format!("  {}", source.source)], held_cells(&source.held)].concat());
        }
    }
    print!("{}", table(&rows));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Todo {
    /// A fetch downloads it, in a bake or in the prepare that reads it.
    Download,
    /// `obc data source prepare` makes a version that covers the region.
    Prepare,
}

/// What `sources --region` and `sources --box` write.
#[derive(Serialize, JsonSchema)]
pub struct RegionSources {
    pub region: String,
    pub name: String,
    /// Its boxes, and the bounds of its Geofabrik areas in the cached index.
    pub boxes: Vec<Bbox>,
    /// The countries that select the national models; empty selects each that meets a box.
    pub countries: Vec<String>,
    pub sources: Vec<RegionSource>,
    /// The sum of the known `bytes` of each `download`.
    pub download_bytes: u64,
    /// The sources to download whose size is unknown.
    pub unknown: Vec<String>,
    /// The sources to prepare.
    pub prepare: Vec<String>,
}

/// A source that a bake of the region needs.
#[derive(Serialize, JsonSchema)]
pub struct RegionSource {
    pub source: String,
    /// Its status for the region.
    pub status: SourceStatus,
    /// What a bake of the region still does; `null` when the region is held.
    pub todo: Option<Todo>,
    /// The bytes of a `download`; `null` when unknown.
    pub bytes: Option<u64>,
    /// How this machine holds the source, as its status for the region reads it.
    pub held: Held,
}

pub(super) fn region(
    holdings: &Holdings,
    sources: &[Source],
    (root, region, bbox): (&Path, Option<&str>, Option<&str>),
) -> Result<RegionSources, Error> {
    let mut settings = Settings::default();
    super::update_cli::select(root, (region, bbox), &mut settings)?;
    let regions = settings.regions().map_err(|e| Code::Usage.error(e))?;
    let id = settings.region.clone();
    let area = overview::area(&regions, &id, &|| Ok(holdings.index.as_ref())).expect("the region was selected");
    if let Some(missing) = area.missing {
        return Err(Code::Usage.error(format!("region `{id}`: {missing}")));
    }
    let countries = countries(&regions, &id)?;
    let leaves = regions.leaves(&id).map_err(|e| Code::Usage.error(e))?;
    let areas: Option<Vec<&String>> = leaves
        .iter()
        .map(|leaf| match &regions.get(leaf).expect("a leaf is a region").area {
            Area::Geofabrik { areas } => Some(areas),
            _ => None,
        })
        .try_fold(Vec::new(), |mut all, areas| {
            all.extend(areas?);
            Some(all)
        });
    let mut out = RegionSources {
        region: id,
        name: area.name,
        boxes: area.boxes,
        countries,
        sources: Vec::new(),
        download_bytes: 0,
        unknown: Vec::new(),
        prepare: Vec::new(),
    };
    for source in sources.iter().filter(|source| needs(holdings, source, &out.boxes, &out.countries)) {
        let (status, todo, bytes, held) = here(holdings, source, (&out.boxes, areas.as_deref()));
        match (todo, bytes) {
            (Some(Todo::Prepare), _) => out.prepare.push(source.id.clone()),
            (Some(Todo::Download), Some(bytes)) => out.download_bytes += bytes,
            (Some(Todo::Download), None) => out.unknown.push(source.id.clone()),
            (None, _) => {}
        }
        out.sources.push(RegionSource { source: source.id.clone(), status, todo, bytes, held });
    }
    Ok(out)
}

/// The countries of region `id`: its own, or else those of each leaf when each names some.
fn countries(regions: &Regions, id: &str) -> Result<Vec<String>, Error> {
    let own = &regions.get(id).expect("the region was selected").countries;
    if !own.is_empty() {
        return Ok(own.clone());
    }
    let mut countries = BTreeSet::new();
    for leaf in regions.leaves(id).map_err(|e| Code::Usage.error(e))? {
        let named = &regions.get(leaf).expect("a leaf is a region").countries;
        if named.is_empty() {
            return Ok(Vec::new());
        }
        countries.extend(named.iter().cloned());
    }
    Ok(countries.into_iter().collect())
}

/// Whether a bake of the region of `boxes` and `countries` needs `source`.
fn needs(holdings: &Holdings, source: &Source, boxes: &[Bbox], countries: &[String]) -> bool {
    let meets = |[w, s, e, n]: [f64; 4]| boxes.iter().any(|b| b.west < e && w < b.east && b.south < n && s < b.north);
    if source.kind == Kind::Tool || source.extent.is_some_and(|extent| !meets(extent)) {
        return false;
    }
    let index = holdings.index.as_ref();
    let bounds: Vec<Bbox> = source.areas.iter().filter_map(|area| Some(index?.get(area)?.bounds)).collect();
    if !bounds.is_empty() && !bounds.iter().any(|b| meets([b.west, b.south, b.east, b.north])) {
        return false;
    }
    let national = source.id.strip_prefix("dtm-").or(source.id.strip_prefix("canopy-").filter(|key| *key != "global"));
    let country = national.and_then(|key| key.split('-').next()).map(str::to_uppercase);
    country.is_none_or(|country| countries.is_empty() || countries.contains(&country))
}

/// The status of `source` for the region of `boxes` and of the Geofabrik `areas` (`None` when a
/// part is a box), what a bake still does, the bytes of a download, and the holding that the
/// status reads.
fn here(
    holdings: &Holdings,
    source: &Source,
    (boxes, areas): (&[Bbox], Option<&[&String]>),
) -> (SourceStatus, Option<Todo>, Option<u64>, Held) {
    let held = holdings.held(source);
    let r2 = held.status == SourceStatus::Prepared && source.kind == Kind::Asset;
    if matches!(held.status, SourceStatus::Archived | SourceStatus::Live) || r2 {
        return (held.status, None, None, held);
    }
    if source.fetch.kind == FetchKind::Prepared {
        let (status, todo) = match holdings.homes.get(&source.id) {
            Some(Ok(home)) if holdings.covers_boxes(home, boxes) => (SourceStatus::Prepared, None),
            Some(Ok(_)) => (SourceStatus::Partial, Some(Todo::Prepare)),
            _ => (SourceStatus::Missing, Some(Todo::Prepare)),
        };
        return (status, todo, None, held);
    }
    if holdings.readers(source).iter().any(|(_, _, home)| holdings.covers_boxes(home, boxes)) {
        return (SourceStatus::Prepared, None, None, held);
    }
    let held = holdings.kept(source);
    let params = crate::fetch::params(source);
    let whole =
        params.is_empty() && !matches!(source.fetch.kind, FetchKind::Capture | FetchKind::Glo30 | FetchKind::Dtm);
    let stored = holdings.stored.get(&source.id).filter(|stored| !stored.snapshots.is_empty());
    let has_area = |area: &&String| {
        let requests = stored.iter().flat_map(|stored| &stored.requests);
        requests.flat_map(|request| &request.params).any(|(name, value)| name == "area" && value == *area)
    };
    let each_area = params == ["area"] && areas.is_some_and(|areas| areas.iter().all(has_area));
    if stored.is_some() && (whole || each_area) {
        return (held.status, None, None, held);
    }
    let bytes = held.download_bytes.filter(|_| whole);
    (held.status, Some(Todo::Download), bytes, held)
}

pub(super) fn print_region(region: &RegionSources) {
    let countries = match region.countries.is_empty() {
        true => "no countries".to_string(),
        false => format!("countries {}", region.countries.join(", ")),
    };
    let boxes: Vec<String> =
        region.boxes.iter().map(|b| format!("{},{},{},{}", b.west, b.south, b.east, b.north)).collect();
    println!("{} ({}): box {}, {countries}", region.region, region.name, boxes.join(" + "));
    let mut rows = vec![super::cells(["SOURCE", "STATUS", "WHERE", "TODO"])];
    for source in &region.sources {
        let place = held_cells(&source.held).swap_remove(3);
        let todo = match (source.todo, source.bytes) {
            (None, _) => "—".to_string(),
            (Some(Todo::Prepare), _) => "prepare".into(),
            (Some(Todo::Download), Some(size)) => format!("download, {}", bytes(size)),
            (Some(Todo::Download), None) => "download, unknown".into(),
        };
        rows.push(vec![source.source.clone(), word(source.status), place, todo]);
    }
    print!("{}", table(&rows));
    if region.download_bytes == 0 && region.unknown.is_empty() && region.prepare.is_empty() {
        println!("A bake of the region downloads only what is live, and prepares nothing.");
        return;
    }
    if region.download_bytes > 0 {
        println!("A bake of the region downloads {} of known size.", bytes(region.download_bytes));
    }
    if !region.unknown.is_empty() {
        println!("Unknown size: {}.", region.unknown.join(", "));
    }
    if !region.prepare.is_empty() {
        println!("To prepare: {}.", region.prepare.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str) -> &'static Source {
        crate::sources::all().iter().find(|source| source.id == id).unwrap()
    }

    fn dataset(id: &str, state: DatasetState, (listed, verified): (usize, usize)) -> ArchiveDataset {
        ArchiveDataset {
            dataset: id.into(),
            sources: vec![id.into()],
            coverage: "World".into(),
            root: Some("/archive".into()),
            state,
            error: None,
            version: None,
            got: Some("2026-10-28".into()),
            listed,
            listed_bytes: 300,
            verified,
            bytes: 100 * verified as u64,
            bytes_estimate: Some(500),
        }
    }

    /// The archive holds `land-polygons` and one of three files of `egdi-geology`; `terrain` has a
    /// version of a box in Switzerland that read `dtm-ch` and GLO-30; the store has one Swiss OSM
    /// extract of `terrain-osm`.
    fn holdings() -> Holdings {
        let bbox = Bbox::new([8.0, 46.0, 9.0, 47.0]).unwrap();
        let region = crate::regions::Region {
            id: "box".into(),
            name: "Box".into(),
            area: Area::Box { bbox },
            countries: Vec::new(),
            time_zone: None,
        };
        let mut scope = Settings::default();
        scope.select(&Regions::new(vec![region]).unwrap(), "box").unwrap();
        let newest = Version {
            version: "2026-10-28".into(),
            manifest_sha256: "aa".into(),
            format: 3,
            bytes: 7,
            time: "2026-10-28T00:00:00Z".into(),
            region: Some("box".into()),
        };
        let inputs = ["dtm-ch", "copernicus-glo-30"].map(|id| (id.to_string(), newest.version.clone()));
        let home = Home {
            folder: "/archive/prepared/terrain".into(),
            newest,
            scopes: vec![Some(Scope { region: scope.region, definitions: scope.definitions })],
            inputs: inputs.into(),
        };
        let request = StoredRequest {
            params: vec![("area".into(), "europe/switzerland".into())],
            version: "2026-10-01".into(),
            bytes: 40,
            fetched: Some("2026-10-01T08:00:00Z".into()),
        };
        let osm = InStore {
            snapshots: vec![Stored { version: "2026-10-01".into(), bytes: 40 }],
            requests: vec![request],
            retrieved: Some("2026-10-01T08:00:00Z".into()),
        };
        let qrank = InStore {
            snapshots: vec![Stored { version: "a".repeat(64), bytes: 9 }],
            requests: Vec::new(),
            retrieved: Some("2026-10-01T08:00:00Z".into()),
        };
        let mut glo30 = dataset("copernicus-glo-30", DatasetState::Absent, (0, 0));
        glo30.root = None;
        Holdings {
            datasets: vec![
                dataset("land-polygons", DatasetState::Complete, (1, 1)),
                dataset("egdi-geology", DatasetState::Incomplete, (3, 1)),
                glo30,
            ],
            homes: [("terrain".to_string(), Ok(home))].into(),
            stored: [("terrain-osm".to_string(), osm), ("qrank".to_string(), qrank)].into(),
            index: None,
        }
    }

    #[test]
    fn each_source_has_one_status_from_the_archive_the_prepared_homes_and_the_store() {
        let holdings = holdings();
        let held = |id| holdings.held(source(id));
        let status = |id| {
            let held = held(id);
            (held.status, held.place, held.note, held.download_bytes)
        };
        assert_eq!(status("land-polygons"), (SourceStatus::Archived, Some("/archive".into()), None, None));
        let partial = Some("1 of 3 files in the archive".into());
        assert_eq!(status("egdi-geology"), (SourceStatus::Partial, Some("/archive".into()), partial, Some(200)));
        assert_eq!(
            status("copernicus-glo-30"),
            (SourceStatus::Missing, None, None, Some(500)),
            "a prepared version that reads a source of a bake is no copy of it"
        );
        let terrain = held("terrain");
        assert_eq!((terrain.status, terrain.covers.as_str(), terrain.bytes), (SourceStatus::Prepared, "box", Some(7)));
        let dtm = held("dtm-ch");
        assert_eq!((dtm.status, dtm.prepared_by.as_deref()), (SourceStatus::Prepared, Some("terrain")));
        assert_eq!(dtm.covers, "europe/switzerland, europe/liechtenstein", "the area paths without an index");
        assert_eq!(status("dtm-fr"), (SourceStatus::Missing, None, Some("gap: GLO-30 only".into()), None));
        let stored = Some("1 request in the store".into());
        assert_eq!(status("terrain-osm"), (SourceStatus::Partial, Some("store".into()), stored, None));
        assert_eq!(held("osm-planet").status, SourceStatus::Live);
        assert_eq!(status("nasa-svs-moon").2.as_deref(), Some("no dataset in data/archive/"));
        assert_eq!(status("qrank").2.as_deref(), Some("1 version in the store"));
        assert_eq!(held("query-model").status, SourceStatus::Prepared, "R2 keeps our own asset");
    }

    #[test]
    fn a_region_needs_the_sources_that_meet_it_and_says_what_a_bake_still_does() {
        let holdings = holdings();
        let inside = [Bbox::new([8.2, 46.2, 8.5, 46.5]).unwrap()];
        let outside = [Bbox::new([6.5, 46.2, 7.0, 46.5]).unwrap()];
        let swiss = ["CH".to_string()];
        assert!(needs(&holdings, source("dtm-ch"), &inside, &swiss));
        assert!(!needs(&holdings, source("dtm-fr"), &outside, &swiss), "a national model of another country");
        assert!(
            needs(&holdings, source("dtm-fr"), &outside, &[]),
            "a region without countries reads each that meets it"
        );
        assert!(!needs(&holdings, source("land-cover-ie"), &inside, &swiss), "an extent that does not meet it");
        assert!(!needs(&holdings, source("planetiler"), &inside, &swiss), "a tool");

        let area = "europe/switzerland".to_string();
        let todo = |id, boxes: &[Bbox], areas: Option<&[&String]>| {
            let (status, todo, bytes, _) = here(&holdings, source(id), (boxes, areas));
            (status, todo, bytes)
        };
        assert_eq!(todo("terrain", &inside, None), (SourceStatus::Prepared, None, None));
        assert_eq!(todo("dtm-ch", &inside, None), (SourceStatus::Prepared, None, None));
        assert_eq!(todo("terrain", &outside, None), (SourceStatus::Partial, Some(Todo::Prepare), None));
        assert_eq!(todo("dtm-ch", &outside, None), (SourceStatus::Missing, Some(Todo::Download), None));
        assert_eq!(todo("terrain-osm", &outside, Some(&[&area])), (SourceStatus::Partial, None, None));
        assert_eq!(todo("terrain-osm", &outside, None), (SourceStatus::Partial, Some(Todo::Download), None));
        assert_eq!(todo("egdi-geology", &outside, None), (SourceStatus::Partial, Some(Todo::Download), Some(200)));
        assert_eq!(
            todo("copernicus-glo-30", &outside, None),
            (SourceStatus::Missing, Some(Todo::Download), None),
            "tiles"
        );
        assert_eq!(todo("land-polygons", &outside, None), (SourceStatus::Archived, None, None));
        assert_eq!(todo("qrank", &outside, None), (SourceStatus::Partial, None, None), "the store has it whole");
        assert_eq!(todo("nasa-svs-moon", &outside, None), (SourceStatus::Missing, Some(Todo::Download), None));
    }
}
