// A line's look in a piece's look word, as `lookOf` of roads/section.ts: the corners (corners.wgsl) and a stop arm's
// mouth (piece.wgsl) read it.
// #include ../terrain/records.wgsl
// #include ../roads/look-codes.wgsl

// As CODE_PATH of ground/roads.wgsl: the drawn code of a concrete path, which has no vehicle tracks.
const LINE_PATH_CODE: u32 = 6u;
// The band's priority field of a look word, which a piece takes for its frame's clamp.
const PIECE_INSIDE: u32 = 65536u;

// The look and the drawn code of a line's finish word; a soil line of a path (M_PATH) wears a trail.
fn line_look(finish: u32) -> vec2u {
    let code = finish & 0xffu;
    let material = (finish >> M_MATERIAL_SHIFT) & 0xffu;
    if ((finish & M_RAIL) != 0u || code >= 9u) {
        return vec2u(LOOK_BALLAST, code);
    }
    if (code == 8u || material == MATERIAL_PAVERS) {
        return vec2u(LOOK_PAVERS, code);
    }
    if (material == MATERIAL_SETT) {
        return vec2u(LOOK_SETT, code);
    }
    if (material == MATERIAL_CONCRETE || material == MATERIAL_STRIPS) {
        return vec2u(select(LOOK_CONCRETE, LOOK_STRIPS, material == MATERIAL_STRIPS), select(code, LINE_PATH_CODE, (finish & M_PATH) != 0u));
    }
    if ((finish & M_MARKED) != 0u) {
        return vec2u(select(LOOK_MAIN, LOOK_MOTORWAY, (finish & M_MOTORWAY) != 0u), code);
    }
    if (code == 3u) {
        return vec2u(LOOK_GRAVEL, code);
    }
    if (code == 4u || code == 5u) {
        return vec2u(select(LOOK_DIRT, LOOK_TRAIL, (finish & M_PATH) != 0u), code);
    }
    if (code == 6u) {
        return vec2u(LOOK_TRAIL, code);
    }
    return vec2u(LOOK_ASPHALT, 2u);
}

// Whether a look is soil: its tread keeps its own edge, with no frame clamp.
fn soil_look(mat: u32) -> bool {
    return mat == LOOK_DIRT || mat == LOOK_TRAIL;
}
