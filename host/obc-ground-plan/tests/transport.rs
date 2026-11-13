//! The request codec, the flat plan and its digest over a cell of the TrailView fixture.

#[path = "fixture/mod.rs"]
mod fixture;

use fixture::{default_dir, Fixture};
use obc_ground_plan::cell::{build_cell, build_cell_with, cell_graph, STAGES};
use obc_ground_plan::faces::{records, Rules};
use obc_ground_plan::faces::{CornerArc, Edge, EdgeKind, Face, FacePlan, FaceStats, HeightRule, Owner, Surface};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::tile::TileCache;
use obc_ground_plan::transport::{
    vocabulary, FaceWords, Flat, Request, CORNER_WORDS, CROSSING_WORDS, EDGE_WORDS, FACE_WORDS, POINT_WORDS,
    RING_WORDS, WAY_WORDS,
};

/// A town cell with junctions and two terrain levels.
const CELL: CellKey = CellKey { x: 34296, y: 23098 };

fn request() -> Request {
    Fixture::open(&default_dir()).request(CELL)
}

#[test]
fn a_request_round_trips_and_its_bytes_do_not_depend_on_the_order_of_its_inputs() {
    let request = request();
    assert!(request.pages.len() >= 2 && !request.tiles.is_empty());
    let bytes = request.encode();
    let mut shuffled = request.clone();
    shuffled.pages.reverse();
    shuffled.tiles.reverse();
    assert_eq!(shuffled.encode(), bytes);
    let decoded = Request::decode(&bytes).unwrap();
    assert_eq!(decoded.encode(), bytes);
    assert_eq!(decoded.pages.len(), request.pages.len());
}

#[test]
fn decode_refuses_every_malformed_request() {
    let request = request();
    let bytes = request.encode();
    // Each case names a word of the error that it must give.
    let refused = |bytes: &[u8], what: &str, word: &str| {
        let error = Request::decode(bytes).expect_err(what);
        assert!(error.contains(word), "{what}: {error}");
    };
    let mut magic = bytes.clone();
    magic[0] = b'X';
    refused(&magic, "a wrong magic", "OGPR");
    refused(&bytes[..bytes.len() - 2], "a truncation in the codes of a page", "fewer than");
    refused(&bytes[..10], "a truncation in the head", "inside a field");
    refused(&[bytes.as_slice(), &[0; 4]].concat(), "trailing bytes", "after its last page");
    let raw = |r: &Request| {
        let mut one = r.clone();
        one.pages.truncate(1);
        one.tiles.truncate(1);
        one
    };
    let mut repeated = raw(&request);
    repeated.pages.push(repeated.pages[0].clone());
    refused(&repeated.encode(), "a repeated page", "repeats");
    let mut far = raw(&request);
    far.pages[0].x += 40;
    refused(&far.encode(), "a page that the cell does not want", "reads no page");
    let mut tile = raw(&request);
    tile.tiles[0].column += 5;
    refused(&tile.encode(), "a tile that the cell does not read", "reads no tile");
    let mut tiles = raw(&request);
    tiles.tiles.push(tiles.tiles[0].clone());
    refused(&tiles.encode(), "a repeated tile", "repeats");
    let mut padded = raw(&request);
    let length = padded.tiles[0].bytes.len() / 4 * 4 - 1;
    padded.tiles[0].bytes.truncate(length);
    let mut bytes = padded.encode();
    let end = 4 + 8 + 4 + 12 + padded.tiles[0].bytes.len();
    bytes[end] = 1;
    refused(&bytes, "padding that is not zero", "padding");
    let mut garbled = raw(&request);
    garbled.tiles[0].bytes.truncate(7);
    assert!(build_cell(&garbled.encode(), Rules::embedded(), &mut || 0.0).is_err(), "a tile that does not decode");
}

