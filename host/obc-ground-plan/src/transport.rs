//! The bytes between a caller and `cell::build_cell`: the request of one cell, its flat plan, their
//! digests and the vocabulary of the codes. The bake, the WASM bridge and a later iOS library share
//! them, so every caller builds from the same bytes.
//!
//! A request, little-endian and 4-byte aligned:
//!
//! | Field | Type |
//! | --- | --- |
//! | magic | `b"OGPR"` |
//! | cell x, y | `u32`, `u32` (zoom 16) |
//! | tile count | `u32` |
//! | per tile | `column: u32`, `row: u32`, `length: u32`, the plain format-4 bytes, zero padding to 4 |
//! | page count | `u32` |
//! | per page | `level: u32`, `x: u32`, `y: u32`, `offset: f32`, `scale: f32`, 65,536 × `u16` codes |
//!
//! The encoding is canonical: the tiles come in the order of `CellKey::feature_tiles`, the pages by
//! (level, x, y), and `decode` refuses any other order. So equal inputs give equal bytes.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use serde_json::{json, Value};

use crate::cell::{CellBuild, STAGES};
use crate::debug;
use crate::faces::{FacePlan, HeightRule, Owner};
use crate::frame::CellKey;
use crate::graph::{CrossingKind, Family, Level, Network, Source};
use crate::terrain::{StoredPage, Terrain, PAGE};
use crate::tile::TileCache;
use crate::CellInput;

const REQUEST_MAGIC: &[u8; 4] = b"OGPR";
const PLAN_MAGIC: &[u8; 4] = b"OGPP";
const CODES: usize = PAGE * PAGE;

/// The plain bytes of one zoom-13 feature tile.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureTile {
    pub column: u32,
    pub row: u32,
    pub bytes: Vec<u8>,
}

/// The inputs of one cell: its feature tiles that exist and its stored pages.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub key: CellKey,
    pub tiles: Vec<FeatureTile>,
    pub pages: Vec<StoredPage>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).filter(|&end| end <= self.bytes.len());
        let end = end.ok_or_else(|| format!("a request ends at byte {} inside a field", self.bytes.len()))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn word(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut tiles: Vec<&FeatureTile> = self.tiles.iter().collect();
        tiles.sort_by_key(|t| (t.row, t.column));
        let mut pages: Vec<&StoredPage> = self.pages.iter().collect();
        pages.sort_by_key(|p| (p.level, p.x, p.y));
        let mut out = REQUEST_MAGIC.to_vec();
        let word = |out: &mut Vec<u8>, w: u32| out.extend_from_slice(&w.to_le_bytes());
        word(&mut out, self.key.x);
        word(&mut out, self.key.y);
        word(&mut out, tiles.len() as u32);
        for t in tiles {
            word(&mut out, t.column);
            word(&mut out, t.row);
            word(&mut out, t.bytes.len() as u32);
            out.extend_from_slice(&t.bytes);
            out.resize(out.len().next_multiple_of(4), 0);
        }
        word(&mut out, pages.len() as u32);
        for p in pages {
            for w in [u32::from(p.level), p.x, p.y, p.offset.to_bits(), p.scale.to_bits()] {
                word(&mut out, w);
            }
            out.extend(p.codes.iter().flat_map(|c| c.to_le_bytes()));
        }
        out
    }

    /// The request of canonical bytes; an error names what is wrong.
    pub fn decode(bytes: &[u8]) -> Result<Request, String> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != REQUEST_MAGIC {
            return Err("a request does not start with OGPR".into());
        }
        let key = CellKey { x: r.word()?, y: r.word()? };
        let names = key.feature_tiles();
        let mut last_tile = None;
        let mut tiles = Vec::new();
        for _ in 0..r.word()? {
            let (column, row, length) = (r.word()?, r.word()?, r.word()? as usize);
            let at = names.iter().position(|&t| t == (column, row));
            let at = at.ok_or_else(|| format!("cell {}/{} reads no tile 13/{column}/{row}", key.x, key.y))?;
            if last_tile.is_some_and(|last| last >= at) {
                return Err(format!("tile 13/{column}/{row} repeats or comes out of order"));
            }
            last_tile = Some(at);
            let tile = r.take(length)?.to_vec();
            if r.take(length.next_multiple_of(4) - length)?.iter().any(|&b| b != 0) {
                return Err(format!("tile 13/{column}/{row} has padding that is not zero"));
            }
            tiles.push(FeatureTile { column, row, bytes: tile });
        }
        let wanted = Terrain::wanted(key);
        let mut pages: Vec<StoredPage> = Vec::new();
        for _ in 0..r.word()? {
            let level = r.word()?;
            let (x, y) = (r.word()?, r.word()?);
            let (offset, scale) = (f32::from_bits(r.word()?), f32::from_bits(r.word()?));
            if !wanted.iter().any(|k| u32::from(k.level) == level && (k.x, k.y) == (x, y)) {
                return Err(format!("cell {}/{} reads no page {level}/{x}/{y}", key.x, key.y));
            }
            let level = level as u8;
            if pages.last().is_some_and(|p| (p.level, p.x, p.y) >= (level, x, y)) {
                return Err(format!("page {level}/{x}/{y} repeats or comes out of order"));
            }
            let codes = r.take(2 * CODES).map_err(|_| format!("page {level}/{x}/{y} has fewer than {CODES} codes"))?;
            let codes = codes.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
            pages.push(StoredPage { level, x, y, offset, scale, codes });
        }
        if r.at != bytes.len() {
            return Err(format!("a request has {} bytes after its last page", bytes.len() - r.at));
        }
        Ok(Request { key, tiles, pages })
    }

    /// The cell input, as `CellInput::new` reads it.
    pub fn input(self) -> Result<CellInput, String> {
        let mut tiles: BTreeMap<(u32, u32), Vec<u8>> =
            self.tiles.into_iter().map(|t| ((t.column, t.row), t.bytes)).collect();
        let mut cache = TileCache::new(4);
        CellInput::new(self.key, &mut cache, |column, row| tiles.remove(&(column, row)), self.pages)
            .map_err(|e| e.to_string())
    }
}

