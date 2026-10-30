// PROTOTYPE road system (a study; never merges). The looks of the ways, materials 20 to 39 of the
// corridors (LOOK of section.ts): asphalt with its grain, wear, repairs, cracks and markings by class;
// concrete, setts and pavers; gravel, dirt and grass tracks; trodden trails; steps; a track's ballast,
// sleepers, rails, rack and cable. Each detail fades to its mean once a pixel spans it, so the far
// look keeps the colour of the mask's paint. corridor.wgsl calls `look_fragment`, `look_sink` and
// `corridor_fine`.
// #include ../ground/look.wgsl

// As LOOK of section.ts.
const LOOK_ASPHALT: u32 = 20u;
const LOOK_MAIN: u32 = 21u;
const LOOK_MOTORWAY: u32 = 22u;
const LOOK_CONCRETE: u32 = 23u;
const LOOK_SETT: u32 = 24u;
const LOOK_PAVERS: u32 = 25u;
const LOOK_GRAVEL: u32 = 26u;
const LOOK_DIRT: u32 = 27u;
const LOOK_TRAIL: u32 = 28u;
const LOOK_BALLAST: u32 = 29u;
// The detail meshes of details.ts, which keep their place to a fraction of a world unit and sink far off.
const LOOK_TREAD: u32 = 30u;
const LOOK_RISER: u32 = 31u;
const LOOK_SLEEPER: u32 = 32u;
const LOOK_SLEEPER_WOOD: u32 = 33u;
const LOOK_RAIL: u32 = 34u;
const LOOK_RAIL_HEAD: u32 = 35u;
const LOOK_RACK: u32 = 36u;
const LOOK_CABLE: u32 = 37u;

// The detail meshes sink into the ballast or the core from DETAIL_M to DETAIL_END_M from the eye, where
// a sleeper spans about a pixel; the paint under them shows the same sleepers and rails.
const DETAIL_M: f32 = 140.0;
const DETAIL_END_M: f32 = 190.0;
// Swiss dashes: a centre line 3 m on, 6 m off; a lane line on a motorway 6 m on, 12 m off.
const DASH_ON_M: f32 = 3.0;
const DASH_PERIOD_M: f32 = 9.0;
const LANE_ON_M: f32 = 6.0;
const LANE_PERIOD_M: f32 = 18.0;

struct WayLook {
    albedo: vec3f,
    // A bump of the normal in the world's x and y.
    bump: vec2f,
    // The share of the ground look that shows through.
    ground: f32,
    // Gloss: polished steel, wet mud, tar.
    spec: f32,
    // The share of a mirror: a puddle.
    mirror: f32,
}

fn is_detail(mat: u32) -> bool {
    return mat >= LOOK_TREAD && mat <= 39u;
}

// The fraction of a world unit that a detail vertex keeps in its half-width word (fine of details.ts).
fn corridor_fine(colour: vec4f, road: vec2f) -> vec2f {
    let mat = u32(round(colour.r * 255.0));
    if ((u32(round(colour.a * 255.0)) & 12u) != 12u || !is_detail(mat)) {
        return vec2f(0.0);
    }
    let q = u32(round(road.y * 32767.0));
    return vec2f(f32(q >> 8u) / 127.0, f32(q & 255u) / 255.0) - 0.5;
}

// How far a detail vertex sinks at `d` metres from the eye.
fn look_sink(colour: vec4f, d: f32) -> f32 {
    return select(0.0, 0.4 * smoothstep(DETAIL_M, DETAIL_END_M, d), is_detail(u32(round(colour.r * 255.0))));
}

// A detail of `size` metres where a pixel is `pw` metres wide: 1 while it is resolved, 0 once a pixel spans it.
fn seen(size: f32, pw: f32) -> f32 {
    return 1.0 - smoothstep(0.25 * size, size, pw);
}

// A dashed line: the share of a pixel `aw` metres long along the line that falls on a dash, `on` metres
// of each `period`; far off, the dashes' mean.
fn dashes(along: f32, on: f32, period: f32, phase: f32, aw: f32) -> f32 {
    let x = fract((along + phase) / period) * period;
    let cov = band(x, 0.0, on, aw) + band(x, period, period + on, aw) + band(x, -period, on - period, aw);
    return mix(min(cov, 1.0), on / period, smoothstep(0.15 * on, 0.6 * on, aw));
}

// Pebbles about `size` metres across: x their tone in [-0.5, 0.5], y how deep in a joint (0 in the joint),
// z a second hash; faded to their mean beyond resolution.
fn pebbles(p: vec2f, size: f32, pw: f32) -> vec3f {
    let s = seen(size, pw);
    if (s <= 0.0) {
        return vec3f(0.0, 1.0, 0.5);
    }
    let v = voronoi(p, size);
    return vec3f((v.x - 0.5) * s, mix(1.0, smoothstep(0.0, 0.3, v.y), s), v.z);
}

