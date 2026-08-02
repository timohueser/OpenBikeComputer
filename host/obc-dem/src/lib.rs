//! `obc-dem` — the source DEM to OBCT terrain cells, deterministically.
//!
//! This crate owns exactly one arrow of the elevation epic (#1068): **Copernicus GLO-30 → OBCT**.
//! It knows nothing about OSM, maps, routes or the catalog. `obc-pack` samples the cells this
//! writes; it never sees a GeoTIFF, which is what keeps libGEOS the last native dependency in the
//! tree (#907) — this crate has none at all.
//!
//! ```text
//! obc-dem fetch --bbox 46.48,8.15,46.73,8.47 --out sources/
//! obc-dem bake  --sources sources/ --bbox 46.48,8.15,46.73,8.47 --out cells/
//! obc-dem bake  --sources sources/ --bbox 46.48,8.15,46.73,8.47 --shard grimsel.obcd
//! ```
//!
//! `fetch` is the only thing here that touches the network, and `bake` never does — a bake is a
//! pure function of a directory of tiles and a bounding box, which is the precondition for the
//! determinism contract below.
//!
//! ## What a bake is, precisely
//!
//! For every OBCT lattice point (`OBCT_Spec.md` §1.1) owned by a cell that intersects the requested
//! box, take a **bilinear point sample of the source surface at that exact coordinate** and quantise
//! it to whole metres. There is no reprojection, no smoothing and no gap filling. The lattice is
//! µdeg-uniform, so the ground spacing is anisotropic (≈ 57 × 39 m at 47 °N) and narrows towards the
//! poles — accepted exactly as OBCA accepts non-square-on-the-ground cells, and for the same reason:
//! the whole addressing contract rests on the lattice being a shift of the coordinate.
//!
//! ## Determinism is a contract, not an aspiration
//!
//! Same tiles + same box ⇒ **byte-identical** output, on any host. Four things make that true, and
//! all four are load-bearing:
//!
//! 1. **The resample is `f64` with a fixed expression order** ([`geotiff::DemMosaic::height`]).
//!    IEEE-754 `+ - * /` are correctly rounded, Rust does not contract them into FMAs, and nothing
//!    here reads a rounding mode — so the arithmetic is a function of its inputs alone.
//! 2. **One stated rounding rule**: half away from zero ([`bake::quantise`]), the same rule
//!    `OBCT_Spec.md` §5.2 pins for the read side, so the producer and the consumer round the same
//!    way and a value never shifts a metre as it crosses the format.
//! 3. **Fixed iteration order everywhere.** Cells are walked row-major over the rectangle, tiles
//!    are loaded in sorted path order, and nothing in the crate iterates a `HashMap`.
//! 4. **Rows are flipped once, on ingest** (`OBCT_Spec.md` §2), so no downstream step has to decide
//!    which way is north.
//!
//! [`bake::bake_cell`] is therefore a pure function of `(mosaic, cell)` and nothing else — which is
//! why a cell baked inside a wide shard is byte-identical to the same cell baked on its own, and
//! why the tests can pin a digest.
//!
//! ## Attribution is a licence obligation
//!
//! [`COPERNICUS_ATTRIBUTION`] must travel with anything derived from GLO-30. `bake` prints it, and
//! the catalog (EL3) and the builder (EL4) carry it onward to a rider. It is a `const` here so
//! there is one copy of the wording in the repository.

/// The producer half — a GeoTIFF decoder and an HTTP client — behind the default `dem` feature.
/// [`container`] stands without them, so the assembler (EL4) can reuse the one OBCT writer from a
/// browser tab; see the `[features]` note in `Cargo.toml`.
#[cfg(feature = "dem")]
pub mod bake;
pub mod container;
#[cfg(feature = "dem")]
pub mod fetch;
#[cfg(feature = "dem")]
pub mod geotiff;

/// The credit the Copernicus DEM licence requires on any product derived from the dataset, verbatim.
///
/// The licence ("Copernicus DEM Instance COP-DEM-GLO-30-F") requires this exact notice wherever the
/// data have been adapted or modified — which a resample to a different lattice certainly is. It is
/// not a courtesy and it is not paraphrasable: EL3 stamps it into the catalog, EL4 surfaces it in
/// the builder, and `obc-dem bake` prints it at the end of every run so an operator producing cells
/// cannot fail to have seen it.
pub const COPERNICUS_ATTRIBUTION: &str = "produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 \
and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union and \
ESA; all rights reserved";

