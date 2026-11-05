//! The near mesh of a cell as the viewer's `plan=mesh` draws it (`plan/mesh.ts`): a vertex per face
//! corner with its group's height, normal and look word, the vertical faces between height groups,
//! and the floor that holds the terrain grid under the mesh.
//!
//! Little-endian: `MAGIC`, the vertex count, the index count, the floor side (0 or `FLOOR_SIDE`); the
//! vertices (`VERTEX` bytes: x, y as i32 frame units from the cell's origin, z as f32 metres, the look
//! word and the place word of `roads/piece.ts`, the octahedral normal as two snorm16 in east, north,
//! up); the indices as u32; the floors as f32 metres, row by row from the north-west.

use obc_ground_plan::faces::heights::Step;
use obc_ground_plan::faces::records::{LABEL, LINE, RAIL, SAMPLE};
use obc_ground_plan::faces::{FacePlan, HeightRule, Owner, Surface, OUTSIDE};
use obc_ground_plan::mesh::params::{CELLS17, FLOOR_MARGIN_M, L17};
use obc_ground_plan::mesh::{Built, Range};

pub const MAGIC: u32 = u32::from_le_bytes(*b"GPM1");
/// The floor grid: the cell's level-18 grid points, both borders included.
const FLOOR_SIDE: i32 = CELLS17 * 2 + 1;
/// A grid point without a floor.
const NO_FLOOR: f32 = 1e30;
/// As `MAT` and `LOOK` of `roads/section.ts`.
const MAT_GROUND: u32 = 1;
const MAT_KERB: u32 = 2;
const MAT_WALL: u32 = 48;
const LOOK_ASPHALT: u32 = 20;
const LOOK_MAIN: u32 = 21;
const LOOK_PAVERS: u32 = 25;
const LOOK_GRAVEL: u32 = 26;
const LOOK_DIRT: u32 = 27;
const LOOK_TRAIL: u32 = 28;
const LOOK_BALLAST: u32 = 29;
/// A face without a frame has no edge in its look: this far across a way this wide on each side, as a
/// junction's faces in `roads/section.ts`.
const FACE_ACROSS_M: f64 = 2.0;
const FACE_HALF_M: f64 = 12.0;
/// The octahedral code of the down normal: a vertex with it takes the terrain's own normal.
const TERRAIN_NORMAL: [i16; 2] = [i16::MAX, i16::MAX];

/// The look word and the place word of `roads/piece.ts`.
fn look_word(mat: u32, code: u32, half_m: f64) -> u32 {
    mat | code << 7 | ((half_m / 0.025).round().min(511.0) as u32) << 19
}

fn place_word(across_m: f64, along_m: f64) -> u32 {
    let across = ((across_m / 16.0 * 2047.0).round() as i32 & 0xfff) as u32;
    across | ((along_m * 64.0).round().clamp(0.0, f64::from((1 << 20) - 1)) as u32) << 12
}

/// A face's look: material, surface code, and the material line whose frame gives its across, along and
/// half-width (a way's own carriageway and rail bed, every face of a soil way), as `lookOf` of
/// `roads/section.ts` reads the finish.
fn look(plan: &FacePlan, words: &[u32], labels: &[Option<u32>], face: u32) -> (u32, u32, Option<u32>) {
    let f = &plan.faces[face as usize];
    let line =
        labels[face as usize].map(|l| words[words[5] as usize + LABEL * l as usize + 2]).filter(|&l| l != u32::MAX);
    let finish = line.map(|l| words[words[6] as usize + LINE * l as usize + 1]);
    let framed = if matches!(f.owner, Owner::Way(_)) { line } else { None };
    let way = |finish: Option<u32>| match finish {
        Some(w) if w & RAIL != 0 || (w & 0xff) >= 9 => (LOOK_BALLAST, w & 0xff),
        Some(w) => {
            let code = w & 0xff;
            let look = match code {
                1 | 7 => LOOK_MAIN,
                3 => LOOK_GRAVEL,
                4 | 5 => LOOK_DIRT,
                6 => LOOK_TRAIL,
                8 => LOOK_PAVERS,
                _ => LOOK_ASPHALT,
            };
            (look, code)
        }
        None => (LOOK_ASPHALT, 2),
    };
    // A soil way has no surface of its own in a junction, only its tread or ruts: there it keeps the
    // frame of its material line, as GP7 section 10 does for every junction face.
    let soil = |(look, _): (u32, u32)| [LOOK_GRAVEL, LOOK_DIRT, LOOK_TRAIL].contains(&look);
    // The ranks of `plan.json`: 3 is a paved shoulder.
    let ((mat, code), frame) = match f.surface {
        Surface::Carriageway if soil(way(finish)) => (way(finish), line),
        Surface::Carriageway => (way(finish), framed),
        Surface::CornerFill | Surface::Crossing if soil(way(finish)) => (way(finish), line),
        Surface::CornerFill | Surface::Crossing => (way(finish), None),
        Surface::Shoulder if f.rank == 3 => (way(finish), None),
        Surface::RailBed => ((LOOK_BALLAST, finish.map_or(9, |w| w & 0xff)), framed),
        Surface::Platform => ((LOOK_PAVERS, 0), None),
        _ => ((MAT_GROUND, 0), None),
    };
    (mat, code, frame)
}

