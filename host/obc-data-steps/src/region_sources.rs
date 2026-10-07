//! Source-area resolution shared by the device maps and the planner.

use std::collections::{BTreeMap, BTreeSet};

use obc_bake::coverage::Coverage;
use obc_data::engine::{Client, Code, Input, Run, Step};
use obc_data::env::Env;
use obc_data::product::{version, Unplanned, Wanted};
use obc_data::regions::{geofabrik, Area, Regions};
use obc_data::store::Store;
use serde_json::json;

use crate::maps::{box_poly, text, EXTRACTS, POLY};

const INDEX: &str = "geofabrik-index";

pub struct Source {
    pub id: String,
    pub extract: String,
    pub poly: String,
    pub coverage: Coverage,
}

impl Source {
    pub fn input(&self, source: &str) -> Input {
        Input::Snapshot {
            source: source.into(),
            version: if source == EXTRACTS { self.extract.clone() } else { self.poly.clone() },
            params: vec![("area".into(), self.id.clone())],
            files: Vec::new(),
        }
    }
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
    env: &Env,
    regions: &Regions,
    store: &Store,
    wanted: &mut Vec<Wanted>,
) -> Result<Option<Selection>, Unplanned> {
    let mut ids = BTreeSet::new();
    let mut boxes = Vec::new();
    for id in regions.leaves(&env.region).map_err(Unplanned::Failed)? {
        let region = regions.get(id).expect("region leaf exists");
        match &region.area {
            Area::Geofabrik { areas } => ids.extend(areas.iter().cloned()),
            Area::Box { bbox } => boxes.push(Coverage::parse_poly(&box_poly(bbox)).map_err(Unplanned::Failed)?),
            Area::Union { .. } => unreachable!("region leaf is not a union"),
        }
    }
    let mut index_version = None;
    let index = if boxes.is_empty() {
        None
    } else {
        let Some(body) = text(env, store, INDEX, &[], wanted)? else { return Ok(None) };
        index_version = Some(
            version(env, store, INDEX, &[])
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
                Coverage::parse_poly(&crate::maps::catalog::poly(area)).map(|coverage| (id.clone(), coverage))
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
        let mut sources = Vec::new();
        let mut missing = false;
        for id in selected {
            let params = vec![("area".to_string(), id.clone())];
            let Some(poly) = text(env, store, POLY, &params, wanted)? else {
                missing = true;
                continue;
            };
            let poly_version = version(env, store, POLY, &params)
                .map_err(Unplanned::Failed)?
                .map_err(|fetch| Unplanned::NeedsFetch(vec![fetch]))?;
            sources.push(Source {
                id,
                coverage: Coverage::parse_poly(&poly).map_err(Unplanned::Failed)?,
                poly: poly_version,
                extract: String::new(),
            });
        }
        if missing {
            return Ok(None);
        }
        let coverage = Coverage::union(&sources.iter().map(|source| &source.coverage).collect::<Vec<_>>())
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
        for source in &mut sources {
            let params = [("area".to_string(), source.id.clone())];
            match version(env, store, EXTRACTS, &params).map_err(Unplanned::Failed)? {
                Ok(version) => source.extract = version,
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
        let direct = regions.get(&env.region).is_some_and(|region| region.source_area().is_some());
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
    let coverage = Coverage::union(&selected.iter().map(|id| &shapes[id]).collect::<Vec<_>>());
    if !coverage.map(|coverage| coverage.covers_coverage(requested)).transpose()?.unwrap_or(false) {
        return Err("Geofabrik index does not cover the complete box; change its bounds".into());
    }
    Ok(selected)
}

/// Each request keeps its exact source version and files in its own intermediate receipt.
pub fn inputs(prefix: &str, selection: &Selection) -> Vec<Step> {
    selection
        .sources
        .iter()
        .map(|source| Step {
            name: format!("{prefix}/source/{}", source.id),
            inputs: [source.input(EXTRACTS), source.input(POLY)]
                .into_iter()
                .chain(selection.index.iter().map(|version| Input::Snapshot {
                    source: INDEX.into(),
                    version: version.clone(),
                    params: Vec::new(),
                    files: Vec::new(),
                }))
                .collect(),
            options: json!({}),
            code: Code { crates: vec!["obc-osm".into()], ..Default::default() },
            outputs: vec!["source.osm.pbf".into(), "source.poly".into()],
            run: Run::Rust(obc_osm::step::area),
            client: Client::None,
        })
        .collect()
}

pub fn combined(name: &str, inputs: &[Step], binding: Option<&obc_data::engine::Library>) -> Result<Step, Unplanned> {
    let mut step = crate::python(
        name,
        inputs
            .iter()
            .map(|step| Input::Layer { name: step.name.clone(), files: vec!["source.osm.pbf".into()] })
            .collect(),
        json!({}),
        ("tools.region_osm", None),
        &["tools/region_osm.py"],
        &["osm.pbf"],
    );
    step.client = Client::None;
    step.code.libraries.extend(binding.cloned());
    Ok(step)
}

#[cfg(test)]
mod tests;
