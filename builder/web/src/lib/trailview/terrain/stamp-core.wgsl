// The stamps: per texel, the interval [lo, hi] of each feature near it, intersected in priority
// buckets that combine from the top, then clamp(base, lo, hi). The result does not depend on the
// order of the features, and neighbouring nodes get the same intervals where their data overlap.
// stamp.wgsl stamps pages with it and audit.wgsl checks against it. Include it with
// `// #include stamp-core.wgsl`; bins.ts packs the data and mirrors the gates of `road_shows` and `path_shows`.

// Per node: the record count, the entry count, GRID² + 1 cell offsets, the cell entries, then RECORD
// words per primitive. A cell lists its junctions, then per lake its edges and its marker, then the
// segments of the roads and paths, line by line: so no list of lakes or lines has a size limit.
@group(0) @binding(1) var<storage, read> stamp_data: array<u32>;

const GRID: i32 = 18;
const CELL_TEXELS: i32 = 16;
const RECORD: u32 = 16u;
const HEAD: u32 = 2u;
const OFFSETS: u32 = 325u;
const INSIDE: u32 = 0x40000000u;
const NO_DATA: u32 = 0xffffffffu;
const ROAD: u32 = 1u;
const PATH: u32 = 2u;
const EDGE: u32 = 3u;
const LAKE: u32 = 4u;
const JUNCTION: u32 = 5u;
const FORD: u32 = 1u;
const CORE_A: u32 = 2u;
const CORE_B: u32 = 4u;
const CONE_A: u32 = 8u;
const CONE_B: u32 = 16u;

// The buckets, from the top.
const WATER_CORE: u32 = 0u;
const ROAD_CORE: u32 = 2u;
const BANKS: u32 = 4u;
const CONES: u32 = 5u;
const BENCH: u32 = 6u;
const BUCKETS: u32 = 7u;

const PI: f32 = 3.14159265;
// As EARTH_RADIUS of frame.ts.
const RADIUS: f32 = 6371000.0;
// The buffer of the feature tiles: no stamp reaches further, less two texels and a metre.
const BUFFER_M: f32 = 40.0;
// A level shows a road at least one mesh cell (4 texels) wide. Its core holds MARGIN_TEXELS past the
// edge: a point of the mesh reads texels within 13 texels of it, through its triangle, the four texels
// of each vertex and the slide of a morphing vertex toward the coarser grid.
const SHOW_CELLS: f32 = 1.0;
const MESH_TEXELS: f32 = 4.0;
const MARGIN_TEXELS: f32 = 13.0;
// Road cores: z ± 2 cm, ± 10 cm at a junction, where the tolerance grows with the grade of its roads.
const CORE_TOL: f32 = 0.02;
const JUNCTION_TOL: f32 = 0.1;
const JUNCTION_CAP: f32 = 15.0;
// Cut and fill of a road at the prototype's GRADE_BANK; it stops binding at its reach.
const CONE_REACH: f32 = 25.0;
const CONE_LEAST: f32 = 5.0;
const CONE_SLOPE: f32 = 0.85;
// The path bench: cut 65°, fill 35°, reach 3 m.
const BENCH_REACH: f32 = 3.0;
const BENCH_CUT: f32 = 2.1445;
const BENCH_FILL: f32 = 0.7002;
// A lake bed: at least LAKE_DEPTH below the level, deeper by BED_SLOPE per metre from the shore to BED_DEPTH.
const LAKE_DEPTH: f32 = 0.1;
const BED_SLOPE: f32 = 0.2;
const BED_DEPTH: f32 = 2.0;
// Banks: at least FREEBOARD above the level over BANK_M, plus the margins, from the shore.
const BANK_M: f32 = 2.0;
const FREEBOARD: f32 = 0.3;
const BANK_RAMP: f32 = 8.0;
const INF: f32 = 1e30;

// Metres per world unit at world row y.
fn unit_metres(y: i32) -> f32 {
    return 2.0 * PI * RADIUS / 1073741824.0 / cosh(PI * (1.0 - f32(y) / 536870912.0));
}

