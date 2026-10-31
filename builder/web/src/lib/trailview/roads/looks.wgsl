// The looks of the faces of a corridor but its band: asphalt with its grain and markings by class; concrete, setts
// and pavers; gravel, dirt and grass tracks; trodden trails; a track's ballast; a town street's kerb and sidewalk. Each detail
// fades to its mean once a pixel spans it, so the far look keeps the colour of the paint. A look says the share
// of the ground that shows through it, and corridor.wgsl draws that share of its pixels as the ground.
// What moves a way's shape reads the place, so it runs on over any border. Only what shades reads the way's count,
// which takes a new base at most once in REPEAT of sweep.ts: a puddle or a streak can stop at an owner's border.
// #include ../ground/look.wgsl
// #include details.wgsl

// As MAT and LOOK of section.ts.
const MAT_KERB: u32 = 2u;
const MAT_SIDEWALK: u32 = 3u;
const LOOK_ASPHALT: u32 = 20u;
const LOOK_MOTORWAY: u32 = 22u;
const LOOK_CONCRETE: u32 = 23u;
const LOOK_SETT: u32 = 24u;
const LOOK_PAVERS: u32 = 25u;
const LOOK_GRAVEL: u32 = 26u;
const LOOK_DIRT: u32 = 27u;
const LOOK_TRAIL: u32 = 28u;
const LOOK_BALLAST: u32 = 29u;
const LOOK_STRIPS: u32 = 30u;
// As CF of section.ts.
const CF_NO_LEFT_LINE: u32 = 8u;
const CF_NO_RIGHT_LINE: u32 = 16u;
// As RANGE_M of roads.ts: asphalt turns to each class's paint toward it. Not the word `corridors`, which falls
// while an owner is on its way.
const CORRIDOR_RANGE_M: f32 = 600.0;
// A lane line on a motorway 6 m on, 12 m off; as DASH_PERIOD_M, its period divides REPEAT of sweep.ts.
const LANE_ON_M: f32 = 6.0;
const LANE_PERIOD_M: f32 = 18.0;

// The first material of details.wgsl.
const DETAIL_MATS: u32 = 40u;

struct WayLook {
    albedo: vec3f,
    // A bump of the normal in the world's x and y.
    bump: vec2f,
    // The share of the ground that shows through.
    ground: f32,
    // Gloss, and the share of a mirror: a puddle.
    spec: f32,
    mirror: f32,
}

// What a pixel of a way shows: `p` in world metres, `across` metres from the middle and `half` metres wide on
// each side, `along` its count along the way, a pixel `pw` metres wide that spans `cw` across and `aw` along.
struct WayPixel {
    p: vec2f,
    across: f32,
    half: f32,
    along: f32,
    pw: f32,
    cw: f32,
    aw: f32,
}

fn surface_colour(region: u32, code: u32) -> vec3f {
    return rule(region, slot_roads() + i32(code) - 1).rgb;
}

// A detail of `size` metres where a pixel is `pw` metres wide: 1 while it is resolved, 0 once a pixel spans it.
fn seen(size: f32, pw: f32) -> f32 {
    return 1.0 - smoothstep(0.25 * size, size, pw);
}

