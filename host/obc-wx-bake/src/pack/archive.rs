//! Where a *past* upstream object actually lives.
//!
//! The baker only ever knows the live key schema. For MRMS that schema points at
//! `noaa-mrms-pds`, a bucket with a short retention window — perfect for the operational service,
//! useless for capturing an event from 2020. This module is the one place that knows the
//! historical mirrors, and it is deliberately a *pure URL rewrite*: the bytes an archive serves
//! are byte-identical to what the live bucket served, so a captured member replays under its
//! canonical URL and the baker never learns an archive exists.
//!
//! Verified working anonymously on 2026-08-10:
//!
//! | live source | archive | reach |
//! |---|---|---|
//! | MRMS `PrecipRate` | Iowa State MTArchive | back to Oct 2014, full 2-minute cadence |
//! | HRRR subhourly | the same `noaa-hrrr-bdp-pds` bucket | full archive; no rewrite needed |
//!
//! Deliberately **not** here yet (they are later steps, and saying so beats a silent surprise):
//! OPERA/CIRRUS on CloudFerro (needs the COG adapter, step 3) and DWD RADOLAN YW
//! (`opendata.dwd.de/.../5_minutes/radolan/recent/bin/YW-YYMMDD.tar.gz`, ~Feb 2025 onward) — YW
//! is an *observation* product in a different container from the operational RV composite the
//! `dwd_rv` adapter reads, so it needs its own adapter, not a URL rewrite.

use crate::source::{hrrr, mrms, us, NOAA_TERMS_URL};

/// Iowa State University's MTArchive: the long-history MRMS mirror.
pub const MTARCHIVE: &str = "https://mtarchive.geol.iastate.edu";

pub const NOAA_LICENCE: &str = "NOAA Open Data Dissemination — public-use U.S. government data, no endorsement implied";

/// The adapters an event pack can capture today. Anything else fails loudly rather than
/// producing a pack whose `upstream/` cannot be replayed.
pub const SUPPORTED_ADAPTERS: [&str; 1] = [us::ID];

/// When MRMS `PrecipRate` becomes fetchable, relative to the observation instant its key names.
///
/// Measured against the live bucket on 2026-08-10 (`Last-Modified` on *current* objects is the
/// real publication time; only the 2014-2021 backfills carry a mirror's ingest time and are
/// useless): 11:40 → +2:49, 12:00 → +2:52, 12:18 → +3:01, 12:24 → +2:58. `mrms.rs` cites WX1's
/// 2 min 44 s, which is the fast end of that spread. **The constant here rounds up, on purpose**:
/// treating an object as unpublished a little too long only makes a capture more conservative,
/// while treating it as published too early is exactly the failure this guard exists to prevent.
pub const MRMS_PUBLICATION_LAG_SECONDS: i64 = 180;

/// When an HRRR subhourly *run* becomes usable, relative to its run hour.
///
/// One constant for the whole run rather than a per-file table, because
/// [`crate::source::hrrr::select_run`] requires all four `wrfsubhf` objects — only the slowest
/// matters. Measured on 2026-08-09/10: the 11Z set landed at +53:38, +55:49, +56:51, +58:53, and
/// 06Z/00Z at +55:56/+55:18. `hrrr.rs` assumes objects appear in lead order, and mostly they do —
/// but 2026-08-09's 18Z run wrote `wrfsubhf01`'s index at **+62:21**, *after* `wrfsubhf04`'s
/// +55:45, so the set was genuinely incomplete until past the hour. 65 minutes is the conservative
/// ceiling over everything observed.
pub const HRRR_RUN_COMPLETE_LAG_SECONDS: i64 = 65 * 60;

/// The instant `url`'s bytes first became fetchable, derived from the key alone.
///
/// This is the whole basis of the as-of guard ([`crate::pack::capture::AsOf`]). It needs no
/// response header — which matters, because headers cannot answer it: MTArchive reports its own
/// 2020-08-11 ingest time for a 2020-08-10 object, and NOAA's HRRR bucket reports a 2021
/// re-upload. The key, by contrast, states the observation instant or the run hour outright, and
/// the lag from that to publication is a measured property of the source.
pub fn published_at(url: &str) -> Result<i64, String> {
    if let Some(rest) = url.strip_prefix(mrms::BUCKET) {
        return Ok(mrms_valid_at(rest)? + MRMS_PUBLICATION_LAG_SECONDS);
    }
    if let Some(rest) = url.strip_prefix(hrrr::BUCKET) {
        return Ok(hrrr_run(rest)? + HRRR_RUN_COMPLETE_LAG_SECONDS);
    }
    Err(format!("cannot derive a publication instant for {url}"))
}