fn margin(texel: f32, baked: bool) -> f32 {
    return select(MARGIN_TEXELS * texel, 0.0, baked);
}

fn reach_limit(texel: f32) -> f32 {
    return BUFFER_M - 2.0 * texel - 1.0;
}

// Whether a level of this texel size shows a road of half-width h. The bake conditions every level that holds it.
fn road_shows(h: f32, texel: f32, baked: bool) -> bool {
    return (baked || 2.0 * h >= SHOW_CELLS * MESH_TEXELS * texel) && h + margin(texel, baked) + CONE_LEAST <= reach_limit(texel);
}

fn cone_reach(h: f32, texel: f32, baked: bool) -> f32 {
    return min(CONE_REACH, reach_limit(texel) - h - margin(texel, baked));
}

// A path bench shows from one texel: the mesh cannot show it, the normals can.
fn path_shows(h: f32, texel: f32) -> bool {
    return 2.0 * h >= texel && h + BENCH_REACH <= reach_limit(texel);
}

fn bank_width(texel: f32, baked: bool) -> f32 {
    return BANK_M + 2.0 * margin(texel, baked);
}

fn banks_show(texel: f32, baked: bool) -> bool {
    return bank_width(texel, baked) + BANK_RAMP <= reach_limit(texel);
}

// How far past the shore the water height goes, under the bank, so the water meets the terrain.
fn water_reach(texel: f32, baked: bool) -> f32 {
    return max(0.0, min(margin(texel, baked) + texel, reach_limit(texel)));
}

// A width that grows at `slope` and goes to infinity at `reach`, so an interval stops binding
// smoothly instead of with a step.
fn widen(x: f32, reach: f32, slope: f32) -> f32 {
    if (x >= reach) {
        return INF;
    }
    return slope * x * reach / (reach - x);
}

fn word(at: u32) -> u32 {
    return stamp_data[at];
}

fn real(at: u32) -> f32 {
    return bitcast<f32>(stamp_data[at]);
}

fn point(at: u32) -> vec2i {
    return vec2i(i32(stamp_data[at]), i32(stamp_data[at + 1u]));
}

// Where a texel lies on a road or path segment: metres from its line, the height there, the side,
// and whether the core and the cone apply. Inside the segment's mitre quad the height runs from end
// to end along it; past an end of a line, a round cap holds the end's height.
struct Along {
    lateral: f32,
    z: f32,
    side: f32,
    core: bool,
    cone: bool,
    // Metres from the segment's line where the foot of the perpendicular lies on the segment, else INF.
    beside: f32,
}

fn along(r: u32, p: vec2i, unit: f32) -> Along {
    let a = point(r);
    let b = point(r + 2u);
    let flags = word(r + 5u) >> 8u;
    let ab = vec2f(b - a);
    let ap = vec2f(p - a);
    let bp = vec2f(p - b);
    let dir = ab / max(length(ab), 1e-6);
    // Distances past the mitre lines, measured along the segment, so that the height grows evenly along its middle.
    let ma = vec2f(real(r + 9u), real(r + 10u));
    let mb = vec2f(real(r + 11u), real(r + 12u));
    let da = dot(ap, ma) / max(dot(dir, ma), 0.1);
    let db = -dot(bp, mb) / max(dot(dir, mb), 0.1);
    let cross = dir.x * ap.y - dir.y * ap.x;
    let foot = dot(ap, dir);
    let beside = select(INF, abs(cross) * unit, foot >= 0.0 && foot <= length(ab));
    if (da >= 0.0 && db >= 0.0) {
        let t = da / max(da + db, 1e-6);
        return Along(abs(cross) * unit, mix(real(r + 6u), real(r + 7u), t), sign(cross), true, true, beside);
    }
    if (da < 0.0 && db >= 0.0 && (flags & (CORE_A | CONE_A)) != 0u) {
        return Along(length(ap) * unit, real(r + 6u), sign(cross), (flags & CORE_A) != 0u, (flags & CONE_A) != 0u, beside);
    }
    if (db < 0.0 && da >= 0.0 && (flags & (CORE_B | CONE_B)) != 0u) {
        return Along(length(bp) * unit, real(r + 7u), sign(cross), (flags & CORE_B) != 0u, (flags & CONE_B) != 0u, beside);
    }
    return Along(INF, 0.0, 0.0, false, false, beside);
}

