//! The measure of the openings: the ground of the ground plan about each, as the device builds it, with the road's cut
//! in it and the stretch to a moved opening on the ground.

use std::collections::BTreeMap;

use obc_ground_plan::faces::heights::check;
use obc_ground_plan::faces::{records, Rules};
use obc_ground_plan::frame::{CellKey, GlobalPoint, FRAME_BITS};
use obc_ground_plan::graph::{self, CrossingKind, Family, Level, OpenEnd};
use obc_ground_plan::terrain::Terrain as Pages;
use obc_ground_plan::tile::Geometry;

use super::{left_of, offset, Input};
use crate::levels::{run, Reader};
use crate::tiles::{Tile, MAX_ZOOM};

/// What to read about one opening: its point and unit direction into the tube, the metres across its frame row to the
/// right and to the left, per metre in from it the points of the centre line and of both outer walls, the ids of its
/// chain's ways, and other points whose ground it needs.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ask {
    pub p: [f64; 2],
    pub dir: [f64; 2],
    pub right: u32,
    pub left: u32,
    pub stations: Vec<[[f64; 2]; 3]>,
    pub ids: Vec<u32>,
    pub points: Vec<[f64; 2]>,
}

/// What the ground plan gives about an opening: the ground at each station point (centre, left wall, right wall); the
/// ground each metre along the frame row, from `right` metres right to `left` metres left; per deck way over the row
/// its metres to the left and its underside; the room of each crossing where a way of the chain is the lower way;
/// whether a building ring holds a station of the centre line; the ground at the ask's other points; and the faces of
/// the ground plan about those points (`Field`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Site {
    pub stations: Vec<[Option<f64>; 3]>,
    pub row: Vec<Option<f64>>,
    pub decks: Vec<(f64, f64)>,
    pub rooms: Vec<f64>,
    pub building: bool,
    pub points: Vec<Option<f64>>,
    pub fields: Vec<Field>,
}

/// A face of the ground plan near an ask's other points: its rings (frame points of the ground plan, the outer ring
/// first) and the height of its own ground, by its rule, at each of those points within `FIELD_M` of its box, by index.
/// The ground is smooth inside a face; it steps only at a face's edge.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Field {
    pub rings: Vec<Vec<[i64; 2]>>,
    pub heights: Vec<(u32, f64)>,
}

/// A face reads its ground this far past its box (m): a sample step and a half, so each grid cell that meets the face
/// has its four corners.
const FIELD_M: f64 = 1.0;

impl Site {
    /// The ground at metres `m` to the left of the axis on the row (negative: right), linear between its metres.
    pub fn across(&self, right: u32, m: f64) -> Option<f64> {
        let x = m + f64::from(right);
        if x < 0.0 || x > (self.row.len() - 1) as f64 {
            return None;
        }
        let (k, f) = (x.floor() as usize, x - x.floor());
        let a = self.row[k]?;
        if f <= 0.0 {
            return Some(a);
        }
        Some(a + f * (self.row.get(k + 1).copied().flatten()? - a))
    }
}

/// A cell as the measure reads it: its record words and terrain for the heights, the rooms of the crossings over a
/// tunnel way (its feature id and the room), the deck undersides (two frame points and their undersides) and the
/// building rings.
struct Cell {
    words: Vec<u32>,
    terrain: Pages,
    rooms: Vec<(u32, f64)>,
    decks: Vec<([f64; 2], [f64; 2], f64, f64)>,
    buildings: Vec<Vec<Vec<[i64; 2]>>>,
    faces: Vec<Face>,
}

/// A face of level 0 of a cell: its rings in frame points, and its label.
type Face = (Vec<Vec<[i64; 2]>>, Option<u32>);

/// A job's cell: its key, its reads and the asks that read its faces.
type Job = (CellKey, Reads, Vec<usize>);

const SQUARE: f64 = (1u64 << FRAME_BITS) as f64;

fn frame(p: [f64; 2]) -> [f64; 2] {
    [p[0] * SQUARE, p[1] * SQUARE]
}

fn key_of(q: [f64; 2]) -> CellKey {
    CellKey::of(GlobalPoint { x: q[0].floor() as i64, y: q[1].floor() as i64 })
}

