// Strewn rocks, as their instances (rocks_of of dressing.wgsl) and the ground (look.wgsl) share them.
// A rock stands at a point of a jittered grid of ROCK_SHIFT, kept by the density of the ground there.
// Past the inner share 1 − ROCK_HANDOFF of the reach the ground paints each rock's footprint under it, and
// each instance ends at its own distance in that band: the rock stays as its footprint, and no edge
// shows where the instances end.
// look.wgsl includes it, and it reads `rule()` of look.wgsl.
// #include ../ground/look.wgsl
// #include ../instances/hash.wgsl

// Units along a rock point's cell: about 3.3 m at 47° N.
const ROCK_SHIFT: u32 = 7u;
// Rock instances stand within ROCK_REACH_M of the eye, as ROCK_REACH_M of dressing.ts, and end over its
// outer ROCK_HANDOFF.
const ROCK_REACH_M: f32 = 1000.0;
const ROCK_HANDOFF: f32 = 0.45;
// The ground in square metres that a rock point stands for at the rules' share 0.55, at every latitude.
const ROCK_POINT_M2: f32 = 17.0;
// The slope (rise per metre) past which no rock lies.
const STEEPEST: f32 = 0.85;
// Rocks are 0.25 to 1.3 m tall, the share u² between. A rock h tall covers an ellipse of half-axes 2.8 h
// and 2.1 h as a slab and a circle of radius 1.1 h as a boulder (models/rocks.ts), 0.7 of them slabs:
// 6.3 m² on average. A rock sinks into a slope and into the coarser far ground, so the ground paints
// PAINTED of a footprint's size: as much as shows of the instances there, by eye.
const ROCK_HEIGHT_M: vec2f = vec2f(0.25, 1.3);
const SLAB_AXES: vec2f = vec2f(2.8, 2.1);
const BOULDER_RADIUS: f32 = 1.1;
const ROCK_AREA_M2: f32 = 6.3;
const PAINTED: f32 = 0.6;
// The rocks' lit tops and their flanks against them (models/rocks.ts), linear; the sky's light on a face
// against the sun's.
const ROCK_ALBEDO: vec3f = vec3f(0.366, 0.371, 0.332);
const FLANK: f32 = 0.65;
const SKY: f32 = 0.35;

// Rocks per square metre on ground of `cell` (r the material, g the lithology, b the region) at `slope`:
// on rough ground or exposed rock as the region's outcrops, the slope and the rock's hardness say. A
// negative slope gives the most that any slope gives.
fn rock_density(cell: vec4u, slope: f32) -> f32 {
    // kinds.a rough, kinds.z rocky.
    let kinds = rule(cell.b, slot_kinds() + i32(cell.r));
    let outcrops = rule(cell.b, slot_cliff() + 3).x;
    if ((kinds.a == 0.0 && kinds.z == 0.0) || outcrops <= 0.0 || slope > STEEPEST) {
        return 0.0;
    }
    let hardness = rule(cell.b, slot_hardness() + i32(cell.g)).x;
    // As the prototype's near field: hard rock breaks through even on level ground; mapped rock shows as rocks.
    let mapped = 0.9 * kinds.z * smoothstep(0.0, 0.6, outcrops) * smoothstep(0.2, 0.6, hardness);
    let lie = select(clamp(1.4 * slope + 0.6 * (hardness - 0.5) - 0.05, 0.0, 1.0), 1.0, slope < 0.0);
    return max(outcrops * (0.3 + 1.4 * hardness) * lie, mapped) * 0.55 / ROCK_POINT_M2;
}

// A rock point's hash.
fn rock_hash(point: vec2i) -> u32 {
    return mix_bits3(point.x, point.y, 77u);
}

// A rock point keeps a rock where this is below the density: a share of 1 over its cell's area, at the
// cell's own latitude (as texel_metres of features.wgsl). Where the density times the area passes 1, as a
// rules' share above ROCK_POINT_M2 / (12.5 m² × cos² of the latitude) does, every point keeps one and
// the ground holds fewer rocks than its rules ask.
fn rock_pick(point: vec2i, h: u32) -> f32 {
    let side = 2.0 * 3.14159265 * 6371000.0 / 8388608.0 / cosh(3.14159265 * (1.0 - f32(point.y) / 4194304.0));
    return unit_float(mix_bits(h + 3u)) / (side * side);
}

