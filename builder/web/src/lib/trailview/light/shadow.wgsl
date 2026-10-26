// The object shadows of shadow.ts: the share of the sun that the trees, houses and structures about the
// eye let through to a point. Include it with `// #include ../light/shadow.wgsl`.
// #include shade.wgsl

// A cascade of shadow.ts.
struct Cascade {
    // Render space to the map: x along the sun, y across it (-1 to 1), z the depth (0 to 1).
    map: mat4x4f,
    // x, y: metres per texel along and across the sun. z: metres of height from depth 0 to 1. w: the
    // depth of the receivers' top.
    texel: vec4f,
    // x: the share that the receivers take from the map, 0 without one.
    weight: vec4f,
}

// The cascades are fields, not an array: a read of an array by a variable index copies it whole.
struct Shadows {
    near: Cascade,
    far: Cascade,
    // x: the share of the object shadows by the sun's altitude. y: the sine of the sun's altitude that
    // the maps shear by.
    sun: vec4f,
}

@group(2) @binding(8) var shadow_maps: texture_depth_2d_array;
@group(2) @binding(9) var shadow_compare: sampler_comparison;
@group(2) @binding(10) var<uniform> shadows: Shadows;

// Texels a side of a map and its mip levels, as SIDE and LEVELS of shadow.ts.
const SHADOW_SIDE: f32 = 2048.0;
const SHADOW_LEVELS: u32 = 12u;
// Half the sun's width (0.53°) in radians: a shadow's edge spreads by twice this per metre from its caster.
const SUN_HALF_WIDTH: f32 = 0.00465;
// The widest half edge that the filter spreads over, in metres across the sun: at REACH_M of shadow.ts.
const SOFT_M: f32 = 4.65;
// A caster's depth lies this many metres above a receiver's own before it shadows it.
const BIAS_M: f32 = 0.05;
// How far a receiver reads toward the sun, in metres, chosen and not measured: the ground casts nothing,
// so a lift over the depth's step (2.4 cm over the far map's span) does; a house's or a structure's face
// lies on its own caster, which the slope bias moves back; a crown's fine mesh lies up to about this far
// inside the coarsest mesh that it casts with.
const LIFT_GROUND: f32 = 0.05;
const LIFT_SOLID: f32 = 0.3;
const LIFT_CROWN: f32 = 1.5;
// The filter: a fixed spiral of reads (Vogel) in the edge, the same in every frame.
fn taps(c: u32, uv: vec2f, radius: vec2f, depth: f32) -> f32 {
    let read = vec4f(
        tap(c, uv + vec2f(0.250, 0.000) * radius, depth) + tap(c, uv + vec2f(-0.319, 0.292) * radius, depth),
        tap(c, uv + vec2f(0.049, -0.557) * radius, depth) + tap(c, uv + vec2f(0.402, 0.525) * radius, depth),
        tap(c, uv + vec2f(-0.739, -0.131) * radius, depth) + tap(c, uv + vec2f(0.700, -0.445) * radius, depth),
        tap(c, uv + vec2f(-0.234, 0.870) * radius, depth) + tap(c, uv + vec2f(-0.446, -0.859) * radius, depth));
    return dot(read, vec4f(0.125));
}

fn tap(c: u32, uv: vec2f, depth: f32) -> f32 {
    return textureSampleCompareLevel(shadow_maps, shadow_compare, uv, c, depth);
}

// The least depth, the highest caster, within `half` texels of map point `uv`: of the mip level whose
// texels span twice that, over the two by two texels that hold the square.
fn highest(c: u32, uv: vec2f, half: f32) -> f32 {
    let level = min(u32(ceil(log2(max(2.0 * half, 1.0)))), SHADOW_LEVELS - 1u);
    let size = i32(SHADOW_SIDE) >> level;
    let lo = clamp(vec2i(floor(uv * f32(size) - half / f32(1u << level))), vec2i(0), vec2i(size - 1));
    let hi = min(lo + 1, vec2i(size - 1));
    return min(min(textureLoad(shadow_maps, lo, c, level), textureLoad(shadow_maps, vec2i(hi.x, lo.y), c, level)),
        min(textureLoad(shadow_maps, vec2i(lo.x, hi.y), c, level), textureLoad(shadow_maps, hi, c, level)));
}