/// The frame of a point on a line's samples: metres across (left positive), metres along and the
/// half-width (m), by the nearest segment.
fn frame(words: &[u32], line: u32, p: [i32; 2], mpu: f64) -> (f64, f64, f64) {
    let l = &words[words[6] as usize + LINE * line as usize..];
    let (first, count) = (l[2] as usize, l[3] as usize);
    let sample = |k: usize| {
        let w = &words[words[7] as usize + SAMPLE * (first + k)..];
        (
            [f64::from(w[0] as i32), f64::from(w[1] as i32)],
            f64::from(f32::from_bits(w[2])),
            f64::from(f32::from_bits(w[3])),
        )
    };
    let q = [f64::from(p[0]), f64::from(p[1])];
    let mut best = (f64::INFINITY, 0.0, 0.0, 0.0);
    for k in 0..count.saturating_sub(1) {
        let ((a, ha, la), (b, hb, lb)) = (sample(k), sample(k + 1));
        let d = [b[0] - a[0], b[1] - a[1]];
        let len2 = (d[0] * d[0] + d[1] * d[1]).max(1e-9);
        let t = (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0);
        let foot = [a[0] + t * d[0], a[1] + t * d[1]];
        let dist = libm::hypot(q[0] - foot[0], q[1] - foot[1]);
        if dist < best.0 {
            // Left of the direction, frame y south.
            let side = (q[0] - foot[0]) * d[1] - (q[1] - foot[1]) * d[0];
            best = (dist, dist.copysign(side) * mpu, la + t * (lb - la), (ha + t * (hb - ha)) * mpu);
        }
    }
    (best.1, best.2, best.3)
}

#[derive(Default)]
struct Out {
    vertices: Vec<u8>,
    count: u32,
    indices: Vec<u32>,
}

impl Out {
    fn vertex(&mut self, p: [i32; 2], z: f64, word: u32, place: u32, normal: [i16; 2]) -> u32 {
        for v in
            [p[0].to_le_bytes(), p[1].to_le_bytes(), (z as f32).to_le_bytes(), word.to_le_bytes(), place.to_le_bytes()]
        {
            self.vertices.extend(v);
        }
        self.vertices.extend(normal[0].to_le_bytes());
        self.vertices.extend(normal[1].to_le_bytes());
        self.count += 1;
        self.count - 1
    }
}

/// Packs a horizontal unit normal (east, north) octahedrally, as `vertices::octahedral` does.
fn flat(e: f64, n: f64) -> [i16; 2] {
    let s = e.abs() + n.abs();
    [e / s, n / s].map(|v| (v * 32767.0).round() as i16)
}

