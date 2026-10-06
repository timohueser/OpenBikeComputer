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
    store: &Store,
    outlines: &[Coverage],
    listed: &[Step],
    extract: &str,
    glo30: &str,
    version: String,
    body: &str,
) -> Result<Step, Unplanned> {
    let areas = obc_data::regions::geofabrik::parse(body.as_bytes()).map_err(Unplanned::Failed)?;
    if !areas.contains_key(&env.region) {
        return Err(Unplanned::Invalid(format!("Geofabrik index has no area `{}`", env.region)));
    }
    let source_coverage = Coverage::union(&outlines.iter().collect::<Vec<_>>())
        .ok_or("cannot union source coverage")
        .map_err(|e| Unplanned::Failed(e.into()))?;
    let mut coverage = BTreeMap::new();
    let mut available = BTreeMap::new();
    for band in BandTable::recommended().bands {
        let boundary = source_coverage.boundary_cells(band.cell_log2);
        let cells = source_coverage.cells(band.cell_log2);
        coverage.insert(
            band.id.clone(),
            cells.iter().map(|id| (id.to_string(), !source_coverage.covers(*id, &boundary))).collect(),
        );
        available.insert(band.id, cells);
    }
    let terrain_available = source_coverage.cells(V1_CELL_LOG2.into());
    let params = [("area".into(), env.region.clone())];
    let poly_version = version_of_poly(env, store, &params)?;
    let (_, digest) = file(env, store, POLY, &poly_version, &params)?;
    let primary_poly = std::fs::read_to_string(store.object(&digest)).map_err(|e| Unplanned::Failed(e.to_string()))?;
    let mut picks = Vec::new();
    let bbox = source_coverage.bbox();
    for area in areas.values() {
        if area.id != env.region
            && (area.bounds.east * 1e6 < bbox.0 as f64
                || area.bounds.west * 1e6 > bbox.2 as f64
                || area.bounds.north * 1e6 < bbox.1 as f64
                || area.bounds.south * 1e6 > bbox.3 as f64)
        {
            continue;
        }
        let poly = if area.id == env.region { primary_poly.clone() } else { poly(area) };
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
    if !picks.iter().any(|pick| pick.id == env.region) {
        return Err(Unplanned::Invalid(format!("catalog has no complete selection for `{}`", env.region)));
    }
    let ids: BTreeSet<String> = picks.iter().map(|pick| pick.id.clone()).collect();
    for pick in &mut picks {
        pick.parent = pick.parent.take().filter(|parent| ids.contains(parent));
    }
    let options = Options {
        sources: vec![CellSource { extract_id: env.region.clone(), snapshot: extract.into() }],
        coverage,
        picks,
        posting_log2: V1_POSTING_LOG2,
        terrain_cell_log2: V1_CELL_LOG2,
        dataset_version: glo30.into(),
    };
    let mut inputs: Vec<Input> =
        listed.iter().filter(|step| !step.client.is_none()).map(|step| Input::layer(&step.name)).collect();
    inputs.push(Input::Snapshot { source: INDEX.into(), version, params: Vec::new(), files: Vec::new() });
    Ok(Step {
        name: LAYER.into(),
        inputs,
        options: serde_json::to_value(options).map_err(|e| Unplanned::Failed(e.to_string()))?,
        code: Code { paths: vec!["builder/presets".into()], crates: vec!["obc-pack".into()] },
        outputs: ["catalog.json", "schema.json", "terrain.json", "LICENSE.txt", "regions", "objects"]
            .map(String::from)
            .into(),
        run: Run::Rust(obc_pack::catalog::engine::build),
        client: Client::Paths(vec!["objects".into()]),
    })
}

fn version_of_poly(env: &Env, store: &Store, params: &[(String, String)]) -> Result<String, Unplanned> {
    version(env, store, POLY, params).map_err(Unplanned::Failed)?.map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))
}

fn poly(area: &obc_data::regions::geofabrik::Area) -> String {
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

pub fn pointer() -> PointerFn {
    |release, store| {
        let layer = release.layers.iter().find(|layer| layer.step == LAYER).ok_or("release has no maps catalog")?;
        let mut named = BTreeMap::new();
        let mut document = None;
        for file in &layer.files {
            if file.path.starts_with("objects/") {
                continue;
            }
            let body = std::fs::read(store.object(&file.sha256)).map_err(|e| format!("{}: {e}", file.path))?;
            if file.path == "catalog.json" {
                document = Some(
                    serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&body)
                        .map_err(|e| e.to_string())?,
                );
            } else {
                named.insert(file.path.clone(), body);
            }
        }
        Ok(Pointer { document: document.ok_or("catalog receipt has no root")?, named })
    }
}

mod verify;
pub use verify::verify;
