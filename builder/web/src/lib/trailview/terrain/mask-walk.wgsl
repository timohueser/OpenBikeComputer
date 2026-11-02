// The byte of a mask texel that holds what lies past a way's bare surface, unless the texel is a track's: the
// width of the sidewalk on the texel's side, whether the road holds a shoulder, whether the way is trodden, and
// whether the road wears lines. A sidewalk is at most 2 m wide (`sidewalk_m` of data/trailview/roads.json), so
// the width's top value WALK_BARE is free: it marks a way with no corridor on the ground (a gallery, a bridge's
// abutment, a tunnel's portal), whose ground the terrain draws. Then the rule of the ground that a way owns, which
// the stamp pass and the probes of features.wgsl both read.

const WALK_STEP_M: f32 = 0.1;
const WALK_BARE: u32 = 31u;
const MARKED_BIT: u32 = 32u;
const TRODDEN_BIT: u32 = 64u;
const SHOULDER_BIT: u32 = 128u;

fn walk_byte(walk: f32, shoulder: bool, trodden: bool, marked: bool, bare: bool) -> u32 {
    return select(u32(round(walk / WALK_STEP_M)), WALK_BARE, bare) | select(0u, SHOULDER_BIT, shoulder) | select(0u, TRODDEN_BIT, trodden) | select(0u, MARKED_BIT, marked);
}

fn walk_metres(byte: u32) -> f32 {
    return select(f32(byte & WALK_BARE) * WALK_STEP_M, 0.0, is_bare(byte));
}

fn is_bare(byte: u32) -> bool {
    return (byte & WALK_BARE) == WALK_BARE;
}

fn is_trodden(byte: u32) -> bool {
    return (byte & TRODDEN_BIT) != 0u;
}

fn is_marked(byte: u32) -> bool {
    return (byte & MARKED_BIT) != 0u;
}

fn holds_shoulder(byte: u32) -> bool {
    return (byte & SHOULDER_BIT) != 0u;
}

// A way owns its core and this far past it: the widest shoulder (SHOULDER_M of bins.ts, 0.6 m) and a texel, so the
// vertices that the snap moves onto the core's edge drop with the core.
const OWN_MARGIN_M: f32 = 0.8;

// Whether a way's corridor draws the ground at a texel of the mask, and the terrain lies under it: a way of surface
// `code` with a corridor on the ground, out of water and buildings, `past` metres past the carriageway's edge less
// its sidewalk, within OWN_MARGIN_M; or on its cut or fill face; or in a conflict where two footprints meet, such as
// the ramp between two legs of a hairpin, whose skirts both draw it.
fn owns(code: u32, taken: bool, bare: bool, past: f32, faced: bool, conflict: bool) -> bool {
    return code != 0u && !taken && !bare && (past < OWN_MARGIN_M || faced || conflict);
}