// The asphalt's own texture: tone patches, aggregate, chips. Mean about 1.
fn asphalt_grain(p: vec2f, pw: f32) -> f32 {
    var t = 0.94 + 0.08 * noise(p / 9.0) + 0.04 * noise(p / 37.0 + 11.0);
    let fine = seen(0.03, pw);
    let mid = seen(0.12, pw);
    t *= 1.0 + 0.16 * mid * (noise(p / 0.05 + 3.0) - 0.5) + 0.3 * fine * (noise(p / 0.011) - 0.5);
    // Light chips of aggregate, about 1 cm.
    let chip = step(0.9, hash(floor(p / 0.012)));
    t *= 1.0 + fine * (0.5 * chip - 0.05);
    return t;
}

// Tar-sealed cracks of asphalt at p, along and across a road `half` metres wide: a share of the pixel.
fn cracks(p: vec2f, across: f32, half: f32, along: f32, pw: f32, cw: f32, aw: f32) -> f32 {
    let vis = seen(0.08, pw);
    if (vis <= 0.0) {
        return 0.0;
    }
    // The joint in the middle, sealed in stretches.
    let seam_x = across - 0.03 * (noise(vec2f(along / 3.0, 5.0)) - 0.5);
    var c = 0.7 * band(seam_x, -0.01, 0.01, cw) * smoothstep(0.45, 0.5, noise(vec2f(along / 25.0, 1.0)));
    // Cracks across, about every 7 m in some cells.
    let cell = floor(along / 7.0);
    let hc = hash(vec2f(cell, 3.0));
    if (hc < 0.45) {
        let at = (cell + 0.2 + 0.6 * hash(vec2f(cell, 9.0))) * 7.0 + 0.25 * (noise(vec2f(across / 0.8, cell)) - 0.5) + 0.1 * across * (hc - 0.2);
        let reach = mix(0.3, 1.0, hash(vec2f(cell, 13.0))) * half;
        c = max(c, band(along, at - 0.01, at + 0.01, aw) * step(abs(across - (hc - 0.2) * half), reach));
    }
    // Map cracks: lines of a noise field where it crosses its mean, in patches.
    let field = noise(p / 1.6 + 31.0);
    let fw = max(fwidth(field), 1e-4);
    let patchy = smoothstep(0.62, 0.7, noise(p / 13.0 + 2.0)) + 0.6 * smoothstep(0.5, 0.15, half - abs(across)) * step(0.5, noise(p / 5.0));
    c = max(c, min(patchy, 1.0) * (1.0 - smoothstep(0.0, 1.5 * fw + 0.004, abs(field - 0.5))));
    return c * vis;
}

// A repair of asphalt along a road: 1 inside a patch, its seal on the edge; x the patch, y its seal.
fn repairs(across: f32, half: f32, along: f32, cw: f32, aw: f32) -> vec2f {
    let cell = floor(along / 23.0);
    let h = hash(vec2f(cell, 41.0));
    if (h > 0.4) {
        return vec2f(0.0);
    }
    let start = (cell + 0.1 + 0.4 * hash(vec2f(cell, 42.0))) * 23.0;
    var a0 = start;
    var a1 = start + 2.0 + 9.0 * hash(vec2f(cell, 43.0));
    var x0 = 0.0;
    var x1 = half;
    let kind = hash(vec2f(cell, 44.0));
    if (kind < 0.35) {
        // A trench for a pipe along the road.
        let at = (hash(vec2f(cell, 45.0)) - 0.5) * 1.4 * half;
        x0 = at - 0.35;
        x1 = at + 0.35;
        a1 = start + 8.0 + 14.0 * hash(vec2f(cell, 46.0));
    } else if (kind < 0.6) {
        // A square over a pothole or a manhole's dig.
        let at = (hash(vec2f(cell, 47.0)) - 0.5) * 1.5 * half;
        x0 = at - 0.6;
        x1 = at + 0.6;
        a1 = start + 1.2;
    } else if (kind < 0.8) {
        x0 = -half;
        x1 = 0.0;
    }
    let inside = band(across, x0, x1, cw) * band(along, a0, a1, aw);
    let seal = inside * (1.0 - band(across, x0 + 0.03, x1 - 0.03, cw) * band(along, a0 + 0.03, a1 - 0.03, aw));
    return vec2f(inside, seal);
}