/// The bytes of a cell's near mesh, its vertical faces and its floors, with the counts of
/// triangles, vertical faces and those whose step no rule names (`unnamed_steps` of GP7).
pub fn bytes(built: &Built, plan: &FacePlan, words: &[u32], mpu: f64) -> (Vec<u8>, [usize; 3]) {
    let Some(near) = built.ranges.iter().find(|r| r.range == Range::Near) else { return (Vec::new(), [0, 0, 0]) };
    let (t, heights) = (&near.tris, &near.heights);
    let mut out = Out::default();
    let looks: Vec<_> = (0..plan.faces.len() as u32).map(|f| look(plan, words, &built.labels, f)).collect();
    let terrain = |f: u32| plan.faces[f as usize].rule == HeightRule::Terrain;
    let index: Vec<u32> = (heights.corners.iter())
        .map(|c| {
            let (mat, code, line) = looks[c.face as usize];
            let p = t.points[c.point as usize];
            // A repeat of the looks along keeps its phase: as the corridors' base of 720.
            let (across, along, half) = line.map_or((FACE_ACROSS_M, 0.0, FACE_HALF_M), |l| frame(words, l, p, mpu));
            let normal = if terrain(c.face) { TERRAIN_NORMAL } else { c.normal };
            out.vertex(p, c.h, look_word(mat, code, half), place_word(across, along.rem_euclid(720.0)), normal)
        })
        .collect();
    let corner = |p: u32, f: u32| heights.at.get(&(p, f)).copied();
    // The floor's level-17 cells: the lowest height of the triangles whose box meets each.
    let mut low = vec![f64::INFINITY; (CELLS17 * CELLS17) as usize];
    let mut lower = |pts: &[[i32; 2]], h: f64| {
        let span = |k: usize| {
            let (a, b) = (pts.iter().map(|p| p[k]).min().unwrap(), pts.iter().map(|p| p[k]).max().unwrap());
            (a.div_euclid(L17).max(0), (b.div_euclid(L17) - i32::from(b.rem_euclid(L17) == 0)).min(CELLS17 - 1))
        };
        let ((x0, x1), (y0, y1)) = (span(0), span(1));
        for j in y0..=y1 {
            for i in x0..=x1 {
                let k = (j * CELLS17 + i) as usize;
                low[k] = low[k].min(h);
            }
        }
    };
    let mut triangles = 0;
    for (v, face) in &t.tris {
        let Some(c) = v.iter().map(|&p| corner(p, *face)).collect::<Option<Vec<u32>>>() else { continue };
        out.indices.extend(c.iter().map(|&k| index[k as usize]));
        let h = c.iter().map(|&k| heights.corners[k as usize].h).fold(f64::INFINITY, f64::min);
        lower(&v.map(|p| t.points[p as usize]), h);
        triangles += 1;
    }
    // The vertical faces: where the two faces of a face edge lie in different groups at an end.
    let (mut walls, mut unnamed) = (0, 0);
    for s in t.segments.iter().filter(|s| !s.locked && s.left != OUTSIDE && s.right != OUTSIDE) {
        let ends = [s.a, s.b].map(|p| [s.left, s.right].map(|f| corner(p, f)));
        let Some(c) = ends.iter().flatten().copied().collect::<Option<Vec<u32>>>() else { continue };
        let c = c.iter().map(|&k| &heights.corners[k as usize]).collect::<Vec<_>>();
        // c: a left, a right, b left, b right.
        if c[0].group == c[1].group && c[2].group == c[3].group {
            continue;
        }
        let (pa, pb) = (t.points[s.a as usize], t.points[s.b as usize]);
        // East and north of the edge, and its left side (frame y south).
        let d = [f64::from(pb[0] - pa[0]), f64::from(pa[1] - pb[1])];
        let length = libm::hypot(d[0], d[1]);
        let left = [-d[1] / length, d[0] / length];
        let rise = (c[0].h + c[2].h) - (c[1].h + c[3].h);
        let kerb = c.iter().any(|k| k.own.step == Step::Kerb);
        let word = look_word(if kerb { MAT_KERB } else { MAT_WALL }, 0, FACE_HALF_M);
        let (along_a, along_b) = (place_word(0.0, 0.0), place_word(0.0, length * mpu));
        // Each side its own vertices with its normal: the front toward the lower side, the back toward
        // the higher one, so a vertical face needs no cull of its own.
        for toward in [1.0, -1.0] {
            let sign = if rise > 0.0 { -toward } else { toward };
            let normal = flat(sign * left[0], sign * left[1]);
            let q = [
                out.vertex(pa, c[0].h, word, along_a, normal),
                out.vertex(pb, c[2].h, word, along_b, normal),
                out.vertex(pb, c[3].h, word, along_b, normal),
                out.vertex(pa, c[1].h, word, along_a, normal),
            ];
            // Counter-clockwise seen from the side that the normal points to (east, north, up).
            let at = |p: [i32; 2], h: f64| [f64::from(p[0]) * mpu, -f64::from(p[1]) * mpu, h];
            let (a0, b0, b1) = (at(pa, c[0].h), at(pb, c[2].h), at(pb, c[3].h));
            let a1 = at(pa, c[1].h);
            let turn = |p: [f64; 3], q: [f64; 3], r: [f64; 3]| {
                let (u, v) = ([q[0] - p[0], q[1] - p[1], q[2] - p[2]], [r[0] - p[0], r[1] - p[1], r[2] - p[2]]);
                let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2]];
                n[0] * sign * left[0] + n[1] * sign * left[1]
            };
            let quad = if turn(a0, b0, b1) + turn(a0, b1, a1) > 0.0 {
                [q[0], q[1], q[2], q[0], q[2], q[3]]
            } else {
                [q[0], q[2], q[1], q[0], q[3], q[2]]
            };
            out.indices.extend(quad);
        }
        walls += 1;
        unnamed += usize::from(c.iter().all(|k| k.own.step == Step::None));
    }
    // The floors: a grid point whose level-17 cells about it all lie in R ∖ S holds the terrain under
    // the triangles over its cells in this cell.
    let mut floors = Vec::with_capacity((FLOOR_SIDE * FLOOR_SIDE) as usize);
    for j in 0..FLOOR_SIDE {
        for i in 0..FLOOR_SIDE {
            let around = |g: i32| ((g - 1).div_euclid(2), g.div_euclid(2));
            let ((x0, x1), (y0, y1)) = (around(i), around(j));
            let mut inside = true;
            let mut h = f64::INFINITY;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    inside &= built.region.interior(x, y);
                    if (0..CELLS17).contains(&x) && (0..CELLS17).contains(&y) {
                        h = h.min(low[(y * CELLS17 + x) as usize]);
                    }
                }
            }
            floors.push(if inside && h.is_finite() { (h - FLOOR_MARGIN_M) as f32 } else { NO_FLOOR });
        }
    }
    let mut bytes = Vec::new();
    for w in [MAGIC, out.count, out.indices.len() as u32, FLOOR_SIDE as u32] {
        bytes.extend(w.to_le_bytes());
    }
    bytes.extend(out.vertices);
    out.indices.iter().for_each(|i| bytes.extend(i.to_le_bytes()));
    floors.iter().for_each(|f| bytes.extend(f.to_le_bytes()));
    (bytes, [triangles, walls, unnamed])
}
