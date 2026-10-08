//! Receipt-based catalog step and its publication boundary.

use super::*;
use obc_data::product::{Pointer, PointerFn};
use obc_pack::catalog::boundary::{simplified_rings, DEFAULT_TOLERANCE_UDEG};
use obc_pack::catalog::engine::{Options, Pick};
use obc_pack::catalog::{Boundary, CellSource};

pub const INDEX: &str = "geofabrik-index";
pub const LAYER: &str = "maps/catalog";

#[allow(clippy::too_many_arguments)]
pub fn step(
    env: &Env,
    region: &obc_data::regions::Region,
    store: &Store,
    selection: &crate::region_sources::Selection,
    listed: &[Step],
    glo30: &str,
    version: String,
    body: &str,
) -> Result<Step, Unplanned> {
    let areas = obc_data::regions::geofabrik::parse(body.as_bytes()).map_err(Unplanned::Failed)?;
    let primary: BTreeMap<_, _> = selection
        .sources
        .iter()
        .map(|source| {
            let params = [("area".into(), source.id.clone())];
            let (_, digest) = file(env, store, POLY, &source.poly, &params)?;
            let poly = std::fs::read_to_string(store.object(&digest)).map_err(|e| Unplanned::Failed(e.to_string()))?;
            Ok((source.id.clone(), poly))
        })
        .collect::<Result<_, Unplanned>>()?;
    if primary.keys().any(|id| !areas.contains_key(id)) {
        return Err(Unplanned::Invalid("Geofabrik index lacks a selected source area".into()));
    }
    let source_coverage = &selection.coverage;
    let requested = Coverage::union(&selection.outlines.iter().collect::<Vec<_>>())
        .ok_or_else(|| Unplanned::Failed("cannot union requested region coverage".into()))?;
    let mut coverage = BTreeMap::new();
    let mut available = BTreeMap::new();
    for band in BandTable::recommended().bands {
        let boundary = source_coverage.boundary_cells(band.cell_log2);
        let cells = requested.cells(band.cell_log2);
        coverage.insert(
            band.id.clone(),
            cells.iter().map(|id| (id.to_string(), !source_coverage.covers(*id, &boundary))).collect(),
        );
        available.insert(band.id, cells);
    }
    let terrain_available = requested.cells(V1_CELL_LOG2.into());
    let mut picks = Vec::new();
    let bbox = requested.bbox();
    for area in areas.values() {
        if !primary.contains_key(&area.id)
            && (area.bounds.east * 1e6 < bbox.0 as f64
                || area.bounds.west * 1e6 > bbox.2 as f64
                || area.bounds.north * 1e6 < bbox.1 as f64
                || area.bounds.south * 1e6 > bbox.3 as f64)
        {
            continue;
        }
        let poly = primary.get(&area.id).cloned().unwrap_or_else(|| poly(area));
        let outline = Coverage::parse_poly(&poly).map_err(Unplanned::Failed)?;
        // Candidate selections are admitted only when every band and terrain cell is built.
        let mut cells = BTreeMap::new();
        let mut complete = true;
        for band in BandTable::recommended().bands {
            let selected = outline.cells(band.cell_log2);
            if !selected.is_subset(&available[&band.id]) {
                complete = false;
                break;
            }
            cells.insert(band.id, selected.into_iter().map(|id| id.to_string()).collect());
        }
        if !complete {
            continue;
        }
        let terrain = outline.cells(V1_CELL_LOG2.into());
        if !terrain.is_subset(&terrain_available) {
            continue;
        }
        picks.push(Pick {
            id: area.id.clone(),
            name: area.name.clone(),
            parent: area.parent.clone(),
            boundary: Boundary {
                tolerance_udeg: DEFAULT_TOLERANCE_UDEG,
                rings: simplified_rings(&poly, DEFAULT_TOLERANCE_UDEG).map_err(Unplanned::Failed)?,
            },
            cells,
            terrain: terrain.into_iter().map(|id| id.to_string()).collect(),
        });
    }
    if !selection.direct || selection.sources[0].id != env.region {
        let shape = requested.geojson();
        let parsed: serde_json::Value = serde_json::from_str(&shape).map_err(|e| Unplanned::Failed(e.to_string()))?;
        let mut outline = format!("{}\n", region.id);
        for (polygon, rings) in parsed["coordinates"]
            .as_array()
            .ok_or_else(|| Unplanned::Failed("region outline has no polygons".into()))?
            .iter()
            .enumerate()
        {
            for (ring, points) in rings.as_array().expect("polygon rings").iter().enumerate() {
                outline.push_str(&format!("{}{polygon}-{ring}\n", if ring == 0 { "" } else { "!" }));
                for point in points.as_array().expect("ring points") {
                    outline.push_str(&format!(" {} {}\n", point[0], point[1]));
                }
                outline.push_str("END\n");
            }
        }
        outline.push_str("END\n");
        picks.retain(|pick| pick.id != region.id);
        picks.push(Pick {
            id: region.id.clone(),
            name: region.name.clone(),
            parent: None,
            boundary: Boundary {
                tolerance_udeg: DEFAULT_TOLERANCE_UDEG,
                rings: simplified_rings(&outline, DEFAULT_TOLERANCE_UDEG).map_err(Unplanned::Failed)?,
            },
            cells: available
                .iter()
                .map(|(band, cells)| (band.clone(), cells.iter().map(ToString::to_string).collect()))
                .collect(),
            terrain: terrain_available.iter().map(ToString::to_string).collect(),
        });
    }
    if !picks.iter().any(|pick| pick.id == env.region) {
        return Err(Unplanned::Invalid(format!("catalog has no complete selection for `{}`", env.region)));
    }
    let ids: BTreeSet<String> = picks.iter().map(|pick| pick.id.clone()).collect();
    for pick in &mut picks {
        pick.parent = pick.parent.take().filter(|parent| ids.contains(parent));
    }
    let options = Options {
        sources: selection
            .sources
            .iter()
            .map(|source| CellSource { extract_id: source.id.clone(), snapshot: source.extract.clone() })
            .collect(),
        coverage,
        picks,
        posting_log2: V1_POSTING_LOG2,
        terrain_cell_log2: V1_CELL_LOG2,
        dataset_version: glo30.into(),
    };
    let mut inputs: Vec<Input> = listed
        .iter()
        .filter(|step| !step.client.is_none())
        .filter(|step| !matches!(step.name.split('/').nth(1), Some("landmark-content" | "peak-content")))
        .map(|step| Input::layer(&step.name))
        .collect();
    inputs.push(Input::Snapshot { source: INDEX.into(), version, params: Vec::new(), files: Vec::new() });
    Ok(Step {
        name: LAYER.into(),
        inputs,
        options: serde_json::to_value(options).map_err(|e| Unplanned::Failed(e.to_string()))?,
        code: Code {
            paths: vec!["builder/presets".into()],
            crates: vec!["obc-pack".into()],
            sources: vec!["osm-planet".into(), "copernicus-glo-30".into()],
            ..Default::default()
        },
        outputs: ["catalog.json", "schema.json", "terrain.json", "LICENSE.txt", "regions", "objects"]
            .map(String::from)
            .into(),
        run: Run::Rust(obc_pack::catalog::engine::build),
        client: Client::Paths(vec!["objects".into()]),
    })
}