// The road paint by class: `mat` 21 main roads, 22 motorways and trunks; `oneway` without a middle line.
fn markings(mat: u32, oneway: bool, across: f32, half: f32, along: f32, cw: f32, aw: f32) -> f32 {
    let inside = half - abs(across);
    var m = 0.0;
    if (mat == LOOK_MOTORWAY) {
        m = band(inside, 0.25, 0.45, cw);
        let lanes = max(1.0, round(2.0 * half / select(3.6, 3.5, oneway)));
        if (oneway) {
            let lane = 2.0 * half / lanes;
            for (var k = 1.0; k < lanes; k += 1.0) {
                m = max(m, band(across + half - k * lane, -0.075, 0.075, cw) * dashes(along, LANE_ON_M, LANE_PERIOD_M, k * 3.0, aw));
            }
        } else {
            m = max(m, band(across, -0.075, 0.075, cw));
        }
    } else if (half >= 2.75) {
        m = band(inside, 0.2, 0.35, cw);
        if (!oneway) {
            m = max(m, band(across, -0.06, 0.06, cw) * dashes(along, DASH_ON_M, DASH_PERIOD_M, 0.0, aw));
        } else {
            let lanes = round(2.0 * half / 3.4);
            if (lanes >= 2.0) {
                let lane = 2.0 * half / lanes;
                for (var k = 1.0; k < lanes; k += 1.0) {
                    m = max(m, band(across + half - k * lane, -0.06, 0.06, cw) * dashes(along, DASH_ON_M, DASH_PERIOD_M, 0.0, aw));
                }
            }
        }
    }
    return m;
}

// A paved road: `base` its colour, the wheel paths of its lanes polished, the crown lighter, the edges
// dirtier, repairs, sealed cracks and, by class, markings. Past `half` it is the shoulder: a paved strip
// on a main road, a gravel verge on a minor one, each fraying into the ground at its outer edge.
fn paved_look(mat: u32, code: u32, region: u32, p: vec2f, across: f32, half: f32, along: f32, pw: f32, cw: f32, aw: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let base = rule(region, slot_roads() + i32(select(code, 1u, code == 7u)) - 1).rgb;
    let x = abs(across);
    let edge = half - x;
    if (edge < 0.0) {
        let out_m = -edge;
        if (mat == LOOK_ASPHALT) {
            // A gravel verge: pebbles and grass that takes over toward its outer edge.
            let peb = pebbles(p, 0.035, pw);
            out.albedo = rule(region, slot_roads() + 2).rgb * (0.92 + 0.12 * noise(p / 0.7)) * (1.0 + 0.4 * peb.x) * mix(0.75, 1.0, peb.y);
            out.ground = smoothstep(0.05, 0.45, out_m + 0.18 * (noise(p / 0.6) - 0.5) + 0.1 * (noise(p / 0.17) - 0.5));
        } else {
            // A paved shoulder: the road's asphalt, dustier, with grit; grass creeps over its outer edge.
            out.albedo = base * 1.08 * asphalt_grain(p, pw) * (1.0 + 0.1 * (noise(p / 0.4) - 0.5));
            out.ground = smoothstep(0.42, 0.62, out_m + 0.12 * (noise(p / 0.5) - 0.5) + 0.06 * (noise(p / 0.11) - 0.5));
        }
        return out;
    }
    var t = asphalt_grain(p, pw);
    // Stretches resurfaced in different years, each its own tone, the newer darker.
    let stretch = floor(along / 70.0 + 0.6 * noise(vec2f(along / 70.0, 4.0)));
    t *= 0.9 + 0.17 * hash(vec2f(stretch, 21.0));
    // Lanes: two on a road wider than 4.8 m, one elsewhere; the wheels run 0.8 m either side of a lane's middle.
    let lanes = select(1.0, max(2.0, round(2.0 * half / 3.5)), half >= 2.4);
    let lane = 2.0 * half / lanes;
    let in_lane = fract((across + half) / lane) * lane - 0.5 * lane;
    let wob = 0.06 * (noise(vec2f(along / 11.0, across)) - 0.5);
    let wheels = smoothstep(0.4, 0.12, abs(abs(in_lane + wob) - 0.85));
    // Polished wheel paths: darker and smoother; oil down a lane's middle; the crown between lanes lighter.
    t *= 1.0 - 0.15 * wheels - 0.07 * smoothstep(0.3, 0.0, abs(in_lane)) * (0.6 + 0.8 * noise(vec2f(along / 2.0, across))) * seen(0.5, pw);
    t *= 1.0 + 0.05 * smoothstep(0.6, 0.0, abs(abs(in_lane) - 0.5 * lane)) * step(1.5, lanes);
    // The edges: dust and grit, browner.
    let dirt = smoothstep(0.6, 0.0, edge + 0.15 * (noise(p / 0.8) - 0.5));
    var c = base * t;
    c = mix(c, c * vec3f(1.12, 1.06, 0.95) * 1.05, 0.55 * dirt);
    // Repairs: a darker, finer patch with a tar seal on its edge.
    let fix = repairs(across, half, along, cw, aw);
    c = mix(c, base * 0.86 * (0.97 + 0.06 * noise(p / 0.3)), fix.x * 0.8);
    let tar = max(cracks(p, across, half, along, pw, cw, aw), 0.4 * fix.y);
    c = mix(c, vec3f(0.035, 0.034, 0.033), 0.85 * tar);
    out.spec = 0.25 * tar + 0.06 * wheels;
    if (mat != LOOK_ASPHALT) {
        let paint = markings(mat, code == 7u, across, half, along, cw, aw);
        let worn = 1.0 - 0.45 * seen(0.02, pw) * smoothstep(0.6, 0.7, noise(p / 0.018) * 0.6 + noise(p / 0.4) * 0.5) - 0.25 * wheels;
        c = mix(c, rule(region, slot_road_line()).rgb * 1.08, paint * worn);
    }
    out.albedo = c;
    // A narrow way without a shoulder: grass creeps over its edges.
    if (half < 2.0) {
        out.ground = 1.0 - smoothstep(0.0, 0.1, edge + 0.08 * (noise(p / 0.3) - 0.5) + 0.04 * (noise(p / 0.07) - 0.5));
    }
    return out;
}

