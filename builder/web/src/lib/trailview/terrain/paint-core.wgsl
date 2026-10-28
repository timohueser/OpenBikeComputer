// The records of a node's stamp data and the paint at a point: the road, the sidewalk or the junction's
// corner that paints it, by their ranks. stamp-core.wgsl writes the paint of each texel to the mask with
// it, and roads.wgsl paints the ground near the eye with `paint_at`. Include it with
// `// #include paint-core.wgsl`; bins.ts packs the data and the profiles.

// Per node: the record count, the entry count, GRID² + 1 cell offsets (each with the move of the
// cell's origin in its top byte, x then y, 8 + units), the cell entries, then RECORD
// words per primitive. A cell lists its junctions, then per lake and pad its edges and its marker, then the
// junctions' corners, then the segments of the roads, paths and rivers, line by line, then the walks to the
// houses: so no list of lakes or lines has a size limit. A near node ends with its paint lists: GRID² + 1
// offsets and the entries, of the corners and the segments only, which paint_at reads.
@group(0) @binding(1) var<storage, read> stamp_data: array<u32>;

// The profile of a road or path class, as `Profile` in bins.ts: the core tolerance, the reach of the
// cut and fill, the least of them that a level must fit to stamp a road, the cut and the fill slopes,
// how far a path's bench may follow the hillside across it, and the width of a road's sidewalks. A
// uniform array element takes a multiple of 16 bytes.
struct Profile {
    tol: f32,
    reach: f32,
    least: f32,
    cut: f32,
    fill: f32,
    tilt: f32,
    @size(8) walk: f32,
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
const RAIL: u32 = 10u;
const PAD: u32 = 11u;
const AREA: u32 = 12u;
const CORNER: u32 = 14u;
// As RAIL_CODE and SLEEPER_M of bins.ts: the surface code of the first track, and the spacing of its sleepers.
const TRACK_CODE: u32 = 9u;
const SLEEPER_M: f32 = 0.6;
const FORD: u32 = 1u;
const CORE_A: u32 = 2u;
const CORE_B: u32 = 4u;
const CONE_A: u32 = 8u;
const CONE_B: u32 = 16u;
const JOIN_A: u32 = 32u;
const JOIN_B: u32 = 64u;
// A segment's code byte holds its surface code in the low 4 bits, and above them the sides with a
// sidewalk, as LEFT and RIGHT of town.ts: its left and its right.
const SURFACE_BITS: u32 = 15u;
const LEFT_WALK: u32 = 16u;
const RIGHT_WALK: u32 = 32u;

// A paint list holds every record whose rank at a point of its cell is at most PAINT_M: past a road's
// or its sidewalk's outer edge, the metres to that edge.
const PAINT_M: f32 = 1.0;

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
const INF: f32 = 1e30;
// As WARP_M of bins.ts: the metres over which the cross-slope of a junction fades out.
const WARP_M: f32 = 6.0;
// A bridge rests on its abutments for ABUTMENT_CELLS mesh cells, and a tunnel runs on the ground into its
// portal for PORTAL_CELLS, from an end that meets the ground, as stamp-core.wgsl holds their ground.
const ABUTMENT_CELLS: f32 = 2.0;
const PORTAL_CELLS: f32 = 2.0;

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

fn word(at: u32) -> u32 {
    return stamp_data[at];
}

fn real(at: u32) -> f32 {
    return bitcast<f32>(stamp_data[at]);
}

fn point(at: u32) -> vec2i {
    return vec2i(i32(stamp_data[at]), i32(stamp_data[at + 1u]));
}

// Where a point p + f lies on a road or path segment, f the part of a unit: metres from its line, the
// height there, the side, and whether the core, the cone and the paint apply. Inside the segment's mitre
// quad the height runs from end to end along it; past an end of a line, a round cap holds the end's
// height. Near a junction the height tilts across the segment by the cross-slopes of its ends.
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

fn along(r: u32, p: vec2i, f: vec2f, unit: f32) -> Along {
    let a = point(r);
    let b = point(r + 2u);
    let flags = (word(r + 5u) >> 8u) & 0xffu;
    let ab = vec2f(b - a);
    let ap = vec2f(p - a) + f;
    let bp = vec2f(p - b) + f;
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

// A track's sleepers from its start at a texel: a track counts them in its last words; a bridge, a tunnel
// or a gallery of a track counts SLEEPER_M from an end that meets the ground.
fn sleepers_at(r: u32, kind: u32, g: Along) -> f32 {
    if (kind == RAIL) {
        return mix(real(r + 14u), real(r + 15u), clamp(g.da / max(g.da + g.db, 1e-6), 0.0, 1.0));
    }
    let start = real(r + 14u);
    return select(-(real(r + 15u) + g.db), start + g.da, start < 1e29) / SLEEPER_M;
}

// What paints a point: the record of the best rank; the edge of the carriageway of its road (negative
// inside), the metres from its middle (signed by side), its surface, the width of its sidewalk on the
// point's side, a ford, a track's sleepers from its start.
struct Paint {
    rank: f32,
    record: u32,
    road: f32,
    across: f32,
    surface: u32,
    walk: f32,
    ford: bool,
    sleepers: f32,
    // The nearest corner's distance to its arc and its sidewalk's width, INF where no corner holds the point.
    corner: f32,
    corner_walk: f32,
}

fn no_paint() -> Paint {
    return Paint(INF, NO_DATA, INF, 0.0, 0u, 0.0, false, 0.0, INF, 0.0);
}

// Whether a point p + f lies in a junction's corner, where the arc is the nearest edge: between the ends
// of its arc seen from its centre, off both roads or less than the narrower one's half-width onto them,
// as inCorner of corners.ts has it. Then the metres inside the arc, the sidewalk's width there, and the
// point and the arc's ends about its centre in units.
struct InCorner {
    holds: bool,
    d: f32,
    walk: f32,
    e: vec2f,
    ea: vec2f,
    eb: vec2f,
}

fn in_corner(r: u32, p: vec2i, f: vec2f, unit: f32, level: u32) -> InCorner {
    let o = point(r);
    let ea = vec2f(point(r + 2u) - o);
    let eb = vec2f(point(r + 10u) - o);
    let e = vec2f(p - o) + f;
    let onto = real(r + 12u) / unit;
    var out = InCorner(false, 0.0, 0.0, e, ea, eb);
    // The arc's centre lies far off both roads; atan2 knows no angle there.
    if (dot(e, e) == 0.0 || !road_shows(real(r + 12u), profiles[(word(r + 5u) >> 16u) & 0xffu], level)) {
        return out;
    }
    let t = atan2(ea.x * e.y - ea.y * e.x, dot(ea, e)) / atan2(ea.x * eb.y - ea.y * eb.x, dot(ea, eb));
    if (t < 0.0 || t > 1.0 || dot(e - ea, ea) > onto * length(ea) || dot(e - eb, eb) > onto * length(eb)) {
        return out;
    }
    let walks = word(r + 8u);
    out.holds = true;
    out.d = (mix(length(ea), length(eb), t) - length(e)) * unit;
    out.walk = mix(f32(walks & 0xffu), f32((walks >> 8u) & 0xffu), t) * 0.1;
    return out;
}

// A corner paints its asphalt and the sidewalk round it, and outranks all but a carriageway.
fn paint_corner(paint: ptr<function, Paint>, r: u32, corner: InCorner) {
    if (corner.d < (*paint).corner) {
        (*paint).corner = corner.d;
        (*paint).corner_walk = corner.walk;
    }
    if (corner.d - 1500.0 < (*paint).rank) {
        // Far from any road's middle: no middle line.
        *paint = Paint(corner.d - 1500.0, r, corner.d, 1e4, word(r + 5u) & SURFACE_BITS, corner.walk, false, 0.0, (*paint).corner, (*paint).corner_walk);
    }
}

// A line's half-width at a point: the line's own gates it on every segment alike. Beside the line's
// index, the 2 cm steps that its edge lies out at the segment's start and end taper it to a partner's
// at a joint.
fn line_half(r: u32, g: Along) -> f32 {
    let taper = bitcast<i32>(word(r + 13u));
    return real(r + 8u) + mix(f32((taper << 8u) >> 24u), f32(taper >> 24u), clamp(g.da / max(g.da + g.db, 1e-6), 0.0, 1.0)) * 0.02;
}

// The width of a town road's sidewalk on a point's side, 0 for none.
fn line_walk(r: u32, rule: Profile, g: Along) -> f32 {
    return select(0.0, rule.walk, (word(r + 5u) & select(LEFT_WALK, RIGHT_WALK, g.side > 0.0)) != 0u);
}

// Whether a bridge's deck or a tunnel's bore holds a point: `cells` mesh cells past its ends that meet
// the ground. It paints nothing there.
fn off_ground(r: u32, g: Along, rule: Profile, level: u32, texel: f32, cells: f32) -> bool {
    return g.lateral < INF && road_shows(real(r + 8u), rule, level) && min(real(r + 14u) + g.da, real(r + 15u) + g.db) >= cells * MESH_TEXELS * texel;
}

// The paint follows the carriageway: a road that ends at a junction ends at its mitre line, but for the
// round cap that fills the outer wedge of a bend where the through road runs on. A point in a carriageway
// takes it, then one in a road's sidewalk that road, so no path or walk paints over either, and the snap
// finds the sidewalk's outer edge under a house's forecourt. Elsewhere, and between sidewalks, the
// nearest outer edge wins: two roads' sidewalks meet in a corner, and a side road's mouth cuts through
// the through road's sidewalk.
fn paint_line(paint: ptr<function, Paint>, r: u32, kind: u32, g: Along, h: f32, walk: f32) {
    let paved = kind == ROAD || kind == RAIL || kind == BRIDGE || kind == TUNNEL;
    let x = g.lateral - h;
    let rank = select(select(x - walk, x - walk - 1000.0, walk > 0.0 && x < walk), x - 2000.0, paved && x < 0.0);
    if (g.paint && rank < (*paint).rank) {
        let surface = word(r + 5u) & SURFACE_BITS;
        *paint = Paint(rank, r, x, g.side * g.lateral, surface, walk, ((word(r + 5u) >> 8u) & FORD) != 0u,
            select(0.0, sleepers_at(r, kind, g), surface >= TRACK_CODE), (*paint).corner, (*paint).corner_walk);
    }
}

// The paint after the last record: on a carriageway in a corner the arc is the kerb, with no shadow or
// edge line of the road's edge that it replaces.
fn painted(paint: Paint) -> Paint {
    var out = paint;
    if (out.road < 0.0 && out.corner < INF) {
        out.road = min(out.road, out.corner);
        out.walk = out.corner_walk;
    }
    return out;
}

// The paint at a point p + f of a near node, from the node's paint lists: what the stamp writes to the
// mask, exact, where the rank is at most PAINT_M. Past that a list may lack the record that paints.
fn paint_at(level: u32, first: vec2i, data: u32, p: vec2i, f: vec2f) -> Paint {
    var paint = no_paint();
    let shift = 22u - level;
    let unit = unit_metres(p.y);
    let texel = unit * f32(1 << shift);
    let size = CELL_TEXELS << shift;
    let cell = clamp((p - ((first << vec2u(shift)) - vec2i(size))) / size, vec2i(0), vec2i(GRID - 1));
    let records = data + HEAD + OFFSETS + word(data + 1u);
    let lists = records + RECORD * word(data);
    let c = lists + u32(cell.y * GRID + cell.x);
    for (var e = word(c); e < word(c + 1u); e++) {
        let r = records + RECORD * word(lists + OFFSETS + e);
        let kind = word(r + 4u);
        if (kind == CORNER) {
            let corner = in_corner(r, p, f, unit, level);
            if (corner.holds) {
                paint_corner(&paint, r, corner);
            }
            continue;
        }
        let rule = profiles[(word(r + 5u) >> 16u) & 0xffu];
        let g = along(r, p, f, unit);
        if (g.lateral < INF && !(kind == BRIDGE && off_ground(r, g, rule, level, texel, ABUTMENT_CELLS))
            && !(kind == TUNNEL && off_ground(r, g, rule, level, texel, PORTAL_CELLS))) {
            paint_line(&paint, r, kind, g, line_half(r, g), line_walk(r, rule, g));
        }
    }
    return painted(paint);
}
