// The feature mask of near pages, by the page that draws a texel, in group 3. Include it after
// drawn-height.wgsl with `// #include features.wgsl`.
// #include drawn-height.wgsl

// Metres from the eye that the mask pool holds the near pages to, and per page layer the mask layer + 1, 0 for none.
struct Masks {
    reach: f32,
    layer: array<u32>,
}
@group(3) @binding(0) var<storage, read> masks: Masks;
// Per texel: the distance to the nearest road edge (negative inside), ±MASK_M metres as 0..1, filtered;
// the fraction of a track's sleepers from its start there, unfiltered; the metres from the middle of that
// road (signed by side), as the distance, or ±TRACK_M for a track, filtered; then the road surface code
// (0 none) in the low 4 bits of a byte and the flags above them, unfiltered.
@group(3) @binding(1) var mask_texels: texture_2d_array<f32>;
// Per mesh vertex of a mask's page (64 × 64, every fourth texel corner), in the mask's layer: the
// vertex's move onto the nearest road edge in units, x and y as 16-bit signed halves. The stamp pass
// writes it.
@group(3) @binding(2) var snaps: texture_2d_array<u32>;

const WALL: u32 = 1u;
const CONFLICT: u32 = 2u;
const FORDED: u32 = 4u;
// The coarsest level with masks, as NEAR of bins.ts.
const MASK_LEVEL: u32 = 16u;
// As MASK_RANGE of stamp.wgsl.
const MASK_M: f32 = 12.75;
// As RAIL_CODE and SLEEPER_M of bins.ts: the tracks from this surface code on, and the spacing of their sleepers.
const RAIL_CODE: u32 = 8u;
const SLEEPER_SPACING_M: f32 = 0.6;
// As TRACK_RANGE of stamp.wgsl: a track's paint lies this near its middle, so its rails take finer steps.
const TRACK_M: f32 = 3.2;
// As SURFACES of bins.ts: the half-widths of a standard and a narrow track, whose codes alternate from RAIL_CODE on.
const RAIL_HALF_M: f32 = 1.9;
const NARROW_HALF_M: f32 = 1.5;

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
    return mask_byte(page, t) >> 4u;
}

// 1 where a texel lies on a road's cut or fill face.
fn faced(texel: vec4f) -> f32 {
    return select(0.0, 1.0, ((u32(round(texel.w * 255.0)) >> 4u) & WALL) != 0u);
}

struct Mask {
    // The distances of the texel layout, and in w the share of the texels on a road's cut or fill face.
    shape: vec4f,
    // The surface code and the flags.
    kind: vec2u,
    // The level of the page that holds it; 0 where no page within reach has a mask.
    level: u32,
    // The direction in LOD space away from the nearest road edge, from the filtered distance; zero
    // where the four texels do not tell it.
    away: vec2f,
    // The half-width of the nearest road, filtered from each texel's: the filtered distances miss it
    // by up to half a texel on the road's middle, where the distance to the edge turns.
    half: f32,
    // A track's sleepers from its start, in its fraction.
    sleepers: f32,
    // The metres from the middle of the nearest road, signed by side, filtered.
    across: f32,
    // The unit direction in LOD space across the nearest road, from the rise of `across`, which has no
    // ridge on the road's middle as the distance to its edge has; `away` where it does not tell it.
    axis: vec2f,
}

// Metres across a texel of a level in texel row `row`, as unit_metres of stamp-core.wgsl.
fn texel_metres(level: u32, row: i32) -> f32 {
    let shift = 22u - level;
    return 2.0 * 3.14159265 * 6371000.0 / 1073741824.0 / cosh(3.14159265 * (1.0 - f32(row << shift) / 536870912.0)) * f32(1 << shift);
}

// The metres from the middle of a texel's road.
fn across_metres(texel: vec4f) -> f32 {
    let track = (u32(round(texel.w * 255.0)) & 15u) >= RAIL_CODE;
    return select(mask_metres(texel.z), (texel.z - 0.5) * 2.0 * TRACK_M, track);
}

