use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::Path,
};

#[test]
fn device_and_search_keep_the_same_shared_places_and_search_metadata() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = obc_pack::config::Config::load(root.join("builder/presets/schema.json").to_str().unwrap()).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let policy = temporary.path().join("policy.json");
    fs::write(&policy, r#"{"countries":{"de":{"names":{"name":"Deutschland"},"postcode":{"pattern":"ddddd"}}},"levels":[{"tags":{"place":{"city":16,"town":[18,16],"village":[19,16],"hamlet":20},"highway":{"":26},"boundary":{"administrative8":16}}}]}"#).unwrap();
    for (n, source) in
        ["builder/tests/corpus/data/poi.osm.pbf", "host/obc-search-bake/tests/data/places.osm.pbf"].iter().enumerate()
    {
        let source = root.join(source);
        let device = obc_pack::ingest::ingest_osm(
            &[source.to_str().unwrap().to_owned()],
            &config,
            None,
            &obc_pack::Progress::silent(),
        )
        .unwrap();
        let output = temporary.path().join(format!("{n}.jsonl.zst"));
        obc_search_bake::bake(&source, &output, "de", None, &policy).unwrap();
        let stream = zstd::stream::read::Decoder::new(fs::File::open(output).unwrap()).unwrap();
        let records: Vec<Value> = BufReader::new(stream)
            .lines()
            .map(|l| serde_json::from_str::<Value>(&l.unwrap()).unwrap())
            .filter(|v| v["type"] == "Place")
            .flat_map(|v| v["content"].as_array().unwrap().clone())
            .collect();
        let actual: BTreeMap<_, _> = records
            .iter()
            .filter_map(|p| {
                let kind = obc_places::classify([(p["osm_key"].as_str()?, p["osm_value"].as_str()?)])?;
                Some((
                    format!("{}{}", p["object_type"].as_str()?.to_lowercase(), p["object_id"]),
                    (
                        kind.subtype,
                        obc_places::to_udeg(p["centroid"][0].as_f64()?),
                        obc_places::to_udeg(p["centroid"][1].as_f64()?),
                    ),
                ))
            })
            .collect();
        let expected: BTreeMap<_, _> = device
            .pois
            .iter()
            .map(|p| {
                let kind = p.metadata.source.0 >> 62;
                let id = p.metadata.source.0 & ((1 << 62) - 1);
                (format!("{}{id}", ['n', 'w', 'r'][(kind - 1) as usize]), (p.subtype, p.lon_udeg, p.lat_udeg))
            })
            .collect();
        assert_eq!(actual, expected, "{}", source.display());
        if n == 1 {
            assert!(actual.contains_key("r20"));
            assert!(!actual.contains_key("r21"));
            let bakery = records.iter().find(|p| p["object_id"] == 2).unwrap();
            assert_eq!(bakery["name"]["name:en"], "Bakery");
            assert_eq!(bakery["extra"]["contact:website"], "https://bakery.example");
            assert_eq!(bakery["extra"]["opening_hours"], "Mo-Fr 08:00-18:00");
            assert!(records.iter().any(|p| p["osm_value"] == "cafe"));
            assert!(records.iter().any(|p| p["osm_value"] == "bus_stop"));
            let village = records.iter().find(|p| p["object_type"] == "N" && p["object_id"] == 7).unwrap();
            assert_eq!(village["bbox"], serde_json::json!([8.002, 48.002, 8.004, 48.004]));
            assert_eq!(village["name"]["name:en"], "Test Village");
        }
    }
}
