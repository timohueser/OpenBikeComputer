//! Candidates: polygons that the rules make, each piece with the key by which it wins, and the
//! labels that a patch takes from them.

use std::collections::BTreeMap;

use super::geom::{area2, bbox, near_edge, winding, P};
use super::types::{HeightRule, Owner, Surface, WayId};

/// What a face is. Patches with equal labels merge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Label {
    pub owner: Owner,
    pub surface: Surface,
    pub rule: HeightRule,
    pub material: Option<WayId>,
    pub crossing: Option<WayId>,
    pub rank: u8,
    pub reach_m: f32,
}

impl Label {
    pub const TERRAIN: Label = Label {
        owner: Owner::Terrain,
        surface: Surface::Terrain,
        rule: HeightRule::Terrain,
        material: None,
        crossing: None,
        rank: u8::MAX,
        reach_m: 0.0,
    };

    fn order(&self) -> (Owner, u8, u8, Option<WayId>, Option<WayId>, u8, u32) {
        (
            self.owner,
            self.surface as u8,
            self.rule as u8,
            self.material,
            self.crossing,
            self.rank,
            self.reach_m.to_bits(),
        )
    }
}

/// The order in which pieces win, least first: rank, the class order of the material way, the
/// owner's kind, a global identity of the owner (so two cells order alike), the number within its rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub rank: u8,
    pub class: u8,
    pub kind: u8,
    pub gid: [i64; 5],
    pub number: u32,
}

impl Key {
    pub const TERRAIN: Key = Key { rank: u8::MAX, class: u8::MAX, kind: u8::MAX, gid: [i64::MAX; 5], number: u32::MAX };
}

/// The owner kinds in key order.
pub const KIND_JUNCTION: u8 = 0;
pub const KIND_BEND: u8 = 1;
pub const KIND_WAY: u8 = 2;
pub const KIND_PLATFORM: u8 = 3;

pub struct Candidate {
    pub label: u32,
    /// A region where the candidate does not hold (the half disk past a host's middle line).
    pub minus: Vec<Vec<P>>,
}

pub struct Piece {
    pub cand: u32,
    pub key: Key,
    /// A point is in the piece where the winding number of its rings is not 0.
    pub rings: Vec<Vec<P>>,
    pub bbox: [i32; 4],
}

/// The labels by id, each once.
#[derive(Default)]
pub struct Labels {
    pub list: Vec<Label>,
    /// The key of the piece that made each label: a thin piece's target compares by it.
    pub keys: Vec<Key>,
    index: BTreeMap<(Owner, u8, u8, Option<WayId>, Option<WayId>, u8, u32), u32>,
}

impl Labels {
    pub fn new() -> Labels {
        let mut labels = Labels::default();
        labels.id(Label::TERRAIN, Key::TERRAIN);
        labels
    }

    /// The id of a label; a new label keeps the key it first came with.
    pub fn id(&mut self, label: Label, key: Key) -> u32 {
        let next = self.list.len() as u32;
        let id = *self.index.entry(label.order()).or_insert(next);
        if id == next {
            self.list.push(label);
            self.keys.push(key);
        } else if key < self.keys[id as usize] {
            self.keys[id as usize] = key;
        }
        id
    }
}

pub const TERRAIN: u32 = 0;

/// The candidates of one cell and level, with a grid over their pieces' boxes.
pub struct Cands {
    pub labels: Labels,
    pub cands: Vec<Candidate>,
    pub pieces: Vec<Piece>,
    grid: Grid,
}

/// A uniform grid of piece indices over the window.
pub struct Grid {
    origin: [i32; 2],
    shift: u32,
    n: [i32; 2],
    cells: Vec<Vec<u32>>,
}

impl Grid {
    pub fn new(window: [i32; 4], shift: u32) -> Grid {
        let n = [((window[2] - window[0]) >> shift) + 1, ((window[3] - window[1]) >> shift) + 1];
        Grid { origin: [window[0], window[1]], shift, n, cells: vec![Vec::new(); (n[0] * n[1]) as usize] }
    }

    fn range(&self, b: [i32; 4]) -> Option<[i32; 4]> {
        let c = |v: i32, o: i32, n: i32| ((v.saturating_sub(o)) >> self.shift).clamp(0, n - 1);
        let r = [
            c(b[0], self.origin[0], self.n[0]),
            c(b[1], self.origin[1], self.n[1]),
            c(b[2], self.origin[0], self.n[0]),
            c(b[3], self.origin[1], self.n[1]),
        ];
        let outside = b[2] < self.origin[0] || b[3] < self.origin[1];
        (!outside).then_some(r)
    }

    pub fn insert(&mut self, b: [i32; 4], id: u32) {
        if let Some([x0, y0, x1, y1]) = self.range(b) {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    self.cells[(y * self.n[0] + x) as usize].push(id);
                }
            }
        }
    }

    /// The ids whose boxes may hold a doubled point.
    pub fn at(&self, q: [i64; 2]) -> &[u32] {
        let p = [(q[0] >> 1) as i32, (q[1] >> 1) as i32];
        match self.range([p[0], p[1], p[0], p[1]]) {
            Some([x, y, ..]) => &self.cells[(y * self.n[0] + x) as usize],
            None => &[],
        }
    }
}

