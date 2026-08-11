//! DWD RV composite adapter: Germany's 1 km rain nowcast, tier 1.
//!
//! Upstream is the maintained raw OpenData tar of 25 ODIM HDF5 members (leads 000..120 in
//! five-minute steps). Every member is validated against the WX1-pinned contract before the
//! nine published leads (+0, +15, ..., +120) are selected; the discarded intermediate frames are
//! never interpolated. Reprojection is nearest-neighbour from the pinned polar-stereographic
//! raster straight onto a fixed window of the canonical 0.01 degree lattice — no smoothing, and
//! since [`GEOMETRY`] moved onto that lattice, no second rounding downstream either.

use chrono::NaiveDateTime;
use hdf5_pure::{AttrValue, File as Hdf5File};
use obc_formats::precip4;
use std::collections::HashMap;
use std::io::Read;

use crate::fetch::{FetchOutcome, Upstream};
use crate::geometry::GridGeometry;
use crate::grib::{MAX_COMPRESSED_BYTES, MAX_DECOMPRESSED_BYTES};
use crate::source::{Adapter, Attribution, BakedFrame, BakedSource, SourceClass};
use crate::stereo;

pub const ID: &str = "dwd-rv";
pub const LATEST_URL: &str = "https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_LATEST.tar";

/// The **source window**: a regular lat/lon rectangle over the composite's trapezoid, and — since
/// #1246 deleted the live per-product tree that used to publish on it — **a window of the canonical
/// lattice**. The pitch is [`crate::canonical::CELL_UDEG`] in both axes and the origin is a whole
/// number of canonical cells from [`crate::canonical::CANONICAL`]'s -90/-180 origin, so every cell
/// here *is* a canonical cell: the nearest-neighbour pick `Mosaic::fill` makes for a lattice cell
/// lands on that cell itself, which is what [`crate::source::mrms`] already had. It is an identity
/// resample and not a fast path — `fill` has no alignment branch and wants none; what the alignment
/// buys is positional accuracy, not speed. Cells outside the projected raster are no-data, and the
/// mosaic reads that as "not covered" and falls through to the next-priority source.
///
/// **What the alignment replaced.** The window used to be 9,000 x 14,000 udeg — square ~1 km cells
/// chosen to suit the native raster rather than the lattice — which cost every published German
/// cell a *second* nearest-neighbour hop: the lattice cell picked the nearest window cell, which
/// had itself picked the nearest stereographic cell. Two independent roundings of a ~1 km field put
/// the worst case at ~1 km — 501 m lattice-to-window plus 500 m window-to-native — on a source
/// whose whole value is that it resolves a kilometre. On the lattice there is one rounding and only
/// one, so the worst case halves to 500 m: [`source_index_map`] projects the lattice cell's own
/// centre through [`stereo::native_index`], and nothing downstream rounds again. A lattice centre
/// can no longer land on a window cell boundary either — it is offset half a cell from one by
/// construction — so the half-open edge rule `canonical::source_column` documents cannot bite here.
///
/// The extent covers everything the old window did and a shade more, because it is the old extent
/// rounded **outwards** onto the lattice and never inwards: north 55.868 -> 55.87 N, east
/// 18.736 -> 18.74 E, with the south-west corner already on it.
///
/// What this rectangle does **not** take in — and the old one did not either — is the raster's
/// north bulge. A polar-stereographic north edge curves poleward away from its corners, so the
/// frame reaches 56.219 N at 10 E against the 55.862 N its UL corner reports. The strip above the
/// top row's centre is a crescent from 1.565 E to 18.444 E, ~0.35 degrees thick in the middle and
/// tapering to nothing at both ends: **29,083 native cells, 2.20 %** of the raster, all of it
/// Denmark, the North Sea and the western Baltic — no German ground, which tops out at 55.058 N.
/// Those cells are answered by the next-priority source. This window clips 166 fewer of them than
/// the old one did, incidentally rather than by design; taking the bulge in outright costs 35 more
/// rows (`height: 1_054`, +3.4 %) and is a **coverage** decision rather than an alignment one, so
/// it is not made here.
///
/// `cell_size_m` stays the source's nominal native resolution — 1 km, the figure MRMS states on the
/// identical pitch — and is descriptive here; a published frame states
/// [`crate::canonical::LATTICE_CELL_SIZE_M`] instead.
pub const GEOMETRY: GridGeometry = GridGeometry {
    south_lat_udeg: 45_680_000,
    west_lon_udeg: 1_460_000,
    cell_lat_udeg: 10_000,
    cell_lon_udeg: 10_000,
    width: 1_728,
    height: 1_019,
    cell_size_m: 1_000,
    tile_edge: 32,
    entries_per_page: 512,
};

