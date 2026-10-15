//! Source-area resolution shared by the device maps and the planner.

use std::collections::{BTreeMap, BTreeSet};

use obc_bake::coverage::Coverage;
use obc_data::engine::{Client, Input, Layer, Run, Shape, Step};
use obc_data::product::{version, Inputs, Unplanned, Wanted};
use obc_data::regions::{geofabrik, Area, Bbox, Regions};
use obc_data::store::Store;
use serde_json::json;

use crate::device::{box_poly, text, EXTRACTS, POLY};

const INDEX: &str = "geofabrik-index";
const PLANET: &str = "osm-planet";
const REPLICATION: &str = "osm-replication";

/// The OSM of a region, from its source areas: the steps of the layers that read OSM read it.
pub static OSM: Layer = Layer { name: "osm", shape: Shape::Whole, format: 1, revision: 2, readers: &[] };

pub struct Source {
    pub id: String,
    pub osm: Osm,
    pub coverage: Coverage,
}

/// The versions of the fetches that give the OSM of a source.
pub enum Osm {
    /// The extract and the `.poly` of the Geofabrik area.
    Geofabrik { extract: String, poly: String },
    /// `osm-planet` of `base`, and the daily diffs of `osm-replication` after it up to `day`.
    Planet { base: String, day: String },
}

impl Source {
    /// The fetches of its source step.
    pub fn reads(&self) -> Vec<Input> {
        let read = |source: &str, version: &String, params: Vec<(String, String)>| Input::Snapshot {
            source: source.into(),
            version: version.clone(),
            params,
            files: Vec::new(),
        };
        match &self.osm {
            Osm::Geofabrik { extract, poly } => {
                let area = vec![("area".to_string(), self.id.clone())];
                vec![read(EXTRACTS, extract, area.clone()), read(POLY, poly, area)]
            }
            Osm::Planet { base, day } => vec![read(PLANET, base, Vec::new()), read(REPLICATION, day, from(base))],
        }
    }
}

/// The params of the daily diffs after `osm-planet` of `base`.
fn from(base: &str) -> Vec<(String, String)> {
    vec![("from".into(), base.into())]
}

/// Whether the region `id` holds the planet.
pub fn planet(regions: &Regions, id: &str) -> Result<bool, Unplanned> {
    let leaves = regions.leaves(id).map_err(Unplanned::Failed)?;
    Ok(leaves.iter().any(|leaf| regions.get(leaf).is_some_and(|region| region.area == Area::Planet)))
}

pub struct Selection {
    pub sources: Vec<Source>,
    pub outlines: Vec<Coverage>,
    pub coverage: Coverage,
    /// Single-area catalogs and held captures retain their existing names.
    pub direct: bool,
    pub index: Option<String>,
}