/// FNV-1a 64: it finds a difference; it is no security hash.
#[derive(Clone, Copy)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

/// The hash of a `Debug` text that also notes a NaN: the text of a NaN drops its sign and payload,
/// which WASM does not fix, so a NaN would hide a difference between two builds.
#[derive(Default)]
struct DebugHash {
    hash: Fnv,
    nan: bool,
}

impl fmt::Write for DebugHash {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // A float's Debug writes "NaN" in one piece, and the graph holds no text of its own.
        self.nan |= s.contains("NaN");
        self.hash.write(s.as_bytes());
        Ok(())
    }
}

impl Fnv {
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    /// 16 hex digits.
    pub fn hex(self) -> String {
        format!("{:016x}", self.0)
    }
}

pub fn fnv(bytes: &[u8]) -> String {
    let mut h = Fnv::default();
    h.write(bytes);
    h.hex()
}

/// Words per page of `wanted_words`: level, page x, page y, then the stored tile that holds it (z, x, y)
/// and its quadrant.
pub const WANTED_WORDS: usize = 7;

/// The pages that a cell reads (`Terrain::wanted`), `WANTED_WORDS` per page.
pub fn wanted_words(key: CellKey) -> Vec<u32> {
    (Terrain::wanted(key).into_iter())
        .flat_map(|page| {
            let (z, x, y, quadrant) = page.tile();
            [u32::from(page.level), page.x, page.y, z, x, y, quadrant as u32]
        })
        .collect()
}

/// Words of a way in `Flat::ways`: id, family, level, flags, first sample, sample count.
pub const WAY_WORDS: usize = 6;
/// `Flat::ways` flags.
pub const WAY_LINK: u32 = 1;
pub const WAY_SYNTHETIC_DECK: u32 = 2;
/// Words of a point in `Flat::points`: kind (`POINTS`), network, x, y, owned.
pub const POINT_WORDS: usize = 5;
const POINTS: [&str; 3] = ["junction", "joint", "end"];
/// Words of a crossing in `Flat::crossings`: x, y, kind, source, owned.
pub const CROSSING_WORDS: usize = 5;

