//! The step `planner/trailview-features` of `obc data`: `trailview-features.pmtiles` of a region from
//! the OSM of `planner/osm` and the master terrain of the grid of `planner/terrain`, by the kind
//! table and the road classes of its options.

use obc_data::engine::Request;

use crate::bake::{bake, Options};
use crate::{kinds, terrain};

pub fn step(request: &Request) -> Result<(), String> {
    let options = &request.options;
    let kinds = kinds::validate(&options["kinds"])?;
    let (class_names, classes) = kinds::classes(&options["roads"])?;
    let bounds_json =
        options["bounds"].as_array().filter(|b| b.len() == 4).ok_or("option `bounds` is not four numbers")?;
    let bounds: Vec<f64> = bounds_json.iter().filter_map(|v| v.as_f64()).collect();
    let bounds: [f64; 4] = bounds.try_into().map_err(|_| "option `bounds` is not four numbers")?;
    let attribution = options["attribution"].as_str().ok_or("option `attribution` is not a string")?.to_string();
    let options = Options { kinds, bounds, bounds_json: bounds_json.clone(), attribution, classes, class_names };
    let layer = |name: &str| request.layers.get(name).ok_or(format!("the step has no input `{name}`"));
    let [pbf] = layer("planner/osm")?.values().collect::<Vec<_>>()[..] else {
        return Err("`planner/osm` is not one file".into());
    };
    let terrain = terrain::Tiles::grid(layer("planner/terrain/grid")?)?;
    let parent = request.output.parent().ok_or("the output has no directory")?;
    let work = tempfile::Builder::new().prefix(".trailview-osm-").tempdir_in(parent).map_err(|e| e.to_string())?;
    // Osmium finds the format of a file by its name; a store object has none.
    let osm = work.path().join("input.osm.pbf");
    std::os::unix::fs::symlink(pbf, &osm).map_err(|e| format!("{}: {e}", osm.display()))?;
    let output = request.output.join("trailview-features.pmtiles");
    // The process of a step gets the memory that `obc data` reserves for it in its environment.
    let reserved = request.memory_bytes.or_else(|| std::env::var("OBC_DATA_MEMORY_BYTES").ok()?.parse().ok());
    let metrics = bake(&options, &osm, &terrain, &output, work.path(), free_memory(reserved))?;
    std::fs::write(&request.metrics, metrics.to_string()).map_err(|e| e.to_string())
}