pub fn resolve(
    inputs: &Inputs,
    regions: &Regions,
    store: &Store,
    wanted: &mut Vec<Wanted>,
) -> Result<Option<Selection>, Unplanned> {
    if planet(regions, &inputs.settings.region)? {
        let base = version(inputs, store, PLANET, &[]).map_err(Unplanned::Failed)?;
        let Some(base) = base.map_err(|fetch| wanted.push(fetch)).ok() else { return Ok(None) };
        let day = version(inputs, store, REPLICATION, &from(&base)).map_err(Unplanned::Failed)?;
        let Some(day) = day.map_err(|fetch| wanted.push(fetch)).ok() else { return Ok(None) };
        let world = Coverage::parse_poly(&box_poly(&Bbox::WORLD)).map_err(Unplanned::Failed)?;
        let source = Source { id: "planet".into(), osm: Osm::Planet { base, day }, coverage: world.clone() };
        let (sources, outlines) = (vec![source], vec![world.clone()]);
        return Ok(Some(Selection { sources, outlines, coverage: world, direct: false, index: None }));
    }
    let mut ids = BTreeSet::new();
    let mut boxes = Vec::new();
    for id in regions.leaves(&inputs.settings.region).map_err(Unplanned::Failed)? {
        let region = regions.get(id).expect("region leaf exists");
        match &region.area {
            Area::Geofabrik { areas } => ids.extend(areas.iter().cloned()),
            Area::Box { bbox } => boxes.push(Coverage::parse_poly(&box_poly(bbox)).map_err(Unplanned::Failed)?),
            Area::Union { .. } | Area::Planet => unreachable!("region leaf is not a union or the planet"),
        }
    }
    let mut index_version = None;
    let index = if boxes.is_empty() {
        None
    } else {
        let Some(body) = text(inputs, store, INDEX, &[], wanted)? else { return Ok(None) };
        index_version = Some(
            version(inputs, store, INDEX, &[])
                .map_err(Unplanned::Failed)?
                .map_err(|fetch| Unplanned::NeedsFetch(vec![fetch]))?,
        );
        Some(geofabrik::parse(body.as_bytes()).map_err(Unplanned::Failed)?)
    };
    let mut box_ids = BTreeSet::new();
    if let Some(index) = &index {
        let shapes = index
            .iter()
            .map(|(id, area)| {
                Coverage::parse_poly(&crate::device::catalog::poly(area)).map(|coverage| (id.clone(), coverage))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(Unplanned::Failed)?;
        for requested in &boxes {
            box_ids.extend(select_box(index, &shapes, requested).map_err(Unplanned::Invalid)?);
        }
    }
    // Index shapes only choose candidates. The full fetched polygons establish coverage.
    loop {
        let selected = ids.union(&box_ids).cloned().collect::<Vec<_>>();
        let mut areas = Vec::new();
        let mut missing = false;
        for id in selected {
            let params = vec![("area".to_string(), id.clone())];
            let Some(poly) = text(inputs, store, POLY, &params, wanted)? else {
                missing = true;
                continue;
            };
            let poly_version = version(inputs, store, POLY, &params)
                .map_err(Unplanned::Failed)?
                .map_err(|fetch| Unplanned::NeedsFetch(vec![fetch]))?;
            areas.push((id, poly_version, Coverage::parse_poly(&poly).map_err(Unplanned::Failed)?));
        }
        if missing {
            return Ok(None);
        }
        let coverage = Coverage::union(&areas.iter().map(|(_, _, coverage)| coverage).collect::<Vec<_>>())
            .ok_or_else(|| Unplanned::Failed("cannot union region source coverage".into()))?;
        let complete = boxes
            .iter()
            .map(|requested| coverage.covers_coverage(requested))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Unplanned::Failed)?
            .into_iter()
            .all(|covered| covered);
        if !complete {
            let index = index.as_ref().expect("boxes have an index");
            let parents = box_ids
                .iter()
                .map(|id| index[id].parent.clone().unwrap_or_else(|| id.clone()))
                .collect::<BTreeSet<_>>();
            if parents == box_ids {
                return Err(Unplanned::Invalid(
                    "Geofabrik source polygons do not cover the complete box; change its bounds".into(),
                ));
            }
            box_ids = parents;
            continue;
        }
        let mut sources = Vec::new();
        for (id, poly, coverage) in areas {
            let params = [("area".to_string(), id.clone())];
            match version(inputs, store, EXTRACTS, &params).map_err(Unplanned::Failed)? {
                Ok(extract) => sources.push(Source { id, osm: Osm::Geofabrik { extract, poly }, coverage }),
                Err(fetch) => {
                    wanted.push(fetch);
                    missing = true;
                }
            }
        }
        if missing {
            return Ok(None);
        }
        let mut outlines = boxes;
        outlines.extend(sources.iter().filter(|source| ids.contains(&source.id)).map(|source| source.coverage.clone()));
        let direct = regions.get(&inputs.settings.region).is_some_and(|region| region.source_area().is_some());
        return Ok(Some(Selection { sources, outlines, coverage, direct, index: index_version }));
    }
}

fn select_box(
    index: &BTreeMap<String, geofabrik::Area>,
    shapes: &BTreeMap<String, Coverage>,
    requested: &Coverage,
) -> Result<BTreeSet<String>, String> {
    fn descend(
        id: &str,
        index: &BTreeMap<String, geofabrik::Area>,
        shapes: &BTreeMap<String, Coverage>,
        requested: &Coverage,
        selected: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        let Some(part) = shapes[id].intersection(requested)? else { return Ok(()) };
        let mut children = Vec::new();
        for child in index.values().filter(|area| area.parent.as_deref() == Some(id)) {
            if shapes[&child.id].intersection(&part)?.is_some() {
                children.push(child.id.as_str());
            }
        }
        let union = Coverage::union(&children.iter().map(|id| &shapes[*id]).collect::<Vec<_>>());
        if union.as_ref().map(|union| union.covers_coverage(&part)).transpose()?.unwrap_or(false) {
            for child in children {
                descend(child, index, shapes, &part, selected)?;
            }
        } else {
            selected.insert(id.into());
        }
        Ok(())
    }
    let mut selected = BTreeSet::new();
    for area in index.values().filter(|area| area.parent.is_none()) {
        descend(&area.id, index, shapes, requested, &mut selected)?;
    }
    let covers = |ids: &BTreeSet<String>| -> Result<bool, String> {
        let union = Coverage::union(&ids.iter().map(|id| &shapes[id]).collect::<Vec<_>>());
        Ok(union.map(|union| union.covers_coverage(requested)).transpose()?.unwrap_or(false))
    };
    if !covers(&selected)? {
        return Err("Geofabrik index does not cover the complete box; change its bounds".into());
    }
    // Aggregates such as `europe/dach` overlap their sibling countries: drop each area that the
    // others already cover, largest first.
    let mut largest = selected.iter().cloned().collect::<Vec<_>>();
    largest.sort_by(|a, b| shapes[b].area_km2().total_cmp(&shapes[a].area_km2()));
    for id in largest {
        selected.remove(&id);
        if !covers(&selected)? {
            selected.insert(id);
        }
    }
    Ok(selected)
}

/// Each request keeps its exact source version and files in its own intermediate receipt. The
/// source step of the planet applies the diffs, and the outline is the world.
pub fn inputs(prefix: &str, selection: &Selection) -> Vec<Step> {
    selection
        .sources
        .iter()
        .map(|source| {
            let (run, options) = match source.osm {
                Osm::Geofabrik { .. } => (Run::Rust(obc_osm::step::area), json!({})),
                Osm::Planet { .. } => (Run::Rust(obc_osm::step::planet), json!({"poly": box_poly(&Bbox::WORLD)})),
            };
            Step {
                layer: &OSM,
                name: format!("{prefix}/source/{}", source.id),
                inputs: source
                    .reads()
                    .into_iter()
                    .chain(selection.index.iter().map(|version| Input::Snapshot {
                        source: INDEX.into(),
                        version: version.clone(),
                        params: Vec::new(),
                        files: Vec::new(),
                    }))
                    .collect(),
                options,
                outputs: vec!["source.osm.pbf".into(), "source.poly".into()],
                run,
                client: Client::None,
            }
        })
        .collect()
}

pub fn combined(name: &str, inputs: &[Step]) -> Step {
    let mut step = crate::python(
        &OSM,
        name,
        inputs
            .iter()
            .map(|step| Input::Step { name: step.name.clone(), files: vec!["source.osm.pbf".into()] })
            .collect(),
        json!({}),
        ("tools.region_osm", None),
        &["osm.pbf"],
    );
    step.client = Client::None;
    step
}

#[cfg(test)]
mod tests;
