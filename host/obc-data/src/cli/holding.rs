//! The source overview of `obc data sources`: how this machine holds the copy of each source that
//! a bake reads, by source, by layer and for a region. It reads only the archive manifests, the
//! homes of the prepared sources, the records of the store and the cached Geofabrik index, and it
//! fetches nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{bytes, listing, overview, table, Code, Error, Stored, StoredRequest};
use crate::datasets::{ArchiveDataset, DatasetState};
use crate::engine::Input;
use crate::prepared::{Prepared, Read, Scope, Version};
use crate::product::{Inputs, Product, RequestKey, Wanted};
use crate::regions::{geofabrik, Bbox, Regions};
use crate::settings::Settings;
use crate::sources::{FetchKind, Keep, Kind, Refresh, Registry, Source};
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Rule {
    /// `keep = "archive"`.
    Keep,
    /// `keep = "prepared"`.
    Prepare,
    /// `keep = "store"`.
    Store,
    /// `keep = "store"` with a `refresh` in days: the OSM sources.
    Live,
}

/// How this machine holds a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceStatus {
    /// The dataset of the source is complete in the archive.
    Archived,
    /// A prepared version holds it, or R2 keeps our own asset.
    Prepared,
    /// A bake reads it upstream when it is due.
    Live,
    /// The store has a version of a source that a fetch takes whole.
    Held,
    /// The archive holds some files of its dataset, or the store some requests of it.
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
    /// The verified and the listed files of its dataset; `null` while no manifest lists a file.
    pub archive_files: Option<Files>,
    /// The versions of the source in the store.
    pub store_versions: usize,
    /// The requests of the source with params in the store, the newest of each set of params.
    pub store_requests: usize,
    /// A national terrain model that nothing holds: the terrain there is GLO-30 only.
    pub gap: bool,
    /// `keep = "archive"`, and no dataset of `data/archive/` has the source.
    pub no_dataset: bool,
    /// Why its dataset or its prepared versions cannot be read.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Files {
    pub verified: usize,
    pub listed: usize,
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
    /// Each source that the newest version or a version that it extends reads, with the newest
    /// such version.
    inputs: BTreeMap<String, String>,
}

/// A file that an archive manifest lists.
struct ArchiveFile {
    bytes: Option<u64>,
    /// In `files`, not in `missing`.
    verified: bool,
}

/// What this machine holds, read once.
pub(super) struct Holdings {
    datasets: Vec<ArchiveDataset>,
    /// The files of the manifests, by their path under the root.
    archive: HashMap<String, ArchiveFile>,
    homes: BTreeMap<String, Result<Home, String>>,
    /// The prepared source whose producer reads each source that only a prepare reads.
    readers: BTreeMap<String, String>,
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
        let mut archive = HashMap::new();
        for dataset in &datasets {
            let Some(root) = &dataset.root else { continue };
            let path = Path::new(root).join("manifest").join(format!("{}.json", dataset.dataset));
            let Ok(manifest) = crate::fetch::archive::Manifest::read(&path) else { continue };
            for file in manifest.files {
                archive.insert(file.path, ArchiveFile { bytes: Some(file.bytes), verified: true });
            }
            for file in manifest.missing {
                archive.insert(file.path, ArchiveFile { bytes: file.bytes, verified: false });
            }
        }
        let (mut homes, mut readers) = (BTreeMap::new(), BTreeMap::new());
        for source in registry.sources.iter().filter(|source| source.fetch.kind == FetchKind::Prepared) {
            let producer = crate::prepared::producer(producers, &source.id);
            if let Some(home) = home(roots, &source.id, producer.map(|producer| producer.format())).transpose() {
                homes.insert(source.id.clone(), home);
            }
            for input in registry.sources.iter().filter(|input| producer.is_some_and(|p| p.reads(input))) {
                readers.insert(input.id.clone(), source.id.clone());
            }
        }
        Ok(Holdings { datasets, archive, homes, readers, stored, index })
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
                Some(Err(error)) => held.error = Some(error.clone()),
                None => {}
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
        if let Some(stored) = stored {
            (held.store_versions, held.store_requests) = (stored.snapshots.len(), stored.requests.len());
        }
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
            held.archive_files =
                (dataset.listed > 0).then_some(Files { verified: dataset.verified, listed: dataset.listed });
            held.error = dataset.error.clone();
            if dataset.state == DatasetState::Complete {
                (held.status, held.place, held.date) =
                    (SourceStatus::Archived, dataset.root.clone(), dataset.got.clone());
                held.bytes = Some(dataset.bytes);
                return held;
            }
            held.download_bytes = match dataset.listed {
                0 => dataset.bytes_estimate,
                _ => Some(dataset.listed_bytes.saturating_sub(dataset.bytes)),
            };
            if dataset.verified > 0 {
                (held.status, held.place, held.date) =
                    (SourceStatus::Partial, dataset.root.clone(), dataset.got.clone());
                held.bytes = Some(dataset.bytes);
                return held;
            }
        }
        if stored.is_some() {
            held.status = if whole(source) { SourceStatus::Held } else { SourceStatus::Partial };
            in_store(&mut held);
            return held;
        }
        held.gap = source.id.starts_with("dtm-");
        held.no_dataset = source.keep == Keep::Archive && dataset.is_none();
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
            archive_files: None,
            store_versions: 0,
            store_requests: 0,
            gap: false,
            no_dataset: false,
            error: None,
        }
    }

    /// The prepared sources whose newest version or a version that it extends reads `source`
    /// when only a prepare reads it, with the newest such version, in the order of their ids.
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

    /// Whether the scopes of `home` cover the region of `boxes` and `countries`: a `null` scope, or
    /// for each box a scope box that holds it, of a scope that names no countries or each of
    /// `countries`. `None` when a scope has a Geofabrik area that the cached index lacks.
    fn covers_region(&self, home: &Home, boxes: &[Bbox], countries: &[String]) -> Option<bool> {
        let (mut covering, mut unknown) = (Vec::new(), false);
        for scope in &home.scopes {
            let Some(scope) = scope else { return Some(true) };
            let settings = Settings {
                region: scope.region.clone(),
                definitions: scope.definitions.clone(),
                ..Settings::default()
            };
            let Ok(regions) = settings.regions() else { continue };
            let Some(area) = overview::area(&regions, &scope.region, &|| Ok(self.index.as_ref())) else { continue };
            unknown |= area.missing.is_some();
            let named = region_countries(&regions, &scope.region).unwrap_or_default();
            let inside = countries.iter().all(|country| named.contains(country));
            if named.is_empty() || !countries.is_empty() && inside {
                covering.extend(area.boxes);
            }
        }
        let held =
            |b: &Bbox, s: &Bbox| s.west <= b.west && s.south <= b.south && s.east >= b.east && s.north >= b.north;
        let all = boxes.iter().all(|b| covering.iter().any(|s| held(b, s)));
        if all {
            Some(true)
        } else {
            (!unknown).then_some(false)
        }
    }
}

