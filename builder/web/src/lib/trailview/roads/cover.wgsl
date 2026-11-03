// The cover of the corridors, for the snap pass of terrain/stamp.wgsl: the near faces of the L16 pieces that draw, seen
// from above into one tile per mask page, the page and its margin. A texel is covered where its centre lies in a face
// on the ground; the second channel marks a face of a way's surface, and the depth holds the least height of the faces
// that are no skirt's rows, whose heights only the stamped ground gives. Decks and details give no cover.

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
// As COVER_TILE and COVER_MARGIN of cover.ts.
const TILE: f32 = 272.0;
const MARGIN: f32 = 8.0;
// The depth of a height: metres from LOWEST_M over SPAN_M, about 0.6 mm per step of the depth's float.
const LOWEST_M: f32 = -1000.0;
const SPAN_M: f32 = 10000.0;

struct CoverIn {
    // As CorridorIn of corridor.wgsl.
    @location(0) at: vec2i,
    @location(1) z: f32,
    @location(2) word: u32,
    // The piece's middle less the page's corner, in world units, and the world units of a texel of the page.
    @location(3) middle: vec2i,
    @location(4) texel: f32,
}

struct CoverOut {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(flat) word: u32,
}

@vertex
fn cover_vertex(v: CoverIn) -> CoverOut {
    let fine = vec2i(vec2u(v.word >> FINE_SHIFT, v.word >> (FINE_SHIFT + 2u)) & vec2u(3u));
    let u = vec2f(v.middle) + vec2f(v.at * 4 + fine) * CORRIDOR_STEP;
    // Texel rows run along +y of world units, pixel rows down the tile.
    let t = (u / v.texel + MARGIN) / TILE;
    let skirt = (((v.word >> FLAGS_SHIFT) & FLAGS_BITS) & CF_FILL) != 0u;
    let depth = select(clamp((v.z - LOWEST_M) / SPAN_M, 0.0, 1.0), 1.0, skirt);
    return CoverOut(vec4f(2.0 * t.x - 1.0, 1.0 - 2.0 * t.y, depth, 1.0), v.word);
}

@fragment
fn cover_fragment(in: CoverOut) -> @location(0) vec4f {
    let mat = in.word & MAT_BITS;
    if (mat >= MAT_RAIL || (((in.word >> FLAGS_SHIFT) & FLAGS_BITS) & CF_DECK) != 0u) {
        discard;
    }
    // Gravel, dirt and a trail fray into the ground, where plants grow over their margins.
    let surface = mat != MAT_GROUND && (mat < LOOK_GRAVEL || mat > LOOK_TRAIL);
    return vec4f(1.0, select(0.0, 1.0, surface), 0.0, 1.0);
}
