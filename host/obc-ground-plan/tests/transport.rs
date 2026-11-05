//! The request codec, the flat plan and its digest over a cell of the TrailView fixture.

#[path = "fixture/mod.rs"]
mod fixture;

use fixture::{default_dir, Fixture};
use obc_ground_plan::cell::{build_cell, STAGES};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::sections::Sections;
use obc_ground_plan::transport::{vocabulary, Flat, Request, CROSSING_WORDS, POINT_WORDS, WAY_WORDS};

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
    assert!(build_cell(&garbled.encode(), Sections::embedded(), &mut || 0.0).is_err(), "a tile that does not decode");
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
        build_cell(&bytes, Sections::embedded(), &mut || 0.0).unwrap(),
        build_cell(&bytes, Sections::embedded(), &mut clock).unwrap(),
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
    assert_eq!(stats["inputs"]["request"].as_str(), Some(a.request.as_str()));
    // A changed sample changes the digest.
    let mut moved = a.clone();
    moved.graph.samples[0].z_m += 1e-9;
    assert_ne!(Flat::of(&moved).digest(&moved).unwrap(), digest);
    // A NaN hides its bits in the text: the digest refuses it.
    moved.graph.samples[0].z_m = f64::NAN;
    assert!(Flat::of(&moved).digest(&moved).unwrap_err().contains("NaN"));
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
    assert_eq!(v["crossings"].as_array().unwrap().len(), 8);
}