/// The parts of a version manifest that the overview reads; serde skips the others, such as
/// `files`.
#[derive(Deserialize)]
struct Chain {
    inputs: Vec<Read>,
    scope: Option<Scope>,
    #[serde(default)]
    extends: Vec<Option<Scope>>,
}

/// The newest version of the prepared source `id` in the format `format`, or else in the format
/// of its newest version, with the versions that it extends; `None` while no root has a home with
/// such a version.
fn home(roots: &[PathBuf], id: &str, format: Option<u32>) -> Result<Option<Home>, String> {
    let Some(root) = crate::prepared::home(roots, id) else { return Ok(None) };
    let folder = crate::prepared::folder(root, id);
    let mut versions = crate::prepared::versions(&folder, id)?;
    let Some(format) = format.or(versions.last().map(|version| version.format)) else { return Ok(None) };
    versions.retain(|version| version.format == format);
    let Some(newest) = versions.last().cloned() else { return Ok(None) };
    let chain = |version: &str| -> Result<Chain, String> {
        let path = format!("{version}/{}", crate::prepared::MANIFEST);
        let bytes = folder.get(&path)?.ok_or_else(|| format!("{id}/{path}: no manifest"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{id}/{path}: {e}"))
    };
    let last = chain(&newest.version)?;
    let scopes: Vec<Option<Scope>> = last.extends.iter().cloned().chain([last.scope.clone()]).collect();
    // Only the versions whose region a scope of `extends` names; one that cannot be read gives no
    // inputs, and the newest version still serves.
    let extended = |version: &&Version| {
        last.extends.iter().any(|scope| scope.as_ref().map(|scope| scope.region.as_str()) == version.region.as_deref())
    };
    let mut inputs = BTreeMap::new();
    for version in versions[..versions.len() - 1].iter().filter(extended) {
        let Ok(earlier) = chain(&version.version) else { continue };
        if last.extends.contains(&earlier.scope) {
            inputs.extend(earlier.inputs.into_iter().map(|read| (read.source, version.version.clone())));
        }
    }
    inputs.extend(last.inputs.into_iter().map(|read| (read.source, newest.version.clone())));
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

/// Whether a fetch takes `source` whole: it has no params, and is no capture, GLO-30, national
/// model, installed tool or prepared source.
fn whole(source: &Source) -> bool {
    use FetchKind::{Capture, Dtm, Glo30, Installed, Prepared};
    crate::fetch::params(source).is_empty()
        && !matches!(source.fetch.kind, Capture | Glo30 | Dtm | Installed | Prepared)
}

/// A source that only a prepare reads: the prepared source that reads it stands for it in a bake.
fn prepare_input(source: &Source) -> bool {
    source.kind == Kind::Data && source.keep == Keep::Prepared && source.fetch.kind != FetchKind::Prepared
}

/// The JSON word of an enum.
fn word(value: impl Serialize) -> String {
    serde_json::to_value(value).ok().and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default()
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// The status of a source, with what it holds or lacks and what a get still takes.
fn status_text(held: &Held) -> String {
    let mut text = word(held.status);
    match (held.status, held.archive_files) {
        (SourceStatus::Partial, Some(files)) if files.verified > 0 => {
            text += &format!(", {} of {} files in the archive", files.verified, files.listed)
        }
        (SourceStatus::Partial, _) if held.store_requests > 0 => {
            text += &format!(", {} in the store", count(held.store_requests, "request"))
        }
        (SourceStatus::Partial, _) => text += &format!(", {} in the store", count(held.store_versions, "version")),
        _ => {}
    }
    if held.gap {
        text += ", gap: GLO-30 only";
    }
    if held.no_dataset {
        text += ", no dataset in data/archive/";
    }
    if let Some(error) = &held.error {
        text += &format!(", {error}");
    }
    if let Some(download) = held.download_bytes.filter(|bytes| *bytes > 0) {
        text += &format!(", {} to get", bytes(download));
    }
    text
}

fn place_text(held: &Held) -> String {
    match (&held.place, &held.dataset) {
        (Some(place), Some(dataset)) if place != "store" => format!("{place} · {dataset}"),
        (Some(place), _) => place.clone(),
        (None, _) => "—".into(),
    }
}

/// RULE, COVERS, STATUS, WHERE, SIZE and DATE.
pub(super) fn held_cells(held: &Held) -> Vec<String> {
    vec![
        word(held.rule),
        held.covers.clone(),
        status_text(held),
        place_text(held),
        held.bytes.map_or("—".into(), bytes),
        held.date.clone().unwrap_or_else(|| "—".into()),
    ]
}

/// What `sources --layer` writes.
#[derive(Serialize, JsonSchema)]
pub struct SourceLayers {
    pub layers: Vec<LayerSources>,
}

/// A name of `used_by` and the sources that it reads. Whether a bake of a region still downloads
/// or prepares is in `sources --region`.
#[derive(Serialize, JsonSchema)]
pub struct LayerSources {
    pub layer: String,
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
        let sources = reads.map(|source| HeldSource { source: source.id.clone(), held: holdings.held(source) });
        LayerSources { layer: layer.into(), sources: sources.collect() }
    });
    Ok(SourceLayers { layers: layers.collect() })
}

pub(super) fn print_layers(layers: &SourceLayers) {
    let mut rows = vec![super::cells(["  SOURCE", "RULE", "COVERS", "STATUS", "WHERE", "SIZE", "DATE"])];
    for layer in &layers.layers {
        rows.push(vec![layer.layer.clone()]);
        for source in &layer.sources {
            rows.push([vec![format!("  {}", source.source)], held_cells(&source.held)].concat());
        }
    }
    print!("{}", table(&rows));
}

/// What a bake of a region still needs of a source or a layer, from the least to the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Need {
    /// The store, the archive or a prepared version holds what the bake reads.
    Held,
    /// A fetch downloads a part, in the bake or in the prepare that reads it.
    Download,
    /// `obc data source prepare` makes a version that covers the region.
    Prepare,
    /// Unknown: a step list needs a snapshot that the store lacks, or the cached index lacks an
    /// area of a scope.
    Unknown,
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
    /// The products with each optional part on, and whether their step lists are complete.
    pub products: Vec<RegionProduct>,
    /// The layers of the products.
    pub layers: Vec<RegionLayer>,
    /// The sources that the step lists read, and the sources that only a prepare reads of the
    /// prepared sources among them.
    pub sources: Vec<RegionSource>,
    /// The sum of the known bytes to download.
    pub download_bytes: u64,
    /// The sources with a download of unknown size, or whose need is unknown.
    pub unknown: Vec<String>,
    /// The prepared sources to prepare.
    pub prepare: Vec<String>,
}

/// A product of the region.
#[derive(Serialize, JsonSchema)]
pub struct RegionProduct {
    pub product: String,
    /// Its step list stopped: a snapshot that it needs is not in the store, or it does not suit
    /// the region. Its other sources come from the table of what it can read.
    pub incomplete: bool,
    /// Why it stopped.
    pub reason: Option<String>,
}

/// A layer and what a bake of the region still needs for it: the most of its sources.
#[derive(Serialize, JsonSchema)]
pub struct RegionLayer {
    pub layer: String,
    pub need: Need,
    /// The sources that its steps read.
    pub sources: Vec<String>,
    /// Why its step list cannot be listed: then `need` is `unknown`.
    pub reason: Option<String>,
}

/// A source that a bake of the region reads.
#[derive(Serialize, JsonSchema)]
pub struct RegionSource {
    pub source: String,
    /// How this machine holds the source.
    pub held: Held,
    pub for_region: ForRegion,
}

/// What a bake of the region reads of a source, and what it still needs.
#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub struct ForRegion {
    pub need: Need,
    /// The fetches of the source that the step lists read: one per set of params.
    pub requests: usize,
    /// Of them, those that the store or the archive holds.
    pub held_requests: usize,
    /// The known bytes of the held ones.
    pub held_bytes: u64,
    /// The known bytes to download.
    pub download_bytes: u64,
    /// The fetches to download of unknown size.
    pub unknown_requests: usize,
    /// For a source that only a prepare reads: the prepared source whose need it has.
    pub read_by: Option<String>,
    /// The step list of a product that reads it stopped before it read the source: its fetches
    /// are not known, and `need` is `unknown` but for a prepared source.
    pub after_stop: bool,
}

