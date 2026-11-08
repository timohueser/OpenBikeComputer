//! The near mesh of a cell: its sharp faces, one ring of ground about them and the strip that meets
//! the terrain grid, as flat 2D topology for the GPU. The CPU builds no height: the GPU gives each
//! corner its height, look fields and normal from the one height function (`terrain/heights.wgsl`
//! with the corner's label), once per cell.
//!
//! The stages: R and its strip S from the sharp faces' region cells of the cell and its neighbours
//! (`region`, `Reads`); the points and constraints of D = (R ∖ S) ∩ cell (`points`); one bulk-loaded
//! constrained Delaunay triangulation with a flood fill for each triangle's face (`cdt`); the strip, a
//! fixed fan of each strip cell (`strip`); then the corners and the step slots.
//!
//! The grid is that of the terrain's nodes of `params::MESH_LEVEL`, and a region cell is a grid cell
//! of the level above (2 × 2 grid cells).
//!
//! A corner is a point with a label. A sharp face's triangles take the face's label; a ground
//! triangle takes at each point the point's one ground corner, labelled by the ground face that holds
//! the point (the face's label is `records::find`'s there). So every face edge between two labels has
//! the two corners of each side at its ends, and its step slot closes it at any heights.
//!
//! # Bytes (`CellMesh::to_bytes`), little-endian u32 words
//!
//! | Words | Holds |
//! | --- | --- |
//! | head | `HEAD` words: `MAGIC`, the cell's x and y (zoom 16), the counts of corners, triangles, strip triangles, triangle runs, step slots and step runs, and of the last triangle runs that draw only over level-18 nodes |
//! | corners | 2 words each (below), sorted by point (y, then x) |
//! | triangle runs | 2 words each: the run's count of triangles and its base corner |
//! | triangles | 3 u16 each, the run's base subtracted, turning as the terrain's `gridIndices` (negative signed area in x east, y south); the strip's triangles last, and last of all, in runs of their own, the strip's triangles of halves that the level-17 terrain draws (`strip`); a zero u16 pads an odd count of u16 |
//! | step runs | 2 words each, as the triangle runs |
//! | steps | 4 u16 each, the run's base subtracted: `a_p, a_q, b_q, b_p` (below) |
//! | region | `REGION_WORDS` words: R on the cell and one ring of region cells, bit (cy + 1) × (`CELLS` + 2) + cx + 1 for region cell (cx, cy) of the cell, −1 ≤ cx, cy ≤ `CELLS` |
//!
//! A run is a stretch of triangles (or slots) whose corners lie within 65,536 of its base: draw it
//! with `baseVertex` = the base (`drawIndexed` with 16-bit indices), and a slot run with its base from
//! the run table.
//!
//! A corner: word 0 holds x in bits 0–18 and the label's bits 0–12 in bits 19–31; word 1 holds y in
//! bits 0–18, the label's bits 13–24 in bits 19–30 and the terrain-vertex flag in bit 31. x and y are
//! frame units from the cell's origin (x east, y south), 0 to 2^18. The label indexes the labels of
//! the cell's near records (the block built with the faces); `NO_LABEL` is the terrain rule (the plain
//! terrain). A terrain vertex is a grid point on ∂R: it draws with the terrain's own vertex code
//! (`vertex_place`, `place_height`), so it slides, morphs and takes the page's height as the terrain
//! grid does; its label is `NO_LABEL`.
//!
//! A step slot stands on a face-edge piece p → q between two labels: a holds the corners at p and q of
//! the face left of p → q (where the shoelace sum of (p, q, point) is positive), b those of the face
//! right of it. Draw every slot as two triangles every frame. Where h(a) − h(b) changes sign between p
//! and q, the two triangles meet at its zero point, so the slot never twists; a slot of equal heights
//! covers no pixel. A slot faces its lower side, so it draws without back-face culling or turns toward
//! that side; the step's look (kerb or wall) comes from the two labels and the step's height.
//!
//! # Sunk points
//!
//! A grid point (i, j) of the cell, 0 ≤ i, j ≤ 2 · `CELLS`, is sunk when every region cell that
//! touches it is in R: cells ⌊(i − 1) / 2⌋ to ⌊i / 2⌋ across, the same for j down (two cells on an
//! even line, one between; in WGSL `(i - 1) >> 1`, as `/` truncates −1 / 2 to 0). The terrain draws a
//! sunk vertex far below the mesh. Every terrain triangle inside R then has a sunk corner, or all
//! three corners on ∂R: a strip ear at a north-west or south-east convex corner of R, which the
//! terrain draws and the strip leaves out. So the terrain never shows through the mesh
//! and needs no floors. "Far" is more than the ground can change over one region cell (a kilometre is
//! safe).
//!
//! The same holds over a level-17 node, whose vertices are the even grid points: the strip draws there
//! as at morph 1 and leaves out each half of a strip cell that is a terrain triangle of level 17
//! (`strip`). It needs the terrain's vertices off their morph to level 16 and without the snap to road
//! edges (`vertex_place` with the snap on moves a vertex up to 15/16 of a grid cell), so the terrain
//! sinks a point only where the cell's mesh draws whole.

