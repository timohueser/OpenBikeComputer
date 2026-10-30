// PROTOTYPE road system (a study; never merges). The roadside details of roads/details.ts: the vertex stage
// decides from the stamped ground where a guardrail, a delineator or a retaining wall stands, and the
// fragment stage paints them. corridor.wgsl includes it.
// #include hole.wgsl
// #include ../ground/look.wgsl

// Materials, as DM of details.ts.
const DM_RAIL: u32 = 40u;
const DM_RAIL_POST: u32 = 41u;
const DM_RAIL_BLOCK: u32 = 42u;
const DM_DELI: u32 = 44u;
const DM_DELI_BAND: u32 = 45u;
const DM_REFLECTOR: u32 = 46u;
const DM_WALL: u32 = 48u;
const DM_CAP: u32 = 49u;
const DM_KERB: u32 = 50u;
const DM_EDGING: u32 = 51u;
// As CF.BARRIER of section.ts.
const DETAIL_BARRIER: u32 = 4u;
// Every vertex of one station of a detail decides alike: each holds the station's point on E, and its own place
// is an offset from that point (detail_offset), so all sample the same ground and get the same share.
// A guardrail stands where the road edge lies DROP_M over the ground DROP_OUT_M past E, and RAIL_DILATE_M along
// past that; then it ramps down over RAIL_RAMP_M, as a terminal does. Its samples lie RAIL_STEP_M apart.
const DROP_M: f32 = 1.35;
const DROP_OUT_M: f32 = 4.0;
const RAIL_STEP_M: f32 = 2.5;
const RAIL_DILATE_M: f32 = 4.0;
const RAIL_RAMP_M: f32 = 6.0;
// A wall stands where the ground WALL_OUT_M past E climbs CUT_M over the road edge, and WALL_DILATE_M along past
// that; then it ramps down over WALL_RAMP_M. Its height is the cut's mean over the same samples, so its top runs
// with the road's grade and not with every bump of the cut.
const CUT_M: f32 = 1.55;
const WALL_OUT_M: f32 = 2.2;
const WALL_STEP_M: f32 = 3.0;
const WALL_DILATE_M: f32 = 5.0;
const WALL_RAMP_M: f32 = 7.0;
const WALL_OVER_M: f32 = 0.3;
const WALL_MIN_M: f32 = 0.6;
const WALL_MAX_M: f32 = 3.5;
// The backfill behind a wall slopes from its top down to this far under the cut WALL_OUT_M past E.
const FILL_UNDER_M: f32 = 0.25;
// Where each detail meets the ground, past E: a rail's posts, a delineator, the far end of a wall's backfill
// (the lowest ground under a wall on a fill).
const RAIL_GROUND_M: f32 = 0.62;
const DELI_GROUND_M: f32 = 0.55;
const WALL_GROUND_M: f32 = 2.5;
// The tops of a rail's beam and posts over the road edge and of a delineator over the ground, as details.ts.
const RAIL_TOP_M: f32 = 0.75;
const POST_TOP_M: f32 = 0.71;
const DELI_TOP_M: f32 = 1.0;
// A detail that does not stand sinks whole, until its top lies this far under the lower of the ground and the road edge.
const DETAIL_SINK_M: f32 = 0.6;
// A vertex's parameter byte holds its station's metres along to the nearest junction mouth or town, in steps of
// ZONE_STEP_M up to ZONE_FAR_M (`zones` of details.ts). A detail ramps down over ZONE_RAMP_M before one, and
// reads no ground that far along, so a cut or a drop in a mouth raises nothing beside it.
const ZONE_STEP_M: f32 = 0.05;
const ZONE_FAR_M: f32 = 12.75;
const ZONE_RAMP_M: f32 = 5.0;
// A detail vertex this far outside the sides of the view (or of a shadow map) skips the ground's samples: no face
// of it reaches the view, however it stands, as a face spans 3.2 m and a detail rises 4.2 m at most.
const DETAIL_UNSEEN_M: f32 = 10.0;

// Metres per world unit at world row y.
fn detail_unit_m(y: i32) -> f32 {
    return 2.0 * 3.14159265 * 6371000.0 / 1073741824.0 / cosh(3.14159265 * (1.0 - f32(y) / 536870912.0));
}

struct DetailFrame {
    // World units per metre outward and along.
    o: vec2f,
    t: vec2f,
    grade: f32,
    // The vertex's metres along from its station's point: a side of a post or a delineator.
    along: f32,
}