impl ForRegion {
    fn of(need: Need) -> Self {
        ForRegion {
            need,
            requests: 0,
            held_requests: 0,
            held_bytes: 0,
            download_bytes: 0,
            unknown_requests: 0,
            read_by: None,
            after_stop: false,
        }
    }
}

pub(super) fn region(
    (holdings, store, roots): (&Holdings, &Store, &[PathBuf]),
    (products, producers): (&[&dyn Product], &[&dyn Prepared]),
    sources: &[Source],
    (root, region, bbox): (&Path, Option<&str>, Option<&str>),
) -> Result<RegionSources, Error> {
    if products.is_empty() {
        return Err(Code::Usage
            .error("this binary links no product, so it has no step list of a region")
            .fix("Run `obc data sources --region` with `obc data`, not with `obc-data-plumbing`."));
    }
    let mut settings = Settings::default();
    super::update_cli::select(root, (region, bbox), &mut settings)?;
    let mut layers: Vec<String> = products.iter().flat_map(|p| p.optional().iter().map(|l| l.to_string())).collect();
    layers.sort();
    layers.dedup();
    settings.layers = layers;
    let regions = settings.regions().map_err(|e| Code::Usage.error(e))?;
    let id = settings.region.clone();
    let area = overview::area(&regions, &id, &|| Ok(holdings.index.as_ref())).expect("the region was selected");
    if let Some(missing) = area.missing {
        return Err(Code::Usage
            .error(format!("region `{id}`: {missing}"))
            .fix("Run `obc data fetch geofabrik-index`."));
    }
    let mut countries = region_countries(&regions, &id)?;
    if countries.is_empty() {
        countries = box_countries(holdings.index.as_ref(), &area.boxes);
    }

    let mut inputs = Inputs { settings, ..Inputs::default() };
    let prepared: BTreeSet<RequestKey> = sources
        .iter()
        .filter(|source| source.fetch.kind == FetchKind::Prepared)
        .map(|source| (source.id.clone(), Vec::new()))
        .collect();
    crate::prepared::resolve(roots, (sources, &prepared, producers), &mut inputs).map_err(|e| Code::Failed.error(e))?;
    let refuse = |wanted: &Wanted| -> Result<String, Error> {
        let params: String = wanted.params.iter().map(|(name, value)| format!(" {name}={value}")).collect();
        Err(Code::Blocked
            .error(format!("the store lacks `{}`{params}", wanted.source))
            .fix(format!("`obc data fetch {}{params}` gets it.", wanted.source)))
    };
    let listed = listing::steps(root, products, &mut inputs, &regions, store, true, refuse)?;
    // The fetches that the steps read, and those that a step list asked for before it stopped. A
    // fetch without params of a source that a fetch takes with params only picks its version.
    let mut requests: BTreeMap<String, BTreeSet<Vec<(String, String)>>> = BTreeMap::new();
    let read = listed.steps.iter().flat_map(|step| &step.inputs).filter_map(|input| match input {
        Input::Snapshot { source, params, .. } => Some((source.clone(), crate::store::sorted(params))),
        Input::Step { .. } => None,
    });
    for (source, params) in inputs.requests.borrow().iter().cloned().chain(read) {
        let named = sources.iter().find(|s| s.id == source).is_some_and(|s| !crate::fetch::params(s).is_empty());
        if !(named && params.is_empty()) {
            requests.entry(source).or_default().insert(params);
        }
    }
    for params in requests.values_mut().filter(|params| params.len() > 1) {
        params.remove(&Vec::new());
    }
    // A product whose step list stopped names the sources that its later steps can read.
    let stopped: Vec<(&dyn Product, &str)> = products
        .iter()
        .filter_map(|product| {
            let blocked = listed.blocked.iter().find(|blocked| blocked.layer == product.name())?;
            Some((*product, blocked.reason.as_str()))
        })
        .collect();
    let later: BTreeSet<String> = stopped.iter().flat_map(|(product, _)| product.reads(root)).collect();

    let mut out = RegionSources {
        region: id,
        name: area.name,
        boxes: area.boxes,
        countries,
        products: products
            .iter()
            .map(|product| {
                let reason = stopped.iter().find(|(p, _)| p.name() == product.name()).map(|(_, r)| r.to_string());
                RegionProduct { product: product.name().into(), incomplete: reason.is_some(), reason }
            })
            .collect(),
        layers: Vec::new(),
        sources: Vec::new(),
        download_bytes: 0,
        unknown: Vec::new(),
        prepare: Vec::new(),
    };
    let mut needs: BTreeMap<String, Need> = BTreeMap::new();
    for source in sources.iter().filter(|source| source.kind != Kind::Tool || requests.contains_key(&source.id)) {
        let read = requests.get(&source.id);
        let after_stop = read.is_none() && later.contains(&source.id);
        if read.is_none() && !(after_stop && meets(holdings, source, &out.boxes, &out.countries)) {
            continue;
        }
        let mut for_region = match (source.fetch.kind, read) {
            (FetchKind::Prepared, _) => {
                let need = match holdings.homes.get(&source.id) {
                    Some(Ok(home)) => match holdings.covers_region(home, &out.boxes, &out.countries) {
                        Some(true) => Need::Held,
                        Some(false) => Need::Prepare,
                        None => Need::Unknown,
                    },
                    _ => Need::Prepare,
                };
                ForRegion { requests: 1, held_requests: usize::from(need == Need::Held), ..ForRegion::of(need) }
            }
            (_, Some(read)) => fetches(holdings, store, &inputs, source, read.iter())?,
            (_, None) => ForRegion::of(Need::Unknown),
        };
        for_region.after_stop = after_stop;
        needs.insert(source.id.clone(), for_region.need);
        out.sources.push(RegionSource { source: source.id.clone(), held: holdings.held(source), for_region });
    }
    // A source that only a prepare reads has the need of the prepared source that reads it.
    for source in sources.iter().filter(|source| prepare_input(source)) {
        let Some(reader) = holdings.readers.get(&source.id) else { continue };
        let Some(&need) = needs.get(reader) else { continue };
        if !meets(holdings, source, &out.boxes, &out.countries) {
            continue;
        }
        let for_region = ForRegion { read_by: Some(reader.clone()), ..ForRegion::of(need) };
        out.sources.push(RegionSource { source: source.id.clone(), held: holdings.held(source), for_region });
    }

    let mut by_layer: BTreeMap<String, RegionLayer> = BTreeMap::new();
    for step in &listed.steps {
        let layer = by_layer.entry(step.layer.name.to_string()).or_insert_with(|| RegionLayer {
            layer: step.layer.name.to_string(),
            need: Need::Held,
            sources: Vec::new(),
            reason: None,
        });
        for input in &step.inputs {
            let Input::Snapshot { source, .. } = input else { continue };
            if !layer.sources.contains(source) {
                layer.sources.push(source.clone());
            }
            layer.need = layer.need.max(needs.get(source).copied().unwrap_or(Need::Unknown));
        }
    }
    for blocked in &listed.blocked {
        let layer = by_layer.entry(blocked.layer.clone()).or_insert_with(|| RegionLayer {
            layer: blocked.layer.clone(),
            need: Need::Unknown,
            sources: Vec::new(),
            reason: None,
        });
        (layer.need, layer.reason) = (Need::Unknown, Some(blocked.reason.clone()));
    }
    out.layers = by_layer.into_values().collect();
    for source in &out.sources {
        let need = &source.for_region;
        out.download_bytes += need.download_bytes;
        if need.unknown_requests > 0 || need.need == Need::Unknown {
            out.unknown.push(source.source.clone());
        }
        if need.need == Need::Prepare && need.read_by.is_none() {
            out.prepare.push(source.source.clone());
        }
    }
    Ok(out)
}

