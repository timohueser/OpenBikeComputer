// The object shadows of shadow.ts: the share of the sun that the trees, houses and structures about the
// eye let through to a point. Include it with `// #include ../light/shadow.wgsl`.
// #include shade.wgsl

@group(2) @binding(8) var shadow_map: texture_depth_2d;
@group(2) @binding(9) var shadow_compare: sampler_comparison;

// Texels a side of the map, as SIDE of shadow.ts; the jittered frames of a still view (STILL_FRAMES of
// temporal.ts) spread its reads over this many texels, which softens the edge of a shadow.
const SHADOW_SIDE: f32 = 2048.0;
const SHADOW_SPREAD: f32 = 1.5;
// How far a receiver reads toward the sun, in metres: the ground casts nothing; a solid face lies on
// its own caster; a crown's mesh lies inside the coarser mesh that it casts with.
const LIFT_GROUND: f32 = 0.05;
const LIFT_SOLID: f32 = 0.3;
const LIFT_CROWN: f32 = 1.5;

// The share of the sun at point `p` of render space with the normal `n`: the share `terrain` that the
// terrain lets through, times the share that the objects let through. It reads the map `lift` metres
// toward the sun, so that a caster does not shadow its own lit face, and only where the sun can reach
// the face. The objects' share fades to 1 toward the edge of the map.
fn sun_share(terrain: f32, p: vec3f, n: vec3f, lift: f32) -> f32 {
    if (terrain <= 0.0 || dot(n, look.sun.xyz) <= 0.0) {
        return terrain;
    }
    let q = look.sun_view * vec4f(p + look.sun.xyz * lift, 1.0);
    let edge = max(abs(q.x), abs(q.y));
    if (edge >= 1.0) {
        return terrain;
    }
    let k = f32(max(look.modes.y, 1u)) - 0.5;
    let turn = 2.3999632 * k;
    let spread = SHADOW_SPREAD * sqrt(k / 16.0) / SHADOW_SIDE * vec2f(cos(turn), sin(turn));
    let lit = textureSampleCompareLevel(shadow_map, shadow_compare, vec2f(0.5 + 0.5 * q.x, 0.5 - 0.5 * q.y) + spread, q.z);
    return terrain * mix(lit, 1.0, smoothstep(0.8, 1.0, edge));
}