// The frame that details.ts packs: the outward angle (10 bits), the grade (10 bits), centimetres along (4 bits).
fn detail_frame(at: vec2i, packed: f32) -> DetailFrame {
    let p = u32(max(packed, 0.0) + 0.5);
    let angle = f32(p >> 14u) / 1024.0 * 6.2831853 - 3.14159265;
    let m = detail_unit_m(at.y);
    var f: DetailFrame;
    f.o = vec2f(cos(angle), sin(angle)) / m;
    f.t = vec2f(-f.o.y, f.o.x);
    f.grade = (f32((p >> 4u) & 1023u) - 512.0) / 1024.0;
    f.along = (f32(p & 15u) - 8.0) * 0.01;
    return f;
}

fn is_barrier(colour: vec4f) -> bool {
    return (u32(round(colour.a * 255.0)) & 12u) == 12u && (u32(round(colour.b * 255.0)) & DETAIL_BARRIER) != 0u;
}

// The place of a detail vertex about its station's point `at` (whole world units), in world units: `road.x`
// metres past E and its frame's metres along.
fn detail_offset(at: vec2i, colour: vec4f, road: vec2f, packed: f32) -> vec2f {
    if (!is_barrier(colour)) {
        return vec2f(0.0);
    }
    let f = detail_frame(at, packed);
    return f.o * road.x + f.t * f.along;
}

// From a station: metres along to the nearest point where the ground `out` metres past E lies `level` metres or
// more under (dir 1) or over (dir -1) the road edge's grade line, within 4 steps either way and short of a zone
// `zone` metres along (1e9 if none); the weighted mean of that depth (or rise), clamped to 0..`top`; and the
// ground at the station (1e9 if unknown). A crossing between two samples is interpolated, so the distance runs on
// smoothly as the samples move with the station.
fn detail_reach(at: vec2i, f: DetailFrame, edge: f32, out: f32, dir: f32, level: f32, step: f32, top: f32, zone: f32) -> vec3f {
    var d = 1e9;
    var last = -1.0;
    var sum = 0.0;
    var weights = 0.0;
    var here = 1e9;
    for (var k = -4; k <= 4; k++) {
        let x = f32(k) * step;
        let g = units_height(at + vec2i(round(f.o * out + f.t * x)), surface.finest);
        var v = -1.0;
        if (known(g) && (abs(x) < zone || zone > ZONE_FAR_M - ZONE_STEP_M)) {
            let r = dir * (edge + f.grade * x - g);
            v = r - level;
            let w = 5.0 - abs(f32(k));
            sum += w * clamp(r, 0.0, top);
            weights += w;
            if (k == 0) {
                here = g;
            }
        }
        if (v > 0.0) {
            d = min(d, abs(x));
        }
        if (k > -4 && (v > 0.0) != (last > 0.0)) {
            d = min(d, abs(x - step * v / (v - last)));
        }
        last = v;
    }
    return vec3f(d, sum / max(weights, 1e-6), here);
}

// How far a detail stands, 0..1, `d` metres along from where it must: whole within `dilate`, eased down over
// `ramp`, and down over ZONE_RAMP_M before a zone `zone` metres along. Never in part over a long run: a share
// under 1 is a ramp.
fn detail_share(d: f32, dilate: f32, ramp: f32, zone: f32) -> f32 {
    return smoothstep(0.0, 1.0, min(clamp((dilate + ramp - d) / ramp, 0.0, 1.0), zone / ZONE_RAMP_M));
}

fn rail_share(at: vec2i, f: DetailFrame, edge: f32, zone: f32) -> f32 {
    if (zone <= 0.0) {
        return 0.0;
    }
    return detail_share(detail_reach(at, f, edge, DROP_OUT_M, 1.0, DROP_M, RAIL_STEP_M, 1.0, zone).x, RAIL_DILATE_M, RAIL_RAMP_M, zone);
}

// How far a wall stands, its height over the road edge, and the ground WALL_OUT_M past E (1e9 if unknown).
fn wall_share(at: vec2i, f: DetailFrame, edge: f32, zone: f32) -> vec3f {
    if (zone <= 0.0) {
        return vec3f(0.0, WALL_MIN_M, 1e9);
    }
    let w = detail_reach(at, f, edge, WALL_OUT_M, -1.0, CUT_M, WALL_STEP_M, WALL_MAX_M, zone);
    return vec3f(detail_share(w.x, WALL_DILATE_M, WALL_RAMP_M, zone), clamp(w.y + WALL_OVER_M, WALL_MIN_M, WALL_MAX_M), w.z);
}

// Whether render-space point `p` lies more than DETAIL_UNSEEN_M outside a side of the view `view_proj`.
fn detail_unseen(view_proj: mat4x4f, p: vec3f) -> bool {
    let q = vec4f(p, 1.0);
    let w = vec4f(view_proj[0].w, view_proj[1].w, view_proj[2].w, view_proj[3].w);
    let x = vec4f(view_proj[0].x, view_proj[1].x, view_proj[2].x, view_proj[3].x);
    let y = vec4f(view_proj[0].y, view_proj[1].y, view_proj[2].y, view_proj[3].y);
    let planes = array<vec4f, 4>(w + x, w - x, w + y, w - y);
    var out = -1e9;
    for (var i = 0; i < 4; i++) {
        out = max(out, -dot(planes[i], q) / max(length(planes[i].xyz), 1e-9));
    }
    return out > DETAIL_UNSEEN_M;
}