/// The bytes of memory that the step may take: what is free past a quarter of the machine's memory, as
/// other work shares the machine and its memory grows while the step runs, and no more than `obc
/// data` reserves for it. None free and nothing known are both 0: the road jobs then run one at a time.
pub fn free_memory(reserved: Option<u64>) -> usize {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |key: &str| -> Option<u64> {
        let line = meminfo.lines().find(|l| l.starts_with(key))?;
        Some(line.split_whitespace().nth(1)?.parse::<u64>().ok()? << 10)
    };
    let free = match (field("MemAvailable:"), field("MemTotal:")) {
        (Some(available), Some(total)) => available.saturating_sub(total / 4),
        _ => 0,
    };
    free.min(reserved.unwrap_or(u64::MAX)) as usize
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    use image::codecs::webp::WebPEncoder;
    use image::ExtendedColorType;
    use serde_json::{json, Value};

    use super::*;
    use crate::pmtiles::{self, Archive, Header};
    use crate::tiles::{decode, Feature, Tiles, EXTENT, FORMAT, MAX_ZOOM, MIN_ZOOM};

    /// A zoom-13 tile in Engelberg. Each node is (id, u, v) in its tile units.
    const COLUMN: u32 = 4286;
    const ROW: u32 = 2889;
    const NODES: [(u32, u32, u32); 39] = [
        (1, 7000, 4000),
        (2, 17500, 4000),
        (3, 3000, 3000),
        (4, 3400, 3000),
        (5, 3400, 3400),
        (6, 3000, 3400),
        (7, 3100, 3100),
        (8, 3100, 3200),
        (9, 3200, 3200),
        (10, 3200, 3100),
        (11, 5000, 5000),
        (12, 5100, 5000),
        (13, 5100, 5100),
        (14, 1000, 6000),
        (15, 1500, 6000),
        (16, 1000, 7000),
        (17, 2000, 7000),
        (18, 2000, 1000),
        (19, 2600, 1000),
        (20, 2600, 1600),
        (21, 2000, 1600),
        (22, 1500, 500),
        (23, 1700, 700),
        (24, 1800, 900),
        (25, 6000, 1000),
        (26, 6300, 1000),
        (27, 6300, 1100),
        (28, 7000, 1000),
        (29, 7300, 1000),
        (30, 7300, 1100),
        (31, 6000, 2000),
        (32, 6200, 2000),
        (33, 6000, 2500),
        (34, 6400, 2500),
        (35, 6000, 3000),
        (36, 2900, 2900),
        (37, 3500, 2900),
        (38, 3500, 3500),
        (39, 2900, 3500),
    ];

    type Way = (u32, Vec<u32>, Vec<(&'static str, &'static str)>);

    fn ways() -> Vec<Way> {
        vec![
            (100, vec![1, 2], vec![("highway", "track"), ("surface", "gravel"), ("name", "Alpweg")]),
            (101, vec![3, 4, 5, 6, 3], vec![]),
            (102, vec![7, 8, 9, 10, 7], vec![]),
            (103, vec![11, 12, 13, 11], vec![("building", "yes"), ("ruins", "yes")]),
            (104, vec![14, 15], vec![("highway", "secondary"), ("tunnel", "yes")]),
            (105, vec![16, 17], vec![("railway", "rail")]),
            (106, vec![18, 19, 20, 21, 18], vec![("natural", "water")]),
            (107, vec![22, 23, 24, 18], vec![("waterway", "river")]),
            // Dams: a closed way without and with area=yes, and a crest.
            (108, vec![25, 26, 27, 25], vec![("waterway", "dam")]),
            (109, vec![28, 29, 30, 28], vec![("waterway", "dam"), ("area", "yes")]),
            (110, vec![31, 32], vec![("waterway", "dam")]),
            (111, vec![33, 34], vec![("highway", "construction"), ("tunnel", "yes")]),
            (112, vec![36, 37, 38, 39, 36], vec![("landuse", "farmyard")]),
        ]
    }

    /// Longitude and latitude of the point (u, v) in tile units of the deepest tile (column, row).
    fn location(column: u32, row: u32, u: f64, v: f64) -> (f64, f64) {
        let n = (1u64 << MAX_ZOOM) as f64;
        let (x, y) = ((column as f64 + u / EXTENT as f64) / n, (row as f64 + v / EXTENT as f64) / n);
        (x * 360.0 - 180.0, (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees())
    }

    fn tag(tags: &[(&str, &str)]) -> String {
        tags.iter().map(|(k, v)| format!("<tag k=\"{k}\" v=\"{v}\"/>")).collect()
    }

    fn osm() -> String {
        let nodes: String = NODES
            .iter()
            .map(|&(id, u, v)| {
                let (lon, lat) = location(COLUMN, ROW, u as f64, v as f64);
                let tags = if id == 35 { tag(&[("waterway", "weir")]) } else { String::new() };
                format!("<node id=\"{id}\" version=\"1\" lat=\"{lat:.7}\" lon=\"{lon:.7}\">{tags}</node>")
            })
            .collect();
        let ways: String = ways()
            .iter()
            .map(|(id, refs, tags)| {
                let refs: String = refs.iter().map(|r| format!("<nd ref=\"{r}\"/>")).collect();
                format!("<way id=\"{id}\" version=\"1\">{refs}{}</way>", tag(tags))
            })
            .collect();
        let relation = format!(
            "<relation id=\"200\" version=\"1\"><member type=\"way\" ref=\"101\" role=\"outer\"/><member type=\"way\" ref=\"102\" role=\"inner\"/>{}</relation>",
            tag(&[("type", "multipolygon"), ("building", "yes")])
        );
        format!("<osm version=\"0.6\">{nodes}{ways}{relation}</osm>")
    }

    /// The grid of `planner/terrain` with a zoom-13 tile under each of the two feature tiles: a plane that
    /// rises 5 cm per pixel to the east.
    fn grid(root: &Path) -> BTreeMap<String, PathBuf> {
        std::fs::create_dir(root.join("objects")).unwrap();
        let tiles: Vec<(u64, Vec<u8>)> = (0..2)
            .map(|east| {
                let mut rgba = Vec::with_capacity(512 * 512 * 4);
                for _ in 0..512 {
                    for i in 0..512 {
                        let height = 1000.0 + 0.05 * (512.0 * east as f64 + i as f64);
                        let code = ((height + 32768.0) * 256.0).round_ties_even() as u32;
                        rgba.extend([(code >> 16) as u8, (code >> 8) as u8, code as u8, 255]);
                    }
                }
                let mut data = Vec::new();
                WebPEncoder::new_lossless(&mut data).encode(&rgba, 512, 512, ExtendedColorType::Rgba8).unwrap();
                (pmtiles::tile_id(13, COLUMN + east, ROW), data)
            })
            .collect();
        let header =
            Header { tile_type: 4, tile_compression: 1, bounds_e7: [0; 4], center_zoom: 13, center_e7: [0; 2] };
        pmtiles::write(&root.join("objects/archive"), &tiles, &header, "{}").unwrap();
        let name = format!("maps/tiles/terrain/13/11-{}-{}.pmtiles", COLUMN >> 2, ROW >> 2);
        std::fs::write(root.join("index.json"), json!({"files": {name: {"sha256": "archive"}}}).to_string()).unwrap();
        BTreeMap::from([
            ("index.json".to_string(), root.join("index.json")),
            ("objects/archive".to_string(), root.join("objects/archive")),
        ])
    }

    /// The height at u tile units east in the tile COLUMN: its pixel of zoom 13 is u / 32.
    fn height(u: f64) -> f64 {
        1000.0 + 0.05 * (u / 32.0 - 0.5)
    }

    fn table(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_small_region_bakes_into_buffered_tiles_at_every_zoom() {
        let Ok((binary, _)) = crate::osm::probe() else {
            eprintln!("Osmium is unavailable: the bake of a small region does not run");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::write(root.join("region.osm"), osm()).unwrap();
        // The step reads a store object, which has no file extension.
        let out = std::process::Command::new(&binary)
            .args(["cat", "-f", "pbf", "-o"])
            .arg(root.join("object"))
            .arg(root.join("region.osm"))
            .output()
            .unwrap();
        assert!(out.status.success());
        let (west, north) = location(COLUMN, ROW, 0.0, 0.0);
        let (east, south) = location(COLUMN, ROW, 2.0 * EXTENT as f64, EXTENT as f64);
        std::fs::create_dir(root.join("output")).unwrap();
        let request = obc_data::engine::Request {
            step: "planner/trailview-features".into(),
            snapshots: BTreeMap::new(),
            layers: BTreeMap::from([
                ("planner/osm".to_string(), BTreeMap::from([("osm.pbf".to_string(), root.join("object"))])),
                ("planner/terrain/grid".to_string(), grid(root)),
            ]),
            layer_files: BTreeMap::new(),
            options: json!({"bounds": [west, south, east, north], "attribution": "© OSM",
                            "kinds": table(include_str!("../../../data/trailview/kinds.json")),
                            "roads": table(include_str!("../../../data/trailview/roads.json"))}),
            output: root.join("output"),
            metrics: root.join("metrics.json"),
            memory_bytes: Some(1 << 30),
        };
        step(&request).unwrap();
        let archive = Archive::open(&root.join("output/trailview-features.pmtiles")).unwrap();
        let head = archive.bytes(0, 127).unwrap();
        let word = |at: usize| u64::from_le_bytes(head[at..at + 8].try_into().unwrap());
        assert_eq!((head[99], head[98]), (0, 2), "tiles of an unknown type, gzip'd");
        let mut metadata = String::new();
        flate2::read::GzDecoder::new(&archive.bytes(word(24), word(32)).unwrap()[..])
            .read_to_string(&mut metadata)
            .unwrap();
        let metadata = table(&metadata);
        assert_eq!(metadata["features_format"], FORMAT);
        assert_eq!(metadata["bounds"], json!([west, south, east, north]));
        let region = Tiles::new([west, south, east, north]);
        let mut tiles: BTreeMap<(u8, u32, u32), Vec<Feature>> = BTreeMap::new();
        for z in MIN_ZOOM..=MAX_ZOOM {
            let [c0, r0, c1, r1] = region.span(z);
            for (x, y) in (c0..=c1).flat_map(|x| (r0..=r1).map(move |y| (x as u32, y as u32))) {
                if let Some(data) = archive.get(z, x, y).unwrap() {
                    let mut raw = Vec::new();
                    flate2::read::GzDecoder::new(&data[..]).read_to_end(&mut raw).unwrap();
                    tiles.insert((z, x, y), decode(&raw).unwrap());
                }
            }
        }
        let (left, right) = (&tiles[&(MAX_ZOOM, COLUMN, ROW)], &tiles[&(MAX_ZOOM, COLUMN + 1, ROW)]);
        let kinds = |tile: &[Feature]| {
            let mut kinds: Vec<String> = tile.iter().map(|f| f.kind.clone()).collect();
            kinds.sort();
            kinds
        };
        let expected = [
            "building",
            "dam",
            "dam-area",
            "dam-area",
            "dam-point",
            "lake",
            "main-road",
            "railway",
            "river",
            "road",
            "ruin",
        ];
        assert_eq!(kinds(left), expected);
        assert_eq!(kinds(right), ["road"]);
        let find = |tile: &[Feature], kind: &str| tile.iter().find(|f| f.kind == kind).unwrap().clone();
        let tags =
            |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<crate::Tags>();
        // The building in the farmyard keeps its landuse; the farmyard is no feature.
        let building = find(left, "building");
        assert_eq!(building.tags, tags(&[("building", "yes"), ("landuse", "farmyard")]));
        assert_eq!(
            tiles.keys().map(|k| k.0).collect::<std::collections::BTreeSet<_>>(),
            (MIN_ZOOM..=MAX_ZOOM).collect()
        );
        assert_eq!(find(left, "dam").parts[0].xy.len(), 2, "a closed dam way is an area, not a ring line");
        // A tunnel with no terrain over it runs on the ground, as a road.
        let tunnel = find(left, "main-road");
        assert_eq!(tunnel.tags, tags(&[("highway", "secondary")]));
        let road = find(left, "road");
        assert_eq!(road.tags, tags(&[("highway", "track"), ("surface", "gravel")]));
        // A cut keeps the segment whole, so both tiles hold the same one, and its heights follow the terrain.
        assert_eq!(road.parts[0].xy, [[7000, 4000], [17500, 4000]]);
        let shift = EXTENT as i64;
        assert_eq!(right[0].parts[0].xy, [[7000 - shift, 4000], [17500 - shift, 4000]]);
        let cm = |value: f64| (value * 100.0).round_ties_even() / 100.0;
        for feature in [&road, &right[0]] {
            assert_eq!(feature.parts[0].z.as_ref().unwrap(), &[cm(height(7000.0)), cm(height(17500.0))]);
        }
        // A tunnel that meets no road takes the terrain at its ends: nothing else gives it heights.
        for (z, u) in tunnel.parts[0].z.as_ref().unwrap().iter().zip([1000.0, 1500.0]) {
            assert!((z - height(u)).abs() < 0.05);
        }
        let lake = find(left, "lake");
        let level = lake.parts[0].z.as_ref().unwrap();
        assert!(level.iter().all(|&l| l == level[0]), "a lake has one height");
        assert!(height(2000.0) < level[0] && level[0] < height(2300.0), "a low quantile of its shore, inside it");
        // The river flows east, up the plane, into the lake: it holds near the lake's level and never rises.
        let stream = find(left, "river").parts[0].z.clone().unwrap();
        assert!(stream.windows(2).all(|w| w[1] <= w[0]));
        assert!((stream[stream.len() - 1] - level[0]).abs() < 0.05);
        assert_eq!(building.parts.iter().map(|p| p.outer).collect::<Vec<_>>(), [true, false]);
        assert_eq!(building.parts[1].xy.len(), 4);
        // Above zoom 13 a tile holds the water and the main roads only, and the river still ends on the
        // shore of the lake.
        let overview = tiles.iter().find(|(k, _)| k.0 == 11).unwrap().1;
        assert_eq!(kinds(overview), ["lake", "main-road", "river"]);
        let (lake, river) = (find(overview, "lake"), find(overview, "river"));
        assert!(lake.parts[0].xy.contains(river.parts[0].xy.last().unwrap()));
        let metrics = table(&std::fs::read_to_string(root.join("metrics.json")).unwrap());
        assert_eq!(metrics["tiles"], tiles.len());
    }
}