/// Words of a face in `FaceWords::faces`: id, owner (kind, id), surface, level (`i8` as `i32` bits),
/// height rule, material and crossing (`u32::MAX` for none), rank, inside, `reach_m` (`f32` bits), first
/// ring, ring count.
pub const FACE_WORDS: usize = 13;
/// Words of a ring in `FaceWords::rings`: first index into `ring_vertices`, count.
pub const RING_WORDS: usize = 2;
/// Words of an edge in `FaceWords::edges`: a, b, left, right, kind, flags.
pub const EDGE_WORDS: usize = 6;
/// Words of a corner arc in `FaceWords::corners`: owner (kind, id), centre x, y, `radius_m` (`f64` bits,
/// low word first), `t_a` x, y, `t_b` x, y, first point in `corner_points`, point count.
pub const CORNER_WORDS: usize = 12;
/// Owner kinds; the id of `terrain` is `u32::MAX`.
const OWNERS: [&str; 4] = ["terrain", "way", "junction", "platform"];
const RULES: [(HeightRule, &str); 5] = [
    (HeightRule::Terrain, "terrain"),
    (HeightRule::Slope, "slope"),
    (HeightRule::Owner, "owner"),
    (HeightRule::Blend, "blend"),
    (HeightRule::KerbOffset, "kerb_offset"),
];
const NONE: u32 = u32::MAX;

fn owner(o: Owner) -> [u32; 2] {
    match o {
        Owner::Terrain => [0, NONE],
        Owner::Way(id) => [1, id],
        Owner::Junction(id) => [2, id],
        Owner::Platform(id) => [3, id],
    }
}

/// The faces of one level as flat words: every field of `FacePlan`, its floats as their bits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceWords {
    /// x, y of every vertex.
    pub vertices: Vec<i32>,
    pub faces: Vec<u32>,
    /// A face's first ring is its outer ring; the others are holes.
    pub rings: Vec<u32>,
    /// Indices into the vertices.
    pub ring_vertices: Vec<u32>,
    pub edges: Vec<u32>,
    pub corners: Vec<u32>,
    /// x, y of the points of the corners' arcs.
    pub corner_points: Vec<i32>,
}

impl FaceWords {
    pub fn of(plan: &FacePlan) -> FaceWords {
        let mut w = FaceWords { vertices: plan.vertices.iter().flatten().copied().collect(), ..FaceWords::default() };
        for f in &plan.faces {
            let first = (w.rings.len() / RING_WORDS) as u32;
            for ring in &f.rings {
                w.rings.extend([w.ring_vertices.len() as u32, ring.len() as u32]);
                w.ring_vertices.extend(ring);
            }
            let [kind, id] = owner(f.owner);
            w.faces.extend([
                f.id,
                kind,
                id,
                f.surface as u32,
                i32::from(f.level) as u32,
                code(&RULES, f.rule),
                f.material.unwrap_or(NONE),
                f.crossing.unwrap_or(NONE),
                u32::from(f.rank),
                u32::from(f.inside),
                f.reach_m.to_bits(),
                first,
                f.rings.len() as u32,
            ]);
        }
        w.edges =
            (plan.edges.iter()).flat_map(|e| [e.a, e.b, e.left, e.right, e.kind as u32, u32::from(e.flags)]).collect();
        for c in &plan.corners {
            let [kind, id] = owner(c.owner);
            let r = c.radius_m.to_bits();
            let first = (w.corner_points.len() / 2) as u32;
            w.corners.extend([kind, id, c.centre[0] as u32, c.centre[1] as u32, r as u32, (r >> 32) as u32]);
            w.corners.extend([c.t_a[0], c.t_a[1], c.t_b[0], c.t_b[1]].map(|v| v as u32));
            w.corners.extend([first, c.arc.len() as u32]);
            w.corner_points.extend(c.arc.iter().flatten());
        }
        w
    }
}