/// Published leads in minutes: the epic's nine 15-minute frames through +2 h.
pub const LEADS_MIN: [u32; 9] = [0, 15, 30, 45, 60, 75, 90, 105, 120];
/// A radar run refreshes every five minutes; half an hour without a fresh one is the epic's
/// stuck-baker detection horizon, so the product must not outlive it.
pub const STALENESS_SECONDS: i64 = 30 * 60;

pub const ATTRIBUTION: Attribution = Attribution {
    text: "Source: Deutscher Wetterdienst (DWD), radar composite RV; modified/quantized by OpenBikeComputer",
    url: "https://www.dwd.de/EN/service/copyright/copyright_artikel.html",
};

// The WX1-pinned member contract.
const NATIVE_SHAPE: [u64; 2] = [1_200, 1_100];
const GAIN: f64 = 0.000_999_999_931_780_621_3;
const OFFSET: f64 = -0.000_999_999_931_780_621_3;
const NODATA: u64 = 4_294_967_295;
const UNDETECT: u64 = 0;
const CORNERS: [(&str, f64); 8] = [
    ("LL_lon", 3.566_994_635_007_891_4),
    ("LL_lat", 45.696_425_377_390_064),
    ("UL_lon", 1.463_301_510_256_666),
    ("UL_lat", 55.862_087_108_249_824),
    ("UR_lon", 18.731_616_454_667_47),
    ("UR_lat", 55.845_438_563_255_755),
    ("LR_lon", 16.580_869_348_598_274),
    ("LR_lat", 45.684_605_781_370_82),
];

pub struct DwdRv;

impl Adapter for DwdRv {
    fn id(&self) -> &'static str {
        ID
    }

    fn bake(&self, upstream: &mut dyn Upstream, _now: i64, _warnings: &mut Vec<String>) -> Result<BakedSource, String> {
        // No conditional request and no ETag short-circuit: the mosaic needs this source's cells
        // every cycle, so "unchanged" would only save a download it has to do anyway (#1246).
        let fetched = match upstream.fetch(LATEST_URL, MAX_COMPRESSED_BYTES, None)? {
            FetchOutcome::Unchanged => return Err("DWD RV LATEST returned 304 without a validator".into()),
            FetchOutcome::Body(fetched) => fetched,
        };
        let (run, frames) = bake_tar(&fetched.bytes)?;
        // DWD RV is itself a nowcast: its +5 … +120 members are the DWD's own advection scheme,
        // so WXR9 adds nothing here and asks for no motion history.
        Ok(BakedSource {
            id: ID,
            geometry: GEOMETRY,
            reference_time: run,
            attribution: ATTRIBUTION,
            frames,
            motion_history: Vec::new(),
        })
    }
}

/// Validate a complete RV tar and bake the nine published leads. Public for the fixture tests.
pub fn bake_tar(tar_bytes: &[u8]) -> Result<(i64, Vec<BakedFrame>), String> {
    GEOMETRY.validate()?;
    if tar_bytes.is_empty() || tar_bytes.len() as u64 > MAX_COMPRESSED_BYTES {
        return Err("DWD RV tar size is outside the WX1 limits".into());
    }
    let index_map = source_index_map();
    let mut archive = tar::Archive::new(tar_bytes);
    let mut run_time: Option<i64> = None;
    let mut member_count = 0usize;
    let mut frames: Vec<BakedFrame> = Vec::new();
    for entry in archive.entries().map_err(|error| format!("DWD RV tar: {error}"))? {
        let entry = entry.map_err(|error| format!("DWD RV tar: {error}"))?;
        if entry.size() > MAX_DECOMPRESSED_BYTES {
            return Err("DWD RV HDF5 member exceeds the WX1 limit".into());
        }
        let path = entry.path().map_err(|error| format!("DWD RV tar member: {error}"))?;
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or("DWD RV tar member has a non-UTF-8 or empty name")?
            .to_string();
        let (member_run, lead_minutes) = parse_member_name(&name)?;
        if run_time.is_none() {
            run_time = Some(member_run);
        }
        let expected_lead = u32::try_from(member_count).unwrap() * 5;
        if run_time != Some(member_run) || lead_minutes != expected_lead {
            return Err(format!("DWD RV tar member {name} has the wrong run/lead"));
        }
        let mut bytes = Vec::new();
        entry
            .take(MAX_DECOMPRESSED_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("DWD RV member {name}: {error}"))?;
        if bytes.len() as u64 > MAX_DECOMPRESSED_BYTES {
            return Err("DWD RV HDF5 member exceeds the WX1 limit".into());
        }
        let member = validate_member(bytes, member_run, lead_minutes)?;
        // Every member is validated; only the nine published leads are baked.
        if LEADS_MIN.contains(&lead_minutes) {
            frames.push(BakedFrame {
                offset_min: lead_minutes,
                valid_at: member_run + i64::from(lead_minutes) * 60,
                class: if lead_minutes == 0 { SourceClass::Observation } else { SourceClass::Forecast },
                cells: resample(&member, &index_map),
            });
        }
        member_count += 1;
    }
    if member_count != 25 {
        return Err(format!("DWD RV tar contains {member_count} members; expected leads 000..120 every 5 minutes"));
    }
    if frames.len() != LEADS_MIN.len() {
        return Err("DWD RV tar is missing a published lead".into());
    }
    Ok((run_time.expect("25 members imply a run"), frames))
}

