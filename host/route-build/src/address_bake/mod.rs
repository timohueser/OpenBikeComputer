mod areas;
mod countries;
mod enrich;
mod geometry;
mod input;
mod postcodes;

use enrich::{house_numbers, Index};
use osmpbfreader::OsmId;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
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
) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    if output.exists() {
        return Err("The output file must not exist".into());
    }
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
    let index = Index::new(&input, &countries);
    eprintln!("Write address records");
    let raw = OpenOptions::new().write(true).create_new(true).open(output)?;
    let result = (|| -> Result<usize, Box<dyn std::error::Error>> {
        let mut stream = zstd::stream::Encoder::new(BufWriter::new(raw), 3)?;
        serde_json::to_writer(
            &mut stream,
            &json!({"type":"NominatimDumpFile","content":{
                "generator":"obc-address-bake","experimental":true,"scope":"addresses","osm_sha256":osm_hash,"data_timestamp":null,
                "default_country":country,"country_grid_sha256":grid_hash
            }}),
        )?;
        writeln!(stream)?;
        let mut count = 0;
        for (i, f) in input
            .features
            .iter()
            .enumerate()
            .filter(|(_, f)| f.road())
            .chain(input.features.iter().enumerate().filter(|(_, f)| !f.road()))
        {
            let tags = index.tags(i);
            let houses = house_numbers(tags);
            if !f.road() && houses.is_empty() {
                continue;
            }
            let a = index.address(i);
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
                "country_code":countries[i],"centroid":[p.x(),p.y()],"bbox":[extent.lower()[0],extent.lower()[1],extent.upper()[0],extent.upper()[1]],
                "name":name,"address":a,"postcode":a.get("postcode").map(|s| s.as_str()).unwrap_or(""),"importance":0.05});
            if houses.is_empty() {
                serde_json::to_writer(&mut stream, &json!({"type":"Place","content":[record]}))?;
                writeln!(stream)?;
                count += 1;
            } else {
                for house in houses {
                    record["housenumber"] = json!(house);
                    serde_json::to_writer(&mut stream, &json!({"type":"Place","content":[&record]}))?;
                    writeln!(stream)?;
                    count += 1;
                }
            }
        }
        stream.finish()?.flush()?;
        Ok(count)
    })();
    match result {
        Ok(count) => {
            println!(
                "{}",
                json!({"records":count,"seconds":start.elapsed().as_secs_f64(),"osm_sha256":osm_hash,"incomplete_geometries":input.incomplete_geometries})
            );
            Ok(())
        }
        Err(e) => {
            fs::remove_file(output)?;
            Err(e)
        }
    }
}