// The share of a pixel `aw` metres long along a line that falls on a dash, `on` metres of each `period`; far
// off, the dashes' share of the paint's line (roads.wgsl).
fn dashes(along: f32, on: f32, period: f32, phase: f32, aw: f32) -> f32 {
    let x = fract((along + phase) / period) * period;
    let cov = band(x, 0.0, on, aw) + band(x, period, period + on, aw) + band(x, -period, on - period, aw);
    return mix(min(cov, 1.0), on / period, dash_mean(on, aw));
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

// A carriageway's asphalt, a function of the place alone, so that the ways at a junction and its faces show one
// surface. Main, minor and one-way roads share one base near the eye, which turns to each class's own paint
// toward the corridors' reach.
fn asphalt(code: u32, region: u32, p: vec2f, pw: f32) -> vec3f {
    var base = surface_colour(region, code);
    if (code <= 2u || code == CODE_ONEWAY) {
        let mean = 0.5 * (surface_colour(region, 1u) + surface_colour(region, 2u));
        base = mix(mean, base, smoothstep(0.4 * CORRIDOR_RANGE_M, 0.9 * CORRIDOR_RANGE_M, length(p - look.place.xy)));
    }
    return base * asphalt_grain(p, pw);
}

// Whether a pixel lies on a side of its way that opens into a junction's faces: `flags` are the face's (CF of
// section.ts). That edge has no line, and it does not fray.
fn opens(w: WayPixel, flags: u32) -> bool {
    return (w.across > 0.0 && (flags & CF_NO_LEFT_LINE) != 0u) || (w.across < 0.0 && (flags & CF_NO_RIGHT_LINE) != 0u);
}

// The lines of a way, by class: `mat` LOOK_MAIN or LOOK_MOTORWAY, `oneway` without a middle line.
fn markings(mat: u32, oneway: bool, w: WayPixel, flags: u32) -> f32 {
    let inside = w.half - abs(w.across);
    let edge = select(1.0, 0.0, opens(w, flags));
    var m = 0.0;
    if (mat == LOOK_MOTORWAY) {
        m = band(inside, 0.25, 0.45, w.cw) * edge;
        if (oneway) {
            let lanes = max(1.0, round(2.0 * w.half / 3.5));
            let lane = 2.0 * w.half / lanes;
            for (var k = 1.0; k < lanes; k += 1.0) {
                m = max(m, band(w.across + w.half - k * lane, -0.075, 0.075, w.cw) * dashes(w.along, LANE_ON_M, LANE_PERIOD_M, k * 3.0, w.aw));
            }
        } else {
            m = max(m, band(w.across, -0.075, 0.075, w.cw));
        }
    } else if (w.half >= MARKED_HALF_M) {
        m = band(inside, 0.2, 0.35, w.cw) * edge;
        if (!oneway) {
            m = max(m, band(w.across, -0.06, 0.06, w.cw) * dashes(w.along, DASH_ON_M, DASH_PERIOD_M, 0.0, w.aw));
        } else {
            let lanes = round(2.0 * w.half / 3.4);
            if (lanes >= 2.0) {
                let lane = 2.0 * w.half / lanes;
                for (var k = 1.0; k < lanes; k += 1.0) {
                    m = max(m, band(w.across + w.half - k * lane, -0.06, 0.06, w.cw) * dashes(w.along, DASH_ON_M, DASH_PERIOD_M, 0.0, w.aw));
                }
            }
        }
    }
    return m;
}

// A paved road: the asphalt of the place and, by class, markings. Past `half` it is the shoulder: a paved strip
// on a main road, a gravel verge on a minor one, each fraying into the ground at its outer edge.
fn paved_look(mat: u32, code: u32, region: u32, w: WayPixel, flags: u32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    let edge = w.half - abs(w.across);
    if (edge < 0.0) {
        let out_m = -edge;
        if (mat == LOOK_ASPHALT) {
            // A gravel verge: pebbles and grass that takes over toward its outer edge.
            let peb = pebbles(p, 0.035, w.pw);
            out.albedo = surface_colour(region, CODE_GRAVEL) * (0.92 + 0.12 * noise(p / 0.7)) * (1.0 + 0.4 * peb.x) * mix(0.75, 1.0, peb.y);
            out.ground = smoothstep(0.05, 0.45, out_m + 0.18 * (noise(p / 0.6) - 0.5) + 0.1 * (noise(p / 0.17) - 0.5));
        } else {
            // A paved shoulder: the road's asphalt, dustier, with grit; grass creeps over its outer edge.
            out.albedo = asphalt(code, region, p, w.pw) * 1.08 * (1.0 + 0.1 * (noise(p / 0.4) - 0.5));
            out.ground = smoothstep(0.42, 0.62, out_m + 0.12 * (noise(p / 0.5) - 0.5) + 0.06 * (noise(p / 0.11) - 0.5));
        }
        return out;
    }
    var c = asphalt(code, region, p, w.pw);
    if (mat != LOOK_ASPHALT) {
        let paint = markings(mat, code == CODE_ONEWAY, w, flags);
        let worn = 1.0 - 0.45 * seen(0.02, w.pw) * smoothstep(0.6, 0.7, noise(p / 0.018) * 0.6 + noise(p / 0.4) * 0.5);
        c = mix(c, rule(region, slot_road_line()).rgb * 1.08, paint * worn);
    }
    out.albedo = c;
    // A narrow way without a shoulder: grass creeps over its edges.
    if (w.half < 2.0 && !opens(w, flags)) {
        out.ground = 1.0 - smoothstep(0.0, 0.1, edge + 0.08 * (noise(p / 0.3) - 0.5) + 0.04 * (noise(p / 0.07) - 0.5));
    }
    return out;
}

// Concrete: slabs 5 m long with sealed joints; on a narrow track two strips on the way's asphalt.
fn concrete_look(code: u32, region: u32, w: WayPixel, track: bool) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    let across = w.across;
    let along = w.along;
    let wide = w.half > 2.2;
    let slab = vec2f(floor(along / 5.0), select(0.0, sign(across), wide));
    var c = rule(region, slot_concrete()).rgb * (0.95 + 0.1 * hash(slab)) * (0.95 + 0.1 * noise(p / 3.0));
    // Brushed across, fine.
    c *= 1.0 + 0.08 * seen(0.03, w.pw) * (noise(vec2f(along / 0.015, across / 0.5)) - 0.5) + 0.12 * seen(0.02, w.pw) * (noise(p / 0.009) - 0.5);
    let joint = max(band((fract(along / 5.0 + 0.5) - 0.5) * 5.0, -0.008, 0.008, w.aw), select(0.0, band(across, -0.008, 0.008, w.cw), wide));
    c = mix(c, vec3f(0.04), 0.85 * joint);
    // A crack across a corner of some slabs.
    let local = vec2f(fract(along / 5.0) * 5.0, abs(across));
    let diag = abs(local.x * 0.6 + local.y - 1.2 - 2.0 * hash(slab + 4.0)) * 0.8;
    c *= 1.0 - 0.5 * step(hash(slab + 8.0), 0.3) * band(diag + 0.05 * (noise(p / 0.2) - 0.5), -0.005, 0.005, w.cw + w.aw) * seen(0.05, w.pw);
    c *= 1.0 - 0.15 * smoothstep(0.5, 0.0, w.half - abs(across));
    out.albedo = c;
    if (track && !wide) {
        // 0.55 m wide, their middles 0.75 m out.
        out.albedo = mix(asphalt(code, region, p, w.pw), out.albedo * (0.95 + 0.1 * smoothstep(0.2, 0.0, abs(abs(across) - 0.75))), band(abs(across), 0.48, 1.03, w.cw));
    }
    return out;
}