/// The countries of the cached index whose area holds a corner or the centre of one of `boxes`:
/// the areas of one country each, by their polygons.
fn box_countries(index: Option<&BTreeMap<String, geofabrik::Area>>, boxes: &[Bbox]) -> Vec<String> {
    let points: Vec<[f64; 2]> = boxes
        .iter()
        .flat_map(|b| {
            let centre = [(b.west + b.east) / 2.0, (b.south + b.north) / 2.0];
            [[b.west, b.south], [b.east, b.south], [b.west, b.north], [b.east, b.north], centre]
        })
        .collect();
    let mut countries = BTreeSet::new();
    for area in index.into_iter().flat_map(BTreeMap::values) {
        let [country] = &area.countries[..] else { continue };
        if points.iter().any(|point| area.polygons.iter().any(|polygon| inside(polygon, *point))) {
            countries.insert(country.clone());
        }
    }
    countries.into_iter().collect()
}

/// Whether `point` is in the exterior ring of `polygon` and in none of its holes, by the even-odd
/// rule.
fn inside(polygon: &[Vec<[f64; 2]>], [x, y]: [f64; 2]) -> bool {
    let in_ring = |ring: &Vec<[f64; 2]>| {
        let edges = ring.iter().zip(ring.iter().cycle().skip(1));
        edges.filter(|([x0, y0], [x1, y1])| (y0 > &y) != (y1 > &y) && x < x0 + (y - y0) / (y1 - y0) * (x1 - x0)).count()
            % 2
            == 1
    };
    polygon.first().is_some_and(in_ring) && !polygon[1..].iter().any(in_ring)
}