/// The per-cycle nearest-neighbour map: for every cell of the window, the native raster index (or
/// `u32::MAX` outside the projected frame). One trigonometric pass, shared by all nine frames.
///
/// Since [`GEOMETRY`] is a window of the canonical lattice, the centre this projects is the
/// **lattice** cell's own centre, and this is therefore the only rounding standing between the
/// stereographic raster and a published German cell. Two tests in `canonical_mosaic.rs` keep it
/// that way — `every_published_cell_equals_the_quantized_nearest_neighbour_of_the_winning_source`
/// re-derives the equality of centres at every cell it samples, and
/// `the_dwd_window_is_a_window_of_the_canonical_lattice` states it against [`GEOMETRY`] itself — so
/// a window that drifted off the lattice, reintroducing the second hop this pass exists without,
/// fails a test rather than quietly blurring Germany.
fn source_index_map() -> Vec<u32> {
    let mut map = vec![u32::MAX; GEOMETRY.cells()];
    for row in 0..GEOMETRY.height {
        let lat = GEOMETRY.center_lat_deg(row);
        for col in 0..GEOMETRY.width {
            let lon = GEOMETRY.center_lon_deg(col);
            if let Some(index) = stereo::native_index(lat, lon) {
                map[(row * GEOMETRY.width + col) as usize] = index as u32;
            }
        }
    }
    map
}

struct Member {
    raw: Vec<u32>,
}

fn resample(member: &Member, index_map: &[u32]) -> Vec<u8> {
    index_map
        .iter()
        .map(|&source| {
            if source == u32::MAX {
                return precip4::INTENSITY_NODATA;
            }
            let encoded = u64::from(member.raw[source as usize]);
            if encoded == NODATA {
                precip4::INTENSITY_NODATA
            } else if encoded == UNDETECT {
                precip4::INTENSITY_DRY
            } else {
                // mm per 5 minutes -> mm/h. `encoded * gain + offset` per the ODIM scale attrs.
                let mm_5min = encoded as f64 * GAIN + OFFSET;
                precip4::quantize_rate_mm_per_hour(mm_5min * 12.0)
            }
        })
        .collect()
}

