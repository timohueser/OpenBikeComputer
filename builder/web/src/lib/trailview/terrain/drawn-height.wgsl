// The drawn surface: drawnHeight(s) of #2747, for every shader that stands on the terrain. The LOD
// map gives the page of each level, and the LOD rule gives the level and the morph as the mesh has
// them. Include it with `// #include drawn-height.wgsl`.
//
// LOD space s is metres east and north of the eye along the Mercator axes, at the scale of the eye's
// latitude. World positions are whole units (2^30 across the world) about an anchor, so no float
// ever holds a world coordinate.

struct Surface {
    // The eye: whole world units, and the rest of a unit.
    anchor: vec2i,
    part: vec2f,
    // Metres per unit at the eye.
    unit: f32,
    // The eye's height above the ground, for the LOD.
    above: f32,
    // CDLOD: the distance where the LOD is 0. Rings: the ring half-width in nodes.
    range: f32,
    // The finest level that draws.
    finest: u32,
    // The cubic from LOD space to render space (frame metres about the eye): 10 coefficients of x, then 10 of y.
    forward: array<vec4f, 5>,
    // Per level: xy the node at the origin of its LOD map window, zw the ring centre in units about the anchor.
    levels: array<vec4i, 19>,
}

@group(1) @binding(0) var<uniform> surface: Surface;
// MAP × MAP entries per level: the page layer + 1 (0 for no page), with STORED for a stored page.
@group(1) @binding(1) var<storage, read> lod_map: array<u32>;
// Per texel: the bits of a float32 height, or an r16 code with an offset and a scale per page. A
// page is one of 8 × 8 tiles of a layer.
@group(1) @binding(2) var heights: texture_2d_array<u32>;
@group(1) @binding(3) var<storage, read> scales: array<vec2f>;
// Per texel: x east and y north of the unit normal.
@group(1) @binding(4) var normals: texture_2d_array<f32>;

override RINGS: bool = false;
override R16: bool = false;

const MAP: i32 = 32;
const COARSEST: u32 = 1u;
const MORPH: f32 = 0.3;
const STORED: u32 = 0x80000000u;
const UNKNOWN32: u32 = 0xffffffffu;
const UNKNOWN16: u32 = 0xffffu;
// A height that no page knows. Never NaN: shader compilers may assume that there is none.
const NONE: f32 = -1e30;

fn known(h: f32) -> bool {
    return h > -1e29;
}

// Units about the anchor of a point of LOD space.
fn units_of(s: vec2f) -> vec2f {
    return vec2f(s.x, -s.y) / surface.unit + surface.part;
}

// The point of LOD space at units about the anchor.
fn lod_space(u: vec2f) -> vec2f {
    let d = u - surface.part;
    return vec2f(d.x, -d.y) * surface.unit;
}

fn coefficient(i: u32) -> f32 {
    return surface.forward[i / 4u][i % 4u];
}

// The cubic of 10 coefficients from `at`: 1, x, y, x², xy, y², x³, x²y, xy², y³.
fn cubic(at: u32, p: vec2f) -> f32 {
    let x = p.x;
    let y = p.y;
    return coefficient(at) + x * (coefficient(at + 1u) + x * (coefficient(at + 3u) + x * coefficient(at + 6u) + y * coefficient(at + 7u)))
        + y * (coefficient(at + 2u) + y * (coefficient(at + 5u) + y * coefficient(at + 9u) + x * coefficient(at + 8u))) + coefficient(at + 4u) * x * y;
}

// Render space: frame metres east and north of the eye.
fn to_render(s: vec2f) -> vec2f {
    return vec2f(cubic(0u, s), cubic(10u, s));
}

// A position at a level: whole texels, and the part of a texel past the centre of the whole one.
struct Texel {
    whole: vec2i,
    part: vec2f,
}

fn texel_of(s: vec2f, level: u32) -> Texel {
    let shift = 22u - level;
    let low = vec2f(surface.anchor & vec2i((1 << shift) - 1));
    let t = (low + units_of(s)) / f32(1 << shift) - 0.5;
    let whole = floor(t);
    return Texel((surface.anchor >> vec2u(shift)) + vec2i(whole), t - whole);
}

// The same for whole world units, such as a texel centre.
fn texel_of_units(u: vec2i, level: u32) -> Texel {
    let shift = 22u - level;
    let v = u - vec2i(1 << (shift - 1u));
    return Texel(v >> vec2u(shift), vec2f(v & vec2i((1 << shift) - 1)) / f32(1 << shift));
}

// Where a texel of a page lies in its layer.
fn in_layer(page: u32, t: vec2i) -> vec2i {
    return (t & vec2i(255)) + 256 * vec2i(vec2u(page & 7u, (page >> 3u) & 7u));
}

// The LOD map entry of the node that holds a texel, or 0.
fn entry(level: u32, t: vec2i) -> u32 {
    let slot = (t >> vec2u(8u)) - surface.levels[level].xy;
    if (any(slot < vec2i(0)) || any(slot >= vec2i(MAP))) {
        return 0u;
    }
    return lod_map[(i32(level) * MAP + slot.y) * MAP + slot.x];
}

