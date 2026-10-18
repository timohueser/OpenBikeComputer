//! The step `device/map/<i>-<j>`: the map cells of every band of one leaf, from one ingest of its
//! OSM.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use obc_data::engine::{view, Request};
use obc_draw::ingest::Ingested;
use obc_map_core::cell::has_contours;
use obc_map_core::config::{Config, CELL_SCHEMA};
use obc_map_core::grid::{Band, BandTable, CellId};
use obc_map_core::progress::{CancelToken, Progress};
use obc_map_core::terrain::TerrainSet;
use obc_places::routing::RoutableWay;
use serde_json::Value;

use crate::cut::{cut_ingested, CellArtifact, CutOptions};

pub const LAND: &str = "land-polygons";

/// Cut the cells of every band of the leaf, in an order that keeps the peak small: the routable
/// ways only build the nav graph, so the network band goes first and the ways go before the leaf
/// is drawn; coarse and mid read one semantic ladder, which goes before fine.
pub fn map(request: &Request) -> Result<(), String> {
    let job = Job::parse(&request.options)?;
    let config = Config::parse(CELL_SCHEMA)?;
    let files: BTreeMap<&String, &PathBuf> = request.layers.values().flatten().collect();
    let pbfs: Vec<_> = files.iter().filter(|(name, _)| name.ends_with(".osm.pbf")).map(|(_, path)| *path).collect();
    let [pbf] = pbfs.as_slice() else { return Err(format!("the step reads {} .osm.pbf files, not one", pbfs.len())) };
    let terrain: BTreeMap<String, PathBuf> = files
        .iter()
        .filter(|(name, _)| name.ends_with(".obcd"))
        .map(|(name, path)| ((*name).clone(), (*path).clone()))
        .collect();
    let terrain = if terrain.is_empty() {
        None
    } else {
        let dir = request.output.with_file_name("view");
        view(&terrain, &dir)?;
        Some(dir)
    };
    let progress = Progress::new(CancelToken::new(), |_, line| eprintln!("{line}"));
    let tree = request.output.with_file_name("cut");
    let mut opts =
        CutOptions { terrain: terrain.clone(), source_extent: Some(job.leaf.square()), ..CutOptions::default() };
    let mut cut = |ingested: &Ingested, ways: &[RoutableWay], bands: &[&(Band, Vec<CellId>)]| {
        if bands.is_empty() {
            return Ok(Vec::new());
        }
        opts.only_bands = bands.iter().map(|(band, _)| band.id.clone()).collect();
        opts.select = bands.iter().flat_map(|(_, cells)| cells).copied().collect();
        cut_ingested(ingested, ways, &config, &tree, &opts, &progress).map(|summary| summary.cells)
    };
    let (network, drawing): (Vec<_>, Vec<_>) = job.cells.iter().partition(|(band, _)| band.lods.is_empty());
    let (semantic, plain): (Vec<_>, Vec<_>) =
        drawing.iter().copied().partition(|(band, _)| band.lods.iter().any(|&lod| config.lods[lod].semantic_coverage));

    let pbf = pbf.to_string_lossy().into_owned();
    let (mut ingested, ways) = obc_draw::ingest::ingest_osm_ways(&[pbf], &config, None, &progress)?;
    let mut written = cut(&ingested, &ways, &network)?;
    drop(ways);
    if !drawing.is_empty() {
        let land = request.snapshots.get(LAND).map(|files| files.values().collect::<Vec<_>>());
        let Some([land]) = land.as_deref() else { return Err(format!("the step reads no single file of `{LAND}`")) };
        let extent = job.leaf.square();
        obc_draw::land::add_land(&mut ingested, &config, extent, false, Some(land), &progress)?;
        if drawing.iter().any(|(band, _)| has_contours(&config, band)) {
            let terrain = terrain.as_deref().map(TerrainSet::open).transpose()?;
            obc_draw::contour::add_contours(&mut ingested, &config, extent, terrain.as_ref(), &progress)?;
        }
    }
    written.extend(cut(&ingested, &[], &semantic)?);
    written.extend(cut(&ingested, &[], &plain)?);
    job.finish(&tree, &request.output, &written)
}

struct Job {
    leaf: CellId,
    /// The cells of each band, in the order of the band table.
    cells: Vec<(Band, Vec<CellId>)>,
    /// The ids of the cells whose square the sources do not cover.
    partial: BTreeSet<String>,
}

