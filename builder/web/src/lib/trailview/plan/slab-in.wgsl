// A slab's kerb distance per pixel, for the pieces' hooks (plan/piece-in.wgsl, plan/mouth-in.wgsl). The corners hold
// the distance to the paved outline (plan/corners.wgsl); inside a slab it is the least of the distances to straight
// kerbs and arcs, so the interpolation between corners is never more than it. Where the interpolation lies within
// two kerb tops of the kerb, the pixel reads the outline itself: a triangle with its three corners on two kerbs
// draws its kerb tops and its paving.
// #include ../terrain/outline.wgsl

fn slab_across(u: vec2f, word: u32, across: f32) -> f32 {
    if ((word & MAT_BITS) != MAT_SIDEWALK || across >= 2.0 * KERB_TOP_M) {
        return across;
    }
    let q = (u - vec2f(params.origin - surface.anchor)) * 16.0;
    return max(m_outline_metres(params.block, vec2i(round(q)), h_scale(params.block, q)), 0.0);
}