// Concrete: slabs 5 m long with sealed joints; on a track two strips with grass between them.
fn concrete_look(region: u32, p: vec2f, across: f32, half: f32, along: f32, pw: f32, cw: f32, aw: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let base = vec3f(0.33, 0.325, 0.31);
    let slab = vec2f(floor(along / 5.0), select(0.0, sign(across), half > 2.2));
    var c = base * (0.95 + 0.1 * hash(slab)) * (0.95 + 0.1 * noise(p / 3.0));
    // Brushed across, fine.
    c *= 1.0 + 0.08 * seen(0.03, pw) * (noise(vec2f(along / 0.015, across / 0.5)) - 0.5) + 0.12 * seen(0.02, pw) * (noise(p / 0.009) - 0.5);
    let joint = max(band((fract(along / 5.0 + 0.5) - 0.5) * 5.0, -0.008, 0.008, aw), select(0.0, band(across, -0.008, 0.008, cw), half > 2.2));
    c = mix(c, vec3f(0.04), 0.85 * joint);
    // A crack across a corner of some slabs.
    let local = vec2f(fract(along / 5.0) * 5.0, abs(across));
    let diag = abs(local.x * 0.6 + local.y - 1.2 - 2.0 * hash(slab + 4.0)) * 0.8;
    c *= 1.0 - 0.5 * step(hash(slab + 8.0), 0.3) * band(diag + 0.05 * (noise(p / 0.2) - 0.5), -0.005, 0.005, cw + aw) * seen(0.05, pw);
    c *= 1.0 - 0.15 * smoothstep(0.5, 0.0, half - abs(across));
    out.albedo = c;
    if (half <= 2.2) {
        // Two strips 0.55 m wide, their middles 0.75 m out.
        let strip = band(abs(across), 0.48, 1.03, cw);
        out.ground = 1.0 - strip;
        out.albedo *= 0.95 + 0.1 * smoothstep(0.2, 0.0, abs(abs(across) - 0.75));
    }
    return out;
}

// Setts: granite cubes about 11 cm in rows across the way, each its own tone and domed, in sandy joints.
fn sett_look(p: vec2f, across: f32, along: f32, pw: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let s = seen(0.1, pw);
    let row = floor(along / 0.115);
    let q = vec2f(across / 0.11 + 0.5 * (row % 2.0) + 0.3 * hash(vec2f(row, 1.0)), fract(along / 0.115));
    let cell = vec2f(floor(q.x), row);
    let local = vec2f(fract(q.x), q.y) - 0.5 + 0.06 * (vec2f(hash(cell + 3.0), hash(cell + 7.0)) - 0.5);
    let h = hash(cell);
    let tone = mix(vec3f(0.30, 0.29, 0.28), mix(vec3f(0.33, 0.30, 0.285), vec3f(0.25, 0.255, 0.265), step(0.5, hash(cell + 11.0))), step(0.6, h));
    let joint = smoothstep(0.5, 0.36, max(abs(local.x), abs(local.y)));
    let sand = vec3f(0.24, 0.21, 0.17);
    out.albedo = mix(vec3f(0.27, 0.26, 0.25), mix(sand, tone * (0.85 + 0.3 * hash(cell + 5.0)), joint), s);
    out.bump = s * joint * 0.9 * vec2f(local.x, local.y);
    return out;
}