impl Job {
    fn parse(options: &Value) -> Result<Self, String> {
        let numbers = |value: &Value| value.as_array()?.iter().map(Value::as_i64).collect::<Option<Vec<i64>>>();
        let leaf = match numbers(&options["leaf"]).as_deref() {
            Some(&[log2, i, j]) => u32::try_from(log2).ok().and_then(|log2| CellId::new(log2, i, j).ok()),
            _ => None,
        }
        .ok_or("option `leaf` is not [log2, i, j]")?;
        let listed = options["cells"].as_object().ok_or("option `cells` is not an object")?;
        let mut cells = Vec::new();
        for band in BandTable::recommended().bands {
            let Some(list) = listed.get(&band.id) else { continue };
            let mut ids = list
                .as_array()
                .ok_or(format!("the cells of `{}` are not a list", band.id))?
                .iter()
                .map(|cell| match numbers(cell).as_deref() {
                    Some(&[i, j]) => CellId::new(band.cell_log2, i, j).ok(),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .ok_or(format!("a cell of `{}` is not [i, j] of the band", band.id))?;
            ids.sort_unstable();
            ids.dedup();
            // The cutter reads an empty selection as every cell that meets the leaf.
            if ids.is_empty() {
                return Err(format!("band `{}` has no cells", band.id));
            }
            cells.push((band, ids));
        }
        if let Some(band) = listed.keys().find(|id| !cells.iter().any(|(band, _)| &band.id == *id)) {
            return Err(format!("no band `{band}`"));
        }
        let partial: BTreeSet<String> = serde_json::from_value(options["partial_cells"].clone())
            .map_err(|e| format!("option `partial_cells`: {e}"))?;
        let selected: BTreeSet<String> = cells.iter().flat_map(|(_, ids)| ids).map(ToString::to_string).collect();
        if !partial.is_subset(&selected) {
            return Err("partial cell is not selected by this step".into());
        }
        Ok(Job { leaf, cells, partial })
    }

    /// Move the cells from `tree` to `output`. A fully covered cell without content has no
    /// payload: `cells/<band>/empty.json` lists it.
    fn finish(&self, tree: &Path, output: &Path, written: &[CellArtifact]) -> Result<(), String> {
        for (band, cells) in &self.cells {
            let produced: Vec<CellId> =
                written.iter().filter(|cell| cell.band == band.id).map(|cell| cell.id).collect();
            if produced != *cells {
                return Err(format!("the cut wrote other `{}` cells than the cells of the step", band.id));
            }
            let mut empty = Vec::new();
            for cell in written.iter().filter(|cell| cell.band == band.id) {
                let id = cell.id.to_string();
                if cell.empty && !self.partial.contains(&id) {
                    empty.push(id);
                    continue;
                }
                let path = output.join(&cell.path);
                std::fs::create_dir_all(path.parent().expect("cell path has a parent")).map_err(|e| e.to_string())?;
                std::fs::rename(tree.join(&cell.path), &path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            let dir = output.join("cells").join(&band.id);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join("empty.json");
            std::fs::write(&path, serde_json::to_string(&empty).expect("strings serialize"))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_empty_cells_keep_their_bytes_and_covered_empty_cells_have_none() {
        let dir = obcm_testkit::scratch::scratch_dir("step", "empty-coverage");
        let ids = [CellId::new(18, 1204, 1052).unwrap(), CellId::new(18, 1204, 1053).unwrap()];
        let job = Job::parse(&serde_json::json!({
            "leaf": [23, 37, 32],
            "cells": {"network": ids.iter().map(|id| [id.i, id.j]).collect::<Vec<_>>()},
            "partial_cells": [ids[1].to_string()],
        }))
        .unwrap();
        let (tree, output) = (dir.join("cut"), dir.join("output"));
        let written: Vec<CellArtifact> = ids
            .iter()
            .map(|id| {
                let path = obc_map_core::cell::cell_path(&job.cells[0].0, id);
                std::fs::create_dir_all(tree.join(&path).parent().unwrap()).unwrap();
                std::fs::write(tree.join(&path), id.to_string()).unwrap();
                CellArtifact {
                    id: *id,
                    band: "network".into(),
                    path,
                    bytes: 0,
                    sha256: String::new(),
                    partial: false,
                    dropped: 0,
                    pois: 0,
                    nav_nodes: 0,
                    nav_edges: 0,
                    empty: true,
                }
            })
            .collect();
        job.finish(&tree, &output, &written).unwrap();
        let empty: Vec<String> =
            serde_json::from_slice(&std::fs::read(output.join("cells/network/empty.json")).unwrap()).unwrap();
        assert_eq!(empty, [ids[0].to_string()]);
        assert!(!output.join(&written[0].path).exists());
        assert_eq!(std::fs::read_to_string(output.join(&written[1].path)).unwrap(), ids[1].to_string());
    }
}