// Where a rock point's rock stands, in world units.
fn rock_at(point: vec2i, h: u32) -> vec2i {
    return (point << vec2u(ROCK_SHIFT)) + vec2i(vec2f(unit_float(mix_bits(h + 1u)), unit_float(mix_bits(h + 2u))) * f32(1 << ROCK_SHIFT));
}

// Whether a rock is a slab, else a boulder; its full height in metres; its yaw.
fn rock_slab(h: u32) -> bool {
    return unit_float(mix_bits(h + 6u)) < 0.7;
}

fn rock_height(h: u32) -> f32 {
    return ROCK_HEIGHT_M.x + pow(unit_float(mix_bits(h + 7u)), 2.0) * (ROCK_HEIGHT_M.y - ROCK_HEIGHT_M.x);
}

fn rock_yaw(h: u32) -> f32 {
    return 6.2831853 * unit_float(mix_bits(h + 8u));
}

// Where a rock's instance ends, where the instances end at `end`.
fn rock_end(end: f32, h: u32) -> f32 {
    return end * (1.0 - ROCK_HANDOFF * unit_float(mix_bits(h + 10u)));
}

// The share of a pixel at `s`, `far` metres from the eye and `footprint` metres wide, that the ground
// paints as the rocks' footprints. The ground's `cell` and `slope` at `s` stand for those of the rocks
// about it. A footprint is sampled at the pixel's middle with an edge of at most 0.3 of its size, as the
// instances are rasterised, so a far field of rocks stays a speckle as the instances make it. Pixels 8
// to 16 m wide fade to the mean, which wider pixels take.
fn rocks_painted(s: vec2f, cell: vec4u, slope: f32, far: f32, footprint: f32) -> f32 {
    let density = rock_density(cell, slope);
    if (density <= 0.0 || far < (1.0 - ROCK_HANDOFF) * ROCK_REACH_M) {
        return 0.0;
    }
    let mean = min(density * ROCK_AREA_M2 * PAINTED * PAINTED, 1.0);
    if (footprint > 16.0) {
        return mean;
    }
    // The pixel's point cell; a rock reaches at most one cell past its own.
    let base = surface.anchor >> vec2u(ROCK_SHIFT);
    let home = base + vec2i(floor((units_of(s) + vec2f(surface.anchor - (base << vec2u(ROCK_SHIFT)))) / f32(1 << ROCK_SHIFT)));
    var painted = 0.0;
    for (var k = 0; k < 9; k++) {
        let point = home + vec2i(k % 3 - 1, k / 3 - 1);
        let h = rock_hash(point);
        if (rock_pick(point, h) > density) {
            continue;
        }
        let d = s - lod_space(vec2f(rock_at(point, h) - surface.anchor));
        let yaw = rock_yaw(h);
        let along = vec2f(dot(d, vec2f(cos(yaw), sin(yaw))), dot(d, vec2f(-sin(yaw), cos(yaw))));
        let axes = min(select(vec2f(BOULDER_RADIUS), SLAB_AXES, rock_slab(h)) * PAINTED * rock_height(h), vec2f(3.0));
        let edge = min(footprint / min(axes.x, axes.y), 0.3);
        painted = max(painted, 1.0 - smoothstep(1.0 - edge, 1.0 + edge, length(along / axes)));
    }
    return mix(painted, mean, smoothstep(8.0, 16.0, footprint));
}

// The albedo that a painted rock shows in the light of ground of normal `n`, seen along `view` from the
// eye (render space): its lit top, and its flank toward the eye in its own light. A slab's flank shows
// against its top as its height against its width: 3.4 against 18.5 times the sines of the view.
fn rock_seen(view: vec3f, n: vec3f) -> vec3f {
    let s = abs(dot(view, n));
    let c = sqrt(max(1.0 - s * s, 0.0));
    let flank = normalize(vec3f(-view.xy, 0.0) + vec3f(1e-6, 0.0, 0.0));
    let lit = (SKY + max(dot(flank, look.sun.xyz), 0.0)) / (SKY + max(dot(n, look.sun.xyz), 0.0));
    return ROCK_ALBEDO * mix(1.0, FLANK * lit, c / (c + 5.4 * s));
}