/// The observation instant an MRMS key names.
fn mrms_valid_at(key: &str) -> Result<i64, String> {
    let name = key.rsplit('/').next().unwrap_or_default();
    let stamp = name
        .strip_prefix("MRMS_PrecipRate_00.00_")
        .and_then(|rest| rest.split('.').next())
        .ok_or_else(|| format!("unexpected MRMS object name {name}"))?;
    let (date, time) = stamp.split_once('-').ok_or_else(|| format!("unexpected MRMS timestamp {stamp}"))?;
    let time = chrono::NaiveDateTime::parse_from_str(&format!("{date}{time}"), "%Y%m%d%H%M%S")
        .map_err(|error| format!("unexpected MRMS timestamp {stamp}: {error}"))?;
    Ok(time.and_utc().timestamp())
}

/// The run hour an HRRR key names: `/hrrr.YYYYMMDD/conus/hrrr.tHHz.wrfsubhfFF.grib2[.idx]`.
fn hrrr_run(key: &str) -> Result<i64, String> {
    let date = key
        .strip_prefix("/hrrr.")
        .and_then(|rest| rest.split('/').next())
        .ok_or_else(|| format!("unexpected HRRR key {key}"))?;
    let name = key.rsplit('/').next().unwrap_or_default();
    let hour = name
        .strip_prefix("hrrr.t")
        .and_then(|rest| rest.split('z').next())
        .filter(|hour| hour.len() == 2)
        .ok_or_else(|| format!("unexpected HRRR object name {name}"))?;
    // The run hour is the whole of the timestamp; chrono wants a complete one, so state the
    // zero minutes and seconds explicitly rather than leaving them to be inferred.
    let time = chrono::NaiveDateTime::parse_from_str(&format!("{date}{hour}0000"), "%Y%m%d%H%M%S")
        .map_err(|error| format!("unexpected HRRR run {date}/{hour}: {error}"))?;
    Ok(time.and_utc().timestamp())
}

/// The licence and attribution URL that govern `url`'s bytes.
pub fn terms(url: &str) -> Result<(&'static str, &'static str), String> {
    if url.starts_with(mrms::BUCKET) || url.starts_with(hrrr::BUCKET) {
        return Ok((NOAA_LICENCE, NOAA_TERMS_URL));
    }
    Err(format!("no licence record for {url} — add one before capturing it"))
}

/// Rewrite a canonical upstream URL into the archive URL that still serves it.
///
/// HRRR needs no rewrite: NOAA's Big Data bucket *is* the archive. MRMS does, and the rewrite is
/// mechanical — the key's own date and timestamp segments are re-laid-out into MTArchive's tree.
pub fn archive_url(url: &str) -> Result<String, String> {
    if let Some(rest) = url.strip_prefix(mrms::BUCKET) {
        return mrms_archive_url(rest);
    }
    if url.starts_with(hrrr::BUCKET) {
        // The NOAA HRRR bucket keeps every run; the live key is the archive key.
        return Ok(url.to_string());
    }
    Err(format!("no historical archive is wired up for {url} (capture supports {})", SUPPORTED_ADAPTERS.join(", ")))
}