fn segment_distance(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - a - t * ab);
}

// Whether segment c-p crosses edge a-b, half-open at the ends, so each crossing counts once.
fn crosses(c: vec2f, p: vec2f, a: vec2f, b: vec2f) -> bool {
    let side = (p.x - c.x) * (a.y - c.y) - (p.y - c.y) * (a.x - c.x) > 0.0;
    let other = (p.x - c.x) * (b.y - c.y) - (p.y - c.y) * (b.x - c.x) > 0.0;
    let one = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x) > 0.0;
    let two = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x) > 0.0;
    return side != other && one != two;
}

// The intervals at a texel and what the mask holds: the nearest road edge (negative inside), the
// across coordinate and surface of that road, the nearest waterline (negative in water), the water
// height (NONE without water) and the flags.
struct Stamp {
    lo: f32,
    hi: f32,
    conflict: bool,
    water: f32,
    road: f32,
    across: f32,
    surface: u32,
    ford: bool,
    shore: f32,
}

// The stamp of world point p, a texel centre of a node of `level` whose first texel is `first`, from
// the node's data at `data`.
fn stamp_at(level: u32, first: vec2i, data: u32, p: vec2i, baked: bool) -> Stamp {
    var out = Stamp(-INF, INF, false, -1e30, INF, 0.0, 0u, false, INF);
    if (data == NO_DATA) {
        return out;
    }
    let shift = 22u - level;
    let unit = unit_metres(p.y);
    let texel = unit * f32(1 << shift);
    let m = margin(texel, baked);
    let size = CELL_TEXELS << shift;
    let origin = (first << vec2u(shift)) - vec2i(size);
    let cell = clamp((p - origin) / size, vec2i(0), vec2i(GRID - 1));
    // The cell's origin of the lake crossings: its centre, held inside the node, where the feature tile is valid.
    let centre = clamp(origin + cell * size + vec2i(size / 2), first << vec2u(shift), ((first + vec2i(255)) << vec2u(shift)) + vec2i((1 << shift) - 1));
    let c = cell.y * GRID + cell.x;
    let records = data + HEAD + OFFSETS + word(data + 1u);
    var lo: array<f32, BUCKETS>;
    var hi: array<f32, BUCKETS>;
    for (var b = 0u; b < BUCKETS; b++) {
        lo[b] = -INF;
        hi[b] = INF;
    }
    var relax = 0.0;
    // The lake of the edges so far: whether they cross the way from the cell's origin to p, and the shore distance.
    var crossed = false;
    var shore = INF;
    var water_in = -1e30;
    var water_near = -1e30;
    // The road line of the segments so far: whether p lies in its core band, and whether a segment
    // holds it there. Inside a bend tighter than the core, none does: a conflict, where the shader draws a wall.
    var line = NO_DATA;
    var line_near = false;
    var line_held = false;
    let first_entry = word(data + HEAD + u32(c));
    let last_entry = word(data + HEAD + u32(c) + 1u);
    for (var e = first_entry; e < last_entry; e++) {
        let entry = word(data + HEAD + OFFSETS + e);
        let r = records + RECORD * (entry & 0x3fffffffu);
        let kind = word(r + 4u);
        if (kind == JUNCTION) {
            let reach = min(min(JUNCTION_CAP, 2.0 * (real(r + 6u) + m)), reach_limit(texel) * 0.5);
            let d = length(vec2f(p - point(r))) * unit;
            relax = max(relax, (JUNCTION_TOL - CORE_TOL + 2.0 * real(r + 7u) * reach) * max(0.0, 1.0 - d / (2.0 * reach)));
        } else if (kind == EDGE) {
            // Relative to the cell's origin and to the edge, so that every node that holds the edge gets the same bits.
            let a = point(r);
            let b = point(r + 2u);
            crossed = crossed != crosses(vec2f(0.0), vec2f(p - centre), vec2f(a - centre), vec2f(b - centre));
            shore = min(shore, segment_distance(vec2f(p - a), vec2f(0.0), vec2f(b - a)) * unit);
        } else if (kind == LAKE) {
            let level = real(r + 6u);
            if (((entry & INSIDE) != 0u) != crossed) {
                hi[WATER_CORE] = min(hi[WATER_CORE], level - LAKE_DEPTH - min(BED_DEPTH, BED_SLOPE * shore));
                water_in = max(water_in, level);
                out.shore = min(out.shore, -shore);
            } else {
                if (banks_show(texel, baked)) {
                    let x = max(0.0, shore - bank_width(texel, baked));
                    lo[BANKS] = max(lo[BANKS], level + FREEBOARD - widen(x, BANK_RAMP, 1.0));
                }
                if (shore <= water_reach(texel, baked)) {
                    water_near = max(water_near, level);
                }
                out.shore = min(out.shore, shore);
            }
            crossed = false;
            shore = INF;
        } else {
            if (word(r + 13u) != line) {
                out.conflict = out.conflict || (line_near && !line_held);
                line = word(r + 13u);
                line_near = false;
                line_held = false;
            }
            let h = real(r + 8u);
            let g = along(r, p, unit);
            if (kind == ROAD && road_shows(h, texel, baked) && (g.beside <= h + m || (g.core && g.lateral <= h + m))) {
                line_near = true;
                line_held = line_held || (g.core && g.lateral <= h + m);
            }
            if (g.lateral >= INF) {
                continue;
            }
            if (g.lateral - h < out.road) {
                out.road = g.lateral - h;
                out.across = g.side * min(g.lateral / max(h, 0.1), 1.0);
                out.surface = word(r + 5u) & 0xffu;
                out.ford = ((word(r + 5u) >> 8u) & FORD) != 0u;
            }
            if (kind == ROAD && road_shows(h, texel, baked)) {
                let tol = CORE_TOL + relax;
                if (g.core && g.lateral <= h + m) {
                    lo[ROAD_CORE] = max(lo[ROAD_CORE], g.z - tol);
                    hi[ROAD_CORE] = min(hi[ROAD_CORE], g.z + tol);
                }
                let x = max(0.0, g.lateral - h - m);
                let reach = cone_reach(h, texel, baked);
                if (g.cone && x < reach) {
                    let w = tol + widen(x, reach, CONE_SLOPE);
                    lo[CONES] = max(lo[CONES], g.z - w);
                    hi[CONES] = min(hi[CONES], g.z + w);
                }
            } else if (kind == PATH && path_shows(h, texel)) {
                let x = max(0.0, g.lateral - h);
                if ((g.core || g.cone) && x < BENCH_REACH) {
                    lo[BENCH] = max(lo[BENCH], g.z - CORE_TOL - widen(x, BENCH_REACH, BENCH_FILL));
                    hi[BENCH] = min(hi[BENCH], g.z + CORE_TOL + widen(x, BENCH_REACH, BENCH_CUT));
                }
            }
        }
    }
    out.conflict = out.conflict || (line_near && !line_held);
    out.water = select(water_near, water_in, water_in > -1e29);
    // A bucket that is empty, or that would make the interval empty, does not apply: a conflict.
    for (var b = 0u; b < BUCKETS; b++) {
        if (lo[b] <= -INF && hi[b] >= INF) {
            continue;
        }
        let l = max(out.lo, lo[b]);
        let u = min(out.hi, hi[b]);
        if (lo[b] > hi[b] || l > u) {
            out.conflict = true;
            continue;
        }
        out.lo = l;
        out.hi = u;
    }
    return out;
}