// The share of the sun that a cascade's casters let through at point `p` of render space (x), and the
// share that the point takes from the cascade (y). The edge of a shadow widens with the height of its
// caster above the point, as the sun's width spreads it (percentage-closer soft shadows): a caster
// `rise` metres higher reaches SUN_HALF_WIDTH × rise / sine metres across the sun at most. The search
// narrows twice: the highest caster within the widest edge sets the edge that the highest caster
// within it can reach, and that one the width of the filter.
fn cascade_share(cascade: Cascade, c: u32, p: vec3f) -> vec2f {
    if (cascade.weight.x <= 0.0) {
        return vec2f(1.0, 0.0);
    }
    let q = (cascade.map * vec4f(p, 1.0)).xyz;
    let texel = cascade.texel;
    let height = abs(2.0 * (q.z - texel.w) / (1.0 - texel.w) - 1.0);
    let edge = max(max(abs(q.x), abs(q.y)), height);
    if (edge >= 1.0) {
        return vec2f(1.0, 0.0);
    }
    let weight = cascade.weight.x * (1.0 - smoothstep(0.8, 1.0, edge));
    let uv = vec2f(0.5 + 0.5 * q.x, 0.5 - 0.5 * q.y);
    // Texels per metre across the sun: a width across it spans 1 / sine as much along it on the map.
    let sine = shadows.sun.y;
    let per_m = vec2f(1.0 / sine, 1.0) / texel.xy;
    let deepest = q.z - BIAS_M / texel.z;
    var depth = 0.0;
    for (var i = 0u; i < 2u; i++) {
        let half = min(SUN_HALF_WIDTH * (q.z - depth) * texel.z / sine, SOFT_M) * max(per_m.x, per_m.y);
        depth = highest(c, uv, half);
        if (depth >= deepest) {
            return vec2f(1.0, weight);
        }
    }
    let radius = max(min(SUN_HALF_WIDTH * (q.z - depth) * texel.z / sine, SOFT_M) * per_m, vec2f(1.0)) / SHADOW_SIDE;
    return vec2f(taps(c, uv, radius, q.z), weight);
}

// The share of the sun at point `p` of render space with the normal `n`: the share `terrain` that the
// terrain lets through, times the share that the objects let through. It reads the maps `lift` metres
// toward the sun, so that a caster does not shadow its own lit face, and only where the sun can reach
// the face. The near map holds sway where it reaches; toward its edge the far map takes over, and
// toward the far map's edge the objects' share fades to 1.
fn sun_share(terrain: f32, p: vec3f, n: vec3f, lift: f32) -> f32 {
    return shared_sun(terrain, p, n, lift, true);
}

// As `sun_share` for a crown, which reads the near map only: farther away its far tone (draw.wgsl)
// shades it, and the many crowns of a forest would cost more than their shadows show.
fn crown_sun_share(terrain: f32, p: vec3f, n: vec3f) -> f32 {
    return shared_sun(terrain, p, n, LIFT_CROWN, false);
}

fn shared_sun(terrain: f32, p: vec3f, n: vec3f, lift: f32, far_too: bool) -> f32 {
    if (terrain <= 0.0 || dot(n, look.sun.xyz) <= 0.0 || shadows.sun.x <= 0.0) {
        return terrain;
    }
    let at = p + look.sun.xyz * lift;
    let near = cascade_share(shadows.near, 0u, at);
    var lit = mix(1.0, near.x, near.y);
    if (near.y < 1.0 && far_too) {
        let far = cascade_share(shadows.far, 1u, at);
        lit = mix(mix(1.0, far.x, far.y), near.x, near.y);
    }
    return terrain * mix(1.0, lit, shadows.sun.x);
}