/// `/CONUS/PrecipRate_00.00/YYYYMMDD/MRMS_PrecipRate_00.00_YYYYMMDD-HHMMSS.grib2.gz`
/// → `/YYYY/MM/DD/mrms/ncep/PrecipRate/PrecipRate_00.00_YYYYMMDD-HHMMSS.grib2.gz`
///
/// MTArchive drops the `MRMS_` prefix the S3 keys carry. The bytes behind both names are the same
/// object; the WX1 fixture discipline (retrieve independently, byte-compare) is how that claim is
/// kept honest, and the pack's per-member sha256 is where the proof lands.
fn mrms_archive_url(key: &str) -> Result<String, String> {
    const PREFIX: &str = "/CONUS/PrecipRate_00.00/";
    let rest = key.strip_prefix(PREFIX).ok_or_else(|| format!("unexpected MRMS key {key}"))?;
    let (date, file) = rest.split_once('/').ok_or_else(|| format!("unexpected MRMS key {key}"))?;
    if date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("unexpected MRMS date segment {date:?}"));
    }
    let name = file.strip_prefix("MRMS_").ok_or_else(|| format!("unexpected MRMS object name {file}"))?;
    Ok(format!("{MTARCHIVE}/{}/{}/{}/mrms/ncep/PrecipRate/{name}", &date[0..4], &date[4..6], &date[6..8]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_mrms_key_rewrites_into_the_mtarchive_tree() {
        let live = mrms::object_url(crate::manifest::parse_rfc3339("2020-08-10T18:52:00Z").unwrap());
        assert_eq!(
            live,
            "https://noaa-mrms-pds.s3.amazonaws.com/CONUS/PrecipRate_00.00/20200810/MRMS_PrecipRate_00.00_20200810-185200.grib2.gz"
        );
        assert_eq!(
            archive_url(&live).unwrap(),
            "https://mtarchive.geol.iastate.edu/2020/08/10/mrms/ncep/PrecipRate/PrecipRate_00.00_20200810-185200.grib2.gz"
        );
    }

    #[test]
    fn the_hrrr_bucket_is_its_own_archive() {
        let run = crate::manifest::parse_rfc3339("2020-08-10T18:00:00Z").unwrap();
        for url in [hrrr::object_url(run, 2), hrrr::index_url(run, 2)] {
            assert_eq!(archive_url(&url).unwrap(), url);
        }
    }

    /// A source with no wired-up archive must fail loudly at capture time, not produce a pack
    /// whose `upstream/` silently cannot be replayed.
    #[test]
    fn an_unmapped_source_is_refused_rather_than_guessed() {
        for url in [
            "https://opendata.dwd.de/weather/radar/composite/rv/composite_rv_20260809_1420.tar",
            "https://s3.waw3-1.cloudferro.com/openradar-archive/2020/08/10/OPERA/COMP/OPERA@20200810T1852@0@DBZH.tiff",
            "https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.20200810/18/atmos/gfs.t18z.pgrb2.0p25.f001",
        ] {
            assert!(archive_url(url).is_err(), "{url} must be refused");
            assert!(terms(url).is_err(), "{url} must have no licence record");
        }
    }

    /// The publication instant comes out of the key, and it is the number that decides what a
    /// capture is allowed to see.
    #[test]
    fn publication_instants_come_out_of_the_key() {
        let observation = crate::manifest::parse_rfc3339("2020-08-10T18:52:00Z").unwrap();
        assert_eq!(published_at(&mrms::object_url(observation)).unwrap(), observation + MRMS_PUBLICATION_LAG_SECONDS);
        let run = crate::manifest::parse_rfc3339("2020-08-10T18:00:00Z").unwrap();
        for url in [hrrr::object_url(run, 1), hrrr::index_url(run, 4)] {
            assert_eq!(published_at(&url).unwrap(), run + HRRR_RUN_COMPLETE_LAG_SECONDS, "{url}");
        }
        // The measured cases the shipped pack turns on: at 18:52:00Z the newest legal MRMS
        // observation is 18:48, and the 18Z HRRR run is still incomplete.
        let at = crate::manifest::parse_rfc3339("2020-08-10T18:52:00Z").unwrap();
        assert!(published_at(&mrms::object_url(observation)).unwrap() > at);
        assert!(published_at(&mrms::object_url(observation - 120)).unwrap() > at, "18:50 is not published either");
        assert!(published_at(&mrms::object_url(observation - 240)).unwrap() <= at, "18:48 is");
        assert!(published_at(&hrrr::index_url(run, 4)).unwrap() > at, "the 18Z run is not complete at 18:52");
        assert!(published_at(&hrrr::index_url(run - 3_600, 4)).unwrap() <= at, "the 17Z run is");
    }

    #[test]
    fn an_underivable_publication_instant_is_an_error() {
        assert!(published_at("https://example.invalid/whatever").is_err());
        assert!(published_at(&format!("{}/CONUS/PrecipRate_00.00/x/y.grib2.gz", mrms::BUCKET)).is_err());
        assert!(published_at(&format!("{}/hrrr.nonsense/conus/hrrr.tXXz.wrfsubhf01.grib2", hrrr::BUCKET)).is_err());
    }

    #[test]
    fn noaa_members_carry_the_noaa_terms() {
        let (licence, url) = terms(&mrms::object_url(1_800_000_000)).unwrap();
        assert!(licence.contains("no endorsement implied"));
        assert_eq!(url, NOAA_TERMS_URL);
    }
}