// Pavers: concrete blocks 22 by 11 cm in a running bond, grey with a warm tint, bevelled.
fn pavers_look(region: u32, p: vec2f, across: f32, along: f32, pw: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let base = rule(region, slot_roads() + i32(PAVING_CODE) - 1).rgb;
    let s = seen(0.06, pw);
    let row = floor(along / 0.11);
    let q = vec2f(across / 0.22 + 0.5 * (row % 2.0), fract(along / 0.11));
    let cell = vec2f(floor(q.x), row);
    let local = vec2f(fract(q.x) - 0.5, q.y - 0.5);
    let edge = min(0.5 - abs(local.x) * 1.0, (0.5 - abs(local.y)) * 0.5);
    let bevel = smoothstep(0.0, 0.035, edge);
    let tone = base * (0.86 + 0.24 * hash(cell)) * mix(vec3f(1.0), vec3f(1.05, 1.0, 0.92), step(0.7, hash(cell + 2.0)));
    out.albedo = base * (0.97 + 0.06 * noise(p / 4.0));
    out.albedo = mix(out.albedo, tone * mix(0.45, 1.0, bevel) * (1.0 + 0.1 * (noise(p / 0.02) - 0.5)), s);
    out.bump = s * (1.0 - bevel) * 0.6 * sign(local) * vec2f(step(abs(local.y) * 0.5, abs(local.x) - 0.25), step(abs(local.x) - 0.25, abs(local.y) * 0.5));
    return out;
}

// The irregular edge of an unpaved way: metres in from its edge, moved by noise so it frays.
fn frayed(p: vec2f, edge: f32, amount: f32) -> f32 {
    return edge + amount * (0.6 * (noise(p / 0.9) - 0.5) + 0.3 * (noise(p / 0.27) - 0.5) + 0.2 * (noise(p / 0.08) - 0.5));
}

// A gravel track: compacted wheel tracks, loose stones between and beside them, grass in the middle of a
// little-used one, and an edge that frays into the ground. A gravel path is one tread.
fn gravel_look(region: u32, p: vec2f, across: f32, half: f32, along: f32, pw: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let base = rule(region, slot_roads() + 2).rgb;
    let x = abs(across);
    let track = half > 1.3;
    let wheels = select(smoothstep(half * 0.7, 0.0, x), smoothstep(0.32, 0.1, abs(x - 0.78 + 0.05 * (noise(vec2f(along / 9.0, 2.0)) - 0.5))), track);
    let loose = 1.0 - wheels;
    let peb = pebbles(p, mix(0.022, 0.04, loose), pw);
    let big = pebbles(p + 17.0, 0.09, pw);
    var c = base * (0.9 + 0.12 * noise(p / 2.5) + 0.08 * noise(p / 11.0));
    c *= 1.0 + mix(0.35, 0.7, loose) * peb.x;
    c *= mix(mix(0.82, 0.62, loose), 1.0, peb.y);
    // A scatter of bigger stones off the wheel tracks.
    c = mix(c, base * (0.75 + 0.5 * big.z), loose * step(0.82, big.z) * big.y * seen(0.09, pw));
    // Compacted: finer, darker and warmer.
    c = mix(c, base * vec3f(0.86, 0.83, 0.78) * (0.96 + 0.08 * noise(p / 0.3)), 0.5 * wheels);
    out.albedo = c;
    out.bump = loose * 0.35 * (big.y - 1.0) * (vec2f(noise(p / 0.05), noise(p / 0.05 + 9.0)) - 0.5);
    // A few tufts in the middle of a track, never a patch: a patch of shaded meadow reads as a hole.
    let middle = select(0.0, smoothstep(0.3, 0.1, x + 0.1 * (noise(p / 0.3) - 0.5)) * smoothstep(0.45, 0.7, noise(vec2f(along / 20.0, 6.0))), track);
    out.ground = max(1.0 - smoothstep(0.0, 0.3, frayed(p, half - x, 0.35)), 0.35 * middle * smoothstep(0.55, 0.75, noise(p / 0.07)));
    return out;
}

// Dirt and grass tracks: two worn ruts of soil, grass in the middle and beside them, mud in some
// stretches; on a grass track the ruts are grassed over in part.
fn dirt_look(code: u32, region: u32, p: vec2f, across: f32, half: f32, along: f32, pw: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let soil = rule(region, slot_roads() + 3).rgb;
    let x = abs(across);
    let grassy = code == 5u;
    let wob = 0.08 * (noise(vec2f(along / 7.0, sign(across) * 3.0)) - 0.5);
    let rut_w = select(0.25, 0.19, grassy) + 0.06 * noise(vec2f(along / 3.0, sign(across)));
    let d = abs(x - 0.75 + wob);
    let rut = 1.0 - smoothstep(rut_w - 0.04, rut_w + 0.06, d + 0.05 * (noise(p / 0.2) - 0.5));
    // The bottom of the rut darker and moister; mud where the ruts hold water.
    let wet = smoothstep(0.62, 0.78, noise(vec2f(along / 6.0, 7.0) + p * 0.05)) * select(1.0, 0.4, grassy);
    let deep = smoothstep(rut_w, 0.0, d);
    var c = soil * (0.88 + 0.2 * noise(p / 0.6) + 0.1 * noise(p / 3.0)) * (1.0 - 0.15 * deep);
    let peb = pebbles(p, 0.03, pw);
    c *= (1.0 + 0.4 * peb.x * (1.0 - wet)) * mix(0.85, 1.0, peb.y);
    c = mix(c, soil * vec3f(0.45, 0.42, 0.4), wet * deep);
    out.albedo = c;
    out.spec = 0.5 * wet * deep;
    // Puddles where the ruts are wettest.
    out.mirror = smoothstep(0.55, 0.75, wet * deep * (0.7 + 0.6 * noise(p / 0.8)));
    out.bump = vec2f(-sign(across) * sign(x - 0.75 + wob) * 0.25 * rut * (1.0 - deep), 0.0);
    // Grass covers what is not rut; on a grass track the ruts too, in part.
    let grassed = select(0.0, 0.85 * smoothstep(0.5, 0.75, noise(p / 0.45 + 5.0)), grassy);
    // Worn grass beside the ruts: soil shows through.
    let worn = smoothstep(rut_w + 0.3, rut_w, d) * 0.35;
    out.ground = max(1.0 - max(rut, worn), grassed);
    out.ground = max(out.ground, 1.0 - smoothstep(0.0, 0.25, frayed(p, half - x, 0.3)));
    return out;
}

