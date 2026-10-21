// The stamps: per texel, the interval [lo, hi] of each feature near it, intersected in priority
// buckets that combine from the top, then clamp(base, lo, hi). The result does not depend on the
// order of the features, and neighbouring nodes get the same intervals where their data overlap.
// stamp.wgsl stamps pages with it and audit.wgsl checks against it. Include it with
// `// #include stamp-core.wgsl`; bins.ts packs the data and the profiles, and mirrors the gates of `road_shows`,
// `path_shows` and `river_shows`.

// Per node: the record count, the entry count, GRID² + 1 cell offsets (each with the move of the
// cell's origin in its top byte, x then y, 8 + units), the cell entries, then RECORD
// words per primitive. A cell lists its junctions, then per lake its edges and its marker, then the
// segments of the roads, paths and rivers, line by line: so no list of lakes or lines has a size limit.
@group(0) @binding(1) var<storage, read> stamp_data: array<u32>;

// The profile of a road or path class, as `Profile` in bins.ts: the core tolerance, the reach of the
// cut and fill, the least of them that a level must fit to stamp a road, the cut and the fill slopes,
// and how far a path's bench may follow the hillside across it. A uniform array element takes a
// multiple of 16 bytes.
struct Profile {
    tol: f32,
    reach: f32,
    least: f32,
    cut: f32,
    fill: f32,
    @size(12) tilt: f32,
}

// Per class, the row that a segment record names. A uniform, as the audit has every storage buffer that a stage may have.
@group(0) @binding(4) var<uniform> profiles: array<Profile, PROFILE_ROWS>;
const PROFILE_ROWS: u32 = 32u;

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
const RIVER: u32 = 6u;
const FORD: u32 = 1u;
const CORE_A: u32 = 2u;
const CORE_B: u32 = 4u;
const CONE_A: u32 = 8u;
const CONE_B: u32 = 16u;
const JOIN_A: u32 = 32u;
const JOIN_B: u32 = 64u;

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
// A level shows a road at least one texel wide, with the same width and shape on every level, so the
// mesh draws its edges soft by up to a mesh cell (MESH_TEXELS). Water reaches a mesh cell under its
// banks. The gates of a level use its texel at 45° N, so that a feature shows on all of a level or on none of it.
const GATE_COS: f32 = 0.70710678;
const MESH_TEXELS: f32 = 4.0;
// Near a junction, where the cores of its roads overlap, they hold the middle of their heights where
// these disagree; the reach of a junction is at most JUNCTION_CAP.
const JUNCTION_CAP: f32 = 15.0;
// A lake bed: at least LAKE_DEPTH below the level, deeper by BED_SLOPE per metre from the shore to BED_DEPTH.
const LAKE_DEPTH: f32 = 0.1;
const BED_SLOPE: f32 = 0.2;
const BED_DEPTH: f32 = 2.0;
// Banks: at least FREEBOARD above the level over BANK_M and two mesh cells from the shore, then a ramp.
const BANK_M: f32 = 2.0;
const FREEBOARD: f32 = 0.3;
const BANK_RAMP: f32 = 8.0;
const INF: f32 = 1e30;
// As WARP_M of bins.ts: the metres over which the cross-slope of a junction fades out.
const WARP_M: f32 = 6.0;

// Metres per world unit at world row y.
fn unit_metres(y: i32) -> f32 {
    return 2.0 * PI * RADIUS / 1073741824.0 / cosh(PI * (1.0 - f32(y) / 536870912.0));
}

fn reach_limit(texel: f32) -> f32 {
    return BUFFER_M - 2.0 * texel - 1.0;
}

// The texel of a level at 45° N: north of it the texels are smaller, so a gate that holds there holds on every row.
fn gate_texel(level: u32) -> f32 {
    return 2.0 * PI * RADIUS * GATE_COS / 256.0 / f32(1u << level);
}

// Whether a level shows a road of half-width h and a profile.
fn road_shows(h: f32, rule: Profile, level: u32) -> bool {
    let texel = gate_texel(level);
    return 2.0 * h >= texel && h + rule.least <= reach_limit(texel);
}

fn cone_reach(h: f32, rule: Profile, texel: f32) -> f32 {
    return min(rule.reach, reach_limit(texel) - h);
}

// A path bench shows from one texel: the mesh cannot show it, the normals can.
fn path_shows(h: f32, rule: Profile, level: u32) -> bool {
    let texel = gate_texel(level);
    return 2.0 * h >= texel && h + rule.reach <= reach_limit(texel);
}

