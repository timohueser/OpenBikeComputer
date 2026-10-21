// The feature mask of near pages, by the page that draws a texel, in group 3. Include it after
// drawn-height.wgsl with `// #include features.wgsl`.
// #include drawn-height.wgsl

// Per page layer: the mask layer + 1, 0 for none.
@group(3) @binding(0) var<storage, read> masks: array<u32>;
// Per texel, filtered: the distance to the nearest road edge and to the waterline (negative inside),
// the across coordinate of that road, the cut or fill depth; ±MASK_RANGE metres as 0..1.
@group(3) @binding(1) var mask_shapes: texture_2d_array<f32>;
// Per texel: the road surface code (0 none), the water kind (0 none, 1 water), the flags.
@group(3) @binding(2) var mask_kinds: texture_2d_array<u32>;

const BUILDING: u32 = 1u;
const WALL: u32 = 2u;
const CONFLICT: u32 = 4u;
const FORDED: u32 = 8u;
const PORTAL: u32 = 16u;

// The mask layer + 1 of the page that draws a texel, 0 for none.
fn mask_of(level: u32, t: vec2i) -> u32 {
    let page = entry(level, t) & 0xffffu;
    return select(0u, masks[max(page, 1u) - 1u], page != 0u);
}

fn shape_at(page: u32, t: vec2i) -> vec4f {
    return textureLoad(mask_shapes, in_layer(page, t), page >> 6u, 0);
}

struct Mask {
    shape: vec4f,
    kind: vec4u,
    // The level of the page that holds it; 0 where no page within reach has a mask.
    level: u32,
}

// The mask at a point, from the finest page at or above `top` that has a mask: the shapes filtered
// between the four texels around the point, held in that page, so that an edge is smooth below a
// texel; the kinds of the nearest texel.
fn mask_at(s: vec2f, top: u32) -> Mask {
    for (var level = top; level >= COARSEST; level--) {
        let t = texel_of(s, level);
        let whole = t.whole + vec2i(round(t.part));
        let slot = mask_of(level, whole);
        if (slot != 0u) {
            let page = slot - 1u;
            let first = (whole >> vec2u(8u)) << vec2u(8u);
            let a = clamp(t.whole, first, first + vec2i(255));
            let b = clamp(t.whole + vec2i(1), first, first + vec2i(255));
            let shape = mix(mix(shape_at(page, a), shape_at(page, vec2i(b.x, a.y)), t.part.x),
                mix(shape_at(page, vec2i(a.x, b.y)), shape_at(page, b), t.part.x), t.part.y);
            return Mask(shape, textureLoad(mask_kinds, in_layer(page, whole), page >> 6u, 0), level);
        }
    }
    return Mask(vec4f(0.5), vec4u(0u), 0u);
}