/// The cell built as the device builds it, with the open stretches of `ends` near it, and what the measure reads.
fn cell(key: CellKey, reader: &mut Reader, ends: &[OpenEnd], id: &dyn Fn(u32, u32, u32) -> Option<u32>) -> Cell {
    let rules = Rules::embedded();
    let input = reader.input(key);
    let [x0, y0, x1, y1] = key.grown();
    let near: Vec<OpenEnd> =
        (ends.iter().copied()).filter(|e| (x0..=x1).contains(&e.way.x) && (y0..=y1).contains(&e.way.y)).collect();
    let band = rules.structures.edge_band_m;
    let mut g = graph::build_open(key, &input, &rules.sections, Some(band), &near);
    let (plan, near_records, _, _) = records::near_under(&g, &input, rules, key);
    let o = key.origin();
    let labels = obc_ground_plan::mesh::face_labels(&plan.faces, &near_records.words);
    let faces = (plan.faces.iter().zip(labels))
        .filter(|(f, _)| f.level == 0)
        .map(|(f, label)| {
            let ring = |r: &Vec<u32>| {
                r.iter()
                    .map(|&v| plan.vertices[v as usize])
                    .map(|p| [o.x + i64::from(p[0]), o.y + i64::from(p[1])])
                    .collect()
            };
            (f.rings.iter().map(ring).collect(), label)
        })
        .collect();
    let global = |pos: [i32; 2]| [(o.x + i64::from(pos[0])) as f64, (o.y + i64::from(pos[1])) as f64];
    let feature =
        |g: &graph::WayGraph, w: u32| &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
    let mut decks = Vec::new();
    for (w, way) in g.ways.iter().enumerate() {
        if way.level != Level::Deck || way.family == Family::Water {
            continue;
        }
        let f = feature(&g, w as u32);
        let depth = g.deck_depth(&input, w as u32, &rules.sections.way(&f.kind, &f.tags, false));
        let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
        for pair in samples.windows(2) {
            decks.push((global(pair[0].pos), global(pair[1].pos), pair[0].z_m - depth, pair[1].z_m - depth));
        }
    }
    // The ladders after the faces: the faces read none.
    graph::levels(&mut g, &input, &rules.sections, &rules.structures);
    let mut rooms = Vec::new();
    for c in g.crossings.iter().filter(|c| c.kind != CrossingKind::LevelCrossing) {
        let Some(l) = &c.ladder else { continue };
        let (lower, upper) = (&g.ways[l.lower as usize], &g.ways[l.upper as usize]);
        if lower.level != Level::Tunnel || upper.family == Family::Water || lower.family == Family::Water {
            continue;
        }
        let f = feature(&g, l.lower);
        if let Some(id) = id(f.parts[0].tile.0, f.parts[0].tile.1, f.index) {
            rooms.push((id, l.room_m));
        }
    }
    let buildings = (input.features.iter())
        .filter(|f| f.kind == "building" && f.geometry == Geometry::Area)
        .map(|f| f.parts.iter().map(|p| p.points.iter().map(|q| [q.x, q.y]).collect()).collect())
        .collect();
    Cell { words: near_records.words, terrain: input.terrain, rooms, decks, buildings, faces }
}