/// Full WX1 member validation: geometry, projection, scale, timing identity, value sanity.
fn validate_member(bytes: Vec<u8>, run_time: i64, lead_minutes: u32) -> Result<Member, String> {
    let file = Hdf5File::from_bytes(bytes).map_err(|error| format!("DWD RV HDF5: {error}"))?;
    let dataset = file.dataset("dataset1/data1/data").map_err(|error| format!("DWD RV dataset: {error}"))?;
    let shape = dataset.shape().map_err(|error| format!("DWD RV shape: {error}"))?;
    if shape.as_slice() != NATIVE_SHAPE {
        return Err("DWD RV raster is no longer the 1200x1100 native grid".into());
    }
    let attrs =
        |path: &str| file.group(path).and_then(|group| group.attrs()).map_err(|error| format!("{path}: {error}"));
    let data_attrs = attrs("dataset1/data1/what")?;
    let where_attrs = attrs("where")?;
    let root_what = attrs("what")?;
    let dataset_what = attrs("dataset1/what")?;
    if attr_string(&data_attrs, "quantity")? != "ACRR" {
        return Err("DWD quantity is not ACRR".into());
    }
    if attr_f64(&where_attrs, "xscale")? != 1_000.0
        || attr_f64(&where_attrs, "yscale")? != 1_000.0
        || attr_u64(&where_attrs, "xsize")? != u64::from(stereo::NATIVE_COLS)
        || attr_u64(&where_attrs, "ysize")? != u64::from(stereo::NATIVE_ROWS)
        || CORNERS.iter().any(|(name, expected)| attr_f64(&where_attrs, name) != Ok(*expected))
    {
        return Err("DWD RV native grid is no longer the pinned 1 km registration".into());
    }
    if attr_string(&where_attrs, "projdef")? != stereo::DWD_RV_PROJDEF {
        return Err("DWD RV stereographic projection changed".into());
    }
    if attr_f64(&data_attrs, "gain")? != GAIN
        || attr_f64(&data_attrs, "offset")? != OFFSET
        || attr_u64(&data_attrs, "nodata")? != NODATA
        || attr_u64(&data_attrs, "undetect")? != UNDETECT
    {
        return Err("DWD RV scale or missing-value contract changed".into());
    }
    let reference = parse_odim_datetime(&root_what, "date", "time")?;
    let valid_start = parse_odim_datetime(&dataset_what, "startdate", "starttime")?;
    let valid_end = parse_odim_datetime(&dataset_what, "enddate", "endtime")?;
    if valid_end.checked_sub(valid_start) != Some(300) {
        return Err("DWD RV ODIM interval is not exactly five minutes".into());
    }
    let expected_end = run_time + i64::from(lead_minutes) * 60;
    if reference != run_time || valid_end != expected_end {
        return Err("DWD RV member name and internal ODIM times disagree".into());
    }
    let raw = dataset.read_u32().map_err(|error| format!("DWD RV raster: {error}"))?;
    if raw.len() != (NATIVE_SHAPE[0] * NATIVE_SHAPE[1]) as usize {
        return Err("DWD RV raster length disagrees with its shape".into());
    }
    for &encoded in &raw {
        let encoded = u64::from(encoded);
        if encoded == NODATA || encoded == UNDETECT {
            continue;
        }
        let value = encoded as f64 * GAIN + OFFSET;
        if !value.is_finite() || value < 0.0 {
            return Err("DWD RV contains invalid scaled precipitation".into());
        }
    }
    Ok(Member { raw })
}

fn parse_member_name(name: &str) -> Result<(i64, u32), String> {
    let identity = name
        .strip_prefix("composite_rv_")
        .and_then(|value| value.strip_suffix("-hd5"))
        .ok_or("DWD RV tar member name is outside the source contract")?;
    let parts: Vec<_> = identity.split('_').collect();
    if parts.len() != 3 || parts[0].len() != 8 || parts[1].len() != 4 || parts[2].len() != 3 {
        return Err("DWD RV tar member date/time/lead shape changed".into());
    }
    let run = NaiveDateTime::parse_from_str(&format!("{}{}00", parts[0], parts[1]), "%Y%m%d%H%M%S")
        .map_err(|error| format!("DWD RV member run time: {error}"))?
        .and_utc()
        .timestamp();
    let lead = parts[2].parse::<u32>().map_err(|error| format!("DWD RV member lead: {error}"))?;
    if lead > 120 || lead % 5 != 0 {
        return Err("DWD RV tar member lead is outside 000..120/5m".into());
    }
    Ok((run, lead))
}

fn parse_odim_datetime(attrs: &HashMap<String, AttrValue>, date: &str, time: &str) -> Result<i64, String> {
    let compact = format!("{}{}", attr_string(attrs, date)?, attr_string(attrs, time)?);
    Ok(NaiveDateTime::parse_from_str(&compact, "%Y%m%d%H%M%S")
        .map_err(|error| format!("ODIM {date}/{time}: {error}"))?
        .and_utc()
        .timestamp())
}

fn attr_string(attrs: &HashMap<String, AttrValue>, name: &str) -> Result<String, String> {
    attrs
        .get(name)
        .and_then(AttrValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("attribute {name} is not a scalar string"))
}

fn attr_f64(attrs: &HashMap<String, AttrValue>, name: &str) -> Result<f64, String> {
    attrs.get(name).and_then(AttrValue::as_f64).ok_or_else(|| format!("attribute {name} is not a scalar number"))
}

fn attr_u64(attrs: &HashMap<String, AttrValue>, name: &str) -> Result<u64, String> {
    let value = attrs.get(name).ok_or_else(|| format!("attribute {name} is absent"))?;
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    if let Some(value) = value.as_f64() {
        if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64 {
            return Ok(value as u64);
        }
    }
    Err(format!("attribute {name} is not a scalar unsigned integer"))
}
