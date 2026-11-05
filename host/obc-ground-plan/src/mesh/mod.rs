//! The near mesh of a cell: its sharp faces, one ring of ground about them and the strip that meets
//! the terrain grid, as flat 2D topology for the GPU. The CPU builds no height: the GPU gives each
//! corner its height, look fields and normal from the one height function (`terrain/heights.wgsl`
//! with the corner's label), once per cell.
//!
//! The stages: R and its strip S from the sharp faces' level-17 cells of the cell and its neighbours
//! (`region`); the points and constraints of D = (R ∖ S) ∩ cell (`points`); one bulk-loaded
//! constrained Delaunay triangulation with a flood fill for each triangle's face (`cdt`); the strip,
//! the terrain's own triangles of each strip cell (`strip`); then the corners and the step slots.
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
//! | head | `HEAD` words: `MAGIC`, the cell's x and y (zoom 16), the counts of corners, triangles, strip triangles and step slots |
//! | corners | 2 words each (below) |
//! | triangles | 3 corner indices each, turning as the terrain's `gridIndices` (negative signed area in x east, y south); the strip's triangles last |
//! | steps | 4 corner indices each: `a_p, a_q, b_q, b_p` (below) |
//! | sunk | `SUNK_WORDS` words: bit j × 257 + i for level-18 grid point (i, j) of the cell, both borders included |
//!
//! A corner: word 0 holds x in bits 0–18 and the label's bits 0–12 in bits 19–31; word 1 holds y in
//! bits 0–18, the label's bits 13–24 in bits 19–30 and the terrain-vertex flag in bit 31. x and y are
//! frame units from the cell's origin (x east, y south), 0 to 2^18. The label indexes the labels of
//! the cell's near records (the block built with the faces); `NO_LABEL` is the terrain rule (the plain
//! terrain). A terrain vertex is a level-18 grid point on ∂R: it draws with the terrain's own vertex
//! code (`vertex_place`, `place_height`), so it slides, morphs and takes the page's height as the
//! terrain grid does; its label is `NO_LABEL`.
//!
//! A step slot stands on a face-edge piece p → q between two labels: a holds the corners at p and q of
//! the face left of p → q (where the shoelace sum of (p, q, point) is positive), b those of the face
//! right of it. Draw every slot as two triangles every frame. Where h(a) − h(b) changes sign between p
//! and q, the two triangles meet at its zero point, so the slot never twists; a slot of equal heights
//! covers no pixel. A slot faces its lower side, so it draws without back-face culling or turns toward
//! that side; the step's look (kerb or wall) comes from the two labels and the step's height.
//!
//! A sunk point lies strictly inside R: the terrain draws its vertex there far below the mesh. Every
//! terrain triangle inside R then has a sunk corner, or all three corners on ∂R and is the strip's own
//! triangle, so the terrain never shows through the mesh and needs no floors. "Far" is more than the
//! ground can change over one level-17 cell (a kilometre is safe); the terrain sinks a point only
//! where the cell's mesh draws.

pub mod cdt;
pub mod check;
pub mod params;
pub mod points;
pub mod region;
pub mod strip;

use crate::faces::records::find;
use crate::faces::{Face, FacePlan};
use crate::frame::{CellKey, CELL_UNITS};
use params::{sharp, SIDE18};
use region::{grid_of, Region, MARK_WORDS};

/// "OBM" and the layout's version.
pub const MAGIC: u32 = 0x4f42_4d01;
pub const HEAD: usize = 7;
/// The label of a corner of the terrain rule.
pub const NO_LABEL: u32 = (1 << 25) - 1;
pub const SUNK_WORDS: usize = (SIDE18 * SIDE18) as usize / 32 + 1;
/// A corner's key: its label, and this bit for a terrain vertex.
const TERRAIN_VERTEX: u32 = 1 << 25;
const NONE: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub enum MeshError {
    /// Two constraints cross: a fault in the faces. The place in frame units from the cell's origin.
    InvalidPlan { at: [i32; 2], why: String },
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
    pub steps: u32,
    /// Sharp faces' edges outside D, sharp faces with no label in the records, and face-edge pieces
    /// that the triangulation split at a point on them: none expected.
    pub outside_d: u32,
    pub unlabelled: u32,
    pub split_edges: u32,
}

/// The stages of `mesh_cell` in their order: `CellMesh::times` holds one time per stage.
pub const STAGES: [&str; 4] = ["region", "points", "triangulation", "corners"];