/// What the store and the archive hold of the fetches of `source` with each of `params`.
fn fetches<'a>(
    holdings: &Holdings,
    store: &Store,
    inputs: &Inputs,
    source: &Source,
    params: impl Iterator<Item = &'a Vec<(String, String)>>,
) -> Result<ForRegion, Error> {
    let mut need = ForRegion::of(Need::Held);
    for params in params {
        need.requests += 1;
        let version = crate::product::version(inputs, store, &source.id, params)?.ok();
        let stored = match &version {
            Some(version) => stored_bytes(store, &source.id, version, params)?,
            None => None,
        };
        // A fetch reads the archive file of each URL before it downloads.
        let urls =
            source.fetch.url.as_deref().and_then(|url| crate::fetch::expand(url, version.as_deref(), params).ok());
        let listed: Option<Vec<&ArchiveFile>> = urls
            .filter(|_| matches!(source.fetch.kind, FetchKind::Http | FetchKind::Glo30 | FetchKind::Github))
            .and_then(|urls| {
                urls.iter().map(|url| holdings.archive.get(&crate::fetch::archive::mirror_path(url)?)).collect()
            });
        match (stored, listed) {
            (Some(bytes), _) => {
                (need.held_requests, need.held_bytes) = (need.held_requests + 1, need.held_bytes + bytes)
            }
            (None, Some(files)) if files.iter().all(|file| file.verified) => {
                need.held_requests += 1;
                need.held_bytes += files.iter().filter_map(|file| file.bytes).sum::<u64>();
            }
            (None, Some(files)) if files.iter().all(|file| file.bytes.is_some()) => {
                need.download_bytes +=
                    files.iter().filter(|file| !file.verified).filter_map(|file| file.bytes).sum::<u64>();
            }
            (None, _) => need.unknown_requests += 1,
        }
    }
    if need.held_requests < need.requests {
        need.need = Need::Download;
    }
    Ok(need)
}

/// The bytes of the files of the fetch of `source` with `params` at `version` in the store;
/// `None` while the store lacks them.
fn stored_bytes(store: &Store, source: &str, version: &str, params: &[(String, String)]) -> Result<Option<u64>, Error> {
    let Some(files) = crate::engine::snapshot_files(store, source, version, params, &[])? else { return Ok(None) };
    let snapshot = store.snapshot(source, version)?;
    let records = snapshot.iter().flat_map(|snapshot| &snapshot.files);
    Ok(Some(records.filter(|file| files.contains_key(&file.name)).map(|file| file.size).sum()))
}

