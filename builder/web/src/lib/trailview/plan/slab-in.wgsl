// A slab's kerb distance per pixel, for the pieces' hooks (plan/piece-in.wgsl, plan/mouth-in.wgsl). The corners hold
// the distance to the paved outline (plan/corners.wgsl); inside a slab it is the least of the distances to straight
// kerbs and arcs, so the interpolation between corners is never more than it. Wherever the interpolation lies within
// the kerb top and two pixels, the pixel reads the outline's chords itself (m_outline_out): no triangle draws the kerb
// top from a distance that is too small by more than a round's sagitta.
// #include ../terrain/outline.wgsl
// The metres that a pixel spans on the ground, for slab_across.
fn slab_pixel(in: CorridorOut) -> f32 {
    return max(length(dpdx(in.relative)), length(dpdy(in.relative)));
}

fn slab_across(u: vec2f, word: u32, across: f32, pixel: f32) -> f32 {
    let top = KERB_TOP_M + 2.0 * pixel;
    if ((word & MAT_BITS) != MAT_SIDEWALK || across >= top) {
        return across;
    }
    let q = (u - vec2f(params.origin - surface.anchor)) * 16.0;
    return m_outline_out(params.block, q, h_scale(params.block, q));
}
