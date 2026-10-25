//! Receipt-based catalog step and its publication boundary.

use super::*;
use obc_pack::catalog::boundary::{simplified_rings, DEFAULT_TOLERANCE_UDEG};
use obc_pack::catalog::engine::{Credits, Options, Pick};
use obc_pack::catalog::{Boundary, CellSource};

pub const INDEX: &str = "geofabrik-index";
pub const LAYER: &str = "device/catalog";

/// The catalog that lists every map, terrain and landmark file of the device layers.
pub static DEVICE_CATALOG: Layer = Layer {
    name: "device/catalog",
    shape: obc_data::engine::Shape::Whole,
    format: obc_pack::catalog::CATALOG_SCHEMA_VERSION,
    revision: 2,
    readers: &[&crate::readers::MAP_BUILDER, &crate::readers::DESKTOP_APP],
};

#[allow(clippy::too_many_arguments)]
pub fn step(
    inputs: &Inputs,
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
            let (_, digest) = file(store, POLY, &source.poly, &params)?;
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
    let id = &inputs.settings.region;
    if !selection.direct || selection.sources[0].id != *id {
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
    if !picks.iter().any(|pick| pick.id == *id) {
        return Err(Unplanned::Invalid(format!("catalog has no complete selection for `{id}`")));
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
        credits: Credits::of_sources(),
    };
    let mut reads: Vec<Input> = listed
        .iter()
        .filter(|step| !step.client.is_none())
        .filter(|step| !matches!(step.name.split('/').nth(1), Some("landmark-content" | "peak-content")))
        .map(|step| Input::step(&step.name))
        .collect();
    reads.push(Input::Snapshot { source: INDEX.into(), version, params: Vec::new(), files: Vec::new() });
    Ok(Step {
        layer: &DEVICE_CATALOG,
        name: LAYER.into(),
        inputs: reads,
        options: serde_json::to_value(options).map_err(|e| Unplanned::Failed(e.to_string()))?,
        outputs: ["catalog.json", "schema.json", "terrain.json", "LICENSE.txt", "regions", "objects"]
            .map(String::from)
            .into(),
        run: Run::Rust(&functions::DEVICE_CATALOG),
        client: Client::All,
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

/// The documents of the catalog beside its root: the schema, the terrain, the licence and the
/// regions.
pub fn named(outputs: &[Output]) -> Result<Vec<(String, LayerFile)>, String> {
    let output = outputs.iter().find(|output| output.step == LAYER).ok_or("release has no device catalog")?;
    let named = output.files.iter().filter(|file| !file.path.starts_with("objects/"));
    Ok(named.map(|file| (file.path.clone(), file.clone())).collect())
}

mod verify;
pub use verify::verify;
