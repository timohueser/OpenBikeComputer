//! Baked cell grids → OBCG objects + their manifest entries.
//!
//! Every emitted object is immediately re-validated through `obc_formats::obcg::validate` — the
//! same fail-closed validator the shared vectors pin — so a baker bug can never publish an
//! object the phone would reject.

use obc_formats::obcg::{self, FrameInput};

use crate::manifest;
use crate::source::BakedProduct;

/// One publishable frame object.
pub struct EmittedFrame {
    pub key: String,
    pub bytes: Vec<u8>,
    pub object_crc32: u32,
    pub entry: manifest::Frame,
}

pub fn emit_product(product: &BakedProduct) -> Result<Vec<EmittedFrame>, String> {
    let geometry = product.geometry;
    geometry.validate()?;
    let mut scratch = vec![0u8; usize::from(geometry.tile_edge) * usize::from(geometry.tile_edge)];
    let mut validate_scratch = vec![0u8; scratch.len()];
    let mut emitted = Vec::with_capacity(product.frames.len());
    for frame in &product.frames {
        if frame.cells.len() != geometry.cells() {
            return Err(format!("{}: frame f{} cell count disagrees with the geometry", product.id, frame.offset_min));
        }
        let input = FrameInput {
            product_id: product.product_code,
            tier: product.tier,
            flags: frame.flags,
            valid_at: frame.valid_at,
            reference_time: product.reference_time,
            south_lat_udeg: geometry.south_lat_udeg,
            west_lon_udeg: geometry.west_lon_udeg,
            cell_lat_udeg: geometry.cell_lat_udeg,
            cell_lon_udeg: geometry.cell_lon_udeg,
            width: geometry.width,
            height: geometry.height,
            cell_size_m: geometry.cell_size_m,
            tile_edge: geometry.tile_edge,
            entries_per_page: geometry.entries_per_page,
            cells: &frame.cells,
        };
        let len = obcg::encoded_len(&input, &mut scratch)
            .map_err(|error| format!("{} f{}: {error:?}", product.id, frame.offset_min))? as usize;
        let mut bytes = vec![0u8; len];
        obcg::encode_format(&input, &mut scratch, &mut bytes)
            .map_err(|error| format!("{} f{}: {error:?}", product.id, frame.offset_min))?;
        let header = obcg::validate(&bytes, &mut validate_scratch)
            .map_err(|error| format!("{} f{}: emitted object failed self-validation: {error:?}", product.id, frame.offset_min))?;
        let key = manifest::frame_key(product.id, product.reference_time, frame.offset_min);
        let entry = manifest::Frame {
            offset_min: frame.offset_min,
            valid_at: manifest::rfc3339(frame.valid_at),
            source_class: if frame.flags & obcg::FLAG_OBSERVED != 0 {
                manifest::SourceClass::Observation
            } else {
                manifest::SourceClass::Forecast
            },
            key: key.clone(),
            bytes: bytes.len() as u64,
            object_crc32: format!("0x{:08X}", header.object_crc32),
            geometry: manifest::FrameGeometry {
                south_udeg: geometry.south_lat_udeg,
                west_udeg: geometry.west_lon_udeg,
                cell_lat_udeg: geometry.cell_lat_udeg,
                cell_lon_udeg: geometry.cell_lon_udeg,
                width: geometry.width,
                height: geometry.height,
                cell_size_m: geometry.cell_size_m,
                tile_edge: geometry.tile_edge,
                entries_per_page: geometry.entries_per_page,
            },
        };
        emitted.push(EmittedFrame { key, bytes, object_crc32: header.object_crc32, entry });
    }
    Ok(emitted)
}

/// The manifest entry for a freshly baked product. The product bbox is the intersection of its
/// frames' windows; with per-product fixed geometry that is the window itself, and a future
/// heterogeneous product (WX6's MRMS+HRRR composition) gets the honest intersection for free.
pub fn product_entry(product: &BakedProduct, frames: Vec<manifest::Frame>, generated_at: i64) -> manifest::Product {
    let mut bbox = manifest::Bbox {
        south_udeg: i64::from(product.geometry.south_lat_udeg),
        west_udeg: i64::from(product.geometry.west_lon_udeg),
        north_udeg: product.geometry.north_lat_udeg(),
        east_udeg: product.geometry.east_lon_udeg(),
    };
    for frame in &frames {
        let geometry = &frame.geometry;
        let north = i64::from(geometry.south_udeg) + i64::from(geometry.height) * i64::from(geometry.cell_lat_udeg);
        let east = i64::from(geometry.west_udeg) + i64::from(geometry.width) * i64::from(geometry.cell_lon_udeg);
        bbox.south_udeg = bbox.south_udeg.max(i64::from(geometry.south_udeg));
        bbox.west_udeg = bbox.west_udeg.max(i64::from(geometry.west_udeg));
        bbox.north_udeg = bbox.north_udeg.min(north);
        bbox.east_udeg = bbox.east_udeg.min(east);
    }
    manifest::Product {
        id: product.id.to_string(),
        tier: product.tier,
        bbox_udeg: bbox,
        cell: manifest::Cell {
            lat_udeg: product.geometry.cell_lat_udeg,
            lon_udeg: product.geometry.cell_lon_udeg,
            nominal_m: product.geometry.cell_size_m,
        },
        reference_time: manifest::rfc3339(product.reference_time),
        generated_at: manifest::rfc3339(generated_at),
        staleness_deadline: manifest::rfc3339(product.staleness_deadline),
        attribution: manifest::AttributionEntry {
            text: product.attribution.text.to_string(),
            url: product.attribution.url.to_string(),
        },
        upstream_etag: product.upstream_etag.clone(),
        frames,
    }
}
