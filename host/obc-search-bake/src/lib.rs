mod areas;
mod countries;
mod enrich;
mod geometry;
mod input;
mod interpolation;
mod places;
mod policy;
mod postcodes;

use enrich::{house_addresses, Index};
use osmpbfreader::OsmId;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    path::Path,
    time::Instant,
};

fn hash(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut source = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = source.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn bake(
    osm: &Path,
    output: &Path,
    country: &str,
    country_grid: Option<&Path>,
    policy_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    if output.exists() {
        return Err("The output file must not exist".into());
    }
    let policy = policy::Policy::read(policy_path)?;
    if !policy.has_country(country) {
        return Err("Default country is absent from the policy".into());
    }
    let policy_hash = hash(policy_path)?;
    let osm_hash = hash(osm)?;
    let grid_hash = country_grid.map(hash).transpose()?;
    let input = input::read(osm)?;
    eprintln!(
        "Read {} nodes, {} features; {} incomplete geometries",
        input.nodes,
        input.features.len(),
        input.incomplete_geometries
    );
    let grid = countries::Countries::read(country_grid, &input)?;
    let countries: Vec<_> =
        input.features.iter().map(|f| grid.at(geometry::center(&f.geometry)).unwrap_or(country)).collect();
    eprintln!("Resolved country polygons; build address indexes");
    let index = Index::new(&input, &countries, &policy);
    eprintln!("Write address records");
    let count = write_dump(output, |mut stream| {
        serde_json::to_writer(
            &mut stream,
            &json!({"type":"NominatimDumpFile","content":{
                "generator":"obc-search-bake","scope":"all","osm_sha256":osm_hash,"data_timestamp":null,
                "default_country":country,"country_grid_sha256":grid_hash,"policy_sha256":policy_hash
            }}),
        )?;
        writeln!(stream)?;
        let mut count = 0;
        for (i, f) in input
            .features
            .iter()
            .enumerate()
            .filter(|(_, f)| f.road() && !f.name().is_empty())
            .chain(input.features.iter().enumerate().filter(|(_, f)| !f.road()))
        {
            let tags = index.tags(i);
            let houses = house_addresses(tags);
            if !f.road() && houses.is_empty() {
                continue;
            }
            let (a, country) = index.address(i);
            if !f.road() && a.get("street").is_none_or(|s| s.is_empty()) {
                continue;
            }
            let p = index.center(i);
            let extent = geometry::envelope(&f.geometry);
            let (object_type, object_id) = match f.source {
                OsmId::Node(n) => ("N", n.0),
                OsmId::Way(w) => ("W", w.0),
                OsmId::Relation(r) => ("R", r.0),
            };
            let name: std::collections::BTreeMap<_, _> = f
                .tags
                .iter()
                .filter(|(k, _)| {
                    k.as_str() == "name"
                        || k.starts_with("name:")
                        || matches!(k.as_str(), "alt_name" | "loc_name" | "int_name" | "short_name" | "official_name")
                })
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let mut record = json!({"object_type":object_type,"object_id":object_id,"osm_key":if f.road() { "highway" } else { "building" },
                "osm_value":if f.road() { f.tag("highway") } else { "yes" },"address_type":if f.road() { "street" } else { "house" },
                "country_code":country,"centroid":[p.x(),p.y()],"bbox":[extent.lower()[0],extent.lower()[1],extent.upper()[0],extent.upper()[1]],
                "name":if f.road() { json!(name) } else { json!({}) },"address":a,"postcode":a.get("postcode").map(|s| s.as_str()).unwrap_or(""),"importance":0.05});
            if houses.is_empty() {
                serde_json::to_writer(&mut stream, &json!({"type":"Place","content":[record]}))?;
                writeln!(stream)?;
                count += 1;
            } else {
                for (house, street) in houses {
                    record["housenumber"] = json!(house);
                    if let (geo::Geometry::LineString(line), Some(range)) =
                        (&f.geometry, f.tags.get("_interpolation_range"))
                    {
                        if let Some((first, last)) = range.split_once(':') {
                            let (first, last, number) =
                                (first.parse::<f64>()?, last.parse::<f64>()?, house.parse::<f64>()?);
                            let p = interpolation::point(line, (number - first) / (last - first));
                            record["centroid"] = json!([p.x(), p.y()]);
                        }
                    }
                    record["address"] = json!(&a);
                    if let Some(street) = street {
                        record["address"]["street"] = json!(street);
                    }
                    serde_json::to_writer(&mut stream, &json!({"type":"Place","content":[&record]}))?;
                    writeln!(stream)?;
                    count += 1;
                }
            }
        }
        eprintln!("Wrote {count} address records; write POIs and localities");
        let mut emitted = std::collections::BTreeSet::new();
        for (i, f) in input.features.iter().enumerate() {
            if let Some(record) = places::record(f, i, &index) {
                if emitted.insert(f.source) {
                    serde_json::to_writer(&mut stream, &json!({"type":"Place","content":[record]}))?;
                    writeln!(stream)?;
                    count += 1;
                }
            }
        }
        Ok(count)
    })?;
    println!(
        "{}",
        json!({"records":count,"seconds":start.elapsed().as_secs_f64(),"osm_sha256":osm_hash,"incomplete_geometries":input.incomplete_geometries})
    );
    Ok(())
}

fn write_dump(
    output: &Path,
    write: impl FnOnce(&mut dyn Write) -> Result<usize, Box<dyn std::error::Error>>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut stream = zstd::stream::Encoder::new(BufWriter::new(temporary.as_file_mut()), 3)?;
    let count = write(&mut stream)?;
    stream.finish()?.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(output)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_complete_before_publication_and_never_overwrites_a_previous_bake() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("addresses.jsonl.zst");
        assert!(write_dump(&output, |stream| {
            stream.write_all(b"partial")?;
            Err("input failed".into())
        })
        .is_err());
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        write_dump(&output, |stream| {
            assert!(!output.exists());
            stream.write_all(b"complete")?;
            Ok(1)
        })
        .unwrap();
        assert!(write_dump(&output, |stream| {
            stream.write_all(b"replacement")?;
            Ok(1)
        })
        .is_err());
        assert_eq!(zstd::decode_all(File::open(&output).unwrap()).unwrap(), b"complete");
    }
}