pub(crate) fn poly(area: &obc_data::regions::geofabrik::Area) -> String {
    let mut text = format!("{}\n", area.id);
    for (polygon, rings) in area.polygons.iter().enumerate() {
        for (ring, points) in rings.iter().enumerate() {
            text.push_str(&format!("{}{polygon}-{ring}\n", if ring == 0 { "" } else { "!" }));
            for [lon, lat] in points {
                text.push_str(&format!(" {lon} {lat}\n"));
            }
            text.push_str("END\n");
        }
    }
    text.push_str("END\n");
    text
}

pub fn named(release: &obc_data::engine::release::Release) -> Result<Vec<obc_data::engine::LayerFile>, String> {
    let layer = release.layers.iter().find(|layer| layer.step == LAYER).ok_or("release has no maps catalog")?;
    Ok(layer.files.iter().filter(|file| !file.path.starts_with("objects/")).cloned().collect())
}

pub fn pointer() -> PointerFn {
    |_, release, store| {
        let file =
            release.named.iter().find(|file| file.path == "catalog.json").ok_or("release has no catalog root")?;
        let body = std::fs::read(store.object(&file.sha256)).map_err(|e| format!("{}: {e}", file.path))?;
        let document = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        Ok(Pointer { document })
    }
}

mod verify;
pub(crate) use verify::assemble;
pub use verify::verify;