// A trodden trail: a tread of soil narrower than the way, wandering across it, with stones in it and
// rocks set in it; a trampled margin of grass; on steep or high ground stonier and greyer.
fn trail_look(region: u32, su: vec2f, p: vec2f, across: f32, half: f32, along: f32, pw: f32, slope: f32, altitude: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let rocky = clamp(smoothstep(0.92, 0.7, slope) + smoothstep(1700.0, 2300.0, altitude), 0.0, 1.0);
    let cell = cell_at(su, u32(ceil(lod(su))));
    let rock = rule(cell.b, slot_lithologies() + i32(cell.g)).rgb;
    let soil = mix(mix(rule(region, slot_roads() + 3).rgb, rule(region, slot_roads() + 5).rgb, 0.4), rock * 0.75, 0.85 * rocky);
    // The tread keeps within 0.55 m of the middle, where kept_off of features.wgsl lets plants grow.
    let room = min(max(half - 0.35, 0.05), 0.65);
    let centre = room * 0.45 * (noise(vec2f(along / 13.0, 1.0)) - 0.5) + 0.08 * (noise(vec2f(along / 4.0, 2.0)) - 0.5);
    let tread = min(0.18 + 0.12 * noise(vec2f(along / 5.0, 3.0)) + 0.08 * rocky, half - 0.05);
    let d = frayed(p, tread - abs(across - centre), 0.22);
    // Soil, darker where feet dig in, with grit and small stones.
    let peb = pebbles(p, 0.03, pw);
    var c = soil * (0.85 + 0.25 * noise(p / 0.5) + 0.1 * noise(p / 2.0)) * (1.0 - 0.12 * smoothstep(0.15, 0.0, abs(across - centre)));
    c *= (1.0 + 0.5 * peb.x) * mix(0.8, 1.0, peb.y);
    // Rocks set in the tread, more on rocky ground: domed, in the colour of the rock under it.
    let r = voronoi(p, mix(0.12, 0.17, rocky));
    let set_in = step(1.0 - mix(0.08, 0.25, rocky), r.x) * smoothstep(0.05, 0.16, r.y) * seen(0.2, pw);
    c = mix(c, rock * (0.95 + 0.3 * r.z) * mix(0.8, 1.08, smoothstep(0.05, 0.4, r.y)), set_in);
    out.albedo = c;
    out.bump = set_in * 1.2 * (vec2f(hash(floor(p / 0.2) + 1.0), r.z) - 0.5);
    // A trampled margin: grass with soil showing through.
    out.ground = clamp(1.0 - smoothstep(-0.03, 0.05, d) - 0.35 * smoothstep(-0.3, -0.03, d) * (1.0 - smoothstep(-0.03, 0.05, d)), 0.0, 1.0);
    return out;
}