// Setts: cubes about 11 cm in rows across the way, each its own tone and domed, in sandy joints.
fn sett_look(region: u32, w: WayPixel) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let s = seen(0.1, w.pw);
    let stone = rule(region, slot_setts()).rgb;
    let row = floor(w.along / 0.115);
    let q = vec2f(w.across / 0.11 + 0.5 * (row % 2.0) + 0.3 * hash(vec2f(row, 1.0)), fract(w.along / 0.115));
    let cell = vec2f(floor(q.x), row);
    let local = vec2f(fract(q.x), q.y) - 0.5 + 0.06 * (vec2f(hash(cell + 3.0), hash(cell + 7.0)) - 0.5);
    let h = hash(cell);
    let tone = stone * mix(vec3f(1.11, 1.115, 1.12), mix(vec3f(1.22, 1.155, 1.14), vec3f(0.925, 0.98, 1.06), step(0.5, hash(cell + 11.0))), step(0.6, h));
    let joint = smoothstep(0.5, 0.36, max(abs(local.x), abs(local.y)));
    out.albedo = mix(stone, mix(vec3f(0.24, 0.21, 0.17), tone * (0.85 + 0.3 * hash(cell + 5.0)), joint), s);
    out.bump = s * joint * 0.9 * local;
    return out;
}

