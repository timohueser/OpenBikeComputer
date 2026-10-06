use super::*;
use crate::engine::tests::write;
use crate::store::tests::Scratch;
use crate::store::{sha256_hex, FileRecord, Snapshot};

fn index(store: &Store) -> String {
    let feature = |id: &str, name: &str, parent: Option<&str>, countries: &[&str]| {
        serde_json::json!({
            "type": "Feature", "properties": {"id": id, "name": name, "parent": parent,
                "iso3166-1:alpha2": countries, "urls": {"pbf": format!("https://download.geofabrik.de/europe/{id}-latest.osm.pbf")}},
            "geometry": {"type": "Polygon", "coordinates": [[[7.0,47.0],[8.0,47.0],[8.0,48.0],[7.0,47.0]]]}
        })
    };
    let body = serde_json::to_vec(&serde_json::json!({"type":"FeatureCollection", "features": [
        feature("germany", "Deutschland", None, &["DE"]),
        feature("bayern", "Bayern", Some("germany"), &[]),
        feature("switzerland", "Switzerland", None, &["CH", "LI"]),
        feature("missing-metadata", "Unknown country", None, &[])
    ]}))
    .unwrap();
    let sha256 = sha256_hex(&body);
    write(&store.object(&sha256), std::str::from_utf8(&body).unwrap());
    store
        .put_snapshot(&Snapshot {
            source: "geofabrik-index".into(),
            version: "2026-10-01".into(),
            files: vec![FileRecord {
                name: "index-v1.json".into(),
                url: INDEX_URL.into(),
                size: body.len() as u64,
                sha256: sha256.clone(),
                retrieved: "2026-10-01T00:00:00Z".into(),
            }],
        })
        .unwrap();
    sha256
}

fn args(id: &str) -> Create {
    Create {
        id: id.into(),
        name: "One region".into(),
        areas: vec!["europe/switzerland".into(), "europe/bayern".into(), "europe/bayern".into()],
        bbox: None,
        countries: Vec::new(),
        time_zone: "Europe/Berlin".into(),
    }
}

#[test]
fn cached_suggestions_search_names_and_paths_without_geometry_or_downloads() {
    let scratch = Scratch::new("region-suggestions");
    let store = Store::at(scratch.0.join("store"));
    assert_eq!(suggestions(&store, "").unwrap_err().code, Code::Blocked);
    assert!(!store.root().exists(), "a read does not create the store");
    let sha = index(&store);
    let names = suggestions(&store, "  BAYERN ").unwrap();
    assert_eq!(names.areas[0].countries, ["DE"]);
    assert_eq!(suggestions(&store, "europe/bay").unwrap().areas.len(), 1);
    assert!(suggestions(&store, "absent").unwrap().areas.is_empty());
    assert!(serde_json::to_value(names).unwrap()["areas"][0].get("polygons").is_none());
    write(&store.object(&sha), "corrupt");
    assert_eq!(suggestions(&store, "Bayern").unwrap_err().code, Code::InvalidData);
}

#[test]
fn creation_saves_one_normalized_selection_and_never_overwrites_a_definition() {
    let scratch = Scratch::new("region-create");
    let store = Store::at(scratch.0.join("store"));
    index(&store);
    let selected = args("ride/alps");
    let region = create(&scratch.0, &store, selected).unwrap();
    assert_eq!(region.countries, ["CH", "DE", "LI"]);
    assert_eq!(region.time_zone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(
        region.area,
        crate::regions::Area::Geofabrik { areas: vec!["europe/bayern".into(), "europe/switzerland".into()] }
    );
    assert_eq!(Regions::load(&scratch.0).unwrap().iter().count(), 1, "no hidden child definitions");
    let path = scratch.0.join("data/regions/ride/alps.toml");
    let original = std::fs::read(&path).unwrap();
    assert!(create(&scratch.0, &store, args("ride/alps")).is_err());
    assert_eq!(std::fs::read(path).unwrap(), original);
    for invalid in ["../outside", "bad-zone", "unknown-area"] {
        let mut selected = args(invalid);
        if invalid == "bad-zone" {
            selected.time_zone = "Not/AZone".into();
        }
        if invalid == "unknown-area" {
            selected.areas = vec!["europe/missing".into()];
        }
        assert!(create(&scratch.0, &store, selected).is_err());
        assert!(!scratch.0.join(format!("data/regions/{invalid}.toml")).exists());
    }
    let mut fallback = args("explicit-country");
    fallback.areas = vec!["europe/missing-metadata".into()];
    assert_eq!(
        create(&scratch.0, &store, Create { areas: fallback.areas.clone(), ..args("no-country") }).unwrap_err().code,
        Code::Blocked
    );
    fallback.countries = vec!["DE".into()];
    assert_eq!(create(&scratch.0, &store, fallback).unwrap().countries, ["DE"]);
    let boxed =
        Create { bbox: Some("7,47,8,48".into()), countries: vec!["DE".into()], areas: Vec::new(), ..args("box-ride") };
    assert!(matches!(create(&scratch.0, &store, boxed).unwrap().area, crate::regions::Area::Box { .. }));
}