pub mod cdt;
pub mod check;
pub mod params;
pub mod points;
pub mod region;
pub mod strip;

use crate::faces::records::find;
use crate::faces::{Face, FacePlan};
use crate::frame::{CellKey, CELL_UNITS};
use params::{sharp, SIDE};
use region::{grid_of, Region, MARK_WORDS};

pub use region::REGION_WORDS;

/// "OBM" and the layout's version.
pub const MAGIC: u32 = 0x4f42_4d03;
pub const HEAD: usize = 10;
/// The label of a corner of the terrain rule.
pub const NO_LABEL: u32 = (1 << 25) - 1;
/// A corner's key: its label, and this bit for a terrain vertex.
const TERRAIN_VERTEX: u32 = 1 << 25;
const NONE: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub enum MeshError {
    /// Two constraints cross: a fault in the faces. The place in frame units from the cell's origin.
    InvalidPlan { at: [i32; 2], why: String },
    /// The corners of one triangle or slot span more than 16 bits of index: the corner order failed.
    Wide,
}

/// What a cell's neighbours read of it: its sharp marks (`region::sharp_cells`) and the coordinates
/// along its north, east, south and west lines of its plan vertices on them.
#[derive(Clone, Debug, PartialEq)]
pub struct Rim {
    pub cell: CellKey,
    pub sharp: Vec<u64>,
    pub lines: [Vec<i32>; 4],
}

impl Rim {
    pub fn of(plan: &FacePlan) -> Rim {
        let c = CELL_UNITS as i32;
        let mut lines: [Vec<i32>; 4] = Default::default();
        for p in &plan.vertices {
            for (side, on, along) in
                [(0, p[1] == 0, p[0]), (1, p[0] == c, p[1]), (2, p[1] == c, p[0]), (3, p[0] == 0, p[1])]
            {
                if on {
                    lines[side].push(along);
                }
            }
        }
        for l in &mut lines {
            l.sort_unstable();
            l.dedup();
        }
        Rim { cell: plan.cell, sharp: region::sharp_cells(plan), lines }
    }

    /// x, y, the sharp marks as pairs of words (low first), then per line its count and coordinates.
    pub fn to_words(&self) -> Vec<u32> {
        let mut out = vec![self.cell.x, self.cell.y];
        out.extend(self.sharp.iter().flat_map(|&w| [w as u32, (w >> 32) as u32]));
        for l in &self.lines {
            out.push(l.len() as u32);
            out.extend(l.iter().map(|&v| v as u32));
        }
        out
    }