/// The countries of region `id`: its own, or else those of each leaf when each names some.
fn region_countries(regions: &Regions, id: &str) -> Result<Vec<String>, Error> {
    let own = &regions.get(id).ok_or_else(|| Code::Usage.error(format!("no region `{id}`")))?.countries;
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

/// Whether a source that only a prepare reads has data for the region of `boxes` and
/// `countries`: by its `extent`, the bounds of its `areas` in the cached index, and for a
/// national model (`dtm-<cc>…`, `canopy-<cc>`) its country.
fn meets(holdings: &Holdings, source: &Source, boxes: &[Bbox], countries: &[String]) -> bool {
    let meets = |[w, s, e, n]: [f64; 4]| boxes.iter().any(|b| b.west < e && w < b.east && b.south < n && s < b.north);
    if source.extent.is_some_and(|extent| !meets(extent)) {
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

fn need_text(need: &ForRegion) -> String {
    let mut text = word(need.need);
    if let Some(reader) = &need.read_by {
        return text + &format!(" (read by {reader})");
    }
    if need.after_stop {
        return text + " (after a missing input)";
    }
    if need.need == Need::Download {
        text += &format!(": {} of {} fetches held", need.held_requests, need.requests);
    } else if need.requests > 1 {
        text += &format!(": {} fetches", need.requests);
    }
    if need.held_bytes > 0 {
        let held = if need.need == Need::Held { "" } else { " held" };
        text += &format!(", {}{held}", bytes(need.held_bytes));
    }
    if need.download_bytes > 0 {
        text += &format!(", {} to download", bytes(need.download_bytes));
    }
    if need.unknown_requests > 0 {
        text += &format!(", {} of unknown size", need.unknown_requests);
    }
    text
}

pub(super) fn print_region(region: &RegionSources) {
    let countries = match region.countries.is_empty() {
        true => "no countries".to_string(),
        false => format!("countries {}", region.countries.join(", ")),
    };
    let boxes: Vec<String> =
        region.boxes.iter().map(|b| format!("{},{},{},{}", b.west, b.south, b.east, b.north)).collect();
    println!("{} ({}): box {}, {countries}", region.region, region.name, boxes.join(" + "));
    for product in region.products.iter().filter(|product| product.incomplete) {
        let reason = product.reason.as_deref().unwrap_or_default();
        println!("{}: incomplete: the list stops at {reason}", product.product);
    }
    let mut rows = vec![super::cells(["LAYER", "NEEDS"])];
    for layer in &region.layers {
        let reason = layer.reason.as_ref().map(|reason| format!(": {reason}")).unwrap_or_default();
        rows.push(vec![layer.layer.clone(), word(layer.need) + &reason]);
    }
    rows.push(vec![String::new()]);
    rows.push(super::cells(["SOURCE", "FOR THE REGION", "STATUS ON THIS MACHINE", "WHERE"]));
    for source in &region.sources {
        let held = &source.held;
        rows.push(vec![source.source.clone(), need_text(&source.for_region), status_text(held), place_text(held)]);
    }
    print!("{}", table(&rows));
    if region.download_bytes > 0 {
        println!("A bake of the region downloads {} of known size.", bytes(region.download_bytes));
    }
    if !region.unknown.is_empty() {
        println!("Unknown size or need: {}.", region.unknown.join(", "));
    }
    if !region.prepare.is_empty() {
        println!("To prepare: {}.", region.prepare.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, step};
    use crate::engine::Run;
    use crate::product::{Pointer, Steps, Unplanned};
    use crate::regions::{Area, Region};
    use crate::store::tests::Scratch;

    fn source(id: &str) -> &'static Source {
        crate::sources::all().iter().find(|source| source.id == id).unwrap()
    }

    fn bbox(edges: [f64; 4]) -> Bbox {
        Bbox::new(edges).unwrap()
    }

    fn region(id: &str, area: Area, countries: &[&str]) -> Region {
        let countries = countries.iter().map(|code| code.to_string()).collect();
        Region { id: id.into(), name: id.into(), area, countries, time_zone: None }
    }

    /// The scope of the region `id` of `regions`.
    fn scope(regions: Vec<Region>, id: &str) -> Option<Scope> {
        let mut settings = Settings::default();
        settings.select(&Regions::new(regions).unwrap(), id).unwrap();
        Some(Scope { region: settings.region, definitions: settings.definitions })
    }

    fn box_scope(edges: [f64; 4], countries: &[&str]) -> Option<Scope> {
        scope(vec![region("box", Area::Box { bbox: bbox(edges) }, countries)], "box")
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

    fn version(name: &str, format: u32) -> Version {
        Version {
            version: name.into(),
            manifest_sha256: "aa".into(),
            format,
            bytes: 7,
            time: "2026-10-28T00:00:00Z".into(),
            region: None,
        }
    }

    const HANSEN: &str = "mirror/storage.googleapis.com/earthenginepartners-hansen/GFC-2023-v1.11/\
                          Hansen_GFC-2023-v1.11_treecover2000_50N_000E.tif";

    /// The archive holds `land-polygons`, one of three files of `egdi-geology` and lists a Hansen
    /// tile that it lacks; `terrain` has a version of a box in Switzerland that read `dtm-ch` and
    /// GLO-30; the store has one Swiss extract of `terrain-osm` and the file of `qrank`.
    fn holdings() -> Holdings {
        let inputs = ["dtm-ch", "copernicus-glo-30"].map(|id| (id.to_string(), "2026-10-28".to_string()));
        let home = Home {
            folder: "/archive/prepared/terrain".into(),
            newest: version("2026-10-28", 3),
            scopes: vec![box_scope([8.0, 46.0, 9.0, 47.0], &[])],
            inputs: inputs.into(),
        };
        let request = StoredRequest {
            params: vec![("area".into(), "europe/switzerland".into())],
            version: "2026-10-01".into(),
            bytes: 40,
            fetched: Some("2026-10-01T08:00:00Z".into()),
        };
        let stored = |version: &str, requests| InStore {
            snapshots: vec![Stored { version: version.into(), bytes: 40 }],
            requests,
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
            archive: [
                (HANSEN.to_string(), ArchiveFile { bytes: Some(100), verified: false }),
                (HANSEN.replace("50N", "40N"), ArchiveFile { bytes: Some(30), verified: true }),
            ]
            .into(),
            homes: [("terrain".to_string(), Ok(home))].into(),
            readers: [("dtm-ch".to_string(), "terrain".to_string())].into(),
            stored: [
                ("terrain-osm".to_string(), stored("2026-10-01", vec![request])),
                ("qrank".to_string(), stored(&"a".repeat(64), Vec::new())),
            ]
            .into(),
            index: None,
        }
    }

    #[test]
    fn each_source_has_one_status_from_the_archive_the_prepared_homes_and_the_store() {
        let holdings = holdings();
        let held = |id| holdings.held(source(id));
        let status = |id| {
            let held = held(id);
            (held.status, held.place, held.download_bytes)
        };
        assert_eq!(status("land-polygons"), (SourceStatus::Archived, Some("/archive".into()), None));
        assert_eq!(status("egdi-geology"), (SourceStatus::Partial, Some("/archive".into()), Some(200)));
        assert_eq!(held("egdi-geology").archive_files, Some(Files { verified: 1, listed: 3 }));
        assert_eq!(
            status("copernicus-glo-30"),
            (SourceStatus::Missing, None, Some(500)),
            "a prepared version that reads a source of a bake is no copy of it"
        );
        let terrain = held("terrain");
        assert_eq!((terrain.status, terrain.covers.as_str(), terrain.bytes), (SourceStatus::Prepared, "box", Some(7)));
        let dtm = held("dtm-ch");
        assert_eq!((dtm.status, dtm.prepared_by.as_deref()), (SourceStatus::Prepared, Some("terrain")));
        assert_eq!(dtm.covers, "europe/switzerland, europe/liechtenstein", "the area paths without an index");
        let dtm = held("dtm-fr");
        assert_eq!((dtm.status, dtm.gap), (SourceStatus::Missing, true));
        let osm = held("terrain-osm");
        assert_eq!((osm.status, osm.store_requests, osm.place.as_deref()), (SourceStatus::Partial, 1, Some("store")));
        assert_eq!(held("qrank").status, SourceStatus::Held, "the store has it whole");
        assert_eq!(held("osm-planet").status, SourceStatus::Live);
        assert!(held("nasa-svs-moon").no_dataset);
        assert_eq!(held("query-model").status, SourceStatus::Prepared, "R2 keeps our own asset");
    }

    #[test]
    fn a_home_reads_the_newest_version_of_the_format_and_the_versions_that_it_extends() {
        let dir = Scratch::new("holding-home");
        let folder = dir.0.join("prepared/terrain");
        let (a, b, c) = (box_scope([0.0, 0.0, 1.0, 1.0], &[]), box_scope([1.0, 0.0, 2.0, 1.0], &[]), None);
        let versions = [
            ("v0", 3, None, &a, vec![]),
            ("v1", 2, None, &a, vec![]),
            ("v2", 3, None, &a, vec![]),
            ("v3", 3, None, &c, vec![]),
            ("v4", 3, Some("dtm-d"), &b, vec![a.clone()]),
        ];
        let mut listed = Vec::new();
        for (name, format, extra, scope, extends) in versions {
            std::fs::create_dir_all(folder.join(name)).unwrap();
            let inputs: Vec<_> = [format!("dtm-{name}")].into_iter().chain(extra.map(str::to_string)).collect();
            let inputs: Vec<_> =
                inputs.iter().map(|id| serde_json::json!({"source": id, "version": "1", "digest": "x"})).collect();
            let manifest =
                serde_json::json!({"inputs": inputs, "scope": scope, "extends": extends, "files": [{"path": "a"}]});
            std::fs::write(folder.join(name).join("manifest.json"), manifest.to_string()).unwrap();
            listed.push(Version { region: scope.as_ref().map(|scope| scope.region.clone()), ..version(name, format) });
        }
        // An earlier version whose manifest cannot be read gives no inputs, and the newest serves.
        std::fs::write(folder.join("v0/manifest.json"), "{").unwrap();
        std::fs::write(folder.join("versions.json"), serde_json::to_vec(&listed).unwrap()).unwrap();
        let found = home(std::slice::from_ref(&dir.0), "terrain", Some(3)).unwrap().unwrap();
        assert_eq!(found.newest.version, "v4");
        assert_eq!(found.scopes, [a, b]);
        let inputs: Vec<(&str, &str)> = found.inputs.iter().map(|(id, v)| (id.as_str(), v.as_str())).collect();
        assert_eq!(
            inputs,
            [("dtm-d", "v4"), ("dtm-v2", "v2"), ("dtm-v4", "v4")],
            "not v0 (unreadable), v1 (format 2) nor v3 (no chain)"
        );
        assert!(home(std::slice::from_ref(&dir.0), "terrain", Some(4)).unwrap().is_none(), "no version of the format");
    }

    #[test]
    fn a_scope_covers_a_region_by_its_boxes_its_countries_and_the_index() {
        let holdings = holdings();
        let home = |scopes| Home { folder: "/h".into(), newest: version("v", 1), scopes, inputs: BTreeMap::new() };
        let inside = [bbox([8.2, 46.2, 8.5, 46.5])];
        let ch = ["CH".to_string()];
        let covers = |scopes, countries: &[String]| holdings.covers_region(&home(scopes), &inside, countries);
        assert_eq!(covers(vec![None], &ch), Some(true), "a null scope");
        assert_eq!(covers(vec![box_scope([8.0, 46.0, 9.0, 47.0], &["CH"])], &ch), Some(true));
        assert_eq!(covers(vec![box_scope([8.0, 46.0, 9.0, 47.0], &["DE"])], &ch), Some(false), "another country");
        assert_eq!(
            covers(vec![box_scope([8.0, 46.0, 9.0, 47.0], &["CH"])], &[]),
            Some(false),
            "a region of no country"
        );
        let union = vec![
            region("west", Area::Box { bbox: bbox([7.0, 46.0, 8.0, 47.0]) }, &[]),
            region("east", Area::Box { bbox: bbox([8.0, 46.0, 9.0, 47.0]) }, &[]),
            region("both", Area::Union { union: vec!["west".into(), "east".into()] }, &[]),
        ];
        assert_eq!(covers(vec![scope(union, "both")], &ch), Some(true), "a box of a member of a union");
        let geofabrik = vec![region("ch", Area::Geofabrik { areas: vec!["europe/switzerland".into()] }, &[])];
        assert_eq!(covers(vec![scope(geofabrik, "ch")], &ch), None, "an area without an index");
    }

    #[test]
    fn a_layer_reads_its_sources_but_not_those_that_only_a_prepare_reads() {
        let holdings = holdings();
        let layers = layers(&holdings, crate::sources::all(), Some("search")).unwrap();
        let ids: Vec<&str> = layers.layers[0].sources.iter().map(|source| source.source.as_str()).collect();
        assert!(ids.contains(&"wiki") && !ids.contains(&"wikidata"), "{ids:?}");
        let all = super::layers(&holdings, crate::sources::all(), None).unwrap();
        assert_eq!(
            all.layers.iter().map(|layer| layer.layer.as_str()).collect::<Vec<_>>(),
            ["device", "planner", "search", "trailview"]
        );
        assert!(
            !all.layers.iter().flat_map(|layer| &layer.sources).any(|source| source.source == "planetiler"),
            "a tool"
        );
        assert_eq!(super::layers(&holdings, crate::sources::all(), Some("nope")).err().unwrap().code, Code::Usage);
    }

    #[test]
    fn the_countries_of_a_union_are_those_of_its_members_when_each_names_some() {
        let member = |id: &str, countries| region(id, Area::Box { bbox: bbox([7.0, 46.0, 8.0, 47.0]) }, countries);
        let union = |members: &[&str]| {
            region("both", Area::Union { union: members.iter().map(|m| m.to_string()).collect() }, &[])
        };
        let regions =
            Regions::new(vec![member("a", &["DE"]), member("b", &["CH", "DE"]), member("c", &[]), union(&["a", "b"])])
                .unwrap();
        assert_eq!(region_countries(&regions, "both").unwrap(), ["CH", "DE"]);
        let regions = Regions::new(vec![member("a", &["DE"]), member("c", &[]), union(&["a", "c"])]).unwrap();
        assert!(region_countries(&regions, "both").unwrap().is_empty(), "a member without countries");
    }

    #[test]
    fn a_source_that_only_a_prepare_reads_meets_a_region_by_extent_areas_and_country() {
        let mut holdings = holdings();
        let inside = [bbox([8.2, 46.2, 8.5, 46.5])];
        let swiss = ["CH".to_string()];
        assert!(meets(&holdings, source("dtm-ch"), &inside, &swiss));
        assert!(!meets(&holdings, source("dtm-fr"), &[bbox([6.5, 46.2, 7.0, 46.5])], &swiss), "another country");
        assert!(meets(&holdings, source("dtm-fr"), &[bbox([6.5, 46.2, 7.0, 46.5])], &[]), "a region of no country");
        assert!(!meets(&holdings, source("dtm-it"), &[bbox([20.0, 47.5, 20.5, 48.0])], &[]), "outside its extent");
        // The extent of `dtm-it-21` meets the box; the bounds of its area do not.
        let area = geofabrik::Area {
            id: "europe/italy/nord-ovest".into(),
            name: "Nord-Ovest".into(),
            parent: None,
            countries: vec!["IT".into()],
            polygons: Vec::new(),
            bounds: bbox([7.5, 44.0, 9.3, 46.0]),
        };
        let edge = [bbox([6.7, 46.2, 7.0, 46.4])];
        assert!(meets(&holdings, source("dtm-it-21"), &edge, &[]));
        holdings.index = Some([(area.id.clone(), area)].into());
        assert!(!meets(&holdings, source("dtm-it-21"), &edge, &[]), "the bounds of its area");
    }

    /// Reads `natural-earth` whole, one Hansen tile and `terrain`, in one step.
    struct Reads;

    impl Product for Reads {
        fn name(&self) -> &'static str {
            "reads"
        }

        fn steps(&self, _: &Path, inputs: &Inputs, _: &Regions, store: &Store) -> Result<Steps, Unplanned> {
            let tile = |name: &str| vec![("tile".to_string(), name.to_string())];
            // A fetch of GLO-30 without params only picks the version of its tiles.
            let reads = [("natural-earth", Vec::new()), ("terrain", Vec::new()), ("copernicus-glo-30", Vec::new())];
            for (id, params) in reads {
                let _ = crate::product::version(inputs, store, id, &params).map_err(Unplanned::Failed)?;
            }
            let snapshot = |id: &str, params| Input::Snapshot {
                source: id.into(),
                version: "1".into(),
                params,
                files: Vec::new(),
            };
            let inputs = vec![
                snapshot("natural-earth", Vec::new()),
                snapshot("hansen-gfc", tile("50N_000E")),
                snapshot("hansen-gfc", tile("40N_000E")),
                snapshot("terrain", Vec::new()),
            ];
            Ok(vec![step("reads/a", inputs, "a", Run::Command(Vec::new()))].into())
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: crate::release::POINTERS[0], step: "reads/a", path: "a" }
        }
    }

    /// Needs a `.poly` that the store lacks.
    struct Lacks;

    impl Product for Lacks {
        fn name(&self) -> &'static str {
            "lacks"
        }

        fn steps(&self, _: &Path, _: &Inputs, _: &Regions, _: &Store) -> Result<Steps, Unplanned> {
            let params = vec![("area".to_string(), "europe/x".to_string())];
            Err(Unplanned::NeedsFetch(vec![Wanted { source: "geofabrik-poly".into(), version: None, params }]))
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: crate::release::POINTERS[1], step: "lacks/a", path: "a" }
        }

        fn reads(&self, _: &Path) -> Vec<String> {
            ["land-polygons", "climate", "land-cover-ie"].map(str::to_string).into()
        }
    }

    #[test]
    fn a_region_lists_what_its_steps_read_and_what_a_bake_still_needs() {
        let fixture = fixture("holding-region");
        fixture.fetched_version("natural-earth", "c", "earth.zip", b"earth\n");
        let holdings = holdings();
        let products: [&dyn Product; 2] = [&Reads, &Lacks];
        let region = |bbox| {
            let place = (&holdings, &fixture.store, &[][..]);
            super::region(place, (&products, &[]), crate::sources::all(), (&fixture.root(), None, Some(bbox))).unwrap()
        };
        let place = (&holdings, &fixture.store, &[][..]);
        let plumbing =
            super::region(place, (&[], &[]), crate::sources::all(), (&fixture.root(), None, Some("8,46,9,47")));
        assert_eq!(plumbing.err().unwrap().code, Code::Usage, "a binary without products");
        let out = region("8.2,46.2,8.5,46.5");
        let need = |id: &str| &out.sources.iter().find(|source| source.source == id).unwrap().for_region;
        assert_eq!((need("natural-earth").need, need("natural-earth").held_bytes), (Need::Held, 6));
        let hansen = need("hansen-gfc");
        let counts =
            (hansen.need, hansen.held_requests, hansen.held_bytes, hansen.download_bytes, hansen.unknown_requests);
        assert_eq!(counts, (Need::Download, 1, 30, 100, 0), "the manifest has one tile verified and lists the other");
        assert_eq!(need("terrain").need, Need::Held, "its scope covers the box");
        assert_eq!((need("dtm-ch").need, need("dtm-ch").read_by.as_deref()), (Need::Held, Some("terrain")));
        let listed = |id: &str| out.sources.iter().any(|source| source.source == id);
        assert!(!listed("dtm-fr") && !listed("nasa-svs-moon") && !listed("copernicus-glo-30"));

        // `lacks` stopped: what it can read comes after the missing input.
        assert!(!out.products[0].incomplete && out.products[1].incomplete);
        let land = need("land-polygons");
        assert_eq!((land.need, land.after_stop), (Need::Unknown, true));
        assert_eq!((need("climate").need, need("climate").after_stop), (Need::Prepare, true), "no version");
        assert!(!listed("land-cover-ie"), "its extent does not meet the box");
        let layers: Vec<(&str, Need)> = out.layers.iter().map(|layer| (layer.layer.as_str(), layer.need)).collect();
        assert_eq!(layers, [("lacks", Need::Unknown), ("test", Need::Download)]);
        assert!(out.layers[0].reason.as_deref().unwrap().contains("the store lacks `geofabrik-poly` area=europe/x"));
        assert_eq!((out.download_bytes, out.prepare.as_slice()), (100, ["climate".to_string()].as_slice()));

        // Outside the scope of `terrain`: a prepare, which its national model follows.
        let out = region("7.0,46.2,7.5,46.5");
        let need = |id: &str| &out.sources.iter().find(|source| source.source == id).unwrap().for_region;
        assert_eq!(need("terrain").need, Need::Prepare);
        assert_eq!(need("dtm-ch").need, Need::Prepare);
        assert_eq!(out.prepare, ["terrain", "climate"], "not the national model, which the prepare of terrain reads");
    }

    #[test]
    fn a_box_without_countries_takes_those_of_the_areas_that_hold_it() {
        let area = |id: &str, country: &str, [w, s, e, n]: [f64; 4]| geofabrik::Area {
            id: id.into(),
            name: id.into(),
            parent: None,
            countries: vec![country.into()],
            polygons: vec![vec![vec![[w, s], [e, s], [e, n], [w, n], [w, s]]]],
            bounds: bbox([w, s, e, n]),
        };
        let index: BTreeMap<String, geofabrik::Area> = [
            ("ch".to_string(), area("ch", "CH", [6.0, 46.0, 10.0, 47.5])),
            ("fr".to_string(), area("fr", "FR", [-5.0, 42.0, 6.0, 51.0])),
        ]
        .into();
        assert_eq!(box_countries(Some(&index), &[bbox([8.2, 46.2, 8.5, 46.5])]), ["CH"]);
        assert_eq!(box_countries(Some(&index), &[bbox([5.5, 46.2, 6.5, 46.5])]), ["CH", "FR"], "across a border");
        assert!(box_countries(None, &[bbox([8.2, 46.2, 8.5, 46.5])]).is_empty(), "without an index");
    }
}