// Whether a level shows a river of half-width h: at least one texel wide, its bed, banks and their ramp inside the buffer.
fn river_shows(h: f32, level: u32) -> bool {
    let texel = gate_texel(level);
    return 2.0 * h >= texel && river_half(h, texel) + water_bank(texel) + BANK_RAMP <= reach_limit(texel);
}

// The half-width of a river's bed on a level: a mesh cell at least, so that the mesh always has a
// row of points under water and draws the river as one ribbon.
fn river_half(h: f32, texel: f32) -> f32 {
    return max(h, MESH_TEXELS * texel);
}

// The width of the banks from a shore or a river's bed. The water reaches a mesh cell under them, and
// the mesh points about its edge lie within two cells, so the banks hold them all above the water.
fn water_bank(texel: f32) -> f32 {
    return BANK_M + 2.0 * MESH_TEXELS * texel;
}

fn banks_show(level: u32) -> bool {
    let texel = gate_texel(level);
    return water_bank(texel) + BANK_RAMP <= reach_limit(texel);
}

// How far past the shore the water goes, under the banks, so that it meets the terrain. A level
// without banks keeps the water at the shore: it has nothing to reach under.
fn water_reach(level: u32, texel: f32) -> f32 {
    return select(0.0, MESH_TEXELS * texel, banks_show(level));
}

// How far a junction's cores reach, with the widest half-width of its roads: twice that is its reach.
fn junction_reach(h: f32, texel: f32) -> f32 {
    return min(min(JUNCTION_CAP, 2.0 * h), reach_limit(texel) * 0.5);
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
// and whether the core, the cone and the paint apply. Inside the segment's mitre quad the height runs
// from end to end along it; past an end of a line, a round cap holds the end's height. Near a junction
// the height tilts across the segment by the cross-slopes of its ends.
struct Along {
    lateral: f32,
    z: f32,
    side: f32,
    core: bool,
    cone: bool,
    paint: bool,
    // Metres from the segment's line where the foot of the perpendicular lies on the segment, else INF.
    beside: f32,
}

// The slope across a segment from one of its ends, `s` metres from it, as packTilt of bins.ts packs it:
// the rise per metre in steps of 1e-4, held up to WARP_M before its reach in cm, and zero past the reach.
fn end_tilt(w: u32, s: f32) -> f32 {
    return f32(bitcast<i32>(w << 16u) >> 16u) * 1e-4 * clamp((f32(w >> 16u) * 0.01 - s) / WARP_M, 0.0, 1.0);
}

fn along(r: u32, p: vec2i, unit: f32) -> Along {
    let a = point(r);
    let b = point(r + 2u);
    let flags = (word(r + 5u) >> 8u) & 0xffu;
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
    let tilt = cross * unit * (end_tilt(word(r + 14u), da * unit) + end_tilt(word(r + 15u), db * unit));
    let foot = dot(ap, dir);
    let beside = select(INF, abs(cross) * unit, foot >= 0.0 && foot <= length(ab));
    if (da >= 0.0 && db >= 0.0) {
        let t = da / max(da + db, 1e-6);
        return Along(abs(cross) * unit, mix(real(r + 6u), real(r + 7u), t) + tilt, sign(cross), true, true, true, beside);
    }
    if (da < 0.0 && db >= 0.0 && (flags & (CORE_A | CONE_A)) != 0u) {
        return Along(length(ap) * unit, real(r + 6u) + tilt, sign(cross), (flags & CORE_A) != 0u, (flags & CONE_A) != 0u, (flags & (CORE_A | JOIN_A)) != 0u, beside);
    }
    if (db < 0.0 && da >= 0.0 && (flags & (CORE_B | CONE_B)) != 0u) {
        return Along(length(bp) * unit, real(r + 7u) + tilt, sign(cross), (flags & CORE_B) != 0u, (flags & CONE_B) != 0u, (flags & (CORE_B | JOIN_B)) != 0u, beside);
    }
    return Along(INF, 0.0, 0.0, false, false, false, beside);
}

fn segment_distance(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - a - t * ab);
}

// Whether w + ε(1, ε) lies left of u. A point on a shore line takes the side of that tiny step,
// so its side does not depend on the origin that counts the crossings. The products are compared,
// not subtracted, so that no fused multiply-add makes a tie uneven.
fn left(u: vec2f, w: vec2f) -> bool {
    let l = u.x * w.y;
    let r = u.y * w.x;
    return l > r || (l == r && (u.y < 0.0 || (u.y == 0.0 && u.x > 0.0)));
}

