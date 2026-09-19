//! OBCT terrain containers, read through `obc-elevation`.

use obc_elevation::surface::SurfaceReader;
use obc_elevation::TerrainReader;
use obc_formats::io::ByteSource;
use obc_formats::obct;

use crate::report::{degrees, Report};

/// The container summary, used for a standalone `.obcd` shard and for the region a map embeds.
pub fn report(source: &dyn ByteSource) -> Result<Report, String> {
    let reader = TerrainReader::parse(source).map_err(|error| format!("OBCT container: {error:?}"))?;
    let header = reader.header();
    let mut out = Report::new();
    out.put("bytes", source.len());

    // A native v1 container carries one level of heights; an indexed container carries a pyramid,
    // and the surface reader is the one that knows how many levels its posting resolves to.
    let surface = SurfaceReader::parse(source).ok();
    out.put("version", if surface.is_some() { obct::SURFACE_VERSION } else { 1 });
    out.put("indexed", header.flags & obct::CELL_INDEX_FLAG != 0);
    out.put("levels", surface.as_ref().map_or(1, SurfaceReader::level_count));

    let mut grid = Report::new();
    grid.put("cell_min_i", header.cell_min_i)
        .put("cell_min_j", header.cell_min_j)
        .put("cell_rows", header.cell_rows)
        .put("cell_cols", header.cell_cols)
        .put("cell_udeg", 1u32 << header.cell_log2)
        .put("posting_udeg", 1u32 << header.posting_log2)
        .put("tile_samples", obct::TILE_SAMPLES as u32)
        .put("directory_offset", header.directory_offset);
    out.group("grid", grid);

    let (min_lat, min_lon, max_lat, max_lon) = header.bbox_udeg();
    let mut bbox = Report::new();
    bbox.put("min_lat", degrees(min_lat))
        .put("min_lon", degrees(min_lon))
        .put("max_lat", degrees(max_lat))
        .put("max_lon", degrees(max_lon));
    out.group("bbox", bbox);
    Ok(out)
}
