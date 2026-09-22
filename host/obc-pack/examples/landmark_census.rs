//! Measure compiled landmark content through the production OSM join and map encoder.
use obc_formats::obcm::{landmarks, POI_HOURS_REF_NONE};
use obc_pack::{config::Config, ingest::ingest_osm_ways, landmark_map, progress::Progress};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut block = [0; 65536];
    loop {
        let size = file.read(&mut block)?;
        if size == 0 {
            break;
        }
        hash.update(&block[..size]);
    }
    Ok(hex(&hash.finalize()))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        return Err("usage: landmark_census SOURCE.osm.pbf schema.json content.json output.json".into());
    }
    let config = Config::load(&args[1])?;
    let links = {
        let (ingested, _) = ingest_osm_ways(&[args[0].clone()], &config, None, &Progress::stdout())?;
        ingested.landmark_links
    };
    let content = [PathBuf::from(&args[2])];
    let records = landmark_map::load(&content, &links, (-180_000_000, -90_000_000, 180_000_000, 90_000_000))?;
    let hours = vec![POI_HOURS_REF_NONE; records.len()];
    let section = landmark_map::serialize(&records, &hours)?;
    let mut photos: Vec<_> = records.iter().map(|r| r.content[2].len()).filter(|n| *n != 0).collect();
    photos.sort_unstable();
    let quantile = |percent: usize| photos.get((photos.len() * percent).div_ceil(100).saturating_sub(1)).copied();
    let blobs: Vec<_> = (0..4).map(|slot| records.iter().map(|r| r.content[slot].len()).sum::<usize>()).collect();
    let index_bytes =
        if records.is_empty() { 0 } else { landmarks::SECTION_HEADER_LEN + records.len() * landmarks::RECORD_LEN };
    let report = json!({
        "schema": 1,
        "source_sha256": digest(Path::new(&args[0]))?,
        "config_sha256": digest(Path::new(&args[1]))?,
        "content_sha256": digest(Path::new(&args[2]))?,
        "osm_links": links.len(),
        "records": records.len(),
        "linked_records": records.iter().filter(|r| r.record.osm.is_some()).count(),
        "mapped_approaches": records.iter().filter(|r| r.record.osm.and_then(|m| m.approach).is_some()).count(),
        "approaches_by_profile": (0..config.routing.profiles.len()).map(|i| records.iter().filter(|r| r.record.osm.and_then(|m| m.approach).is_some_and(|a| a.profile_mask & (1 << i) != 0)).count()).collect::<Vec<_>>(),
        "photos": photos.len(),
        "raw_photo_bytes": photos.len() * landmarks::PHOTO_PIXELS,
        "compressed_photo_bytes": photos.iter().sum::<usize>(),
        "photo_bytes_min": photos.first(),
        "photo_bytes_p50": quantile(50),
        "photo_bytes_p95": quantile(95),
        "photo_bytes_max": photos.last(),
        "name_articles_photo_credit_bytes_before_dedup": blobs,
        "encoded_directory_bytes": index_bytes,
        "encoded_content_bytes_after_dedup": section.len() - index_bytes,
        "encoded_section_bytes": section.len(),
        "encoded_section_sha256": hex(&Sha256::digest(&section)),
        "scope": "Landmark section only; excludes shared hours pool and non-landmark map sections. This is a source join, not a reachability test.",
        "identities": records.iter().map(|r| json!({
            "qid": r.record.qid,
            "source": r.record.osm.map(|m| m.source.0),
            "profile_mask": r.record.osm.and_then(|m| m.approach).map(|a| a.profile_mask),
            "photo_bytes": r.content[2].len(),
        })).collect::<Vec<_>>(),
    });
    fs::write(&args[3], format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    Ok(())
}