#[test]
fn the_flat_plan_indexes_its_samples_and_its_digest_ignores_the_clock() {
    let bytes = request().encode();
    let mut ticks = 0.0;
    let mut clock = || {
        ticks += 7.5;
        ticks
    };
    let (a, b) = (
        build_cell(&bytes, Rules::embedded(), &mut || 0.0).unwrap(),
        build_cell(&bytes, Rules::embedded(), &mut clock).unwrap(),
    );
    assert_eq!(a.times, [0.0; STAGES.len()]);
    assert_eq!(b.times, [7.5; STAGES.len()]);
    let flat = Flat::of(&a);
    let digest = flat.digest(&a).unwrap();
    assert_eq!(digest, Flat::of(&b).digest(&b).unwrap());
    assert_eq!(digest.len(), 16);
    let samples = flat.samples.len() / 2;
    assert_eq!(samples, a.graph.samples.len());
    assert_eq!(flat.ways.len(), WAY_WORDS * a.graph.ways.len());
    for way in flat.ways.chunks(WAY_WORDS) {
        assert!(way[5] >= 2 && (way[4] + way[5]) as usize <= samples);
    }
    let g = &a.graph;
    assert!(!g.junctions.is_empty() && !g.crossings.is_empty());
    assert_eq!(flat.points.len(), POINT_WORDS * (g.junctions.len() + g.joints.len() + g.ends.len()));
    assert_eq!(flat.crossings.len(), CROSSING_WORDS * g.crossings.len());
    let stats: serde_json::Value = serde_json::from_str(&flat.stats).unwrap();
    assert_eq!(stats["graph"]["junctions"].as_u64(), Some(u64::from(g.stats.junctions)));
    assert_eq!(stats["inputs"]["request"].as_str(), a.request.as_deref());
    let f = &flat.faces;
    assert!(!a.faces.faces.is_empty());
    assert_eq!(f.faces.len(), FACE_WORDS * a.faces.faces.len());
    let vertices = (f.vertices.len() / 2) as u32;
    assert!(
        f.ring_vertices.iter().all(|&v| v < vertices) && f.edges.chunks(EDGE_WORDS).all(|e| e[0].max(e[1]) < vertices)
    );
    assert_eq!(stats["faces"]["faces"].as_u64(), Some(u64::from(a.faces.stats.faces)));
    let mut faces = a.clone();
    faces.faces.vertices[0][0] += 1;
    assert_ne!(Flat::of(&faces).digest(&faces).unwrap(), digest, "the digest sees the faces");
    // A changed sample changes the digest.
    let mut moved = a.clone();
    moved.graph.samples[0].z_m += 1e-9;
    assert_ne!(Flat::of(&moved).digest(&moved).unwrap(), digest);
    // A NaN hides its bits in the text: the digest refuses it.
    moved.graph.samples[0].z_m = f64::NAN;
    assert!(Flat::of(&moved).digest(&moved).unwrap_err().contains("NaN"));
}

#[test]
fn a_cell_build_gives_the_faces_and_near_records_of_the_whole_records_build_and_so_does_a_parsed_tile() {
    let bytes = request().encode();
    let rules = Rules::embedded();
    let cell = build_cell(&bytes, rules, &mut || 0.0).unwrap();
    let input = Request::decode(&bytes).unwrap().input(&mut TileCache::new(1)).unwrap();
    let whole = records::build(&cell_graph(CELL, &input, rules), &input, rules, CELL, 0, true);
    assert_eq!(cell.faces, whole.faces);
    assert_eq!(cell.records, whole.near.words);
    // The second build reads the tiles that the first parsed: a load of one would panic.
    let request = Request::decode(&bytes).unwrap();
    let mut tiles = TileCache::new(request.tiles.len());
    for build in 0..2 {
        let again = build_cell_with(&bytes, &mut tiles, rules, &mut || 0.0).unwrap();
        assert_eq!((again.faces, again.records), (cell.faces.clone(), cell.records.clone()));
        if build == 0 {
            for t in &request.tiles {
                tiles.tile(t.column, t.row, || panic!("tile {} {} parses again", t.column, t.row)).unwrap();
            }
        }
    }
}

#[test]
fn the_vocabulary_names_every_code_with_its_colour() {
    let v: serde_json::Value = serde_json::from_str(&vocabulary()).unwrap();
    assert_eq!(v["stages"], serde_json::json!(STAGES));
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 4);
    let colour = |c: &serde_json::Value| c.as_str().is_some_and(|c| c.len() == 7 && c.starts_with('#'));
    assert!(families.iter().all(|f| colour(&f["colour"])));
    assert!(v["colours"].as_object().unwrap().values().all(colour));
    assert_eq!(v["crossings"].as_array().unwrap().len(), 9);
    let names = |key: &str| {
        v[key].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_string()).collect::<Vec<_>>()
    };
    assert_eq!(names("surfaces")[Surface::CornerFill as usize], "corner_fill");
    assert_eq!(names("surfaces").len(), Surface::DeckEdge as usize + 1);
    assert_eq!(names("edges")[EdgeKind::CellBorder as usize], "cell_border");
    assert_eq!(v["surfaces"][0]["fill"], serde_json::Value::Null, "terrain has no fill");
    assert!(v["edges"].as_array().unwrap().iter().all(|e| colour(&e["colour"])));
}