// Pavers: concrete blocks 22 by 11 cm in a running bond, grey with a warm tint, bevelled.
fn pavers_look(region: u32, w: WayPixel) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    let base = rule(region, slot_roads() + i32(PAVING_CODE) - 1).rgb;
    let s = seen(0.06, w.pw);
    let row = floor(w.along / 0.11);
    let q = vec2f(w.across / 0.22 + 0.5 * (row % 2.0), fract(w.along / 0.11));
    let cell = vec2f(floor(q.x), row);
    let local = vec2f(fract(q.x) - 0.5, q.y - 0.5);
    let edge = min(0.5 - abs(local.x), (0.5 - abs(local.y)) * 0.5);
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
fn gravel_look(region: u32, w: WayPixel, flags: u32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    let x = abs(w.across);
    let track = w.half > 1.3;
    // A few tufts in the middle of a track, never a patch: a patch of shaded meadow reads as a hole.
    let middle = select(0.0, smoothstep(0.3, 0.1, x + 0.1 * (noise(p / 0.3) - 0.5)) * smoothstep(0.45, 0.7, noise(vec2f(w.along / 20.0, 6.0))), track);
    let edge = select(1.0 - smoothstep(0.0, 0.3, frayed(p, w.half - x, 0.35)), 0.0, opens(w, flags));
    out.ground = max(edge, 0.35 * middle * smoothstep(0.55, 0.75, noise(p / 0.07)));
    if (out.ground >= 1.0) {
        return out;
    }
    let base = surface_colour(region, CODE_GRAVEL);
    let wheels = select(smoothstep(w.half * 0.7, 0.0, x), smoothstep(0.32, 0.1, abs(x - 0.78 + 0.05 * (noise(p / 9.0 + 2.0) - 0.5))), track);
    let loose = 1.0 - wheels;
    let peb = pebbles(p, mix(0.022, 0.04, loose), w.pw);
    let big = pebbles(p + 17.0, 0.09, w.pw);
    var c = base * (0.9 + 0.12 * noise(p / 2.5) + 0.08 * noise(p / 11.0));
    c *= 1.0 + mix(0.35, 0.7, loose) * peb.x;
    c *= mix(mix(0.82, 0.62, loose), 1.0, peb.y);
    // A scatter of bigger stones off the wheel tracks.
    c = mix(c, base * (0.75 + 0.5 * big.z), loose * step(0.82, big.z) * big.y * seen(0.09, w.pw));
    // Compacted: finer, darker and warmer.
    c = mix(c, base * vec3f(0.86, 0.83, 0.78) * (0.96 + 0.08 * noise(p / 0.3)), 0.5 * wheels);
    out.albedo = c;
    out.bump = loose * 0.35 * (big.y - 1.0) * (vec2f(noise(p / 0.05), noise(p / 0.05 + 9.0)) - 0.5);
    return out;
}

// Dirt and grass tracks: two worn ruts of soil, grass in the middle and beside them, mud in some stretches; on
// a grass track the ruts are grassed over in part.
fn dirt_look(code: u32, region: u32, w: WayPixel) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    let x = abs(w.across);
    let grassy = code == CODE_GRASS;
    let wob = 0.08 * (noise(p / 7.0 + sign(w.across) * 3.0) - 0.5);
    let rut_w = select(0.25, 0.19, grassy) + 0.06 * noise(p / 3.0 + sign(w.across));
    let d = abs(x - 0.75 + wob);
    let rut = 1.0 - smoothstep(rut_w - 0.04, rut_w + 0.06, d + 0.05 * (noise(p / 0.2) - 0.5));
    // Grass covers what is not rut; on a grass track the ruts too, in streaks along them.
    let grassed = select(0.0, 0.7 * smoothstep(0.58, 0.8, noise(vec2f(w.along / 2.5, w.across / 0.3) + 5.0)), grassy);
    // Worn grass beside the ruts: soil shows through.
    let worn = smoothstep(rut_w + 0.3, rut_w, d) * 0.35;
    out.ground = max(max(1.0 - max(rut, worn), grassed), 1.0 - smoothstep(0.0, 0.25, frayed(p, w.half - x, 0.3)));
    if (out.ground >= 1.0) {
        return out;
    }
    let soil = surface_colour(region, CODE_DIRT);
    // The bottom of the rut darker and moister; mud where the ruts hold water.
    let wet = smoothstep(0.62, 0.78, noise(vec2f(w.along / 6.0, 7.0) + p * 0.05)) * select(1.0, 0.4, grassy);
    let deep = smoothstep(rut_w, 0.0, d);
    var c = soil * (0.88 + 0.2 * noise(p / 0.6) + 0.1 * noise(p / 3.0)) * (1.0 - 0.15 * deep);
    let peb = pebbles(p, 0.03, w.pw);
    c *= (1.0 + 0.4 * peb.x * (1.0 - wet)) * mix(0.85, 1.0, peb.y);
    c = mix(c, soil * vec3f(0.45, 0.42, 0.4), wet * deep);
    out.albedo = c;
    out.spec = 0.5 * wet * deep;
    // Puddles where the ruts are wettest.
    out.mirror = smoothstep(0.55, 0.75, wet * deep * (0.7 + 0.6 * noise(p / 0.8)));
    out.bump = vec2f(-sign(w.across) * sign(x - 0.75 + wob) * 0.25 * rut * (1.0 - deep), 0.0);
    return out;
}

