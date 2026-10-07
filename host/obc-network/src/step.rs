//! Network cells from one prepared source leaf, without land or drawing work.

use crate::cut::{bucket_pois, prepare_nav};
use crate::nav::NavGraph;
use crate::serialize::{serialize_nav_section, serialize_poi_section};
use obc_data::engine::{view, Request};
use obc_elevation::{ElevationSource, NullElevation};
use obc_map_core::cell::{cell_path, Job, SourceFiles};
use obc_map_core::config::{Config, CELL_SCHEMA};
use obc_map_core::progress::{CancelToken, Progress};
use obc_map_core::serialize::{validate_chunk_size, MapWriter};
use obc_map_core::terrain::TerrainSet;
use std::path::Path;

pub fn cells(request: &Request) -> Result<(), String> {
    let job = Job::parse(&request.options)?;
    if !job.band.lods.is_empty() {
        return Err("network band carries drawing levels".into());
    }
    let config = Config::parse(CELL_SCHEMA)?;
    validate_chunk_size(config.chunk_size)?;
    let files = SourceFiles::parse(&request.layers)?;
    let terrain = if files.terrain.is_empty() {
        None
    } else {
        let dir = request.output.with_file_name("view");
        view(&files.terrain, &dir)?;
        Some(TerrainSet::open(&dir)?)
    };
    let progress = Progress::new(CancelToken::new(), |_, line| eprintln!("{line}"));
    let places = obc_places::osm::harvest(&[files.pbf.to_string_lossy().into_owned()], &config, &progress)?;
    write_cells(&job, &config, &places, terrain.as_ref(), &request.output, &progress)
}

/// Write network sections from place metadata and routable ways, without drawing work.
pub fn write_cells(
    job: &Job,
    config: &Config,
    places: &obc_places::osm::Places,
    terrain: Option<&TerrainSet>,
    output: &Path,
    progress: &Progress,
) -> Result<(), String> {
    let cut = if job.band.has_nav() { Some(prepare_nav(&places.ways, job.band.cell_log2, progress)?) } else { None };
    let buckets = if job.band.has_poi() { bucket_pois(&places.pois, job.band.cell_log2) } else { Default::default() };
    let tree = output.with_file_name("cut");
    let styles = config.styles();
    let mut artifacts = Vec::new();
    for cell in &job.cells {
        progress.check()?;
        let mut pois = buckets
            .get(&(cell.i, cell.j))
            .map(|indexes| indexes.iter().map(|&i| places.pois[i as usize].clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        let graph = cut
            .as_ref()
            .map_or_else(NavGraph::default, |cut| cut.cell_graph(*cell, config.routing.min_component_edges));
        let mut sampler = terrain.map(|set| set.sampler_for(Some(cell.square()))).transpose()?;
        let mut null = NullElevation;
        let terrain: &mut dyn ElevationSource = match &mut sampler {
            Some(s) => s,
            None => &mut null,
        };
        obc_places::metadata::fill_summit_elevations(&mut pois, terrain);
        let path = tree.join(cell_path(&job.band, cell));
        std::fs::create_dir_all(path.parent().expect("cell path has a parent")).map_err(|e| e.to_string())?;
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut output = std::io::BufWriter::new(file);
        let mut writer = MapWriter::new(&mut output, config.lods.len(), &styles, config.marker_color, cell.square())
            .map_err(|e| e.to_string())?;
        for lod in &config.lods {
            writer.lod(config.chunk_size, lod.max_mpp, None).map_err(|e| e.to_string())?;
        }
        let poi = serialize_poi_section(&pois, cell.square(), writer.position()).map_err(|e| e.to_string())?;
        let nav = serialize_nav_section(
            &graph,
            &config.routing.profiles,
            cell.square(),
            writer.position() + poi.len(),
            terrain,
        );
        writer.finish(&poi, &nav, &[], &[]).map_err(|e| e.to_string())?;
        use std::io::Write;
        output.flush().map_err(|e| e.to_string())?;
        artifacts.push((*cell, path, pois.is_empty() && graph.nodes.is_empty() && graph.edges.is_empty()));
    }
    progress.check()?;
    job.finish(output, &artifacts)
}
