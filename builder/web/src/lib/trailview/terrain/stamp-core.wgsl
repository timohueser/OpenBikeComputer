// The stamps: per texel, the interval [lo, hi] of each feature near it, intersected in priority
// buckets that combine from the top, then clamp(base, lo, hi). The result does not depend on the
// order of the features, and neighbouring nodes get the same intervals where their data overlap.
// stamp.wgsl stamps pages with it and audit.wgsl checks against it. Include it with
// `// #include stamp-core.wgsl`; bins.ts packs the data and the profiles, and mirrors the gates of `road_shows`
// and `path_shows`.

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
const BRIDGE: u32 = 8u;
const TUNNEL: u32 = 9u;
const FORD: u32 = 1u;
const CORE_A: u32 = 2u;
const CORE_B: u32 = 4u;
const CONE_A: u32 = 8u;
const CONE_B: u32 = 16u;
const JOIN_A: u32 = 32u;
const JOIN_B: u32 = 64u;

// The buckets, from the top.
const WATER_CORE: u32 = 0u;
const TROUGH: u32 = 1u;
const ROAD_CORE: u32 = 2u;
// A tunnel's road into its portal yields to every road over it, as under a road on an embankment.
const PORTAL_CORE: u32 = 3u;
// A path's tread outranks the slopes of the roads beside it.
const PATH_CORE: u32 = 4u;
const BANKS: u32 = 5u;
const CONES: u32 = 6u;
const BENCH: u32 = 7u;
const COVER: u32 = 8u;
const BUCKETS: u32 = 9u;

const PI: f32 = 3.14159265;
// As EARTH_RADIUS of frame.ts.
const RADIUS: f32 = 6371000.0;
// The buffer of the feature tiles: no stamp reaches further, less two texels and a metre.
const BUFFER_M: f32 = 40.0;
// A level shows a road or a river at least one texel wide, with the same width and shape on every
// level, so the mesh draws its edges soft by up to a mesh cell (MESH_TEXELS). The gates of a level use
// its texel at 45° N, so that a feature shows on all of a level or on none of it.
const GATE_COS: f32 = 0.70710678;
const MESH_TEXELS: f32 = 4.0;
// Near a junction, where the cores of its roads overlap, they hold the middle of their heights where
// these disagree; the reach of a junction is at most JUNCTION_CAP.
const JUNCTION_CAP: f32 = 15.0;
// A lake's bed falls from its shore at SHORE_SLOPE to BED_DEPTH. On the levels that can hold it, the
// terrain about the lake stays under a ramp that rises from the shore at SHORE_SLOPE over SHORE_CELLS
// mesh cells, then widens until it stops binding at the buffer. So every mesh triangle across a shore
// crosses the level at the shore, and the lake's outline is the water's edge.
const SHORE_SLOPE: f32 = 0.5;
const SHORE_CELLS: f32 = 1.5;
const BED_DEPTH: f32 = 4.0;
// The least widening that a level must fit past the ramp.
const SHORE_EASE: f32 = 4.0;
// Water lies LIFT_M plus LIFT of a mesh cell above the terrain that it meets, so that the mesh between
// the vertices of a ribbon, and the terrain at a lake's shore, stay under it.
const LIFT_M: f32 = 0.03;
const LIFT: f32 = 0.02;
const INF: f32 = 1e30;
// As WARP_M of bins.ts: the metres over which the cross-slope of a junction fades out.
const WARP_M: f32 = 6.0;
// A bridge rests on its abutments for ABUTMENT_CELLS mesh cells from an end that meets the ground: the
// road's core runs on under the deck, so no mesh triangle there reaches the trough. Past them the
// ground stays CLEAR_M under the deck's underside, out to TROUGH_M past the deck's edge, and widens
// from there at 45° until it stops binding TROUGH_REACH_M out.
const ABUTMENT_CELLS: f32 = 2.0;
const CLEAR_M: f32 = 1.0;
const TROUGH_M: f32 = 1.0;
const TROUGH_REACH_M: f32 = 15.0;
// A tunnel runs on the ground into its portal for PORTAL_CELLS mesh cells from an end that meets the
// ground. Past them the ground stays COVER_M over its bore, out to BORE_WALL_M past the road's edge, and
// falls from there at 45° until it stops binding COVER_REACH_M out.
const PORTAL_CELLS: f32 = 2.0;
const COVER_M: f32 = 1.0;
const BORE_WALL_M: f32 = 1.0;
const COVER_REACH_M: f32 = 15.0;
// A gallery's wall and pillars stand GALLERY_BACK_M past its road's edge. Behind them the hill holds the
// top of its roof, less toward its ends, and falls from there at GALLERY_FILL to the hillside and to
// its ends, until it stops binding
// GALLERY_REACH_M out; on the valley side the ground falls away, cut at GALLERY_VALLEY_CUT at most.
const GALLERY_BACK_M: f32 = 1.0;
const GALLERY_FILL: f32 = 0.6;
const GALLERY_REACH_M: f32 = 15.0;
const GALLERY_VALLEY_CUT: f32 = 0.2;

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