// A trodden trail: a tread of soil narrower than the way, wandering across it, with stones in it and rocks set
// in it; a trampled margin of grass; on steep or high ground stonier and greyer.
fn trail_look(region: u32, su: vec2f, w: WayPixel, slope: f32, altitude: f32) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    let rocky = clamp(smoothstep(0.92, 0.7, slope) + smoothstep(1700.0, 2300.0, altitude), 0.0, 1.0);
    // The tread keeps within TREAD_CLEAR_M of the middle, where kept_off of features.wgsl lets plants grow.
    let room = min(max(w.half - 0.35, 0.05), 0.65);
    let centre = room * 0.45 * (noise(p / 13.0 + 1.0) - 0.5) + 0.08 * (noise(p / 4.0 + 2.0) - 0.5);
    let tread = min(0.23 + 0.16 * noise(p / 5.0 + 3.0) + 0.1 * rocky, w.half - 0.05);
    let d = frayed(p, tread - abs(w.across - centre), 0.22);
    // A trampled margin: grass with soil showing through.
    let solid = smoothstep(-0.03, 0.05, d);
    out.ground = clamp(1.0 - solid - 0.35 * smoothstep(-0.3, -0.03, d) * (1.0 - solid), 0.0, 1.0);
    if (out.ground >= 1.0) {
        return out;
    }
    let cell = cell_at(su, u32(ceil(lod(su))));
    let rock = rule(cell.b, slot_lithologies() + i32(cell.g)).rgb;
    let soil = mix(mix(surface_colour(region, CODE_DIRT), surface_colour(region, CODE_PATH), 0.4), rock * 0.75, 0.85 * rocky);
    // Soil, darker where feet dig in, with grit and small stones.
    let peb = pebbles(p, 0.03, w.pw);
    var c = soil * (0.85 + 0.25 * noise(p / 0.5) + 0.1 * noise(p / 2.0)) * (1.0 - 0.12 * smoothstep(0.15, 0.0, abs(w.across - centre)));
    c *= (1.0 + 0.5 * peb.x) * mix(0.8, 1.0, peb.y);
    // Rocks set in the tread, more on rocky ground: domed, in the colour of the rock under it.
    let r = voronoi(p, mix(0.12, 0.17, rocky));
    let set_in = step(1.0 - mix(0.08, 0.25, rocky), r.x) * smoothstep(0.05, 0.16, r.y) * seen(0.2, w.pw);
    c = mix(c, rock * (0.95 + 0.3 * r.z) * mix(0.8, 1.08, smoothstep(0.05, 0.4, r.y)), set_in);
    out.albedo = c;
    out.bump = set_in * 1.2 * (vec2f(hash(floor(p / 0.2) + 1.0), r.z) - 0.5);
    return out;
}