// Whether segment c-p crosses edge a-b. The origin c lies off every edge.
fn crosses(c: vec2f, p: vec2f, a: vec2f, b: vec2f) -> bool {
    let one = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x) > 0.0;
    return left(a - c, p - c) != left(b - c, p - c) && one != left(b - a, p - a);
}

// The intervals at a texel and what the mask holds: the nearest road edge (negative inside), the
// metres from the middle of that road (signed by side) and its surface, the nearest waterline (negative in water), the water height (NONE without water) and the flags.
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
    // The nearest lake shore alone, negative in a lake.
    lake: f32,
}

// The stamp of world point p, a texel centre of a node of `level` whose first texel is `first`, from
// the node's data at `data`.
fn stamp_at(level: u32, first: vec2i, data: u32, p: vec2i) -> Stamp {
    var out = Stamp(-INF, INF, false, -1e30, INF, 0.0, 0u, false, INF, INF);
    if (data == NO_DATA) {
        return out;
    }
    let shift = 22u - level;
    let unit = unit_metres(p.y);
    let texel = unit * f32(1 << shift);
    let size = CELL_TEXELS << shift;
    let origin = (first << vec2u(shift)) - vec2i(size);
    let cell = clamp((p - origin) / size, vec2i(0), vec2i(GRID - 1));
    let c = cell.y * GRID + cell.x;
    let offset = word(data + HEAD + u32(c));
    // The cell's origin of the lake crossings: its centre, held inside the node, where the feature tile
    // is valid, moved off every shore line of the cell by the worker.
    let nudge = vec2i(i32((offset >> 24u) & 15u), i32(offset >> 28u)) - vec2i(8);
    let centre = clamp(origin + cell * size + vec2i(size / 2), first << vec2u(shift), ((first + vec2i(255)) << vec2u(shift)) + vec2i((1 << shift) - 1)) + nudge;
    let records = data + HEAD + OFFSETS + word(data + 1u);
    var lo: array<f32, BUCKETS>;
    var hi: array<f32, BUCKETS>;
    for (var b = 0u; b < BUCKETS; b++) {
        lo[b] = -INF;
        hi[b] = INF;
    }
    // Whether the texel is within the reach of a junction.
    var junction = false;
    // The lake of the edges so far: whether they cross the way from the cell's origin to p, and the shore distance.
    var crossed = false;
    var shore = INF;
    var water_in = -1e30;
    var water_near = -1e30;
    // The road line of the segments so far: whether p lies in its core band, and whether a segment
    // holds it there. Inside a bend tighter than the core, none does: a conflict, which the mask flags.
    var line = NO_DATA;
    var line_near = false;
    var line_held = false;
    let first_entry = offset & 0xffffffu;
    let last_entry = word(data + HEAD + u32(c) + 1u) & 0xffffffu;
    for (var e = first_entry; e < last_entry; e++) {
        let entry = word(data + HEAD + OFFSETS + e);
        let r = records + RECORD * (entry & 0x3fffffffu);
        let kind = word(r + 4u);
        if (kind == JUNCTION) {
            junction = junction || length(vec2f(p - point(r))) * unit < 2.0 * junction_reach(real(r + 6u), texel);
        } else if (kind == EDGE) {
            // Relative to the cell's origin and to the edge, so that every node that holds the edge gets the same bits.
            let a = point(r);
            let b = point(r + 2u);
            crossed = crossed != crosses(vec2f(0.0), vec2f(p - centre), vec2f(a - centre), vec2f(b - centre));
            shore = min(shore, segment_distance(vec2f(p - a), vec2f(0.0), vec2f(b - a)) * unit);
        } else if (kind == LAKE) {
            let water = real(r + 6u);
            if (((entry & INSIDE) != 0u) != crossed) {
                hi[WATER_CORE] = min(hi[WATER_CORE], water - LAKE_DEPTH - min(BED_DEPTH, BED_SLOPE * shore));
                water_in = max(water_in, water);
                out.shore = min(out.shore, -shore);
                out.lake = min(out.lake, -shore);
            } else {
                if (banks_show(level)) {
                    lo[BANKS] = max(lo[BANKS], water + FREEBOARD - widen(max(0.0, shore - water_bank(texel)), BANK_RAMP, 1.0));
                }
                if (shore <= water_reach(level, texel)) {
                    water_near = max(water_near, water);
                }
                out.shore = min(out.shore, shore);
                out.lake = min(out.lake, shore);
            }
            crossed = false;
            shore = INF;
        } else if (kind == RIVER) {
            let h = real(r + 8u);
            let g = along(r, p, unit);
            if (g.lateral >= INF || !river_shows(h, level)) {
                continue;
            }
            // Metres from the bed's edge, negative in the river; the level runs along it as a road's height does.
            let d = g.lateral - river_half(h, texel);
            if (d <= 0.0) {
                hi[WATER_CORE] = min(hi[WATER_CORE], g.z - LAKE_DEPTH - min(BED_DEPTH, BED_SLOPE * -d));
                water_in = max(water_in, g.z);
            } else {
                lo[BANKS] = max(lo[BANKS], g.z + FREEBOARD - widen(max(0.0, d - water_bank(texel)), BANK_RAMP, 1.0));
                if (d <= water_reach(level, texel)) {
                    water_near = max(water_near, g.z);
                }
            }
            out.shore = min(out.shore, d);
        } else {
            if (word(r + 13u) != line) {
                out.conflict = out.conflict || (line_near && !line_held);
                line = word(r + 13u);
                line_near = false;
                line_held = false;
            }
            let h = real(r + 8u);
            let rule = profiles[word(r + 5u) >> 16u];
            let g = along(r, p, unit);
            if (kind == ROAD && road_shows(h, rule, level) && (g.beside <= h || (g.core && g.lateral <= h))) {
                line_near = true;
                line_held = line_held || (g.core && g.lateral <= h);
            }
            if (g.lateral >= INF) {
                continue;
            }
            // The paint follows the carriageway: a road that ends at a junction ends at its mitre line,
            // but for the round cap that fills the outer wedge of a bend where the through road runs on.
            if (g.paint && g.lateral - h < out.road) {
                out.road = g.lateral - h;
                out.across = g.side * g.lateral;
                out.surface = word(r + 5u) & 0xffu;
                out.ford = ((word(r + 5u) >> 8u) & FORD) != 0u;
            }
            if (kind == ROAD && road_shows(h, rule, level)) {
                let tol = rule.tol;
                if (g.core && g.lateral <= h) {
                    lo[ROAD_CORE] = max(lo[ROAD_CORE], g.z - tol);
                    hi[ROAD_CORE] = min(hi[ROAD_CORE], g.z + tol);
                }
                let x = max(0.0, g.lateral - h);
                let reach = cone_reach(h, rule, texel);
                if (g.cone && x < reach) {
                    lo[CONES] = max(lo[CONES], g.z - (tol + widen(x, reach, rule.fill)));
                    hi[CONES] = min(hi[CONES], g.z + (tol + widen(x, reach, rule.cut)));
                }
            } else if (kind == PATH && path_shows(h, rule, level)) {
                let x = max(0.0, g.lateral - h);
                let tol = rule.tol + rule.tilt * min(g.lateral, h);
                if ((g.core || g.cone) && x < rule.reach) {
                    lo[BENCH] = max(lo[BENCH], g.z - tol - widen(x, rule.reach, rule.fill));
                    hi[BENCH] = min(hi[BENCH], g.z + tol + widen(x, rule.reach, rule.cut));
                }
            }
        }
    }
    out.conflict = out.conflict || (line_near && !line_held);
    out.water = select(water_near, water_in, water_in > -1e29);
    // Under water no bank holds: where a river meets a lake or another river.
    if (hi[WATER_CORE] < INF) {
        lo[BANKS] = -INF;
    }
    // Near a junction the cores of its roads, and their cones, hold the middle of their heights where
    // they disagree, and a core outranks the cones: the simple profiles of the bake meet there.
    if (junction) {
        for (var b = ROAD_CORE; b <= CONES; b += CONES - ROAD_CORE) {
            if (lo[b] > hi[b]) {
                let middle = 0.5 * (lo[b] + hi[b]);
                lo[b] = middle;
                hi[b] = middle;
            }
        }
        if (hi[ROAD_CORE] < INF) {
            lo[CONES] = -INF;
            hi[CONES] = INF;
        }
    }
    // A conflict, which the mask flags: an empty bucket holds the middle of its bounds, and a bucket
    // outside the interval pulls it to its nearest end. So the height has no step where a conflict begins.
    for (var b = 0u; b < BUCKETS; b++) {
        if (lo[b] <= -INF && hi[b] >= INF) {
            continue;
        }
        var l = lo[b];
        var u = hi[b];
        if (l > u) {
            out.conflict = true;
            l = 0.5 * (l + u);
            u = l;
        }
        if (l > out.hi || u < out.lo) {
            out.conflict = true;
            let end = select(out.lo, out.hi, l > out.hi);
            out.lo = end;
            out.hi = end;
            continue;
        }
        out.lo = max(out.lo, l);
        out.hi = min(out.hi, u);
    }
    return out;
}
