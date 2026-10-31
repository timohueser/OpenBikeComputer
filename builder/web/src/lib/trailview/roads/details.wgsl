// The looks of what stands beside a road (details.ts): galvanised rails on posts, delineators and masonry walls.
// corridor.wgsl includes it, and looks.wgsl's way_look calls it.
// #include ../ground/look.wgsl

// As MAT of section.ts.
const MAT_RAIL: u32 = 40u;
const MAT_RAIL_POST: u32 = 41u;
const MAT_RAIL_BLOCK: u32 = 42u;
const MAT_DELI: u32 = 44u;
const MAT_DELI_BAND: u32 = 45u;
const MAT_REFLECTOR: u32 = 46u;
const MAT_WALL: u32 = 48u;
const MAT_CAP: u32 = 49u;
// A stone of a wall: its cell along and up. Stones lie flat.
const STONE_LONG_M: f32 = 0.267;
const STONE_HIGH_M: f32 = 0.125;
// A course of two stones' height: seen along the wall, stones merge along it long before their courses do.
const COURSE_M: f32 = 0.25;
// The share of a course that a pixel spans where it starts to fade, and where it has gone: it holds until it spans
// 1.5 pixels. Single stones flicker sooner, measured on a walk beside the wall, so they go by 2.2 pixels.
const FADE_FROM: f32 = 0.4;
const FADE_TO: f32 = 0.67;
const STONE_FROM: f32 = 0.25;
const STONE_TO: f32 = 0.45;
// The mean colour of the stones, and the size of the patches of tone that stay past the courses.
const RUBBLE_MEAN: vec3f = vec3f(0.33, 0.32, 0.30);
const RUBBLE_PATCH: vec2f = vec2f(5.0, 1.3);

// The stone of rubble masonry at q (cell units): x its hash, y the gap to its edge (cell units), z the height of the
// point above the stone's middle over the stone's reach (-1 at its foot, 1 at its top).
fn rubble_stone(q: vec2f) -> vec3f {
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

// Stones each of its own tone between a cool and a warm grey, rounded (lit from above, shaded at their feet), in dark
// recessed joints, which widen with `pixel`, the larger cells that a pixel spans: a thin joint would flicker.
fn rubble_stones(q: vec2f, pixel: f32) -> vec3f {
    let c = rubble_stone(q);
    let tone = mix(vec3f(0.35, 0.345, 0.335), vec3f(0.45, 0.42, 0.37), hash(vec2f(c.x * 91.0, 7.0))) * (0.8 + 0.4 * c.x);
    let rust = mix(vec3f(1.0), vec3f(1.1, 0.94, 0.78), step(0.88, fract(c.x * 7.0)));
    let round = 1.0 + 0.22 * c.z * smoothstep(0.0, 0.35, 1.0 - c.y * 2.0);
    let joint = mix(0.3, 1.0, smoothstep(0.02 - pixel, 0.09 + pixel, c.y));
    return tone * rust * round * joint * (0.92 + 0.16 * noise(q * 6.0));
}

// The tone of a run of stones in a course, a share about 1. A run is as long as two pixels at least, and its length
// doubles by whole steps, so what a pixel shows holds still as the eye moves.
fn course_tone(course: f32, along: f32, aw: f32) -> f32 {
    let steps = log2(max(2.0 * aw / STONE_LONG_M, 1.0));
    let step_down = floor(steps);
    let run = STONE_LONG_M * pow(2.0, step_down);
    let shift = 17.0 * hash(vec2f(course, 3.0));
    let fine = hash(vec2f(floor(along / run + shift), course + 11.0 * step_down));
    let coarse = hash(vec2f(floor(along / (2.0 * run) + shift), course + 11.0 * step_down + 11.0));
    return 0.8 + 0.4 * mix(fine, coarse, steps - step_down);
}

// Courses seen by a pixel `aw` metres long along the wall and `vw` metres up it: their runs of tone, and their dark bed
// joints, whose share of a pixel is its filtered cover.
fn rubble_courses(along: f32, altitude: f32, aw: f32, vw: f32) -> f32 {
    let y = altitude / COURSE_M;
    let course = floor(y);
    let part = y - course;
    let near = select(course - 1.0, course + 1.0, part > 0.5);
    let edge = min(part, 1.0 - part) * COURSE_M;
    let tone = mix(course_tone(course, along, aw), course_tone(near, along, aw), 0.5 * (1.0 - smoothstep(0.0, vw, edge)));
    return tone * (1.0 - 0.45 * band(edge, -0.02, 0.02, vw));
}

// Rubble masonry by a pixel's two reaches on the wall: a wall seen along its length compresses one way only, so what
// the height resolves stays. Stones while a stone spans about 2 pixels both ways, then courses while a course spans
// 1.5 pixels up, then the mean with its coarse tone.
fn rubble(along: f32, altitude: f32, aw: f32, vw: f32) -> vec3f {
    let mean = RUBBLE_MEAN * (0.9 + 0.2 * noise(vec2f(along, altitude) / RUBBLE_PATCH));
    let courses = 1.0 - smoothstep(FADE_FROM, FADE_TO, vw / COURSE_M);
    if (courses <= 0.0) {
        return mean;
    }
    let far = mix(mean, mean * rubble_courses(along, altitude, aw, vw), courses);
    let span = max(aw / STONE_LONG_M, vw / STONE_HIGH_M);
    let stones = 1.0 - smoothstep(STONE_FROM, STONE_TO, span);
    if (stones <= 0.0) {
        return far;
    }
    return mix(far, rubble_stones(vec2f(along / STONE_LONG_M, altitude / STONE_HIGH_M), 0.5 * span), stones);
}

// The look of a face of a detail, of material `mat`.
fn detail_look(mat: u32, su: vec2f, w: WayPixel, altitude: f32) -> WayLook {
    var out = WayLook(vec3f(0.5), vec2f(0.0), 0.0, 0.0, 0.0);
    let p = w.p;
    switch mat {
        case MAT_RAIL, MAT_RAIL_POST, MAT_RAIL_BLOCK: {
            // Galvanised steel: pale grey, mottled with spangle and dull patches.
            out.albedo = vec3f(0.34, 0.355, 0.365) * (0.86 + 0.1 * noise(p / 0.6) + 0.06 * noise(p / 0.07)) * select(0.8, 1.0, mat == MAT_RAIL);
            out.spec = 0.35;
        }
        case MAT_DELI: {
            out.albedo = vec3f(0.78, 0.78, 0.76);
            out.spec = 0.1;
        }
        case MAT_DELI_BAND: {
            out.albedo = vec3f(0.025);
            out.spec = 0.1;
        }
        case MAT_REFLECTOR: {
            out.albedo = vec3f(1.0, 0.47, 0.06);
        }
        default: {
            // The local rock's brightness and a hint of its hue.
            let cell = cell_at(su, u32(ceil(lod(su))));
            let rock = clamp(rule(cell.b, slot_lithologies() + i32(cell.g)).rgb / vec3f(0.3101, 0.3189, 0.2929), vec3f(0.6), vec3f(1.4));
            let tint = mix(vec3f(dot(rock, vec3f(0.333))), rock, 0.35);
            if (mat == MAT_WALL) {
                out.albedo = tint * rubble(w.along, altitude, w.aw, max(length(vec2f(dpdx(altitude), dpdy(altitude))), 1e-4));
            } else {
                out.albedo = tint * vec3f(0.5, 0.49, 0.46) * (0.9 + 0.1 * noise(p / 0.3));
            }
        }
    }
    return out;
}