    /// The rims of a run of `to_words`.
    pub fn from_words(mut w: &[u32]) -> Result<Vec<Rim>, String> {
        let mut out = Vec::new();
        while !w.is_empty() {
            let short = || "a rim's words end early".to_string();
            let marks = w.get(2..2 + 2 * MARK_WORDS).ok_or_else(short)?;
            let sharp = marks.chunks(2).map(|p| u64::from(p[0]) | u64::from(p[1]) << 32).collect();
            let mut at = 2 + 2 * MARK_WORDS;
            let mut lines: [Vec<i32>; 4] = Default::default();
            for l in &mut lines {
                let n = *w.get(at).ok_or_else(short)? as usize;
                *l = w.get(at + 1..at + 1 + n).ok_or_else(short)?.iter().map(|&v| v as i32).collect();
                at += 1 + n;
            }
            out.push(Rim { cell: CellKey { x: w[0], y: w[1] }, sharp, lines });
            w = &w[at..];
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshStats {
    /// Level-17 cells of the cell in R, S and D.
    pub cells: [u32; 3],
    pub points: u32,
    pub grid_points: u32,
    pub corners: u32,
    pub triangles: u32,
    pub strip_triangles: u32,
    /// Strip triangles that draw only over level-18 nodes.
    pub level18_triangles: u32,
    pub steps: u32,
    /// Sharp faces' edges outside D, sharp faces with no label in the records, and face-edge pieces
    /// that the triangulation split at a point on them: none expected.
    pub outside_d: u32,
    pub unlabelled: u32,
    pub split_edges: u32,
}

/// The stages of `mesh_cell` in their order: `CellMesh::times` holds one time per stage.
pub const STAGES: [&str; 3] = ["points", "triangulation", "corners"];

/// A cell's near mesh: see the module's bytes. The triangles and steps hold whole corner indices,
/// in the order of their runs.
#[derive(Clone, Debug, PartialEq)]
pub struct CellMesh {
    pub cell: CellKey,
    pub corners: Vec<[u32; 2]>,
    pub triangles: Vec<[u32; 3]>,
    pub strip_triangles: u32,
    pub triangle_runs: Vec<[u32; 2]>,
    /// The last runs of `triangle_runs`, which draw only over level-18 nodes.
    pub level18_runs: u32,
    pub steps: Vec<[u32; 4]>,
    pub step_runs: Vec<[u32; 2]>,
    pub region: Vec<u32>,
    pub stats: MeshStats,
    pub times: Vec<f64>,
}

/// Indices less a run's base, two u16 to a word, low first; a zero pads an odd count.
fn packed<const K: usize>(items: &[[u32; K]], runs: &[[u32; 2]]) -> Vec<u32> {
    let bases = runs.iter().flat_map(|&[count, base]| std::iter::repeat_n(base, count as usize));
    let short: Vec<u32> = items.iter().zip(bases).flat_map(|(t, base)| t.map(|c| c - base)).collect();
    short.chunks(2).map(|p| p[0] | p.get(1).map_or(0, |&h| h << 16)).collect()
}

/// Cuts a list of triangles or slots into runs: each the longest stretch from its start whose
/// corners lie within 65,536 of its least corner, the base.
fn runs<const K: usize>(items: &[[u32; K]]) -> Result<Vec<[u32; 2]>, MeshError> {
    let mut out: Vec<[u32; 2]> = Vec::new();
    let mut top = 0;
    for t in items {
        let (lo, hi) = (*t.iter().min().unwrap(), *t.iter().max().unwrap());
        if hi - lo > u32::from(u16::MAX) {
            return Err(MeshError::Wide);
        }
        match out.last_mut() {
            Some(run) if hi.max(top) - lo.min(run[1]) <= u32::from(u16::MAX) => {
                run[0] += 1;
                run[1] = run[1].min(lo);
                top = top.max(hi);
            }
            _ => {
                out.push([1, lo]);
                top = hi;
            }
        }
    }
    Ok(out)
}

impl CellMesh {
    pub fn to_bytes(&self) -> Vec<u8> {
        let head = [
            MAGIC,
            self.cell.x,
            self.cell.y,
            self.corners.len() as u32,
            self.triangles.len() as u32,
            self.strip_triangles,
            self.triangle_runs.len() as u32,
            self.steps.len() as u32,
            self.step_runs.len() as u32,
            self.level18_runs,
        ];
        let words = head
            .into_iter()
            .chain(self.corners.iter().flatten().copied())
            .chain(self.triangle_runs.iter().flatten().copied())
            .chain(packed(&self.triangles, &self.triangle_runs))
            .chain(self.step_runs.iter().flatten().copied())
            .chain(packed(&self.steps, &self.step_runs))
            .chain(self.region.iter().copied());
        words.flat_map(u32::to_le_bytes).collect()
    }

    /// A corner's point, label (`NO_LABEL` for the terrain rule) and terrain-vertex flag.
    pub fn corner(&self, k: usize) -> ([i32; 2], u32, bool) {
        let [w0, w1] = self.corners[k];
        let mask = (1 << 19) - 1;
        ([(w0 & mask) as i32, (w1 & mask) as i32], w0 >> 19 | (w1 >> 19 & 0xfff) << 13, w1 >> 31 == 1)
    }
}

/// The labels of a plan's faces in a block: the label whose words match the face's (surface, owner,
/// rank, rule, material and crossing lines, each the line of its way's leg); none for terrain.
pub fn face_labels(faces: &[Face], words: &[u32]) -> Vec<Option<u32>> {
    use crate::faces::records::{owner, JUNCTION, LABEL, LEG_SHIFT, LINE, PLATFORM, TERRAIN, WAY_OWNER};
    use crate::faces::{Owner, Surface};
    let (labels, lines) = (words[2] as usize, words[3] as usize);
    let line_of = |way: Option<u32>, leg: u8| {
        way.map_or(u32::MAX, |w| {
            let at = |l: usize| &words[words[6] as usize + LINE * l..];
            (0..lines)
                .find(|&l| at(l)[0] == w && (at(l)[1] >> LEG_SHIFT) & 0xff == u32::from(leg))
                .map_or(u32::MAX, |l| l as u32)
        })
    };
    faces
        .iter()
        .map(|f| {
            if f.surface == Surface::Terrain {
                return None;
            }
            let (kind, id) = match f.owner {
                Owner::Terrain => (TERRAIN, 0),
                Owner::Way(w) => (WAY_OWNER, w),
                Owner::Junction(j) => (JUNCTION, j),
                Owner::Platform(p) => (PLATFORM, p),
            };
            let w = [
                f.surface as u32 | kind << 8 | u32::from(f.rank) << 16 | (f.rule as u32) << 24,
                id,
                line_of(f.material, f.legs[0]),
                line_of(f.crossing, f.legs[1]),
            ];
            (0..labels as u32).find(|&l| {
                let x = &words[words[5] as usize + LABEL * l as usize..][..4];
                [x[0], owner(words, l), x[2], x[3]] == w
            })
        })
        .collect()
}

/// The corners of a cell, a short list per point.
struct Corners {
    points: Vec<[i32; 2]>,
    head: Vec<u32>,
    next: Vec<u32>,
    keys: Vec<u32>,
    of: Vec<u32>,
}

impl Corners {
    fn get(&mut self, point: u32, key: u32) -> u32 {
        let mut c = self.head[point as usize];
        while c != NONE {
            if self.keys[c as usize] == key {
                return c;
            }
            c = self.next[c as usize];
        }
        let id = self.keys.len() as u32;
        self.keys.push(key);
        self.of.push(point);
        self.next.push(self.head[point as usize]);
        self.head[point as usize] = id;
        id
    }

    /// Appends the corners of a point.
    fn of_point(&self, point: u32, out: &mut Vec<u32>) {
        let mut c = self.head[point as usize];
        while c != NONE {
            out.push(c);
            c = self.next[c as usize];
        }
    }

    fn add_point(&mut self, p: [i32; 2]) -> u32 {
        self.points.push(p);
        self.head.push(NONE);
        self.points.len() as u32 - 1
    }
}

/// What a cell's mesh reads of the rims: R about the cell, and on each side line with a span of D the
/// plan vertices of both cells on it. Two rim sets with equal reads give the same mesh.
#[derive(PartialEq)]
pub struct Reads {
    region: Region,
    lines: [Vec<i32>; 4],
}

impl Reads {
    /// From the rims of the cell and its eight neighbours; a missing neighbour counts as one with no
    /// sharp face.
    pub fn of(plan: &FacePlan, rims: &[Rim]) -> Reads {
        let cell = plan.cell;
        let own;
        let own = match rims.iter().find(|r| r.cell == cell) {
            Some(r) => r,
            None => {
                own = Rim::of(plan);
                &own
            }
        };
        let near = |r: &Rim| {
            let d = [i64::from(r.cell.x) - i64::from(cell.x), i64::from(r.cell.y) - i64::from(cell.y)];
            (d[0].abs() <= 1 && d[1].abs() <= 1 && d != [0, 0]).then_some([d[0] as i32, d[1] as i32])
        };
        let mut marks = vec![([0, 0], own.sharp.as_slice())];
        marks.extend(rims.iter().filter_map(|r| Some((near(r)?, r.sharp.as_slice()))));
        let region = Region::new(&marks);
        let mut spans = [false; 4];
        for b in region.bounds().into_iter().filter(|b| b.span) {
            if let Some(side) = points::side(b.a, b.b) {
                spans[side] = true;
            }
        }
        // Each side line with a span holds the plan vertices of both cells on it.
        let mut lines: [Vec<i32>; 4] = Default::default();
        for (side, d, opposite) in [(0, [0, -1], 2), (1, [1, 0], 3), (2, [0, 1], 0), (3, [-1, 0], 1)] {
            if !spans[side] {
                continue;
            }
            lines[side] = own.lines[side].clone();
            if let Some(r) = rims.iter().find(|r| near(r) == Some(d)) {
                lines[side].extend(&r.lines[opposite]);
                lines[side].sort_unstable();
                lines[side].dedup();
            }
        }
        Reads { region, lines }
    }
}

/// Triangles or slots with their corners renamed by `rank`, in the order of their least corner (a
/// counting sort, stable).
fn by_least<const K: usize>(items: &[[u32; K]], rank: &[u32]) -> Vec<[u32; K]> {
    let renamed: Vec<[u32; K]> = items.iter().map(|t| t.map(|c| rank[c as usize])).collect();
    let least = |t: &[u32; K]| *t.iter().min().unwrap() as usize;
    let mut at = vec![0; rank.len() + 1];
    for t in &renamed {
        at[least(t) + 1] += 1;
    }
    for k in 1..at.len() {
        at[k] += at[k - 1];
    }
    let mut out = vec![[0; K]; renamed.len()];
    for t in &renamed {
        out[at[least(t)]] = *t;
        at[least(t)] += 1;
    }
    out
}

/// The near mesh of a cell from its plan, its near records (`words`) and what it reads of the rims.
/// `now` gives milliseconds; no output depends on it.
pub fn mesh_cell(
    plan: &FacePlan,
    words: &[u32],
    reads: &Reads,
    now: &mut dyn FnMut() -> f64,
) -> Result<CellMesh, MeshError> {
    let mut times = Vec::with_capacity(STAGES.len());
    let mut last = now();
    let mut lap = |times: &mut Vec<f64>| {
        let t = now();
        times.push(t - last);
        last = t;
    };
    let (cell, region) = (plan.cell, &reads.region);
    let input = points::input(plan, region, &reads.lines, cell.origin().y as f64);
    lap(&mut times);
    let (tris, split_edges) = cdt::triangulate(&input.points, &input.segments, input.edges)?;
    lap(&mut times);
    let labels = face_labels(&plan.faces, words);
    let label_of = |f: u32| labels[f as usize].unwrap_or(NO_LABEL);
    let is_sharp = |f: u32| f != cdt::INSIDE && sharp(plan.faces[f as usize].rule);
    let unlabelled = (0..plan.faces.len()).filter(|&f| sharp(plan.faces[f].rule) && labels[f].is_none()).count() as u32;
    // A point's ground face, from the face edges at it.
    let mut ground_face = vec![NONE; input.points.len()];
    for s in &input.segments[..input.edges] {
        for f in [s.left, s.right].into_iter().filter(|&f| !is_sharp(f)) {
            for v in [s.a, s.b] {
                ground_face[v as usize] = ground_face[v as usize].min(f);
            }
        }
    }
    let raster = points::grid_faces(plan);
    let mut ground = vec![NONE; input.points.len()];
    let ground_key = |v: u32, p: [i32; 2], ground: &mut Vec<u32>| {
        if ground.len() <= v as usize {
            ground.resize(v as usize + 1, NONE);
        }
        if ground[v as usize] == NONE {
            let face = match ground_face.get(v as usize) {
                Some(&f) if f != NONE => f,
                _ => grid_of(p).map_or(NONE, |g| raster[(g[1] * SIDE + g[0]) as usize]),
            };
            ground[v as usize] = if face == NONE || face == crate::faces::OUTSIDE {
                find(words, [f64::from(p[0]), f64::from(p[1])]).unwrap_or(NO_LABEL)
            } else {
                label_of(face)
            };
        }
        ground[v as usize]
    };
    let mut corners = Corners {
        points: input.points.clone(),
        head: vec![NONE; input.points.len()],
        next: Vec::new(),
        keys: Vec::new(),
        of: Vec::new(),
    };
    let mut triangles = Vec::with_capacity(tris.len());
    for (t, class) in &tris {
        let key = |v: u32, ground: &mut Vec<u32>| {
            if is_sharp(*class) {
                label_of(*class)
            } else {
                ground_key(v, input.points[v as usize], ground)
            }
        };
        let k = t.map(|v| key(v, &mut ground));
        triangles.push([0, 1, 2].map(|i| corners.get(t[i], k[i])));
    }
    let mut steps = Vec::new();
    for s in &input.segments[..input.edges] {
        let side = |f: u32, v: u32, ground: &mut Vec<u32>| {
            if is_sharp(f) {
                label_of(f)
            } else {
                ground_key(v, input.points[v as usize], ground)
            }
        };
        let (la, lb) = (side(s.left, s.a, &mut ground), side(s.left, s.b, &mut ground));
        let (ra, rb) = (side(s.right, s.a, &mut ground), side(s.right, s.b, &mut ground));
        if la != ra || lb != rb {
            steps.push([corners.get(s.a, la), corners.get(s.b, lb), corners.get(s.b, rb), corners.get(s.a, ra)]);
        }
    }
    // The strip: its grid points are the triangulation's where D's border holds them.
    let mut grid = vec![NONE; (SIDE * SIDE) as usize];
    for (k, &p) in input.points.iter().enumerate() {
        if let Some(g) = grid_of(p) {
            grid[(g[1] * SIDE + g[0]) as usize] = k as u32;
        }
    }
    let first_strip = triangles.len();
    let mut level18 = Vec::new();
    for j in 0..params::CELLS {
        for i in 0..params::CELLS {
            if !region.strip(i, j) {
                continue;
            }
            let halves = strip::halves(i, j).map(|h| h.iter().all(|g| region.on_boundary(g[0], g[1])));
            for (k, tri) in strip::triangles(i, j).into_iter().enumerate() {
                let terrain = tri.map(|g| region.on_boundary(g[0], g[1]));
                if terrain == [true; 3] && strip::TERRAIN_EARS.contains(&k) {
                    continue;
                }
                let out = if halves[k / 3] { &mut level18 } else { &mut triangles };
                out.push([0, 1, 2].map(|c| {
                    let g = tri[c];
                    let at = (g[1] * SIDE + g[0]) as usize;
                    let p = [g[0] * params::GRID, g[1] * params::GRID];
                    if grid[at] == NONE {
                        grid[at] = corners.add_point(p);
                    }
                    let key = if terrain[c] { NO_LABEL | TERRAIN_VERTEX } else { ground_key(grid[at], p, &mut ground) };
                    corners.get(grid[at], key)
                }));
            }
        }
    }
    // The corners by point (y, then x), so that the corners of a triangle or slot lie near each other
    // in the list: the triangulation's points are sorted, the strip's added points follow them.
    let yx = |v: u32| {
        let p = corners.points[v as usize];
        (p[1], p[0])
    };
    let mut added: Vec<u32> = (input.points.len() as u32..corners.points.len() as u32).collect();
    added.sort_unstable_by_key(|&v| yx(v));
    let (mut order, mut k) = (Vec::with_capacity(corners.keys.len()), 0);
    for v in 0..input.points.len() as u32 {
        while k < added.len() && yx(added[k]) < yx(v) {
            corners.of_point(added[k], &mut order);
            k += 1;
        }
        corners.of_point(v, &mut order);
    }
    for &v in &added[k..] {
        corners.of_point(v, &mut order);
    }
    let mut rank = vec![0; order.len()];
    for (new, &old) in order.iter().enumerate() {
        rank[old as usize] = new as u32;
    }
    let shared = [by_least(&triangles[..first_strip], &rank), by_least(&triangles[first_strip..], &rank)].concat();
    let level18 = by_least(&level18, &rank);
    let mut triangle_runs = runs(&shared)?;
    let level18_runs = runs(&level18)?;
    let level18_count = level18.len() as u32;
    triangle_runs.extend(&level18_runs);
    let triangles = [shared, level18].concat();
    let steps = by_least(&steps, &rank);
    let packed = (order.iter())
        .map(|&c| {
            let (key, v) = (corners.keys[c as usize], corners.of[c as usize]);
            let p = corners.points[v as usize].map(|c| c as u32);
            let label = key & NO_LABEL;
            [p[0] | label << 19, p[1] | (label >> 13) << 19 | (key >> 25) << 31]
        })
        .collect::<Vec<_>>();
    let stats = MeshStats {
        cells: region.counts(),
        points: input.points.len() as u32,
        grid_points: input.grid_points,
        corners: packed.len() as u32,
        triangles: triangles.len() as u32,
        strip_triangles: (triangles.len() - first_strip) as u32,
        level18_triangles: level18_count,
        steps: steps.len() as u32,
        outside_d: input.outside_d,
        unlabelled,
        split_edges,
    };
    lap(&mut times);
    Ok(CellMesh {
        cell,
        corners: packed,
        strip_triangles: stats.strip_triangles,
        triangle_runs,
        level18_runs: level18_runs.len() as u32,
        triangles,
        step_runs: runs(&steps)?,
        steps,
        region: region.bits(),
        stats,
        times,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_ends_where_its_corners_would_span_more_than_16_bits() {
        // From base 5, corner 5 + 65,535 fits and 65,541 starts a run.
        let triangles = [[5, 7, 6], [6, 9, 65_540], [7, 65_541, 8]];
        let runs = runs(&triangles).unwrap();
        assert_eq!(runs, [[2, 5], [1, 7]]);
        assert_eq!(super::runs(&[[0, 65_536, 1]]), Err(MeshError::Wide));
        // The u16 0, 2, 1 | 1, 4, 65535 | 0, 65534, 1 and a zero pad.
        assert_eq!(packed(&triangles, &runs), [2 << 16, 1 | 1 << 16, 4 | 0xffff << 16, 0xfffe << 16, 1]);
    }
}