// A track's bed: crushed stone, rusty between and beside the rails, oily in the middle; on its slopes and
// past them dirtier, fraying into the ground; far off the sleepers and rails that the detail meshes drew near.
fn ballast_look(code: u32, p: vec2f, across: f32, half: f32, sleepers: f32, pw: f32, cw: f32, aw: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let narrow = ((code - RAIL_CODE) & 1u) != 0u;
    let gauge = select(0.7525, 0.53, narrow);
    let x = abs(across);
    // Crushed stone: two sizes over each other, each stone its own tone, shallow gaps.
    let peb = pebbles(p, 0.032, pw);
    let big = pebbles(p * 1.13 + 7.0, 0.055, pw);
    let tone = 1.0 + 0.5 * peb.x + 0.35 * big.x;
    var c = BALLAST * vec3f(1.0 + 0.1 * big.x, 1.0, 1.0 - 0.1 * big.x) * (0.9 + 0.15 * noise(p / 2.0)) * tone * mix(0.72, 1.0, min(peb.y, mix(1.0, big.y, 0.6)));
    // Rust from the rails and brake dust, oil down the middle.
    c = mix(c, c * vec3f(1.25, 0.9, 0.7), 0.6 * smoothstep(0.4, 0.0, abs(x - gauge)));
    c *= 1.0 - 0.25 * smoothstep(0.35, 0.0, x);
    let slope = smoothstep(half - 0.45, half, x);
    c = mix(c, c * vec3f(0.9, 0.85, 0.78), slope);
    out.albedo = c;
    // Each stone a facet of its own slope: crushed, not rounded.
    let facet = vec2f(hash(floor(p / 0.038) + 3.0), peb.z) - 0.5;
    out.bump = 0.9 * seen(0.038, pw) * facet;
    // Past the bed: the formation's dirt, weeds and the ground.
    out.ground = smoothstep(0.0, 0.35, frayed(p, x - half, 0.25));
    // The paint of the sleepers and rails where their meshes have sunk.
    let far = smoothstep(DETAIL_M, DETAIL_END_M, length(p - look.place.xy));
    if (far > 0.0 && x < half - 0.3) {
        // As the meshes of details.ts: sleepers 0.26 m wide, rail heads 6 cm, rusty sides, a rack between.
        let sleeper_half = select(1.3, 0.9, narrow);
        let off = (fract(sleepers + 0.5) - 0.5) * SLEEPER_SPACING_M;
        let along_m = aw * SLEEPER_SPACING_M;
        let lying = mix(band(off, -0.13, 0.13, along_m), 0.26 / SLEEPER_SPACING_M, smoothstep(0.1, 0.4, along_m));
        var t = mix(out.albedo, vec3f(0.2, 0.195, 0.185), lying * band(across, -sleeper_half, sleeper_half, cw));
        t = mix(t, vec3f(0.13, 0.075, 0.045), band(abs(across) - gauge, -0.05, 0.05, cw));
        t = mix(t, vec3f(0.42, 0.43, 0.44), band(abs(across) - gauge, -0.031, 0.031, cw));
        if (code >= RAIL_CODE + 2u) {
            t = mix(t, vec3f(0.1), band(across, -0.08, 0.08, cw));
        }
        out.albedo = mix(out.albedo, t, far);
    }
    return out;
}

// Steps: concrete or stone treads in a town, each its own tone with a lighter worn middle and a nosing; on a
// trail, treads of soil behind timber beams. A tread's `along` is metres back from its nosing.
fn step_look(mat: u32, code: u32, region: u32, p: vec2f, across: f32, along: f32, pw: f32, n: vec3f) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let trail = code == 4u || code == 5u || code == 6u;
    if (trail) {
        if (mat == LOOK_RISER) {
            // Timber: brown with its grain along the beam.
            out.albedo = vec3f(0.3, 0.24, 0.17) * (0.8 + 0.4 * noise(vec2f(across / 0.4, p.y / 0.006 + p.x / 0.006)));
        } else {
            let peb = pebbles(p, 0.03, pw);
            let beam = band(along, -1.0, 0.13, max(fwidth(along), 1e-4));
            out.albedo = rule(region, slot_roads() + 3).rgb * 1.2 * (0.85 + 0.25 * noise(p / 0.4)) * (1.0 + 0.4 * peb.x) * mix(0.8, 1.0, peb.y);
            out.albedo = mix(out.albedo, vec3f(0.34, 0.27, 0.19) * (0.85 + 0.3 * noise(vec2f(across / 0.5, along / 0.01))), beam);
            // Grass creeps onto the tread from its sides and its back.
            out.ground = (1.0 - beam) * smoothstep(0.25, 0.55, abs(across) / 0.9 + 0.35 * (noise(p / 0.25) - 0.5) + 0.25 * smoothstep(0.2, 0.6, along));
        }
        return out;
    }
    let stone = vec3f(0.36, 0.355, 0.34);
    let h = hash(floor(p / 0.9) + floor(along * 3.0));
    var c = stone * (0.9 + 0.15 * h) * (0.93 + 0.1 * noise(p / 0.25)) * (1.0 + 0.15 * seen(0.02, pw) * (noise(p / 0.008) - 0.5));
    if (mat == LOOK_RISER) {
        c *= 0.85;
    } else {
        c *= 1.0 + 0.08 * smoothstep(0.5, 0.0, abs(across));
        // The nosing: a lighter worn edge, a dark line where it meets the riser below.
        let aw = max(fwidth(along), 1e-4);
        c *= 1.0 + 0.12 * band(along, 0.0, 0.05, aw) - 0.4 * band(along, -1.0, 0.008, aw);
    }
    out.albedo = c;
    return out;
}