/// The face plan back from its words.
fn plan_of(w: &FaceWords, cell: CellKey) -> FacePlan {
    let pairs = |v: &[i32]| v.chunks(2).map(|p| [p[0], p[1]]).collect::<Vec<_>>();
    let owner =
        |kind: u32, id: u32| [Owner::Terrain, Owner::Way(id), Owner::Junction(id), Owner::Platform(id)][kind as usize];
    let option = |v: u32| (v != u32::MAX).then_some(v);
    const SURFACES: [Surface; 9] = [
        Surface::Terrain,
        Surface::Slope,
        Surface::Carriageway,
        Surface::Shoulder,
        Surface::Verge,
        Surface::CornerFill,
        Surface::Crossing,
        Surface::RailBed,
        Surface::Platform,
    ];
    const RULES: [HeightRule; 5] =
        [HeightRule::Terrain, HeightRule::Slope, HeightRule::Owner, HeightRule::Blend, HeightRule::KerbOffset];
    const KINDS: [EdgeKind; 7] = [
        EdgeKind::Continuous,
        EdgeKind::Kerb,
        EdgeKind::Wall,
        EdgeKind::Headwall,
        EdgeKind::Portal,
        EdgeKind::DeckEdge,
        EdgeKind::CellBorder,
    ];
    let faces = (w.faces.chunks(FACE_WORDS))
        .map(|f| Face {
            id: f[0],
            owner: owner(f[1], f[2]),
            surface: SURFACES[f[3] as usize],
            level: f[4] as i32 as i8,
            rule: RULES[f[5] as usize],
            material: option(f[6]),
            crossing: option(f[7]),
            rank: f[8] as u8,
            inside: f[9] == 1,
            reach_m: f32::from_bits(f[10]),
            legs: [f[11] as u8, (f[11] >> 8) as u8],
            rings: (f[12]..f[12] + f[13])
                .map(|r| {
                    let r = &w.rings[RING_WORDS * r as usize..][..RING_WORDS];
                    w.ring_vertices[r[0] as usize..][..r[1] as usize].to_vec()
                })
                .collect(),
        })
        .collect();
    let edges = (w.edges.chunks(EDGE_WORDS))
        .map(|e| Edge { a: e[0], b: e[1], left: e[2], right: e[3], kind: KINDS[e[4] as usize], flags: e[5] as u8 })
        .collect();
    let corners = (w.corners.chunks(CORNER_WORDS))
        .map(|c| {
            let i = |k: usize| c[k] as i32;
            CornerArc {
                owner: owner(c[0], c[1]),
                centre: [i(2), i(3)],
                radius_m: f64::from_bits(u64::from(c[4]) | u64::from(c[5]) << 32),
                t_a: [i(6), i(7)],
                t_b: [i(8), i(9)],
                arc: pairs(&w.corner_points[2 * c[10] as usize..][..2 * c[11] as usize]),
            }
        })
        .collect();
    let mouths = Vec::new();
    FacePlan {
        cell,
        level: 0,
        vertices: pairs(&w.vertices),
        faces,
        edges,
        corners,
        mouths,
        stats: FaceStats::default(),
    }
}

#[test]
fn the_face_words_keep_every_field_of_the_plan() {
    let cell = CellKey { x: 34288, y: 23104 };
    let face = |id: u32, owner: Owner, surface: Surface, rings: Vec<Vec<u32>>| Face {
        id,
        owner,
        surface,
        level: -1,
        rule: HeightRule::Blend,
        material: None,
        crossing: None,
        rank: 3,
        inside: false,
        reach_m: 0.0,
        legs: [0, 0],
        rings,
    };
    let mut way = face(1, Owner::Way(7), Surface::Carriageway, vec![vec![0, 1, 2, 3], vec![4, 5, 6]]);
    (way.material, way.crossing, way.rank, way.inside, way.reach_m, way.level) = (Some(7), Some(9), 0, true, 2.75, 2);
    way.legs = [2, 1];
    let plan = FacePlan {
        cell,
        level: 0,
        vertices: vec![[0, 0], [100, 0], [100, 100], [0, 100], [10, 10], [20, 10], [-5, 20]],
        faces: vec![
            face(0, Owner::Terrain, Surface::Terrain, vec![vec![0, 1, 2]]),
            way,
            face(2, Owner::Junction(4), Surface::CornerFill, vec![vec![2, 3, 0]]),
            face(3, Owner::Platform(11), Surface::Platform, vec![vec![4, 5, 6]]),
        ],
        edges: vec![
            Edge { a: 0, b: 1, left: 0, right: u32::MAX, kind: EdgeKind::CellBorder, flags: 0 },
            Edge { a: 1, b: 2, left: 1, right: 2, kind: EdgeKind::Kerb, flags: 7 },
        ],
        corners: vec![
            CornerArc {
                owner: Owner::Junction(4),
                centre: [5, -6],
                radius_m: 1.0 / 3.0,
                t_a: [1, 2],
                t_b: [3, -4],
                arc: vec![[2, 3], [3, 3]],
            },
            CornerArc { owner: Owner::Way(7), centre: [0, 0], radius_m: 0.0, t_a: [0, 0], t_b: [0, 0], arc: vec![] },
        ],
        mouths: Vec::new(),
        stats: FaceStats::default(),
    };
    assert_eq!(plan_of(&FaceWords::of(&plan), cell), plan);
}
