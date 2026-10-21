// The feature mask of near pages and the water height of pages with water, by the page that draws a
// texel. Include it after drawn-height.wgsl with `// #include features.wgsl`.
// #include drawn-height.wgsl

// Per page layer: the mask layer + 1 in the low 16 bits and the water layer + 1 in the high 16 bits, 0 for none.
@group(2) @binding(0) var<storage, read> extras: array<u32>;
// Per texel, filtered: the distance to the nearest road edge and to the waterline (negative inside),
// the across coordinate of that road, the cut or fill depth; ±MASK_RANGE metres as 0..1.
@group(2) @binding(1) var mask_shapes: texture_2d_array<f32>;
// Per texel: the road surface code (0 none), the water kind (0 none, 1 lake), the flags.
@group(2) @binding(2) var mask_kinds: texture_2d_array<u32>;
@group(2) @binding(3) var water_levels: texture_2d_array<f32>;

const BUILDING: u32 = 1u;
const WALL: u32 = 2u;
const CONFLICT: u32 = 4u;
const FORDED: u32 = 8u;
const PORTAL: u32 = 16u;

// The extras of the page that draws a texel: the mask layer + 1 and the water layer + 1.
fn extras_of(level: u32, t: vec2i) -> vec2u {
    let page = entry(level, t) & 0xffffu;
    if (page == 0u) {
        return vec2u(0u);
    }
    let e = extras[page - 1u];
    return vec2u(e & 0xffffu, e >> 16u);
}

struct Mask {
    shape: vec4f,
    kind: vec4u,
    // The level of the page that holds it; 0 where no page within reach has a mask.
    level: u32,
}

// The mask at a point: the nearest texel of the finest page at or above `top` that has a mask.
fn mask_at(s: vec2f, top: u32) -> Mask {
    for (var level = top; level >= COARSEST; level--) {
        let t = texel_of(s, level);
        let whole = t.whole + vec2i(round(t.part));
        let slots = extras_of(level, whole);
        if (slots.x != 0u) {
            let page = slots.x - 1u;
            return Mask(textureLoad(mask_shapes, in_layer(page, whole), page >> 6u, 0), textureLoad(mask_kinds, in_layer(page, whole), page >> 6u, 0), level);
        }
    }
    return Mask(vec4f(0.5), vec4u(0u), 0u);
}

// The water height at a point from the finest page at or above `top` with water: NONE without water.
fn water_at(s: vec2f, top: u32) -> f32 {
    for (var level = top; level >= COARSEST; level--) {
        let t = texel_of(s, level);
        let whole = t.whole + vec2i(round(t.part));
        let slots = extras_of(level, whole);
        if (slots.y != 0u) {
            let page = slots.y - 1u;
            return textureLoad(water_levels, in_layer(page, whole), page >> 6u, 0).x;
        }
    }
    return NONE;
}