// The half-width of the road that a texel's distances belong to; a track's from its code, as `across`
// stops at ±TRACK_M.
fn half_width(texel: vec4f) -> f32 {
    let code = u32(round(texel.w * 255.0)) & 15u;
    let track = select(RAIL_HALF_M, NARROW_HALF_M, ((code - RAIL_CODE) & 1u) == 1u);
    return select(abs(across_metres(texel)) - mask_metres(texel.x), track, code >= RAIL_CODE);
}

// The mask at a point, from the finest page at or above `top` that has a mask: the shapes and the
// faces filtered between the four texels around the point, held in that page, so that an edge is
// smooth below a texel; the kinds of the nearest texel.
// With `roads`, none where the nearest texel holds no road within MASK_M: one texel read decides.
fn mask_at(s: vec2f, top: u32, roads: bool) -> Mask {
    for (var level = top; level >= MASK_LEVEL; level--) {
        let t = texel_of(s, level);
        let whole = t.whole + vec2i(round(t.part));
        let slot = mask_of(level, whole);
        if (slot != 0u) {
            let page = slot - 1u;
            let byte = mask_byte(page, whole);
            if (roads && (byte & 15u) == 0u) {
                break;
            }
            let first = (whole >> vec2u(8u)) << vec2u(8u);
            let a = clamp(t.whole, first, first + vec2i(255));
            let b = clamp(t.whole + vec2i(1), first, first + vec2i(255));
            let ab = array(shape_at(page, a), shape_at(page, vec2i(b.x, a.y)), shape_at(page, vec2i(a.x, b.y)), shape_at(page, b));
            let shape = mix(mix(ab[0], ab[1], t.part.x), mix(ab[2], ab[3], t.part.x), t.part.y);
            let face = mix(mix(faced(ab[0]), faced(ab[1]), t.part.x), mix(faced(ab[2]), faced(ab[3]), t.part.x), t.part.y);
            // Texel rows run along -y of LOD space.
            let rise = vec2f(mix(ab[1].x - ab[0].x, ab[3].x - ab[2].x, t.part.y), -mix(ab[2].x - ab[0].x, ab[3].x - ab[1].x, t.part.x));
            let away = select(vec2f(0.0), normalize(rise), dot(rise, rise) > 0.0);
            let half = mix(mix(half_width(ab[0]), half_width(ab[1]), t.part.x), mix(half_width(ab[2]), half_width(ab[3]), t.part.x), t.part.y);
            // The sleepers of the nearest texel, carried on along the track: across the rise of the metres
            // from its middle, which grow to the track's left. A fraction cannot be filtered across its wrap.
            let m = array(across_metres(ab[0]), across_metres(ab[1]), across_metres(ab[2]), across_metres(ab[3]));
            let across = mix(mix(m[0], m[1], t.part.x), mix(m[2], m[3], t.part.x), t.part.y);
            let middle = vec2f(mix(m[1] - m[0], m[3] - m[2], t.part.y), mix(m[2] - m[0], m[3] - m[1], t.part.x));
            let ahead = select(vec2f(0.0), normalize(vec2f(middle.y, -middle.x)), dot(middle, middle) > 0.0);
            let axis = select(away, normalize(vec2f(middle.x, -middle.y)), dot(middle, middle) > 0.0);
            let sleepers = shape_at(page, whole).y + dot(t.part - round(t.part), ahead) * texel_metres(level, whole.y) / SLEEPER_SPACING_M;
            return Mask(vec4f(shape.xyz, face), vec2u(byte & 15u, byte >> 4u), level, away, half, sleepers, across, axis);
        }
    }
    return Mask(vec4f(0.5), vec2u(0u), 0u, vec2f(0.0), 0.0, 0.0, 0.0, vec2f(0.0));
}
