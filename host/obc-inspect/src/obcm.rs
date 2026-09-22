//! OBCM maps, read through `obc-reader`.
//!
//! The section table is assembled from what the reader's own parse hands over: a LOD's chunk
//! region from its LOD-table row, a POI category's and the nav graph's from their directories, and
//! the terrain, landmark and peak regions from the header fields `obc-formats` names. The style
//! table is the one section whose file offset no reader exposes, so it is reported by record count
//! alone.

use obc_formats::io::ByteSource;
use obc_formats::obcm::PoiCategory;
use obc_reader::{landmarks, peaks, MapCache, MapStyleSet, MapTables, Reader};
use serde_json::Value;

use crate::obct;
use crate::report::{bytes, degrees, Report};

pub fn report(source: &dyn ByteSource) -> Result<Report, String> {
    let tables = MapTables::parse(source).map_err(|error| format!("OBCM header: {error:?}"))?;
    let cache = MapCache::new_boxed();
    let reader = Reader::new(source, &tables, &cache);

    let mut out = Report::new();
    out.put("version", tables.version).put("bytes", bytes(source.len()));
    out.put("offset_unit", tables.scale().unit());
    out.put("light_marker_color", format!("0x{:04x}", tables.marker_color(MapStyleSet::Light)));
    out.put("dark_marker_color", format!("0x{:04x}", tables.marker_color(MapStyleSet::Dark)));

    let mut bbox = Report::new();
    bbox.put("min_lat", degrees(tables.bbox.min_lat as i64))
        .put("min_lon", degrees(tables.bbox.min_lon as i64))
        .put("max_lat", degrees(tables.bbox.max_lat as i64))
        .put("max_lon", degrees(tables.bbox.max_lon as i64));
    out.group("bbox", bbox);

    out.put("styles", tables.styles().iter().flatten().count());
    out.list("lods", lods(&tables));
    out.group("poi", poi(&reader));
    out.group("nav", nav(&reader, &tables));
    out.group("terrain", terrain(source, &tables));
    out.group("landmarks", landmark_section(source));
    out.group("peaks", peak_section(source));
    Ok(out)
}

/// The LOD ladder, coarsest first: where each level's index sits and how much chunk data follows.
fn lods(tables: &MapTables) -> Vec<Report> {
    tables
        .lods()
        .iter()
        .enumerate()
        .map(|(level, lod)| {
            let mut row = Report::new();
            row.put("level", level)
                .put("max_mpp", mpp(lod.max_mpp))
                .put("index_offset", lod.index_offset)
                .put("index_nodes", lod.node_count)
                .put("chunks", lod.chunk_count)
                .put("chunk_capacity", lod.chunk_size)
                .put("chunk_bytes", bytes(u64::from(lod.chunk_units_total) * lod.scale.unit()));
            row
        })
        .collect()
}

/// The coarsest level covers every scale above the next one, which no JSON number can hold.
///
/// Widening the stored `f32` to `f64` would print its binary noise (`1.2` as `1.2000000476837158`),
/// so the value goes through the shortest decimal that round-trips it.
fn mpp(value: f32) -> Value {
    if !value.is_finite() {
        return Value::String("unbounded".to_string());
    }
    Value::from(value.to_string().parse::<f64>().unwrap_or(f64::from(value)))
}

fn poi(reader: &Reader) -> Report {
    let directory = reader.poi_directory();
    let mut out = Report::new();
    out.put("chunk_capacity", directory.chunk_size).put("hours_blobs", directory.hours_pool_count);
    out.list(
        "categories",
        directory
            .entries
            .iter()
            .map(|entry| {
                let mut row = Report::new();
                row.put("category", category_name(entry.category_id))
                    .put("index_offset", entry.index_offset)
                    .put("index_nodes", entry.node_count)
                    .put("chunks", entry.chunk_count)
                    .put("data_start", entry.data_start())
                    .put("data_bytes", bytes((entry.chunk_count * directory.chunk_size) as u64));
                row
            })
            .collect(),
    );
    out
}

/// The §7.4 category name where the table has one; the raw id otherwise (landmarks and
/// settlements ride in the same directory without being service categories).
fn category_name(id: u8) -> String {
    match PoiCategory::from_id(id) {
        Some(category) => format!("{id} {category:?}"),
        None => format!("{id}"),
    }
}

fn nav(reader: &Reader, tables: &MapTables) -> Report {
    let directory = reader.nav_directory();
    let mut out = Report::new();
    out.put("present", tables.has_nav_graph());
    out.put("chunk_capacity", directory.chunk_size)
        .put("index_offset", directory.index_offset)
        .put("index_nodes", directory.node_count)
        .put("node_chunks", directory.chunk_count)
        .put("node_data_start", directory.data_start())
        .put("edge_pool_offset", directory.edge_pool_offset)
        .put("edge_chunks", directory.edge_chunk_count)
        .put("snap_index_offset", directory.snap_index_offset)
        .put("snap_chunks", directory.snap_chunk_count)
        .put("profiles", tables.nav_profiles().iter().map(|profile| profile.name().to_string()).collect::<Vec<_>>());
    out
}

/// §1.3: the map's own window onto an OBCT container, and that container's summary read through it.
fn terrain(source: &dyn ByteSource, tables: &MapTables) -> Report {
    let mut out = Report::new();
    let Some(region) = tables.terrain() else {
        out.put("present", false);
        return out;
    };
    out.put("present", true).put("offset", region.offset).put("length", bytes(region.len));
    match obc_formats::io::WindowSource::new(source, region.offset, region.len) {
        Some(window) => match obct::report(&window) {
            Ok(container) => out.group("container", container),
            Err(damage) => out.put("container", damage),
        },
        None => out.put("container", "the declared region is not inside the file"),
    };
    out
}

fn landmark_section(source: &dyn ByteSource) -> Report {
    let mut out = Report::new();
    match landmarks::map_section(source) {
        Ok(None) => out.put("present", false),
        Ok(Some(window)) => match landmarks::LandmarkDirectory::read(&window) {
            Ok(directory) => out
                .put("present", true)
                .put("offset", window.offset())
                .put("length", bytes(window.len()))
                .put("records", directory.count)
                .put("content_bytes", bytes(u64::from(directory.len - directory.payload))),
            Err(error) => out.put("present", true).put("damage", format!("{error:?}")),
        },
        Err(error) => out.put("present", true).put("damage", format!("{error:?}")),
    };
    out
}

fn peak_section(source: &dyn ByteSource) -> Report {
    let mut out = Report::new();
    match peaks::map_section(source) {
        Ok(None) => out.put("present", false),
        Ok(Some(window)) => match peaks::Directory::read(&window) {
            Ok(directory) => out
                .put("present", true)
                .put("offset", window.offset())
                .put("length", bytes(window.len()))
                .put("records", directory.records)
                .put("associations", directory.associations)
                .put("content_bytes", bytes(u64::from(directory.len - directory.payload))),
            Err(error) => out.put("present", true).put("damage", format!("{error:?}")),
        },
        Err(error) => out.put("present", true).put("damage", format!("{error:?}")),
    };
    out
}
