// The cover of the corridors, for the snap pass of terrain/stamp.wgsl: per mask page, the near and the far faces of the L16
// pieces that draw over it, seen from above into one tile, the page and its margin, then their borders (borders.ts). Per
// texel: whether a near face, a far face and a way's surface cover its centre, and whether a border's band reaches it,
// where not all of the texel need lie in the faces. In `heights`, the least height of the faces that have no skirt row,
// whose heights only the stamped ground gives. Decks and details give no cover.

// As piece.ts and corridor.wgsl.
const CORRIDOR_STEP: f32 = 0.083333336;
const MAT_BITS: u32 = 127u;
const FLAGS_SHIFT: u32 = 11u;
const FLAGS_BITS: u32 = 63u;
const FINE_SHIFT: u32 = 28u;
// As MAT, LOOK and CF of section.ts.
const MAT_GROUND: u32 = 1u;
const MAT_RAIL: u32 = 40u;
const LOOK_GRAVEL: u32 = 26u;
const LOOK_TRAIL: u32 = 28u;
const CF_FILL: u32 = 3u;
const CF_DECK: u32 = 32u;
// As COVER_TILE, COVER_MARGIN and COVER_ROW of cover.ts, and its NEAR.
const TILE: u32 = 272u;
const MARGIN: f32 = 8.0;
const ROW: u32 = 4u;
const RANGE_NEAR: u32 = 1u;
// A border's band reaches half a texel's diagonal past it, so it meets every texel that the border passes through.
const REACH: f32 = 0.7072;

// Per tile, per texel: the least height of the faces as the complement of its ordered bits, 0 for none.
@group(0) @binding(0) var<storage, read_write> heights: array<atomic<u32>>;

struct CoverIn {
    // As CorridorIn of corridor.wgsl.
    @location(0) at: vec2i,
    @location(1) z: f32,
    @location(2) word: u32,
    // The piece's middle less the page's corner, in world units, the world units of a texel of the page, and the range.
    @location(3) middle: vec2i,
    @location(4) texel: f32,
    @location(5) range: u32,
}

struct CoverOut {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(flat) word: u32,
    @location(1) @interpolate(flat) range: u32,
    @location(2) z: f32,
    // Over 0 in a face with a skirt's row.
    @location(3) skirt: f32,
}

// The clip point of a point of the tile in texels. Texel rows run along +y of world units, pixel rows down the tile.
fn clip_of(p: vec2f) -> vec4f {
    let t = p / f32(TILE);
    return vec4f(2.0 * t.x - 1.0, 1.0 - 2.0 * t.y, 0.0, 1.0);
}

// As in stamp.wgsl: the ordered bits of a height, so that unsigned atomics take the least and the greatest.
fn ordered(h: f32) -> u32 {
    let b = bitcast<u32>(h);
    return select(b | 0x80000000u, ~b, (b & 0x80000000u) != 0u);
}

@vertex
fn cover_vertex(v: CoverIn) -> CoverOut {
    let fine = vec2i(vec2u(v.word >> FINE_SHIFT, v.word >> (FINE_SHIFT + 2u)) & vec2u(3u));
    let u = vec2f(v.middle) + vec2f(v.at * 4 + fine) * CORRIDOR_STEP;
    let skirt = (((v.word >> FLAGS_SHIFT) & FLAGS_BITS) & CF_FILL) != 0u;
    return CoverOut(clip_of(u / v.texel + MARGIN), v.word, v.range, v.z, select(0.0, 1.0, skirt));
}

@fragment
fn cover_fragment(in: CoverOut) -> @location(0) vec4f {
    let mat = in.word & MAT_BITS;
    if (mat >= MAT_RAIL || (((in.word >> FLAGS_SHIFT) & FLAGS_BITS) & CF_DECK) != 0u) {
        discard;
    }
    if (in.skirt == 0.0) {
        let pixel = vec2u(in.clip.xy);
        let tile = pixel / TILE;
        let local = pixel % TILE;
        atomicMax(&heights[((tile.y * ROW + tile.x) * TILE + local.y) * TILE + local.x], ~ordered(in.z));
    }
    // Gravel, dirt and a trail fray into the ground, where plants grow over their margins.
    let surface = mat != MAT_GROUND && (mat < LOOK_GRAVEL || mat > LOOK_TRAIL);
    let near = in.range == RANGE_NEAR;
    return vec4f(select(0.0, 1.0, near), select(1.0, 0.0, near), 0.0, select(0.0, 1.0, surface));
}

struct BorderIn {
    // The border's two end points in steps about the piece's middle (borders.ts).
    @location(0) ends: vec4i,
    @location(1) middle: vec2i,
    @location(2) texel: f32,
}

// The corners of a border's band, two faces: per vertex, at the first or the second end, and on the right or the left.
const BAND_END = array(0.0, 1.0, 1.0, 0.0, 1.0, 0.0);
const BAND_SIDE = array(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0);

@vertex
fn border_vertex(@builtin(vertex_index) corner: u32, v: BorderIn) -> @builtin(position) vec4f {
    let a = (vec2f(v.middle) + vec2f(v.ends.xy) * CORRIDOR_STEP) / v.texel + MARGIN;
    let b = (vec2f(v.middle) + vec2f(v.ends.zw) * CORRIDOR_STEP) / v.texel + MARGIN;
    let d = b - a;
    let along = select(vec2f(1.0, 0.0), normalize(d), dot(d, d) > 0.0);
    // And a step more for the rounding of the ends to whole steps.
    let reach = REACH + CORRIDOR_STEP / v.texel;
    let end = BAND_END[corner];
    return clip_of(mix(a - along * reach, b + along * reach, end) + vec2f(-along.y, along.x) * reach * BAND_SIDE[corner]);
}

@fragment
fn border_fragment() -> @location(0) vec4f {
    return vec4f(0.0, 0.0, 1.0, 0.0);
}