// A track's bed: crushed stone, rusty between and beside the rails, darker in the middle; on its slopes and past
// them dirtier, fraying into the ground; with its sleepers and rails.
fn ballast_look(code: u32, w: WayPixel) -> WayLook {
    var out = WayLook(vec3f(0.0), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    let narrow = ((code - RAIL_CODE) & 1u) != 0u;
    let gauge = select(0.7525, 0.53, narrow);
    let x = abs(w.across);
    // Crushed stone: two sizes over each other, each stone its own tone, shallow gaps.
    let peb = pebbles(p, 0.032, w.pw);
    let big = pebbles(p * 1.13 + 7.0, 0.055, w.pw);
    let tone = 1.0 + 0.5 * peb.x + 0.35 * big.x;
    var c = BALLAST * vec3f(1.0 + 0.1 * big.x, 1.0, 1.0 - 0.1 * big.x) * (0.9 + 0.15 * noise(p / 2.0)) * tone * mix(0.72, 1.0, min(peb.y, mix(1.0, big.y, 0.6)));
    // Rust from the rails and brake dust.
    c = mix(c, c * vec3f(1.25, 0.9, 0.7), 0.6 * smoothstep(0.4, 0.0, abs(x - gauge)));
    c *= 1.0 - 0.25 * smoothstep(0.35, 0.0, x);
    c = mix(c, c * vec3f(0.9, 0.85, 0.78), smoothstep(w.half - 0.45, w.half, x));
    // Each stone a facet of its own slope: crushed, not rounded.
    let facet = vec2f(hash(floor(p / 0.038) + 3.0), peb.z) - 0.5;
    out.bump = 0.9 * seen(0.038, w.pw) * facet;
    // Past the bed: the formation's dirt, weeds and the ground.
    out.ground = smoothstep(0.0, 0.35, frayed(p, x - w.half, 0.25));
    if (x < w.half - 0.3) {
        // Sleepers 0.26 m wide, rail heads 6 cm, rusty sides, a rack between.
        let reach = select(1.3, 0.9, narrow);
        let off = (fract(w.along + 0.5) - 0.5) * SLEEPER_SPACING_M;
        let along_m = w.aw * SLEEPER_SPACING_M;
        let lying = mix(band(off, -0.13, 0.13, along_m), 0.26 / SLEEPER_SPACING_M, smoothstep(0.1, 0.4, along_m));
        c = mix(c, vec3f(0.2, 0.195, 0.185), lying * band(w.across, -reach, reach, w.cw));
        c = mix(c, vec3f(0.13, 0.075, 0.045), band(x - gauge, -0.05, 0.05, w.cw));
        c = mix(c, vec3f(0.42, 0.43, 0.44), band(x - gauge, -0.031, 0.031, w.cw));
        if (code >= RAIL_CODE + 2u) {
            c = mix(c, vec3f(0.1), band(w.across, -0.08, 0.08, w.cw));
        }
    }
    out.albedo = c;
    return out;
}

// The look of a face of material `mat`; `slope` is its normal's z and `altitude` its height.
fn way_look(mat: u32, code: u32, region: u32, su: vec2f, w: WayPixel, flags: u32, slope: f32, altitude: f32) -> WayLook {
    if (mat >= DETAIL_MATS) {
        return detail_look(mat, su, w, altitude);
    }
    switch mat {
        case MAT_KERB: {
            return WayLook(rule(region, slot_kerb()).rgb * (0.94 + 0.12 * noise(w.p / 0.9)), vec2f(0.0), 0.0, 0.0, 0.0);
        }
        case MAT_SIDEWALK: {
            return WayLook(paving(region, w.p, w.pw), vec2f(0.0), 0.0, 0.0, 0.0);
        }
        case LOOK_CONCRETE, LOOK_STRIPS: {
            return concrete_look(code, region, w, mat == LOOK_STRIPS);
        }
        case LOOK_SETT: {
            return sett_look(region, w);
        }
        case LOOK_PAVERS: {
            return pavers_look(region, w);
        }
        case LOOK_GRAVEL: {
            return gravel_look(region, w, flags);
        }
        case LOOK_DIRT: {
            return dirt_look(code, region, w);
        }
        case LOOK_TRAIL: {
            return trail_look(region, su, w, slope, altitude);
        }
        case LOOK_BALLAST: {
            return ballast_look(code, w);
        }
        default: {
            return paved_look(mat, code, region, w, flags);
        }
    }
}

// A look lit, in the haze: `n` the face's normal and `relative` the point about the eye.
fn way_lit(l: WayLook, n: vec3f, su: vec2f, relative: vec3f, altitude: f32) -> vec3f {
    var nn = n;
    if (dot(l.bump, l.bump) > 0.0) {
        nn = normalize(n + vec3f(l.bump, 0.0));
    }
    let share = sun_share(sun_at(su), relative, nn, LIFT_SOLID);
    var lit = light_up(l.albedo, nn, share, 1.0, look.horizon.w);
    if (l.spec > 0.0) {
        let r = reflect(normalize(relative), nn);
        lit += look.sun_colour.rgb * l.spec * share * pow(max(dot(r, look.sun.xyz), 0.0), 40.0) * 0.5;
        lit = mix(lit, sky(r), 0.12 * min(l.spec, 1.0) * (1.0 - max(r.z, 0.0) * 0.5));
    }
    if (l.mirror > 0.0) {
        let r = reflect(normalize(relative), vec3f(0.0, 0.0, 1.0));
        lit = mix(lit, 0.6 * sky(r) * (0.6 + 0.4 * share) + look.sun_colour.rgb * share * pow(max(dot(r, look.sun.xyz), 0.0), 200.0), l.mirror * 0.8);
    }
    return atmosphere(lit, relative, altitude);
}
