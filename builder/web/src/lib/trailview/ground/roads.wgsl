// The paint of roads and paths over the ground, from the feature mask: a port of `roadSurface` and the
// road section of `groundOn` of the prototype. The mask's distance to the road edge is linear across
// it, so the filtered distance keeps the edge sharp to a pixel. look.wgsl includes it, and it reads
// `rule()` and `noise()` of look.wgsl: an include composes each file once.
// #include ../terrain/features.wgsl
// #include look.wgsl

struct Road {
    colour: vec3f,
    // The share of the pixel that the road covers.
    cover: f32,
}

// The share of a pixel w metres wide, centred on x, that falls inside [a, b].
fn band(x: f32, a: f32, b: f32, w: f32) -> f32 {
    return clamp((min(x + 0.5 * w, b) - max(x - 0.5 * w, a)) / w, 0.0, 1.0);
}

// A road's surface at `inside` metres in from its edge and `across` metres from its middle, seen by a
// pixel w metres wide, over the ground colour beside it. Paved main roads keep a line along each
// edge; lanes and tracks wear two wheel ruts, grass and dirt ones with grass between; gravel lies in
// grains. Codes as SURFACES of bins.ts: main, road, gravel, dirt, grass, path.
fn road_surface(region: u32, code: u32, inside: f32, across: f32, p: vec2f, w: f32, ground: vec3f) -> vec3f {
    var c = rule(region, slot_roads() + i32(code) - 1).rgb;
    // Patched asphalt, compacted gravel: broad tone, too faint to alias far off, and a grain near by.
    c *= 0.94 + 0.1 * noise(p / 4.0) + 0.06 * (noise(p / 0.35) - 0.5) * (1.0 - smoothstep(0.05, 0.3, w));
    // Only roads of two full lanes carry edge lines.
    if (code == 1u && inside + across >= 3.5) {
        c = mix(c, rule(region, slot_road_line()).rgb, band(inside, 0.25, 0.37, w));
    }
    if (code == 3u) {
        c *= 0.93 + 0.14 * mix(0.5, noise(p / 0.18), 1.0 - smoothstep(0.05, 0.25, w));
    }
    let ruts = band(across, 0.55, 1.05, w);
    // One car wide: inside and across add up to the half-width.
    let lane = inside + across < 2.6;
    if ((code == 2u && lane) || code == 3u) {
        c *= 1.0 - 0.08 * ruts;
    }
    if (code == 4u) {
        c = mix(c * (1.0 - 0.18 * ruts), ground, band(across, -1.0, 0.3 + 0.12 * noise(p / 0.6), w) * 0.85);
    }
    if (code == 5u) {
        c = mix(mix(ground, c, 0.35), rule(region, slot_roads() + 3).rgb, ruts * 0.8);
    }
    return c;
}

// The road at a point of the ground: `s` in LOD space, `p` in world metres, a pixel `w` metres wide,
// over `ground`. A road narrower than a pixel draws one pixel wide, as faint as its share of it. A
// path in forest shows only where trees are drawn, which `tree` does not yet: L9 draws them.
fn road_at(s: vec2f, p: vec2f, w: f32, ground: vec3f, region: u32, tree: f32) -> Road {
    let mask = mask_at(s, u32(ceil(lod(s))));
    if (mask.level == 0u || mask.kind.x == 0u) {
        return Road(ground, 0.0);
    }
    var edge = mask_metres(mask.shape.x);
    if (edge > w + 0.5) {
        return Road(ground, 0.0);
    }
    let across = abs(mask_metres(mask.shape.z));
    let half = max(across - edge, 0.05);
    let code = min(mask.kind.x, 6u);
    let colour = road_surface(region, code, max(-edge, 0.0), across, p, w, ground);
    // Unpaved edges fray into the ground.
    if (code > 2u) {
        edge += 0.36 * (0.7 * noise(p / 1.1) + 0.3 * noise(p / 0.35) - 0.5);
    }
    let thin = max(0.0, 0.5 * w - half);
    var cover = (1.0 - smoothstep(-w, w, edge - thin)) * min(1.0, 2.0 * half / w + 0.35);
    if ((mask.kind.z & PATHWAY) != 0u) {
        cover *= 1.0 - tree;
    }
    return Road(colour, cover);
}