// The height of a detail vertex (CF.BARRIER) of the station at `at` (whole world units) with its road edge
// `edge`, its metres past E and height code (`road`), its packed frame (`packed`) and its colour; others keep
// `z`. A detail stands or sinks whole: each vertex of a station sinks by the same share of the same depth.
// `view_proj` and `eye` (metres above sea level) are the frame's.
fn detail_height(at: vec2i, edge: f32, z: f32, colour: vec4f, road: vec2f, packed: f32, s: vec2f, view_proj: mat4x4f, eye: f32) -> f32 {
    if (!is_barrier(colour)) {
        return z;
    }
    if (detail_unseen(view_proj, vec3f(to_render(s), edge - eye))) {
        return edge - DETAIL_SINK_M;
    }
    let mat = u32(round(colour.r * 255.0));
    let zone = colour.g * 255.0 * ZONE_STEP_M;
    let f = detail_frame(at, packed);
    let code = road.y;
    let is_wall = mat == DM_WALL || mat == DM_CAP || mat == 1u;
    let is_deli = mat == DM_DELI || mat == DM_DELI_BAND || mat == DM_REFLECTOR;
    let middle = select(select(RAIL_GROUND_M, DELI_GROUND_M, is_deli), WALL_GROUND_M, is_wall);
    let g = units_height(at + vec2i(round(f.o * middle)), surface.finest);
    let ground = select(edge, g, known(g));
    let low = min(ground, edge) - DETAIL_SINK_M;
    var h = z;
    if (is_wall) {
        let w = wall_share(at, f, edge, zone);
        let top = mix(low, edge + w.y, w.x);
        if (code > 4.0) {
            h = top + code - 6.0;
            if (road.x > 1.0) {
                // The backfill's far edge, under the cut where that lies under the wall's top: under the drawn
                // mesh, which strays from the stamped ground between its vertices.
                h = min(h, w.z - FILL_UNDER_M);
            }
        } else if (code < -5.0) {
            // The backfill dives under the ground, or under the wall's top where the ground lies lower.
            h = min(ground, top) + code + 10.0;
        } else {
            h = min(edge + code, top - 0.3);
        }
    } else if (is_deli) {
        // A delineator stands where neither a rail nor a whole wall does: a cut that only nears a wall's keeps it.
        let share = (1.0 - rail_share(at, f, edge, zone)) * (1.0 - smoothstep(0.6, 0.95, wall_share(at, f, edge, ZONE_FAR_M).x));
        h = ground + code + 10.0 - (1.0 - share) * (DELI_TOP_M + ground - low);
    } else {
        let sink = (1.0 - rail_share(at, f, edge, zone)) * (edge + RAIL_TOP_M - low);
        h = edge + code - sink;
        if (code < -5.0) {
            // A post's foot, in the ground and under its top.
            h = min(ground + code + 10.0, edge + POST_TOP_M - sink - 0.1);
        }
    }
    if (!corridor_drawn(s)) {
        h -= 4.0;
    }
    return h;
}

// The stone of rubble masonry at q (cell units): x its hash, y the gap to its edge (cell units), z the
// height of the point above the stone's middle over the stone's reach (-1 at its foot, 1 at its top).
fn detail_stone(q: vec2f) -> vec3f {
    let i = floor(q);
    let f = q - i;
    var d1 = 8.0;
    var d2 = 8.0;
    var id = i;
    var to = vec2f(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2f(f32(x), f32(y));
            let r = o + vec2f(hash(i + o), hash(i + o + 17.0)) * 0.8 + 0.1 - f;
            let d = length(r);
            if (d < d1) {
                d2 = d1;
                d1 = d;
                id = i + o;
                to = r;
            } else if (d < d2) {
                d2 = d;
            }
        }
    }
    return vec3f(hash(id + 3.7), 0.5 * (d2 - d1), clamp(-to.y / 0.5, -1.0, 1.0));
}