const LEVELS: [(Level, &str); 3] = [(Level::Ground, "ground"), (Level::Deck, "deck"), (Level::Tunnel, "tunnel")];
const NETWORKS: [(Network, &str); 3] = [(Network::Way, "way"), (Network::Rail, "rail"), (Network::Water, "water")];
const SOURCES: [(Source, &str); 4] = [
    (Source::Tag, "tag"),
    (Source::SharedVertex, "shared_vertex"),
    (Source::Rule, "rule"),
    (Source::Fallback, "fallback"),
];

fn code<T: PartialEq, U>(table: &[(T, U)], value: T) -> u32 {
    table.iter().position(|(t, _)| *t == value).expect("every value has a code") as u32
}

fn family(f: Family) -> u32 {
    debug::FAMILIES.iter().position(|e| e.0 == f).expect("every family has a code") as u32
}

fn crossing(k: CrossingKind) -> u32 {
    code(&debug::CROSSINGS, k)
}

/// The plan of a cell as flat integer arrays, in the frame of the cell (frame units from its origin),
/// and its stats. The codes index the lists of `vocabulary`.
#[derive(Clone, Debug, PartialEq)]
pub struct Flat {
    pub key: CellKey,
    /// x, y of every sample of the graph.
    pub samples: Vec<i32>,
    pub ways: Vec<u32>,
    pub points: Vec<u32>,
    pub crossings: Vec<u32>,
    /// The faces of level 0.
    pub faces: FaceWords,
    pub stats: String,
}

impl Flat {
    pub fn of(build: &CellBuild) -> Flat {
        let g = &build.graph;
        let samples = g.samples.iter().flat_map(|s| s.pos).collect();
        let synthetic: Vec<u32> = g.decks.iter().filter(|d| d.synthetic).flat_map(|d| d.ways.iter().copied()).collect();
        let mut ways = Vec::with_capacity(WAY_WORDS * g.ways.len());
        for w in &g.ways {
            let flags =
                u32::from(w.link.is_some()) * WAY_LINK + u32::from(synthetic.contains(&w.id)) * WAY_SYNTHETIC_DECK;
            ways.extend([
                w.id,
                family(w.family),
                code(&LEVELS, w.level),
                flags,
                w.samples.start,
                w.samples.len() as u32,
            ]);
        }
        let mut points = Vec::new();
        let mut point = |kind: u32, network: Network, pos: [i32; 2], owned: bool| {
            points.extend([kind, code(&NETWORKS, network), pos[0] as u32, pos[1] as u32, u32::from(owned)]);
        };
        g.junctions.iter().for_each(|j| point(0, j.network, j.pos, j.owned));
        g.joints.iter().for_each(|j| point(1, j.network, j.pos, j.owned));
        g.ends.iter().for_each(|e| point(2, e.network, e.pos, e.owned));
        let crossings = (g.crossings.iter())
            .flat_map(|c| {
                [c.pos[0] as u32, c.pos[1] as u32, crossing(c.kind), code(&SOURCES, c.source), u32::from(c.owned)]
            })
            .collect();
        let mut counters: serde_json::Map<String, Value> =
            g.stats.counters().iter().map(|&(name, n)| (name.to_string(), json!(n))).collect();
        let types: serde_json::Map<String, Value> = (g.stats.crossings.iter())
            .map(|(&(kind, source), &n)| (format!("{kind:?} {}", SOURCES[code(&SOURCES, source) as usize].1), json!(n)))
            .collect();
        counters.insert("crossing_types".into(), Value::Object(types));
        let faces: serde_json::Map<String, Value> =
            build.faces.stats.counters().iter().map(|&(name, n)| (name.to_string(), json!(n))).collect();
        let stats = json!({
            "graph": counters,
            "faces": faces,
            "inputs": {"tiles": build.tiles, "pages": build.pages, "request": build.request},
        })
        .to_string();
        Flat { key: build.key, samples, ways, points, crossings, faces: FaceWords::of(&build.faces), stats }
    }

