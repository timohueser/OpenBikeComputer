//! Drawing cells from one prepared source leaf.

use crate::cut::{prepare_lod, PreparedSemantic};
use crate::semantic::build_semantic_levels;
use crate::serialize::{serialize_tree, Node};
use obc_data::engine::{view, Request};
use obc_map_core::cell::{cell_path, has_contours, Job, SourceFiles};
use obc_map_core::config::{Config, CELL_SCHEMA};
use obc_map_core::progress::{CancelToken, Progress};
use obc_map_core::serialize::{
    emit_nav_section, empty_poi_section, pack_profile_table, validate_chunk_size, LodBytes, MapWriter,
};
use obc_map_core::terrain::TerrainSet;
use std::path::Path;

pub const LAND: &str = "land-polygons";

pub fn cells(request: &Request) -> Result<(), String> {
    let job = Job::parse(&request.options)?;
    if job.band.has_nav() || job.band.has_poi() {
        return Err("drawing band carries network sections".into());
    }
    let config = Config::parse(CELL_SCHEMA)?;
    validate_chunk_size(config.chunk_size)?;
    let files = SourceFiles::parse(&request.layers)?;
    let land = request.snapshots.get(LAND).map(|files| files.values().collect::<Vec<_>>());
    let Some([land]) = land.as_deref() else { return Err(format!("the step reads no single file of `{LAND}`")) };
    let terrain = if files.terrain.is_empty() {
        None
    } else {
        let dir = request.output.with_file_name("view");
        view(&files.terrain, &dir)?;
        Some(TerrainSet::open(&dir)?)
    };
    let progress = Progress::new(CancelToken::new(), |_, line| eprintln!("{line}"));
    let (mut ingested, _) =
        crate::ingest::ingest_osm_ways(&[files.pbf.to_string_lossy().into_owned()], &config, None, &progress)?;
    let extent = job.leaf.square();
    crate::land::add_land(&mut ingested, &config, extent, false, Some(land), &progress)?;
    if has_contours(&config, &job.band) {
        crate::contour::add_contours(&mut ingested, &config, extent, terrain.as_ref(), &progress)?;
    }
    write_cells(&job, &config, &ingested, &request.output, &progress)
}

/// Write only drawing sections from the prepared source of one leaf.
pub fn write_cells(
    job: &Job,
    config: &Config,
    ingested: &crate::ingest::Ingested,
    output: &Path,
    progress: &Progress,
) -> Result<(), String> {
    let extent = job.leaf.square();
    let scheme = config.semantic_scheme();
    let semantic = if job.band.lods.iter().any(|&l| config.lods[l].semantic_coverage) {
        build_semantic_levels(&ingested.features, &config.lods, &scheme, extent, progress)?
    } else {
        vec![None; config.lods.len()]
    };
    let mut merged = std::collections::HashMap::new();
    let sets: Vec<_> = job
        .band
        .lods
        .iter()
        .map(|&lod| {
            prepare_lod(
                ingested,
                config,
                lod,
                job.band.cell_log2,
                PreparedSemantic { features: semantic[lod].as_deref(), scheme: &scheme },
                &mut merged,
                progress,
            )
        })
        .collect();
    let tree = output.with_file_name("cut");
    let styles = config.styles();
    let profiles = pack_profile_table(&config.routing.profiles);
    let mut artifacts = Vec::new();
    for cell in &job.cells {
        progress.check()?;
        let path = tree.join(cell_path(&job.band, cell));
        std::fs::create_dir_all(path.parent().expect("cell path has a parent")).map_err(|e| e.to_string())?;
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut output = std::io::BufWriter::new(file);
        let mut writer = MapWriter::new(&mut output, config.lods.len(), &styles, config.marker_color, cell.square())
            .map_err(|e| e.to_string())?;
        let (mut content, mut dropped) = (false, 0);
        for (lod, spec) in config.lods.iter().enumerate() {
            let encoded = sets.iter().find(|set| set.lod == lod).map(|set| {
                let root = set.cell_tree(*cell, config.chunk_size, progress);
                content |= node_has_features(&root);
                serialize_tree(&root, config.chunk_size)
            });
            let bytes = encoded.as_ref().map(|(index, nodes, chunks, count, _)| LodBytes {
                index,
                nodes: *nodes,
                chunks,
                chunk_count: *count,
            });
            dropped += encoded.as_ref().map_or(0, |encoded| encoded.4);
            writer.lod(config.chunk_size, spec.max_mpp, bytes).map_err(|e| e.to_string())?;
        }
        let poi = empty_poi_section(writer.position());
        let nav = emit_nav_section(writer.position() + poi.len(), &profiles, None, config.routing.profiles.len());
        writer.finish(&poi, &nav, &[], &[]).map_err(|e| e.to_string())?;
        use std::io::Write;
        output.flush().map_err(|e| e.to_string())?;
        artifacts.push((*cell, path, !content && dropped == 0));
    }
    progress.check()?;
    job.finish(output, &artifacts)
}

pub fn node_has_features(node: &Node) -> bool {
    match node {
        Node::Leaf { features, .. } => !features.is_empty(),
        Node::Branch(children) => children.iter().any(node_has_features),
    }
}