// The colour of a corridor fragment of materials 20 to 39, lit, in the haze, with the ground through it.
fn look_fragment(mat: u32, code: u32, su: vec2f, relative: vec3f, altitude: f32, road: vec2f, along: f32, n: vec3f, level: u32) -> vec4f {
    let region = region_at(su);
    let p = look.place.xy + relative.xy;
    let pw = max(length(dpdx(relative)), length(dpdy(relative))) * look.screen.z;
    let cw = max(length(vec2f(dpdx(road.x), dpdy(road.x))), 1e-4);
    let aw_raw = max(length(vec2f(dpdx(along), dpdy(along))), 1e-4);
    let across = road.x;
    let half = road.y;
    var l = WayLook(vec3f(0.5), vec2f(0.0), 0.0, 0.0, 0.0);
    var spec_size = 40.0;
    switch mat {
        case 20u, 21u, 22u: {
            l = paved_look(mat, code, region, p, across, half, along, pw, cw, aw_raw);
        }
        case 23u: {
            l = concrete_look(region, p, across, half, along, pw, cw, aw_raw);
        }
        case 24u: {
            l = sett_look(p, across, along, pw);
        }
        case 25u: {
            l = pavers_look(region, p, across, along, pw);
        }
        case 26u: {
            l = gravel_look(region, p, across, half, along, pw);
        }
        case 27u: {
            l = dirt_look(code, region, p, across, half, along, pw);
        }
        case 28u: {
            l = trail_look(region, su, p, across, half, along, pw, n.z, altitude);
        }
        case 29u: {
            l = ballast_look(code, p, across, half, along, pw, cw, aw_raw);
        }
        case 30u, 31u: {
            l = step_look(mat, code, region, p, across, along, pw, n);
        }
        case 32u: {
            // Concrete sleepers, the dark pads and clips of the rails on their top.
            let narrow = ((code - RAIL_CODE) & 1u) != 0u;
            let gauge = select(0.7525, 0.53, narrow);
            var c = vec3f(0.2, 0.195, 0.185) * (0.9 + 0.15 * noise(p / 0.2)) * mix(vec3f(1.0), vec3f(1.1, 0.92, 0.8), smoothstep(0.35, 0.0, abs(abs(across) - gauge))) * (1.0 + 0.1 * seen(0.02, pw) * (noise(p / 0.01) - 0.5));
            c = mix(c, vec3f(0.05, 0.045, 0.04), band(abs(across), gauge - 0.11, gauge + 0.11, cw) * step(0.8, n.z));
            l.albedo = c;
        }
        case 33u: {
            // Timber sleepers, dark with creosote, their grain across the track.
            l.albedo = vec3f(0.09, 0.065, 0.045) * (0.8 + 0.4 * noise(vec2f(across / 0.5, (p.x + p.y) / 0.01)));
        }
        case 34u: {
            l.albedo = vec3f(0.13, 0.075, 0.045) * (0.8 + 0.3 * noise(p / 0.3));
            l.spec = 0.1;
        }
        case 35u: {
            l.albedo = vec3f(0.42, 0.43, 0.44);
            l.spec = 1.6;
            spec_size = 12.0;
        }
        case 36u: {
            // A ladder rack: rungs 0.1 m apart on its top; dark steel at its sides.
            let m = along * SLEEPER_SPACING_M;
            let rung = band(fract(m / 0.1 + 0.5) - 0.5, -0.25, 0.25, aw_raw * SLEEPER_SPACING_M / 0.1);
            l.albedo = mix(vec3f(0.035, 0.03, 0.028), vec3f(0.2, 0.19, 0.18), select(0.0, rung, n.z > 0.8));
            l.spec = 0.15;
        }
        case 37u: {
            l.albedo = vec3f(0.06, 0.06, 0.065);
            l.spec = 0.5;
        }
        default: {}
    }
    var nn = n;
    if (dot(l.bump, l.bump) > 0.0) {
        nn = normalize(n + vec3f(l.bump, 0.0));
    }
    let share = sun_share(sun_at(su), relative, nn, LIFT_SOLID);
    var lit = light_up(l.albedo, nn, share, 1.0, look.horizon.w);
    if (l.spec > 0.0) {
        let r = reflect(normalize(relative), nn);
        lit += look.sun_colour.rgb * l.spec * share * pow(max(dot(r, look.sun.xyz), 0.0), spec_size) * 0.5;
        lit = mix(lit, sky(r), 0.12 * min(l.spec, 1.0) * (1.0 - max(r.z, 0.0) * 0.5));
    }
    if (l.mirror > 0.0) {
        let r = reflect(normalize(relative), vec3f(0.0, 0.0, 1.0));
        lit = mix(lit, 0.6 * sky(r) * (0.6 + 0.4 * share) + look.sun_colour.rgb * share * pow(max(dot(r, look.sun.xyz), 0.0), 200.0), l.mirror * 0.8);
    }
    var out = atmosphere(lit, relative, altitude);
    if (l.ground > 0.0) {
        out = mix(out, ground_colour(su, relative, altitude, level), clamp(l.ground, 0.0, 1.0));
    }
    return vec4f(out, 1.0);
}