    /// The digest of the canonical bytes and of every field of the graph, its floats in round-trip
    /// form; an error where the graph holds a NaN. It costs about as much as the build of the graph,
    /// so a caller asks for it when needed.
    pub fn digest(&self, build: &CellBuild) -> Result<String, String> {
        let mut h = DebugHash::default();
        h.hash.write(&self.canonical_bytes());
        write!(h, "{:?}", build.graph).expect("a hash takes any text");
        if h.nan {
            return Err(format!("the graph of cell {}/{} holds a NaN", build.key.x, build.key.y));
        }
        Ok(h.hash.hex())
    }

    /// `b"OGPP"`, the cell, each array with its `u32` length, then the stats. No time is in it. A
    /// later stage writes its floats here as `to_bits` words, so the digest sees every bit of them.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = PLAN_MAGIC.to_vec();
        let words = |out: &mut Vec<u8>, words: &mut dyn Iterator<Item = u32>| {
            let at = out.len();
            out.extend([0; 4]);
            let mut n = 0u32;
            for w in words {
                out.extend(w.to_le_bytes());
                n += 1;
            }
            out[at..at + 4].copy_from_slice(&n.to_le_bytes());
        };
        words(&mut out, &mut [self.key.x, self.key.y].into_iter());
        words(&mut out, &mut self.samples.iter().map(|&v| v as u32));
        words(&mut out, &mut self.ways.iter().copied());
        words(&mut out, &mut self.points.iter().copied());
        words(&mut out, &mut self.crossings.iter().copied());
        let f = &self.faces;
        words(&mut out, &mut f.vertices.iter().map(|&v| v as u32));
        words(&mut out, &mut f.faces.iter().copied());
        words(&mut out, &mut f.rings.iter().copied());
        words(&mut out, &mut f.ring_vertices.iter().copied());
        words(&mut out, &mut f.edges.iter().copied());
        words(&mut out, &mut f.corners.iter().copied());
        words(&mut out, &mut f.corner_points.iter().map(|&v| v as u32));
        out.extend((self.stats.len() as u32).to_le_bytes());
        out.extend(self.stats.as_bytes());
        out
    }
}

/// The names and colours of the codes of `Flat`, in code order, and the stages of `times`, as JSON.
pub fn vocabulary() -> String {
    fn names<T>(table: &[(T, &'static str)]) -> Vec<&'static str> {
        table.iter().map(|(_, n)| *n).collect()
    }
    json!({
        "stages": STAGES,
        "words": {
            "wanted": WANTED_WORDS, "way": WAY_WORDS, "point": POINT_WORDS, "crossing": CROSSING_WORDS,
            "face": FACE_WORDS, "ring": RING_WORDS, "edge": EDGE_WORDS, "corner": CORNER_WORDS,
        },
        "way_flags": {"link": WAY_LINK, "synthetic_deck": WAY_SYNTHETIC_DECK},
        "families": debug::FAMILIES.iter().map(|(_, name, rgb)| json!({"name": name, "colour": debug::hex(*rgb)})).collect::<Vec<_>>(),
        "levels": names(&LEVELS),
        "networks": names(&NETWORKS),
        "points": POINTS,
        "crossings": debug::CROSSINGS.iter().map(|(kind, letter)| json!({"name": format!("{kind:?}"), "letter": letter.to_string()})).collect::<Vec<_>>(),
        "sources": names(&SOURCES),
        "owners": OWNERS,
        "surfaces": debug::SURFACES.iter().map(|(_, name, fill)| json!({"name": name, "fill": fill.map(debug::hex_alpha)})).collect::<Vec<_>>(),
        "junction_surfaces": debug::JUNCTION_SURFACES.iter().map(|(s, fill)| json!({"surface": *s as u8, "fill": debug::hex_alpha(*fill)})).collect::<Vec<_>>(),
        "rules": names(&RULES),
        "edges": debug::EDGES.iter().map(|(_, name, rgb)| json!({"name": name, "colour": debug::hex(*rgb)})).collect::<Vec<_>>(),
        "colours": {
            "link": debug::hex(debug::LINK),
            "synthetic_deck": debug::hex(debug::SYNTHETIC_DECK),
            "junction": debug::hex(debug::JUNCTION),
            "joint": debug::hex(debug::JOINT),
            "end": debug::hex(debug::END),
            "crossing": debug::hex(debug::CROSSING),
        },
    })
    .to_string()
}