// The height of a texel; NONE where no page holds it or it is unknown. `stored` reads stored pages only.
fn texel_height(level: u32, t: vec2i, stored: bool) -> f32 {
    let e = entry(level, t);
    if (e == 0u || (stored && e < STORED)) {
        return NONE;
    }
    let page = (e & ~STORED) - 1u;
    let raw = textureLoad(heights, in_layer(page, t), page >> 6u, 0).x;
    if (R16) {
        if (raw == UNKNOWN16) {
            return NONE;
        }
        let scale = scales[page];
        return scale.x + f32(raw) * scale.y;
    }
    if (raw == UNKNOWN32) {
        return NONE;
    }
    return bitcast<f32>(raw);
}

// The bilinear height between four texels, or NONE unless all four are known.
fn bilinear(level: u32, t: Texel) -> f32 {
    let a = texel_height(level, t.whole, false);
    let b = texel_height(level, t.whole + vec2i(1, 0), false);
    let c = texel_height(level, t.whole + vec2i(0, 1), false);
    let d = texel_height(level, t.whole + vec2i(1, 1), false);
    if (!(known(a) && known(b) && known(c) && known(d))) {
        return NONE;
    }
    return mix(mix(a, b, t.part.x), mix(c, d, t.part.x), t.part.y);
}

// The height of the finest level up to `top` that knows a point: a missing height is never 0 m.
fn level_height(s: vec2f, top: u32) -> f32 {
    for (var level = top; level >= COARSEST; level--) {
        let h = bilinear(level, texel_of(s, level));
        if (known(h)) {
            return h;
        }
    }
    return NONE;
}

fn units_height(u: vec2i, top: u32) -> f32 {
    for (var level = top; level >= COARSEST; level--) {
        let h = bilinear(level, texel_of_units(u, level));
        if (known(h)) {
            return h;
        }
    }
    return NONE;
}

// The LOD at a point: a whole level on its plateau, and between two levels in a morph band.
fn lod(s: vec2f) -> f32 {
    if (RINGS) {
        return ring_lod(s);
    }
    let q = log2(surface.range / length(vec3f(s, surface.above)));
    let k = floor(q);
    return clamp(k - 1.0 + min(1.0, (q - k) / MORPH), f32(COARSEST), f32(surface.finest));
}

// The finest ring that holds the point, and its blend by the distance from the eye in nodes.
fn ring_lod(s: vec2f) -> f32 {
    let u = units_of(s);
    let e = max(abs(s.x), abs(s.y)) / surface.unit;
    for (var level = surface.finest; level > COARSEST; level--) {
        let size = f32(1 << (30u - level));
        let c = vec2f(surface.levels[level].zw);
        if (max(abs(u.x - c.x), abs(u.y - c.y)) <= surface.range * size) {
            return f32(level) - clamp((e / size - (surface.range * 0.5 + 0.55)) / (surface.range * 0.5 - 1.6), 0.0, 1.0);
        }
    }
    return f32(COARSEST);
}

// The drawn height at a point of LOD space; NONE where no level knows it.
fn drawn_height(s: vec2f) -> f32 {
    let lambda = lod(s);
    let level = u32(ceil(lambda));
    let fine = level_height(s, level);
    let morph = f32(level) - lambda;
    if (morph <= 0.0 || !known(fine)) {
        return fine;
    }
    let coarse = level_height(s, level - 1u);
    return select(fine, mix(fine, coarse, morph), known(coarse));
}

// The normal of a texel, with z 1; z −1 where no page holds the texel or its height is unknown.
fn texel_normal(level: u32, t: vec2i) -> vec3f {
    let e = entry(level, t);
    if (e == 0u) {
        return vec3f(0.0, 0.0, -1.0);
    }
    let page = (e & ~STORED) - 1u;
    let n = textureLoad(normals, in_layer(page, t), page >> 6u, 0).xy;
    return vec3f(n, select(1.0, -1.0, n.x < -0.99 && n.y < -0.99));
}

fn level_normal(s: vec2f, top: u32) -> vec2f {
    for (var level = top; level >= COARSEST; level--) {
        let t = texel_of(s, level);
        let a = texel_normal(level, t.whole);
        let b = texel_normal(level, t.whole + vec2i(1, 0));
        let c = texel_normal(level, t.whole + vec2i(0, 1));
        let d = texel_normal(level, t.whole + vec2i(1, 1));
        if (min(min(a.z, b.z), min(c.z, d.z)) > 0.0) {
            return mix(mix(a.xy, b.xy, t.part.x), mix(c.xy, d.xy, t.part.x), t.part.y);
        }
    }
    return vec2f(0.0);
}

// The normal of the drawn surface in LOD space axes, with the same level and morph as its height.
fn drawn_normal(s: vec2f) -> vec3f {
    let lambda = lod(s);
    let level = u32(ceil(lambda));
    var n = level_normal(s, level);
    let morph = f32(level) - lambda;
    if (morph > 0.0) {
        n = mix(n, level_normal(s, level - 1u), morph);
    }
    return normalize(vec3f(n, sqrt(max(1.0 - dot(n, n), 1e-4))));
}