/// A cell's near mesh: see the module's bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct CellMesh {
    pub cell: CellKey,
    pub corners: Vec<[u32; 2]>,
    pub triangles: Vec<[u32; 3]>,
    pub strip_triangles: u32,
    pub steps: Vec<[u32; 4]>,
    pub sunk: Vec<u32>,
    pub stats: MeshStats,
    pub times: Vec<f64>,
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
            self.steps.len() as u32,
        ];
        let words = head
            .into_iter()
            .chain(self.corners.iter().flatten().copied())
            .chain(self.triangles.iter().flatten().copied())
            .chain(self.steps.iter().flatten().copied())
            .chain(self.sunk.iter().copied());
        words.flat_map(u32::to_le_bytes).collect()
    }

    /// A corner's point, label (`NO_LABEL` for the terrain rule) and terrain-vertex flag.
    pub fn corner(&self, k: usize) -> ([i32; 2], u32, bool) {
        let [w0, w1] = self.corners[k];
        let mask = (1 << 19) - 1;
        ([(w0 & mask) as i32, (w1 & mask) as i32], w0 >> 19 | (w1 >> 19 & 0xfff) << 13, w1 >> 31 == 1)
    }

    /// Whether level-18 grid point (i, j) of the cell is sunk.
    pub fn is_sunk(&self, i: i32, j: i32) -> bool {
        let k = (j * SIDE18 + i) as usize;
        self.sunk[k / 32] >> (k % 32) & 1 == 1
    }
}

/// The labels of a plan's faces in a block: the label whose words match the face's (surface, owner,
/// rank, rule, material and crossing lines); none for terrain.
pub fn face_labels(faces: &[Face], words: &[u32]) -> Vec<Option<u32>> {
    use crate::faces::records::{JUNCTION, LABEL, LINE, PLATFORM, TERRAIN, WAY_OWNER};
    use crate::faces::{Owner, Surface};
    let (labels, lines) = (words[2] as usize, words[3] as usize);
    let line_of = |way: Option<u32>| {
        way.map_or(u32::MAX, |w| {
            (0..lines).find(|&l| words[words[6] as usize + LINE * l] == w).map_or(u32::MAX, |l| l as u32)
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
                line_of(f.material),
                line_of(f.crossing),
            ];
            (0..labels as u32).find(|&l| words[words[5] as usize + LABEL * l as usize..][..4] == w)
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

    fn add_point(&mut self, p: [i32; 2]) -> u32 {
        self.points.push(p);
        self.head.push(NONE);
        self.points.len() as u32 - 1
    }
}

/// The near mesh of a cell from its plan, its near records (`words`) and the rims of the cell and its
/// eight neighbours; a missing neighbour counts as one with no sharp face. `now` gives milliseconds;
/// no output depends on it.
pub fn mesh_cell(
    plan: &FacePlan,
    words: &[u32],
    rims: &[Rim],
    now: &mut dyn FnMut() -> f64,
) -> Result<CellMesh, MeshError> {
    let mut times = Vec::with_capacity(STAGES.len());
    let mut last = now();
    let mut lap = |times: &mut Vec<f64>| {
        let t = now();
        times.push(t - last);
        last = t;
    };
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
    // Each side line holds the plan vertices of both cells on it.
    let mut lines = own.lines.clone();
    for (side, d, opposite) in [(0, [0, -1], 2), (1, [1, 0], 3), (2, [0, 1], 0), (3, [-1, 0], 1)] {
        if let Some(r) = rims.iter().find(|r| near(r) == Some(d)) {
            lines[side].extend(&r.lines[opposite]);
            lines[side].sort_unstable();
            lines[side].dedup();
        }
    }
    lap(&mut times);
    let input = points::input(plan, &region, &lines, cell.origin().y as f64);
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
                _ => grid_of(p).map_or(NONE, |g| raster[(g[1] * SIDE18 + g[0]) as usize]),
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
    let mut grid = vec![NONE; (SIDE18 * SIDE18) as usize];
    for (k, &p) in input.points.iter().enumerate() {
        if let Some(g) = grid_of(p) {
            grid[(g[1] * SIDE18 + g[0]) as usize] = k as u32;
        }
    }
    let first_strip = triangles.len();
    for j in 0..params::CELLS17 {
        for i in 0..params::CELLS17 {
            if !region.strip(i, j) {
                continue;
            }
            for tri in strip::triangles(i, j) {
                triangles.push(tri.map(|g| {
                    let at = (g[1] * SIDE18 + g[0]) as usize;
                    let p = [g[0] * params::L18, g[1] * params::L18];
                    if grid[at] == NONE {
                        grid[at] = corners.add_point(p);
                    }
                    let key = if region.on_boundary(g[0], g[1]) {
                        NO_LABEL | TERRAIN_VERTEX
                    } else {
                        ground_key(grid[at], p, &mut ground)
                    };
                    corners.get(grid[at], key)
                }));
            }
        }
    }
    let packed = (corners.keys.iter().zip(&corners.of))
        .map(|(&key, &v)| {
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
        steps: steps.len() as u32,
        outside_d: input.outside_d,
        unlabelled,
        split_edges,
    };
    let sunk = region.sunk();
    lap(&mut times);
    Ok(CellMesh { cell, corners: packed, strip_triangles: stats.strip_triangles, triangles, steps, sunk, stats, times })
}