// Rubble masonry: stones about `size` metres, laid flat (wider than high), each its own tone between a
// cool and a warm grey, rounded (lit from above, shaded at their feet), in dark recessed joints; on a
// face, mapped onto the vertical plane it is nearest. Where a pixel spans a stone, they fade to their mean.
fn detail_masonry(p: vec3f, n: vec3f, pixel: f32, size: f32) -> vec3f {
    let q = select(vec2f(0.75 * select(p.x, p.y, abs(n.x) > abs(n.y)), p.z * 1.6), p.xy, abs(n.z) > 0.7) / size;
    let c = detail_stone(q);
    let h2 = hash(vec2f(c.x * 91.0, 7.0));
    let tone = mix(vec3f(0.35, 0.345, 0.335), vec3f(0.45, 0.42, 0.37), h2) * (0.8 + 0.4 * c.x);
    let rust = mix(vec3f(1.0), vec3f(1.1, 0.94, 0.78), step(0.88, fract(c.x * 7.0)));
    let round = 1.0 + 0.22 * c.z * smoothstep(0.0, 0.35, 1.0 - c.y * 2.0);
    let joint = mix(0.3, 1.0, smoothstep(0.02, 0.09, c.y));
    let stone = tone * rust * round * joint * (0.92 + 0.16 * noise(q * 6.0));
    return mix(stone, vec3f(0.33, 0.32, 0.30), smoothstep(0.25, 0.7, pixel / size));
}

// The colour of a detail fragment: material `mat`, `su` its LOD space, `relative` about the eye, `along`
// its packed frame (metres along for a kerb), `n` its normal.
fn detail_fragment(mat: u32, su: vec2f, relative: vec3f, altitude: f32, along: f32, n: vec3f) -> vec4f {
    let p = look.place.xy + relative.xy;
    let w = max(length(dpdx(relative)), length(dpdy(relative)));
    let region = region_at(su);
    var albedo = vec3f(0.5);
    var spec = 0.0;
    var glow = vec3f(0.0);
    switch mat {
        case DM_RAIL, DM_RAIL_POST, DM_RAIL_BLOCK: {
            // Galvanised steel: pale grey, mottled with spangle and dull patches.
            let mottle = 0.86 + 0.1 * noise(p / 0.6) + 0.06 * noise(p / 0.07);
            albedo = vec3f(0.34, 0.355, 0.365) * mottle;
            if (mat != DM_RAIL) {
                albedo *= 0.8;
            }
            spec = 0.35;
        }
        case DM_DELI: {
            albedo = vec3f(0.78, 0.78, 0.76);
            spec = 0.1;
        }
        case DM_DELI_BAND: {
            albedo = vec3f(0.025);
            spec = 0.1;
        }
        case DM_REFLECTOR: {
            albedo = vec3f(0.9, 0.42, 0.05);
            glow = vec3f(0.35, 0.14, 0.01);
        }
        case DM_WALL, DM_CAP: {
            // The local rock's brightness and a hint of its hue.
            let cell = cell_at(su, u32(ceil(lod(su))));
            let rock = clamp(rule(cell.b, slot_lithologies() + i32(cell.g)).rgb / vec3f(0.3101, 0.3189, 0.2929), vec3f(0.6), vec3f(1.4));
            let tint = mix(vec3f(dot(rock, vec3f(0.333))), rock, 0.35);
            let q = vec3f(p, altitude);
            if (mat == DM_WALL) {
                albedo = tint * detail_masonry(q, n, w, 0.2);
            } else {
                albedo = tint * vec3f(0.5, 0.49, 0.46) * (0.9 + 0.1 * noise(p / 0.3));
            }
        }
        case DM_KERB: {
            // Granite blocks a metre long in fine joints, speckled.
            let kerb = rule(region, slot_kerb()).rgb;
            let block = floor(along);
            let joint = (0.5 - abs(fract(along) - 0.5));
            let along_w = max(abs(dpdx(along)), abs(dpdy(along)));
            albedo = kerb * (0.9 + 0.08 * hash(vec2f(block, 3.0)) + 0.08 * noise(p / 0.05));
            albedo *= 1.0 - 0.45 * band(joint, -0.006, 0.006, max(along_w, 1e-4));
            spec = 0.05;
        }
        case DM_EDGING: {
            albedo = vec3f(0.36, 0.355, 0.34) * (0.92 + 0.12 * noise(p / 0.4));
        }
        default: {}
    }
    let shadow = sun_share(sun_at(su), relative, n, LIFT_SOLID);
    var lit = light_up(albedo, n, shadow, 1.0, look.horizon.w);
    if (spec > 0.0) {
        let v = -normalize(relative);
        let hv = normalize(v + look.sun.xyz);
        lit += look.sun_colour.rgb * spec * shadow * pow(max(dot(n, hv), 0.0), 24.0) * step(0.0, dot(n, look.sun.xyz));
        // Sheen of the sky on steel, by the angle of view.
        lit += look.ambient.rgb * spec * 0.4 * pow(1.0 - max(dot(n, v), 0.0), 3.0);
    }
    lit += glow;
    return vec4f(atmosphere(lit, relative, altitude), 1.0);
}