fn shore_band(texel: f32) -> f32 {
    return SHORE_CELLS * MESH_TEXELS * texel;
}

// Whether a level holds the terrain about a lake under its ramp.
fn shore_shows(level: u32) -> bool {
    let texel = gate_texel(level);
    return shore_band(texel) + SHORE_EASE <= reach_limit(texel);
}

// How far water lies above the terrain that it meets, on a level with texels of this size.
fn water_lift(texel: f32) -> f32 {
    return LIFT_M + LIFT * MESH_TEXELS * texel;
}

// The most that the terrain may rise d metres from a lake's shore.
fn shore_cap(d: f32, texel: f32) -> f32 {
    let band = shore_band(texel);
    return SHORE_SLOPE * d + widen(max(0.0, d - band), reach_limit(texel) - band, 1.0);
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
    // Metres past the mitre lines of the start and of the end, measured along the segment.
    da: f32,
    db: f32,
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
    // Bridges, tunnels and galleries hold their ends' metres in those words, not tilts.
    let tilt = select(cross * unit * (end_tilt(word(r + 14u), da * unit) + end_tilt(word(r + 15u), db * unit)), 0.0, word(r + 4u) >= BRIDGE || (word(r + 5u) >> 24u) != 0u);
    let foot = dot(ap, dir);
    let beside = select(INF, abs(cross) * unit, foot >= 0.0 && foot <= length(ab));
    if (da >= 0.0 && db >= 0.0) {
        let t = da / max(da + db, 1e-6);
        return Along(abs(cross) * unit, mix(real(r + 6u), real(r + 7u), t) + tilt, sign(cross), true, true, true, beside, da * unit, db * unit);
    }
    if (da < 0.0 && db >= 0.0 && (flags & (CORE_A | CONE_A)) != 0u) {
        return Along(length(ap) * unit, real(r + 6u) + tilt, sign(cross), (flags & CORE_A) != 0u, (flags & CONE_A) != 0u, (flags & (CORE_A | JOIN_A)) != 0u, beside, da * unit, db * unit);
    }
    if (db < 0.0 && da >= 0.0 && (flags & (CORE_B | CONE_B)) != 0u) {
        return Along(length(bp) * unit, real(r + 7u) + tilt, sign(cross), (flags & CORE_B) != 0u, (flags & CONE_B) != 0u, (flags & (CORE_B | JOIN_B)) != 0u, beside, da * unit, db * unit);
    }
    return Along(INF, 0.0, 0.0, false, false, false, beside, da * unit, db * unit);
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
    // Whether the cones of the roads hold the lower bound (a fill) and the upper bound (a cut).
    fill: bool,
    cut: bool,
}