/// Whether the rings of a polygon (outer and holes) hold frame point q, even-odd.
fn holds(rings: &[Vec<[i64; 2]>], q: [f64; 2]) -> bool {
    let mut inside = false;
    for ring in rings.iter().filter(|r| r.len() >= 3) {
        for i in 0..ring.len() {
            let (a, b) = (ring[i].map(|v| v as f64), ring[(i + 1) % ring.len()].map(|v| v as f64));
            if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Where segment p-q meets segment a-b: the share along p-q and along a-b.
pub(crate) fn meet(p: [f64; 2], q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<(f64, f64)> {
    let (r, s) = ([q[0] - p[0], q[1] - p[1]], [b[0] - a[0], b[1] - a[1]]);
    let den = r[0] * s[1] - r[1] * s[0];
    if den == 0.0 {
        return None;
    }
    let w = [a[0] - p[0], a[1] - p[1]];
    let (t, u) = ((w[0] * s[1] - w[1] * s[0]) / den, (w[0] * r[1] - w[1] * r[0]) / den);
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

/// A point that an ask reads: the ask, its station and the point there (centre, left wall, right wall), its place on
/// the row, or one of its other points.
#[derive(Clone, Copy)]
enum Read {
    Station(usize, usize, usize),
    Row(usize, usize),
    Point(usize, usize),
}

/// The reads in one cell, each with its world point.
type Reads = Vec<(Read, [f64; 2])>;

/// What a cell answers: the ground at its points, and per ask that reads in it the decks over its row, the rooms of the
/// ways on top of its chain and whether a building holds a station of its centre line in the cell.
type Answers = (Vec<(Read, Option<f64>)>, Vec<(usize, Vec<(f64, f64)>, Vec<f64>, bool)>, Vec<(usize, Field)>);

/// The box [x0, y0, x1, y1] of frame points, grown by `by` frame units.
fn frame_box(points: impl Iterator<Item = [f64; 2]>, by: f64) -> [f64; 4] {
    let b = points.fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
        [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])]
    });
    [b[0] - by, b[1] - by, b[2] + by, b[3] + by]
}

fn boxes_meet(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// The sites of the asks. Each cell that holds a point to read is built once: a job takes the cells of one zoom-13
/// tile, one at a time; as many jobs run at once as `memory` holds.
pub(crate) fn sites(asks: &[Ask], input: &Input, ends: &[OpenEnd], memory: usize) -> (Vec<Site>, usize) {
    let z13 = |c: u32, r: u32| input.tiles.tiles.get(&(MAX_ZOOM, c, r));
    let id = |c: u32, r: u32, index: u32| z13(c, r).and_then(|t: &Tile| t.id(index));
    let mut sites: Vec<Site> = (asks.iter())
        .map(|a| Site {
            stations: vec![[None; 3]; a.stations.len()],
            row: vec![None; (a.right + a.left + 1) as usize],
            points: vec![None; a.points.len()],
            ..Site::default()
        })
        .collect();
    // The reads by cell, the cells by zoom-13 tile.
    let mut by_cell: BTreeMap<CellKey, Reads> = BTreeMap::new();
    for (k, a) in asks.iter().enumerate() {
        for (i, st) in a.stations.iter().enumerate() {
            for (w, &p) in st.iter().enumerate() {
                by_cell.entry(key_of(frame(p))).or_default().push((Read::Station(k, i, w), p));
            }
        }
        let left = left_of(a.dir);
        for j in 0..=(a.right + a.left) {
            let p = offset(a.p, left, f64::from(j) - f64::from(a.right));
            by_cell.entry(key_of(frame(p))).or_default().push((Read::Row(k, j as usize), p));
        }
        for (i, &p) in a.points.iter().enumerate() {
            by_cell.entry(key_of(frame(p))).or_default().push((Read::Point(k, i), p));
        }
    }
    // The faces of each cell that the box of an ask's other points meets, grown by FIELD_M.
    let grow = |p: [f64; 2]| FIELD_M / obc_ground_plan::frame::metres_per_unit_at(p[1]);
    let areas: Vec<Option<[f64; 4]>> = (asks.iter())
        .map(|a| a.points.first().map(|&p| frame_box(a.points.iter().map(|&q| frame(q)), grow(frame(p)))))
        .collect();
    let mut fielded: BTreeMap<CellKey, Vec<usize>> = BTreeMap::new();
    for (k, area) in areas.iter().enumerate() {
        let Some(b) = *area else { continue };
        let (lo, hi) = (key_of([b[0], b[1]]), key_of([b[2], b[3]]));
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                fielded.entry(CellKey { x, y }).or_default().push(k);
                by_cell.entry(CellKey { x, y }).or_default();
            }
        }
    }
    let cells = by_cell.len();
    let shift = obc_ground_plan::frame::CELL_ZOOM - u32::from(MAX_ZOOM);
    let mut jobs: BTreeMap<(u32, u32), Vec<Job>> = BTreeMap::new();
    for (key, reads) in by_cell {
        let fields = fielded.remove(&key).unwrap_or_default();
        jobs.entry((key.x >> shift, key.y >> shift)).or_default().push((key, reads, fields));
    }
    let jobs: Vec<_> = jobs.into_values().collect();
    let done: Vec<Vec<Answers>> = run(&jobs, memory, |job| {
        let mut reader = Reader::new(&z13, input.terrain, Some(input.levels));
        job.iter()
            .map(|(key, reads, fielded)| {
                let c = cell(*key, &mut reader, ends, &id);
                let o = key.origin();
                let ground = |p: [f64; 2]| {
                    let q = frame(p);
                    check::at(&c.words, &c.terrain, *key, [q[0] - o.x as f64, q[1] - o.y as f64]).map(|(h, _)| h.h)
                };
                let heights: Vec<(Read, Option<f64>)> = reads.iter().map(|&(r, p)| (r, ground(p))).collect();
                let mut readers: Vec<usize> = (reads.iter())
                    .map(|(r, _)| match *r {
                        Read::Station(k, ..) | Read::Row(k, _) | Read::Point(k, _) => k,
                    })
                    .collect();
                readers.dedup();
                readers.sort_unstable();
                readers.dedup();
                let per_ask = (readers.into_iter())
                    .map(|k| {
                        let a = &asks[k];
                        let left = left_of(a.dir);
                        let (r0, r1) = (
                            frame(offset(a.p, left, -f64::from(a.right))),
                            frame(offset(a.p, left, f64::from(a.left))),
                        );
                        let width = f64::from(a.right + a.left);
                        let decks = (c.decks.iter())
                            .filter_map(|&(da, db, ua, ub)| {
                                let (t, u) = meet(r0, r1, da, db)?;
                                Some((t * width - f64::from(a.right), ua + u * (ub - ua)))
                            })
                            .collect();
                        let rooms = c.rooms.iter().filter(|r| a.ids.contains(&r.0)).map(|r| r.1).collect();
                        let building = (reads.iter())
                            .filter(|(r, _)| matches!(*r, Read::Station(j, _, 0) if j == k))
                            .any(|(_, p)| c.buildings.iter().any(|b| holds(b, frame(*p))));
                        (k, decks, rooms, building)
                    })
                    .collect();
                // The faces about each ask's other points, and the ground of each by its own rule there.
                let mut fields = Vec::new();
                for &k in fielded {
                    let area = areas[k].expect("an ask with points");
                    let points: Vec<[f64; 2]> = asks[k].points.iter().map(|&p| frame(p)).collect();
                    for (rings, label) in &c.faces {
                        let near = frame_box(rings[0].iter().map(|q| [q[0] as f64, q[1] as f64]), grow(points[0]));
                        if !boxes_meet(near, area) {
                            continue;
                        }
                        let heights: Vec<(u32, f64)> = (points.iter().enumerate())
                            .filter(|(_, q)| boxes_meet(near, [q[0], q[1], q[0], q[1]]))
                            .filter_map(|(i, q)| {
                                let local = [q[0] - o.x as f64, q[1] - o.y as f64];
                                check::at_rule(&c.words, &c.terrain, *key, local, *label).map(|h| (i as u32, h.h))
                            })
                            .collect();
                        fields.push((k, Field { rings: rings.clone(), heights }));
                    }
                }
                (heights, per_ask, fields)
            })
            .collect()
    });
    for (heights, per_ask, fields) in done.into_iter().flatten() {
        for (k, field) in fields {
            sites[k].fields.push(field);
        }
        for (r, h) in heights {
            match r {
                Read::Station(k, i, w) => sites[k].stations[i][w] = h,
                Read::Row(k, j) => sites[k].row[j] = h,
                Read::Point(k, i) => sites[k].points[i] = h,
            }
        }
        for (k, decks, rooms, building) in per_ask {
            let site = &mut sites[k];
            site.decks.extend(decks);
            site.rooms.extend(rooms);
            site.building |= building;
        }
    }
    for site in &mut sites {
        site.decks.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)));
        site.decks.dedup();
        site.rooms.sort_by(f64::total_cmp);
    }
    (sites, cells)
}

/// The ask of an opening: `in_m` metres in along its chain from `s` (in `sign` of the chain's metres), with its walls at
/// `wall` metres across, and its row.
pub(crate) fn ask(
    curve: &super::Curve,
    (s, sign): (f64, f64),
    (p, dir): ([f64; 2], [f64; 2]),
    (right, left): (u32, u32),
    in_m: f64,
    wall: f64,
    ids: Vec<u32>,
) -> Ask {
    let mut stations = Vec::new();
    let mut k = 0.0;
    while k <= in_m {
        let at = s + sign * k;
        if !(0.0..=curve.len).contains(&at) {
            break;
        }
        let c = curve.at(at).0.p;
        let d = curve.dir(at);
        let l = left_of(d);
        stations.push([c, offset(c, l, wall), offset(c, l, -wall)]);
        k += 1.0;
    }
    Ask { p, dir, right, left, stations, ids, points: Vec::new() }
}
