// The feature mask of near pages, by the page that draws a texel, in group 3. Include it after
// drawn-height.wgsl with `// #include features.wgsl`.
// #include drawn-height.wgsl

// Metres from the eye that the mask pool holds the near pages to, and per page layer the mask layer + 1, 0 for none.
struct Masks {
    reach: f32,
    layer: array<u32>,
}
@group(3) @binding(0) var<storage, read> masks: Masks;
// Per texel: the distance to the nearest road edge and to the waterline (negative inside) and the
// metres from the middle of that road (signed by side), ±MASK_M metres as 0..1, filtered; then the
// road surface code (0 none) in the low 3 bits of a byte and the flags above them, read unfiltered.
@group(3) @binding(1) var mask_texels: texture_2d_array<f32>;

const BUILDING: u32 = 1u;
const WALL: u32 = 2u;
const CONFLICT: u32 = 4u;
const FORDED: u32 = 8u;
const PORTAL: u32 = 16u;
// The coarsest level with masks, as NEAR of bins.ts.
const MASK_LEVEL: u32 = 16u;
// As MASK_RANGE of stamp.wgsl.
const MASK_M: f32 = 12.75;

// Metres from a code of the mask's shapes.
fn mask_metres(code: f32) -> f32 {
    return (code - 0.5) * 2.0 * MASK_M;
}

// The mask layer + 1 of the page that draws a texel, 0 for none.
fn mask_of(level: u32, t: vec2i) -> u32 {
    let page = entry(level, t) & 0xffffu;
    return select(0u, masks.layer[max(page, 1u) - 1u], page != 0u);
}

fn shape_at(page: u32, t: vec2i) -> vec4f {
    return textureLoad(mask_texels, in_layer(page, t), page >> 6u, 0);
}

// The byte of the surface code and the flags of a texel.
fn mask_byte(page: u32, t: vec2i) -> u32 {
    return u32(round(shape_at(page, t).w * 255.0));
}

fn mask_flags(page: u32, t: vec2i) -> u32 {
    return mask_byte(page, t) >> 3u;
}

struct Mask {
    // The distances of the texel layout, and w 0.
    shape: vec4f,
    // The surface code, 1 in water, the flags.
    kind: vec4u,
    // The level of the page that holds it; 0 where no page within reach has a mask.
    level: u32,
}

// The mask at a point, from the finest page at or above `top` that has a mask: the shapes filtered
// between the four texels around the point, held in that page, so that an edge is smooth below a
// texel; the kinds of the nearest texel.
// With `roads`, none where the nearest texel holds no road within MASK_M: one texel read decides.
fn mask_at(s: vec2f, top: u32, roads: bool) -> Mask {
    for (var level = top; level >= MASK_LEVEL; level--) {
        let t = texel_of(s, level);
        let whole = t.whole + vec2i(round(t.part));
        let slot = mask_of(level, whole);
        if (slot != 0u) {
            let page = slot - 1u;
            let byte = mask_byte(page, whole);
            if (roads && (byte & 7u) == 0u) {
                break;
            }
            let first = (whole >> vec2u(8u)) << vec2u(8u);
            let a = clamp(t.whole, first, first + vec2i(255));
            let b = clamp(t.whole + vec2i(1), first, first + vec2i(255));
            let shape = mix(mix(shape_at(page, a), shape_at(page, vec2i(b.x, a.y)), t.part.x),
                mix(shape_at(page, vec2i(a.x, b.y)), shape_at(page, b), t.part.x), t.part.y);
            return Mask(vec4f(shape.xyz, 0.0), vec4u(byte & 7u, select(0u, 1u, shape.y < 0.5), byte >> 3u, 0u), level);
        }
    }
    return Mask(vec4f(0.5), vec4u(0u), 0u);
}
