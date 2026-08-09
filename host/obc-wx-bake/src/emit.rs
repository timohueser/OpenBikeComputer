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
    product.geometry.validate()?;
    let mut emitted = Vec::with_capacity(product.frames.len());
    for frame in &product.frames {
        // A composed product's frames each carry their own lattice and provenance; a
        // single-source product's frames all inherit the product's (see `BakedFrame::source`).
        let geometry = frame.geometry(product);
        geometry.validate()?;
        let mut scratch = vec![0u8; usize::from(geometry.tile_edge) * usize::from(geometry.tile_edge)];
        let mut validate_scratch = vec![0u8; scratch.len()];
        if frame.cells.len() != geometry.cells() {
            return Err(format!("{}: frame f{} cell count disagrees with the geometry", product.id, frame.offset_min));
        }
        // The key's `f<offset-min>` segment and the header's timestamps are one fact: a frame
        // whose offset does not equal `(valid_at - reference_time) / 60` would publish a key
        // that lies about its own header, so it never leaves the emitter.
        if i64::from(frame.offset_min) * 60 != frame.valid_at - product.reference_time {
            return Err(format!(
                "{}: frame f{} offset disagrees with valid_at - reference_time = {} s",
                product.id,
                frame.offset_min,
                frame.valid_at - product.reference_time
            ));
        }
        let input = FrameInput {
            product_id: frame.product_code(product),
            tier: frame.tier(product),
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
        let header = obcg::validate(&bytes, &mut validate_scratch).map_err(|error| {
            format!("{} f{}: emitted object failed self-validation: {error:?}", product.id, frame.offset_min)
        })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::GridGeometry;
    use crate::source::{Attribution, BakedFrame, FrameSource};

    const SMALL: GridGeometry = GridGeometry {
        south_lat_udeg: 20_000_000,
        west_lon_udeg: -130_000_000,
        cell_lat_udeg: 10_000,
        cell_lon_udeg: 10_000,
        width: 40,
        height: 24,
        cell_size_m: 1_000,
        tile_edge: 16,
        entries_per_page: 512,
    };
    const COARSE: GridGeometry = GridGeometry {
        south_lat_udeg: 20_000_000,
        west_lon_udeg: -130_000_000,
        cell_lat_udeg: 30_000,
        cell_lon_udeg: 30_000,
        width: 12,
        height: 8,
        cell_size_m: 3_000,
        tile_edge: 16,
        entries_per_page: 512,
    };
    const REFERENCE: i64 = 1_800_000_000;

    fn product(frames: Vec<BakedFrame>) -> BakedProduct {
        BakedProduct {
            id: "test",
            product_code: obcg::PRODUCT_EXPERIMENTAL,
            tier: obcg::TIER_RADAR,
            geometry: SMALL,
            reference_time: REFERENCE,
            staleness_deadline: REFERENCE + 600,
            attribution: Attribution { text: "test", url: "https://example.invalid" },
            upstream_etag: None,
            frames,
        }
    }

    fn frame(offset_min: u32, valid_at: i64, source: Option<FrameSource>) -> BakedFrame {
        let geometry = source.map_or(SMALL, |source| source.geometry);
        BakedFrame {
            offset_min,
            valid_at,
            flags: obcg::FLAG_FORECAST,
            source,
            cells: vec![obc_formats::precip4::INTENSITY_NODATA; geometry.cells()],
        }
    }

    /// The key's `f<offset-min>` segment and the header's timestamps are one fact: a frame whose
    /// offset does not equal `(valid_at - reference_time) / 60` would publish a key that lies
    /// about its own header, so it never leaves the emitter.
    #[test]
    fn the_offset_gate_refuses_a_key_that_lies_about_its_header() {
        for (offset_min, valid_at) in [
            (15u32, REFERENCE + 16 * 60), // one minute of drift
            (0, REFERENCE + 60),          // an "observation" that is not at the reference
            (60, REFERENCE),              // a lead with no time between it and the reference
            (60, REFERENCE + 30),         // a sub-minute offset the key cannot express
        ] {
            let Err(error) = emit_product(&product(vec![frame(offset_min, valid_at, None)])) else {
                panic!("the offset gate must refuse f{offset_min} at {valid_at}");
            };
            assert!(error.contains("offset disagrees with valid_at - reference_time"), "{error}");
        }
        // The matching case emits, and the key states the same offset.
        let emitted = emit_product(&product(vec![frame(15, REFERENCE + 15 * 60, None)])).expect("consistent frame");
        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].key.ends_with("/f15.obcg"), "{}", emitted[0].key);
        assert_eq!(emitted[0].entry.offset_min, 15);
    }

    /// A composed product's frames each publish their own geometry and provenance, and the
    /// product bbox is the intersection where the whole timeline is answerable.
    #[test]
    fn a_composed_product_emits_per_frame_geometry_and_provenance() {
        let composed = product(vec![
            frame(0, REFERENCE, None),
            frame(
                30,
                REFERENCE + 30 * 60,
                Some(FrameSource { product_code: obcg::PRODUCT_HRRR, tier: obcg::TIER_MODEL, geometry: COARSE }),
            ),
        ]);
        let emitted = emit_product(&composed).expect("composed product emits");
        let mut scratch = vec![0u8; obc_formats::precip4::MAX_CELLS];
        let anchor = obcg::validate(&emitted[0].bytes, &mut scratch).expect("anchor frame is valid");
        let forward = obcg::validate(&emitted[1].bytes, &mut scratch).expect("forward frame is valid");
        assert_eq!((anchor.width, anchor.height, anchor.cell_size_m), (SMALL.width, SMALL.height, SMALL.cell_size_m));
        assert_eq!(
            (forward.width, forward.height, forward.cell_size_m),
            (COARSE.width, COARSE.height, COARSE.cell_size_m)
        );
        assert_eq!(anchor.product_id, obcg::PRODUCT_EXPERIMENTAL);
        assert_eq!(anchor.tier, obcg::TIER_RADAR);
        assert_eq!(forward.product_id, obcg::PRODUCT_HRRR);
        assert_eq!(forward.tier, obcg::TIER_MODEL);
        assert_eq!(emitted[1].entry.geometry.width, COARSE.width);

        let entries: Vec<_> = emitted.iter().map(|frame| frame.entry.clone()).collect();
        let entry = product_entry(&composed, entries, REFERENCE);
        // COARSE is the smaller window in both axes, so it is the intersection.
        assert_eq!(entry.bbox_udeg.north_udeg, COARSE.north_lat_udeg());
        assert_eq!(entry.bbox_udeg.east_udeg, COARSE.east_lon_udeg());
        assert_eq!(entry.bbox_udeg.south_udeg, i64::from(SMALL.south_lat_udeg));
    }
}
