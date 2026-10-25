mod areas;
mod countries;
mod enrich;
mod features;
mod geometry;
mod input;
mod interpolation;
mod places;
mod policy;
mod postcodes;
pub mod step;

use enrich::{house_addresses, Index};
use osmpbfreader::OsmId;
use rayon::prelude::*;
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

/// Write the search dump of `osm` to `output`, which must not exist. Returns the report: the record
/// count, the time, the OSM digest and the count of incomplete geometries.
pub fn bake(
    osm: &Path,
    output: &Path,
    country: &str,
    country_grid: Option<&Path>,
    policy_path: &Path,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
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
    let input = features::read(osm)?;
    eprintln!(
        "Read {} nodes, {} features; {} incomplete geometries",
        input.nodes,
        input.features.len(),
        input.incomplete_geometries
    );
    let grid = countries::Countries::read(country_grid, &input)?;
    let countries: Vec<_> =
        input.features.par_iter().map(|f| grid.at(geometry::center(&f.geometry)).unwrap_or(country)).collect();
    eprintln!("Resolved country polygons; build address indexes");
    let index = Index::new(&input, &countries, &policy);
    eprintln!("Write address records");
    let count = write_dump(output, |stream| {
        stream.write_all(&line(json!({"type":"NominatimDumpFile","content":{
            "generator":"obc-search-bake","scope":"all","osm_sha256":osm_hash,"data_timestamp":null,
            "default_country":country,"country_grid_sha256":grid_hash,"policy_sha256":policy_hash
        }})))?;
        let addresses: Vec<_> = (0..input.features.len())
            .filter(|&i| input.features[i].road() && !input.features[i].name().is_empty())
            .chain((0..input.features.len()).filter(|&i| !input.features[i].road()))
            .collect();
        let mut count = write_lines(stream, &addresses, |&i, lines| {
            let f = &input.features[i];
            let tags = index.tags(i);
            let houses = house_addresses(tags);
            if !f.road() && houses.is_empty() {
                return Ok(());
            }
            let (a, country) = index.address(i);
            if !f.road() && a.get("street").is_none_or(|s| s.is_empty()) {
                return Ok(());
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
                lines.push((None, line(json!({"type":"Place","content":[record]}))));
            } else {
                for (house, street) in houses {
                    record["housenumber"] = json!(house);
                    if let (geo::Geometry::LineString(line), Some(range)) =
                        (&f.geometry, f.tags.get("_interpolation_range"))
                    {
                        if let Some((first, last)) = range.split_once(':') {
                            let number = |s: &str| s.parse::<f64>().map_err(|e| e.to_string());
                            let (first, last, number) = (number(first)?, number(last)?, number(house)?);
                            let p = interpolation::point(line, (number - first) / (last - first));
                            record["centroid"] = json!([p.x(), p.y()]);
                        }
                    }
                    record["address"] = json!(&a);
                    if let Some(street) = street {
                        record["address"]["street"] = json!(street);
                    }
                    lines.push((None, line(json!({"type":"Place","content":[&record]}))));
                }
            }
            Ok(())
        })?;
        eprintln!("Wrote {count} address records; write POIs and localities");
        let features: Vec<_> = (0..input.features.len()).collect();
        count += write_lines(stream, &features, |&i, lines| {
            let f = &input.features[i];
            if let Some(record) = places::record(f, i, &index) {
                lines.push((Some(f.source), line(json!({"type":"Place","content":[record]}))));
            }
            Ok(())
        })?;
        Ok(count)
    })?;
    Ok(
        json!({"records":count,"seconds":start.elapsed().as_secs_f64(),"osm_sha256":osm_hash,"incomplete_geometries":input.incomplete_geometries}),
    )
}

/// `value` as one line, with the keys of each object in byte order: the order of the dump also in a
/// binary in which another crate enables `serde_json/preserve_order`.
fn line(value: serde_json::Value) -> Vec<u8> {
    let mut line = serde_json::to_vec(&sort_keys(value)).expect("a JSON value serializes");
    line.push(b'\n');
    line
}

/// The lines of one item. Of adjacent lines with the same OSM object, only the first is written; a
/// line without an object is always written.
type Lines = Vec<(Option<OsmId>, Vec<u8>)>;

/// Write the lines of each of `items` in order and return their count. The lines of a batch build
/// on every core, while this thread compresses the batch before it.
fn write_lines<T: Sync>(
    stream: &mut dyn Write,
    items: &[T],
    lines: impl Fn(&T, &mut Lines) -> Result<(), String> + Sync,
) -> Result<usize, Box<dyn std::error::Error>> {
    let (sender, batches) = std::sync::mpsc::sync_channel(1);
    let lines = &lines;
    std::thread::scope(|scope| {
        scope.spawn(move || {
            for batch in items.chunks(1 << 14) {
                let built = batch
                    .par_chunks(256)
                    .map(|chunk| {
                        let mut built = Vec::new();
                        chunk.iter().try_for_each(|item| lines(item, &mut built)).map(|()| built)
                    })
                    .collect::<Result<Vec<_>, _>>();
                let failed = built.is_err();
                if sender.send(built).is_err() || failed {
                    break;
                }
            }
        });
        let (mut count, mut last) = (0, None);
        for built in batches {
            for (source, line) in built?.into_iter().flatten() {
                if source.is_none() || source != last {
                    stream.write_all(&line)?;
                    count += 1;
                    last = source;
                }
            }
        }
        Ok(count)
    })
}

fn sort_keys(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(key, value)| (key, sort_keys(value))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_keys).collect()),
        value => value,
    }
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