// The stamp of world point p, a texel centre of a node of `level` whose first texel is `first`, from
// the node's data at `data`.
fn stamp_at(level: u32, first: vec2i, data: u32, p: vec2i) -> Stamp {
    var out = Stamp(-INF, INF, false, -1e30, INF, 0.0, 0u, false, INF, INF, false, false);
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
    var in_lake = false;
    // The road line of the segments so far: whether p lies in its core band, and whether a segment
    // holds it there. Inside a bend tighter than the core, none does: a conflict, which the mask flags.
    var line = NO_DATA;
    var line_near = false;
    var line_held = false;
    // The metres from the road that holds each bound of the cones.
    var cone_lo_x = INF;
    var cone_hi_x = INF;
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
                hi[WATER_CORE] = min(hi[WATER_CORE], water - min(BED_DEPTH, SHORE_SLOPE * shore));
                water_in = max(water_in, water);
                in_lake = true;
                out.shore = min(out.shore, -shore);
                out.lake = min(out.lake, -shore);
            } else {
                if (shore_shows(level)) {
                    hi[BANKS] = min(hi[BANKS], water + shore_cap(shore, texel));
                }
                out.shore = min(out.shore, shore);
                out.lake = min(out.lake, shore);
            }
            crossed = false;
            shore = INF;
        } else if (kind == RIVER) {
            // A river holds its level over its width as a road holds its height, with banks for cones.
            // In a lake the lake's bed holds: its river line runs on through it. The cell lists its lakes
            // before its lines, so in_lake is final here.
            let h = real(r + 8u);
            let rule = profiles[(word(r + 5u) >> 16u) & 0xffu];
            let g = along(r, p, unit);
            if (g.lateral >= INF || in_lake || !road_shows(h, rule, level)) {
                continue;
            }
            let x = g.lateral - h;
            if (g.core && x <= 0.0) {
                lo[WATER_CORE] = max(lo[WATER_CORE], g.z - rule.tol);
                hi[WATER_CORE] = min(hi[WATER_CORE], g.z + rule.tol);
                water_in = max(water_in, g.z);
            }
            let reach = cone_reach(h, rule, texel);
            if (g.cone && max(x, 0.0) < reach) {
                lo[BANKS] = max(lo[BANKS], g.z - (rule.tol + widen(max(x, 0.0), reach, rule.fill)));
                hi[BANKS] = min(hi[BANKS], g.z + (rule.tol + widen(max(x, 0.0), reach, rule.cut)));
            }
            out.shore = min(out.shore, x);
        } else {
            if (word(r + 13u) != line) {
                out.conflict = out.conflict || (line_near && !line_held);
                line = word(r + 13u);
                line_near = false;
                line_held = false;
            }
            let h = real(r + 8u);
            let rule = profiles[(word(r + 5u) >> 16u) & 0xffu];
            let g = along(r, p, unit);
            // A bridge past its abutments holds the ground under its deck, and paints nothing: the deck draws its road.
            let bridge = kind == BRIDGE;
            if (bridge && g.lateral < INF && road_shows(h, rule, level)
                && min(real(r + 14u) + g.da, real(r + 15u) + g.db) >= ABUTMENT_CELLS * MESH_TEXELS * texel) {
                let x = max(0.0, g.lateral - h - TROUGH_M);
                let reach = min(TROUGH_REACH_M, reach_limit(texel) - h - TROUGH_M);
                hi[TROUGH] = min(hi[TROUGH], g.z - f32(word(r + 5u) >> 24u) * 0.1 - CLEAR_M + widen(x, reach, 1.0));
                continue;
            }
            // A tunnel past its portals holds the ground over its bore, and paints nothing.
            if (kind == TUNNEL && g.lateral < INF && road_shows(h, rule, level)
                && min(real(r + 14u) + g.da, real(r + 15u) + g.db) >= PORTAL_CELLS * MESH_TEXELS * texel) {
                let x = max(0.0, g.lateral - h - BORE_WALL_M);
                let reach = min(COVER_REACH_M, reach_limit(texel) - h - BORE_WALL_M);
                lo[COVER] = max(lo[COVER], g.z + f32(word(r + 5u) >> 24u) * 0.1 + COVER_M - widen(x, reach, 1.0));
                continue;
            }
            let paved = kind == ROAD || bridge || kind == TUNNEL;
            if (paved && road_shows(h, rule, level) && (g.beside <= h || (g.core && g.lateral <= h))) {
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
            // A gallery's floor runs flat to a mesh cell past its walls and pillars, so the mesh's slope up to
            // the hill lies behind its wall. Past that, on the hill side, the hill holds the top of its roof
            // and no cut; on the valley side a shallow cut keeps it open.
            let roof = f32((word(r + 5u) >> 24u) & 0x7fu) * 0.1;
            let gallery = kind <= PATH && roof > 0.0;
            let wall = h + GALLERY_BACK_M + MESH_TEXELS * texel;
            let hill = gallery && (g.side > 0.0) == ((word(r + 5u) >> 31u) != 0u);
            if (hill && g.core && g.lateral > wall) {
                let reach = min(GALLERY_REACH_M, reach_limit(texel) - wall);
                let end = min(real(r + 14u) + g.da, real(r + 15u) + g.db);
                lo[COVER] = max(lo[COVER], g.z + min(roof, GALLERY_FILL * end) - widen(g.lateral - wall, reach, GALLERY_FILL));
                continue;
            }
            if (paved && road_shows(h, rule, level)) {
                let tol = rule.tol;
                let core_half = select(h, wall, gallery);
                if (g.core && g.lateral <= core_half) {
                    let bucket = select(ROAD_CORE, PORTAL_CORE, kind == TUNNEL);
                    lo[bucket] = max(lo[bucket], g.z - tol);
                    hi[bucket] = min(hi[bucket], g.z + tol);
                }
                let x = max(0.0, g.lateral - core_half);
                let reach = cone_reach(core_half, rule, texel);
                if (g.cone && x < reach) {
                    let l = g.z - (tol + widen(x, reach, rule.fill));
                    let u = g.z + (tol + widen(x, reach, select(rule.cut, GALLERY_VALLEY_CUT, gallery)));
                    if (l > lo[CONES] || (l == lo[CONES] && x < cone_lo_x)) {
                        lo[CONES] = l;
                        cone_lo_x = x;
                    }
                    if (u < hi[CONES] || (u == hi[CONES] && x < cone_hi_x)) {
                        hi[CONES] = u;
                        cone_hi_x = x;
                    }
                }
            } else if (kind == PATH && path_shows(h, rule, level)) {
                let x = max(0.0, g.lateral - h);
                let tol = rule.tol + rule.tilt * min(g.lateral, h);
                if (g.core && g.lateral <= h) {
                    lo[PATH_CORE] = max(lo[PATH_CORE], g.z - tol);
                    hi[PATH_CORE] = min(hi[PATH_CORE], g.z + tol);
                }
                if ((g.core || g.cone) && x < rule.reach) {
                    lo[BENCH] = max(lo[BENCH], g.z - tol - widen(x, rule.reach, rule.fill));
                    hi[BENCH] = min(hi[BENCH], g.z + tol + widen(x, rule.reach, rule.cut));
                }
            }
        }
    }
    out.conflict = out.conflict || (line_near && !line_held);
    out.water = water_in;
    // Under a deck the ground falls away: no road's cone or bench holds it up. Their cuts hold, so a
    // road under a bridge keeps its cut slopes.
    if (hi[TROUGH] < INF) {
        lo[CONES] = -INF;
        lo[BENCH] = -INF;
    }
    // Under water no bank holds: where a river meets a lake or another river.
    if (hi[WATER_CORE] < INF) {
        lo[BANKS] = -INF;
        hi[BANKS] = INF;
    }
    // Near a junction the cores of its roads hold the middle of their heights where they disagree by
    // the centimetres of the solver, and a core outranks the cones. Their cones step as anywhere else,
    // so a wall between two roads grows from nothing at their junction.
    if (junction) {
        if (lo[ROAD_CORE] > hi[ROAD_CORE]) {
            let middle = 0.5 * (lo[ROAD_CORE] + hi[ROAD_CORE]);
            lo[ROAD_CORE] = middle;
            hi[ROAD_CORE] = middle;
        }
        if (hi[ROAD_CORE] < INF) {
            lo[CONES] = -INF;
            hi[CONES] = INF;
        }
    }
    // A conflict, which the mask flags: an empty bucket holds the middle of its bounds, but for the
    // cones, and a bucket outside the interval pulls it to its nearest end. So the height has no step
    // where a conflict begins.
    for (var b = 0u; b < BUCKETS; b++) {
        if (lo[b] <= -INF && hi[b] >= INF) {
            continue;
        }
        var l = lo[b];
        var u = hi[b];
        // A portal and the cover of a tunnel yield to the buckets above them without a conflict.
        if ((b == PORTAL_CORE || b == COVER) && (l > out.hi || u < out.lo)) {
            continue;
        }
        if (l > u) {
            out.conflict = true;
            // The cones of two roads: the nearer road's face holds, so the ground steps where the
            // two are equally far, by nothing where the conflict begins. The look draws a wall there.
            if (b == CONES) {
                l = select(u, l, cone_lo_x < cone_hi_x);
            } else {
                l = 0.5 * (l + u);
            }
            u = l;
        }
        if (l > out.hi || u < out.lo) {
            out.conflict = true;
            let end = select(out.lo, out.hi, l > out.hi);
            out.lo = end;
            out.hi = end;
            continue;
        }
        if (l > out.lo) {
            out.lo = l;
            out.fill = b == CONES;
        }
        if (u < out.hi) {
            out.hi = u;
            out.cut = b == CONES;
        }
    }
    return out;
}