fn holds(b: [i32; 4], q: [i64; 2]) -> bool {
    let (x, y) = (q[0], q[1]);
    2 * i64::from(b[0]) <= x && x <= 2 * i64::from(b[2]) && 2 * i64::from(b[1]) <= y && y <= 2 * i64::from(b[3])
}

impl Cands {
    pub fn new(window: [i32; 4]) -> Cands {
        Cands { labels: Labels::new(), cands: Vec::new(), pieces: Vec::new(), grid: Grid::new(window, 13) }
    }

    pub fn add(&mut self, label: Label, key: Key) -> u32 {
        let label = self.labels.id(label, key);
        self.cands.push(Candidate { label, minus: Vec::new() });
        self.cands.len() as u32 - 1
    }

    /// A piece of one ring, turned positive; a ring of zero area is dropped.
    pub fn ring(&mut self, cand: u32, key: Key, mut ring: Vec<P>) {
        ring.dedup();
        while ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        if ring.len() < 3 {
            return;
        }
        let a = area2(&ring);
        if a == 0 {
            return;
        }
        if a < 0 {
            ring.reverse();
        }
        self.rings(cand, key, vec![ring]);
    }

    /// A piece of rings as they are (a platform with holes).
    pub fn rings(&mut self, cand: u32, key: Key, rings: Vec<Vec<P>>) {
        let b = bbox(&rings);
        let id = self.pieces.len() as u32;
        self.grid.insert(b, id);
        self.labels.id(self.labels.list[self.cands[cand as usize].label as usize], key);
        self.pieces.push(Piece { cand, key, rings, bbox: b });
    }

    /// The least key of the pieces that hold a doubled point (or touch it, with `edge`), with the
    /// least rail way among the rail beds that hold it.
    pub fn cover(&self, q: [i64; 2], edge: bool) -> (Option<(Key, u32)>, Option<WayId>) {
        let mut best: Option<(Key, u32)> = None;
        let mut rail: Option<WayId> = None;
        for &id in self.grid.at(q) {
            let piece = &self.pieces[id as usize];
            if !holds(piece.bbox, q) || best.is_some_and(|(k, _)| k <= piece.key) && !self.is_bed(piece) {
                continue;
            }
            let inside = winding(&piece.rings, q) != 0 || edge && near_edge(&piece.rings, q, 0);
            let cand = &self.cands[piece.cand as usize];
            if !inside || !cand.minus.is_empty() && winding(&cand.minus, q) != 0 {
                continue;
            }
            let label = &self.labels.list[cand.label as usize];
            if let (Surface::RailBed, Owner::Way(w)) = (label.surface, label.owner) {
                rail = Some(rail.map_or(w, |r| r.min(w)));
            }
            if best.is_none_or(|(k, _)| piece.key < k) {
                best = Some((piece.key, cand.label));
            }
        }
        (best, rail)
    }

    /// PROBE: every candidate with a piece that holds a doubled point.
    pub fn covering(&self, q: [i64; 2]) -> Vec<u32> {
        let mut out: Vec<u32> = (self.grid.at(q).iter())
            .map(|&id| &self.pieces[id as usize])
            .filter(|p| holds(p.bbox, q) && winding(&p.rings, q) != 0)
            .map(|p| p.cand)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// PROBE: every candidate with a piece whose box holds a doubled point.
    pub fn boxed(&self, q: [i64; 2]) -> Vec<u32> {
        let mut out: Vec<u32> =
            self.grid.at(q).iter().map(|&id| &self.pieces[id as usize]).filter(|p| holds(p.bbox, q)).map(|p| p.cand).collect();
        out.sort();
        out.dedup();
        out
    }

    fn is_bed(&self, piece: &Piece) -> bool {
        self.labels.list[self.cands[piece.cand as usize].label as usize].surface == Surface::RailBed
    }

    /// The label of a patch: by its inner point, else by its vertices; then the crossing rule.
    pub fn label(&mut self, q: Option<[i64; 2]>, vertices: &[P]) -> u32 {
        let (best, rail) = match q {
            Some(q) => self.cover(q, false),
            None => {
                let mut out: (Option<(Key, u32)>, Option<WayId>) = (None, None);
                for v in vertices {
                    let (b, r) = self.cover([2 * i64::from(v[0]), 2 * i64::from(v[1])], true);
                    if b.is_some_and(|b| out.0.is_none_or(|o| b.0 < o.0)) {
                        out.0 = b;
                    }
                    out.1 = match (out.1, r) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    };
                }
                out
            }
        };
        let Some((key, label)) = best else { return TERRAIN };
        let l = self.labels.list[label as usize];
        match (l.surface, rail) {
            (Surface::Carriageway, Some(rail)) => {
                self.labels.id(Label { surface: Surface::Crossing, crossing: Some(rail), ..l }, key)
            }
            _ => label,
        }
    }

    /// The outlines that the slice takes.
    pub fn strings(&self) -> impl Iterator<Item = &Vec<P>> {
        self.pieces.iter().flat_map(|p| &p.rings).chain(self.cands.iter().flat_map(|c| &c.minus))
    }
}
