// A slab's kerb distance per pixel, for the pieces' hooks (plan/piece-in.wgsl, plan/mouth-in.wgsl). The corners hold
// the distance to the paved outline (plan/corners.wgsl); inside a slab it is the least of the distances to straight
// kerbs and arcs, so the interpolation between corners is never more than it, and it grows by a metre per metre
// across a kerb. Where the interpolation lies within two kerb tops of the kerb and grows by less than half that, the
// triangle spans two kerbs and the pixel reads the outline itself (outside it: no winding).
// #include ../terrain/outline.wgsl
// The growth of a pixel's interpolated kerb distance per metre across the screen, for slab_across.
fn slab_grade(in: CorridorOut) -> f32 {
    let metres = max(length(dpdx(in.relative)), length(dpdy(in.relative)));
    return max(abs(dpdx(in.road.x)), abs(dpdy(in.road.x))) / max(metres, 1e-6);
}

fn slab_across(u: vec2f, word: u32, across: f32, grade: f32) -> f32 {
    if ((word & MAT_BITS) != MAT_SIDEWALK || across >= 2.0 * KERB_TOP_M || grade >= 0.5) {
        return across;
    }
    let q = (u - vec2f(params.origin - surface.anchor)) * 16.0;
    return m_outline_out(params.block, q, h_scale(params.block, q));
}