/// The dataset this tool is built for, as the catalog will name it (EL3).
pub const SOURCE_DATASET: &str = "Copernicus DEM GLO-30";

/// A geographic box in integer microdegrees — the unit every OBC coordinate is in, so the box that
/// selects cells is exact rather than a float that nearly lands on a cell boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BboxUdeg {
    pub min_lat: i32,
    pub min_lon: i32,
    pub max_lat: i32,
    pub max_lon: i32,
}

impl BboxUdeg {
    /// Parse `min_lat,min_lon,max_lat,max_lon` in **degrees**.
    ///
    /// Latitude first. That is the opposite of `obc-pack --bbox`, which is `lon,lat,lon,lat`
    /// (Geofabrik/osmium order), and the difference is deliberate rather than accidental: this tool
    /// selects *grid cells*, and every grid expression in the platform — `cell(S, ci, cj)`, the
    /// directory's `(row, col)`, `Cell Min I` / `Cell Min J` — puts latitude first. Getting the two
    /// orders confused mostly produces a box that fails the range checks below; in the Alps, where
    /// both numbers are plausible longitudes, it does not, so the flag is spelled out in every
    /// usage string and in `repack.sh`.
    pub fn parse(text: &str) -> Result<BboxUdeg, String> {
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        let [min_lat, min_lon, max_lat, max_lon] = parts[..] else {
            return Err(format!("bbox `{text}` is not `min_lat,min_lon,max_lat,max_lon`"));
        };
        let deg = |name: &str, text: &str, limit: f64| -> Result<i32, String> {
            let v: f64 = text.parse().map_err(|_| format!("bbox {name}: `{text}` is not a number"))?;
            if !v.is_finite() || v.abs() > limit {
                return Err(format!("bbox {name}: {v} is outside ±{limit}"));
            }
            Ok((v * 1e6).round() as i32)
        };
        let bbox = BboxUdeg {
            min_lat: deg("min_lat", min_lat, 90.0)?,
            min_lon: deg("min_lon", min_lon, 180.0)?,
            max_lat: deg("max_lat", max_lat, 90.0)?,
            max_lon: deg("max_lon", max_lon, 180.0)?,
        };
        if bbox.min_lat >= bbox.max_lat || bbox.min_lon >= bbox.max_lon {
            return Err(format!("bbox `{text}` is empty — expected min_lat,min_lon,max_lat,max_lon"));
        }
        Ok(bbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attribution_is_the_wording_the_licence_names() {
        // Pinned as one line: the `const` is written with continuations, and a stray newline in it
        // would travel into the catalog and the builder.
        assert_eq!(
            COPERNICUS_ATTRIBUTION,
            "produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and \
             Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all \
             rights reserved"
        );
        assert!(!COPERNICUS_ATTRIBUTION.contains('\n'));
    }

    #[test]
    fn a_bbox_is_latitude_first_and_exact_in_microdegrees() {
        let b = BboxUdeg::parse("46.48261,8.15034,46.72070,8.46007").unwrap();
        assert_eq!(b, BboxUdeg { min_lat: 46_482_610, min_lon: 8_150_340, max_lat: 46_720_700, max_lon: 8_460_070 });
        // Whitespace is tolerated; the shape is not.
        assert!(BboxUdeg::parse(" 1, 2, 3, 4 ").is_ok());
        assert!(BboxUdeg::parse("1,2,3").is_err());
        assert!(BboxUdeg::parse("1,2,3,4,5").is_err());
        // An inverted or empty box is a mistake, not an empty result.
        assert!(BboxUdeg::parse("3,2,1,4").is_err());
        assert!(BboxUdeg::parse("1,2,1,4").is_err());
        // A latitude out of range catches most lon,lat,lon,lat mix-ups…
        assert!(BboxUdeg::parse("100,0,110,1").is_err());
        assert!(BboxUdeg::parse("0,-200,1,-190").is_err());
        // …but **not** the Alpine ones, where both numbers are plausible on either axis. Monaco's
        // packer bbox parses here as a box off the coast of Somalia, and nothing about the numbers
        // says otherwise. This is why the flag is spelled out wherever it is written down rather
        // than guarded at parse time — a heuristic that fires on some inputs and not others would
        // be worse than a rule an operator can read.
        assert!(BboxUdeg::parse("7.39,43.71,7.47,43.77").is_ok());
    }
}
