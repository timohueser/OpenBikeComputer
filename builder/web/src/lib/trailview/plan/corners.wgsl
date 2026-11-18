// The corners of a cell's mesh (pieces.ts), once per cell: each corner's height from the one height function
// (height_at with the corner's label over the ground that the water and the structures stamp), its normal from
// the same function, and the look of its label in the corridors' words: material, surface code, half-width,
// and the metres across and along of its line. A ground corner is ground: it shades as the terrain (corridor.wgsl); its words hold its nearest
// way's half-width and the metres past that way's edge, for the ways' ground terms (ground_terms).
// #include ../terrain/drawn-height.wgsl
// #include ../terrain/model.wgsl
// #include ../terrain/stamp-core.wgsl
// #include line-look.wgsl

// A piece's words: its corners (2 words each, as `mesh/mod.rs` of obc-ground-plan packs them), at `out` the
// corners' height, look word, place word and normal, at `slots` its step slots; as pieces.ts lays them out. A
// storey mesh's corners take their offsets (`mesh/shell.rs`).
struct Piece {
    // The cell's north-west corner in whole world units.
    origin: vec2i,
    corners: u32,
    slots: u32,
    out: u32,
    // The first step slot of the end faces (the mesh's last slots).
    faces: u32,
    // The first word of the cell's records block, and the first corner of a dispatch; the top bit of
    // `first`: make only the plain terrain's corners, which read the drawn page.
    block: u32,
    first: u32,
    // The first word of the culvert mouths (piece.wgsl).
    culverts: u32,
    // A storey mesh's: the first word of its corners' height offsets (f32 metres; 0 for a cell's mesh), of its
    // triangles (three corners each) and the count of its top's triangles, the first triangles of its railings and of
    // its piers (piece.wgsl), the first word and the count of its caps (3 words each, sorted by corner), and of its
    // bores' lights (3 words each: a way, its metres along and the metres from there to a portal, `mesh/shell.rs`).
    offsets: u32,
    tris: u32,
    top: u32,
    railings: u32,
    piers: u32,
    caps: u32,
    cap_count: u32,
    lights: u32,
    light_count: u32,
}

@group(0) @binding(0) var<storage, read_write> piece: array<u32>;
@group(0) @binding(2) var<uniform> params: Piece;
// `stamp_data` starts with a table of NODES × NODES level-18 nodes, row by row from one node north-west of the
// cell's first: the offset + 1 of each node's stamp data, 0 for none. Every point that a corner or a normal tap
// reads lies in the node that holds it, as in the neighbour cell, so two cells give a shared corner one height.
const NODES: i32 = 6;

// As NO_LABEL of the mesh: the plain terrain.
const NO_LABEL: u32 = 33554431u;
// The normal is the function's slope over this many metres each side.
const NORMAL_STEP_M: f32 = 0.1;
// As MAT of roads/section.ts, and the surfaces of faces::Surface.
const MAT_GROUND: u32 = 1u;
const MAT_WATER: u32 = 50u;
const S_SHOULDER: u32 = 3u;
const S_WATER: u32 = 9u;
const S_ZONE: u32 = 11u;
const S_SIDEWALK: u32 = 14u;
// As MAT.SIDEWALK of roads/section.ts.
const MAT_SIDEWALK: u32 = 3u;
// The code of a ground-looking shoulder without a line, until its nearest way gives its class.
const CODE_ROAD_SURFACE: u32 = 1u;
// In a corner's look word, the bit of the corridors' fine offset (1 << FINE_SHIFT), which pieces do not read:
// the face lies on the road surface of a wall class (the line's M_WALL, from `leg_wall` of roads.json), so a step
// beside it is a wall (piece.wgsl's WALL_SIDE).
const LOOK_WALL_SIDE: u32 = 268435456u;
// The next bit: that wall is a bridge's (M_BRIDGE_GROUND), of concrete (piece.wgsl's BRIDGE_SIDE). A storey's corner
// has no wall: it takes these bits for its deck's build and planks (M_BUILD_SHIFT), in the bits from DECK_SHIFT.
const LOOK_BRIDGE_SIDE: u32 = 536870912u;
// The next bit, of a cell mesh's corner: its face is a bore's cover, so an end face on a portal line beside it is the
// tunnel's end wall where the cover is its higher side (piece.wgsl's COVER_SIDE).
const LOOK_COVER: u32 = 1073741824u;
const SIDE_SHIFT: u32 = 28u;
const DECK_SHIFT: u32 = 29u;
const S_CORNER_FILL: u32 = 5u;
const S_PLATFORM: u32 = 8u;
// A face without a frame has no edge in its look: this far across a way this wide on each side, as a junction's faces in section.ts.
const FACE_ACROSS_M: f32 = 2.0;
const FACE_HALF_M: f32 = 12.0;
// As piece.ts.
const ACROSS_STEP_M: f32 = 0.0078163;
const ALONG_STEPS: f32 = 64.0;


// The stamps of the water and the structures at a world point, from the level-18 node that holds it.
fn cell_stamp(u: vec2i) -> Stamp {
    let node = u >> vec2u(12u);
    let at = node - (params.origin >> vec2u(12u)) + vec2i(1);
    let entry = stamp_data[at.y * NODES + at.x];
    return stamp_at(18u, node << vec2u(8u), select(NO_DATA, entry - 1u, entry != 0u), u);
}

// A storey's corner lies on no ground: no stamp bounds it. Its plain terrain (a pier's foot) is the stamp's ground,
// which reads no drawn page: the storeys' corners are made in `all` passes only.
fn height_of(q: vec2i, label: u32, storey: bool) -> f32 {
    let u = params.origin + q / 16;
    // The plain terrain's rule: the drawn page, which holds the water's bed and the structures' ground as
    // the terrain beside the mesh draws them.
    if (label == M_NONE && !storey) {
        return units_height(u, 18u);
    }
    // Under a label, the stamp's ground of the level-18 page: the bicubic of the stored pages after the water and
    // the structures, which bound the height as in model_height of stamp.wgsl.
    let base = stored_base(u, 18u);
    if (!known(base)) {
        return NONE;
    }
    let s = cell_stamp(u);
    if (label == M_NONE) {
        return stamped_ground(base, s);
    }
    let h = height_at(params.block, vec2f(q), label, stamped_ground(base, s), 0u);
    return select(stamp_bounded(h.x, h.y == H_PAVED, s), h.x, storey);
}

// The bit of `first` that marks the caps pass: before a storey's corners, each cap's height at its corner's
// point with its offset, into the word after the caps (one call of height_of in the shader, so the driver inlines the
// heights once).
const CAPS_BIT: u32 = 0x40000000u;

// The first cap of storey corner k: a lower bound over the caps, which lie sorted by corner (mesh/shell.rs).
fn first_cap(k: u32) -> u32 {
    var lo = 0u;
    var hi = params.cap_count;
    loop {
        if (lo >= hi) {
            break;
        }
        let mid = (lo + hi) / 2u;
        if (piece[params.caps + 3u * mid] < k) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    return lo;
}

// A bore corner's metres to daylight, in the place of its normal (piece.wgsl's storey_vertex): the least over its way's
// lights of the metres along between the two plus the light's metres, M_FAR for none. Its metres along are those at its
// foot on its label's line, `line` (the bore's, or in front of a portal line the ground way's), as the crate's shell reads
// them (`Lines::foot`): a bore is short.
fn daylight_metres(q: vec2i, line: u32) -> f32 {
    if (line == M_NONE) {
        return M_FAR;
    }
    let lw = params.block + mw(params.block + 6u) + M_LINE * line;
    let first = params.block + mw(params.block + 7u) + M_SAMPLE * mw(lw + 2u);
    let qf = vec2f(q);
    var best = M_FAR;
    var along = 0.0;
    for (var i = 0u; i + 1u < mw(lw + 3u); i++) {
        let wa = first + M_SAMPLE * i;
        let a = vec2f(m_vertex(wa));
        let u = vec2f(m_vertex(wa + M_SAMPLE)) - a;
        let t = clamp(dot(qf - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
        let d = length(qf - a - t * u);
        if (d < best) {
            best = d;
            along = mix(mf(wa + 3u), mf(wa + M_SAMPLE + 3u), t);
        }
    }
    let way = mw(lw);
    var metres = M_FAR;
    for (var j = 0u; j < params.light_count; j++) {
        let at = params.lights + 3u * j;
        if (piece[at] == way) {
            metres = min(metres, abs(along - bitcast<f32>(piece[at + 1u])) + bitcast<f32>(piece[at + 2u]));
        }
    }
    return metres;
}

// Whether storey corner k has a cap: only a bore's corners have, its floor's and its roof's (`mesh/shell.rs`).
fn capped(k: u32) -> bool {
    let j = first_cap(k);
    return j < params.cap_count && piece[params.caps + 3u * j] == k;
}

// Whether a line's finish word is of a wall class: the one flag that heights reads for a wall at its edge.
fn road_wall(finish: u32) -> bool {
    return (finish & M_WALL) != 0u;
}

// The bits of a wall beside a line of finish word `finish`: none, or LOOK_WALL_SIDE for a wall class (road_wall), with
// LOOK_BRIDGE_SIDE on a bridge's ground stretch.
fn wall_side(finish: u32) -> u32 {
    return select(0u, LOOK_WALL_SIDE | select(0u, LOOK_BRIDGE_SIDE, (finish & M_BRIDGE_GROUND) != 0u), road_wall(finish));
}

// The metres past the core edge of the way whose core lies nearest q, its half-width, and its wall_side bits (from
// SIDE_SHIFT): of the lines whose heights reach q's bin, but water. M_FAR and FACE_HALF_M where none does. A bin
// lists every line whose heights reach it, and every class's reach is at least a path's, so within a path's reach
// (ground_terms) the answer does not depend on the bin.
fn nearest_way(q: vec2i, mpu: f32) -> vec3f {
    var out = vec3f(M_FAR, FACE_HALF_M, 0.0);
    let qf = vec2f(q);
    let head = m_bin_head(params.block, qf);
    if (head == 0u) {
        return out;
    }
    let at = h_heights_start(params.block, qf);
    let lines = params.block + mw(params.block + 6u);
    let samples = params.block + mw(params.block + 7u);
    for (var k = 0u; k < (mw(head + 2u) >> 16u); k++) {
        let item = mw(at + k);
        let line = item & 0xffffu;
        let finish = mw(lines + M_LINE * line + 1u);
        if ((finish & (M_WATER | M_CEILING)) != 0u) {
            continue;
        }
        let wa = samples + M_SAMPLE * (mw(lines + M_LINE * line + 2u) + (item >> 16u));
        let wb = wa + M_SAMPLE;
        let a = vec2f(m_vertex(wa));
        let u = vec2f(m_vertex(wb)) - a;
        let t = clamp(dot(qf - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
        let half = mix(mf(wa + 2u), mf(wb + 2u), t) * mpu;
        let edge = length(qf - a - t * u) * mpu - half;
        if (edge < out.x) {
            out = vec3f(edge, half, f32(wall_side(finish) >> SIDE_SHIFT));
        }
    }
    return out;
}

// Metres across from the middle of `line` (signed by side), along it and its half-width, from its nearest segment in
// q's bin; across is M_FAR where none lies there.
fn line_frame(q: vec2i, line: u32, mpu: f32) -> vec3f {
    let block = params.block;
    let segs = m_bin_segments(block, vec2f(q));
    let samples = block + mw(block + 7u);
    let table = block + mw(block + 6u);
    let qf = vec2f(q);
    var best = M_FAR;
    var out = vec3f(M_FAR, 0.0, FACE_HALF_M);
    for (var k = 0u; k < segs.y; k++) {
        let s = mw(segs.x + k);
        if ((s & 0xffffu) != line) {
            continue;
        }
        let wa = samples + M_SAMPLE * (mw(table + M_LINE * line + 2u) + (s >> 16u));
        let wb = wa + M_SAMPLE;
        let a = vec2f(m_vertex(wa));
        let u = vec2f(m_vertex(wb)) - a;
        let t = clamp(dot(qf - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
        let d = length(qf - a - t * u);
        if (d < best) {
            best = d;
            out = vec3f(sign(m_cross(u, qf - a)) * d * mpu, mix(mf(wa + 3u), mf(wb + 3u), t), mix(mf(wa + 2u), mf(wb + 2u), t) * mpu);
        }
    }
    return out;
}

fn place_word(across: f32, along: f32) -> u32 {
    return (bitcast<u32>(i32(round(clamp(across, -16.0, 16.0) / ACROSS_STEP_M))) & 0xfffu) | (u32(clamp(round(along * ALONG_STEPS), 0.0, 1048575.0)) << 12u);
}

// Whether a label's face can take a look other than the ground's: only such a face needs the function's normal.
fn may_pave(label: u32) -> bool {
    if (label == M_NONE) {
        return false;
    }
    let surface_code = mw(m_label(params.block, label)) & 0xffu;
    return m_paved(params.block, label) || surface_code == S_SHOULDER || surface_code == S_CORNER_FILL || surface_code == S_PLATFORM || surface_code == S_WATER || surface_code == S_SIDEWALK;
}

// The look and place words of a sidewalk corner on host line `line`: paving, the slab's width there as its half, and
// its frame: the metres from its kerb (the paved outline's edge: the carriageways that rank before the sidewalks) and
// the host's metres along.
fn sidewalk_look(q: vec2i, line: u32, wall: u32, mpu: f32) -> vec2u {
    let block = params.block;
    let row = h_walk_row(block, line);
    let w = select(0.0, h_walk_width(block, row, vec2f(q), mpu, false), row != 0u);
    let d = m_outline_metres(block, q, mpu);
    return vec2u(look_word(MAT_SIDEWALK, 0u, w) | wall, place_word(max(d, 0.0), line_frame(q, line, mpu).y));
}

// The look word and the place word of a corner with a label. A paved face takes its way's look and a frame:
// a way's carriageway, bed and shoulder their line's, a junction's carriageway its one or two through lines',
// and soil its line's, in a junction the nearest arm's (from the junction's record). A junction's corner fill and a
// platform are one surface without edges. A water face is water. Every other face is ground.
fn corner_look(q: vec2i, label: u32, mpu: f32) -> vec2u {
    let ground = vec2u(MAT_GROUND, 0u);
    // A culvert's zone is ground, though its material is its way's line.
    if (label != M_NONE && (mw(m_label(params.block, label)) & 0xffu) == S_ZONE) {
        return ground;
    }
    if (!may_pave(label)) {
        return ground;
    }
    let lw = m_label(params.block, label);
    let w0 = mw(lw);
    let surface_code = w0 & 0xffu;
    let shoulder = surface_code == S_SHOULDER;
    let line = mw(lw + 2u);
    let lines = params.block + mw(params.block + 6u);
    let finish = select(mw(lines + M_LINE * line + 1u), 0u, line == M_NONE);
    let code = finish & 0xffu;
    let wall = wall_side(finish);
    if (surface_code == S_WATER) {
        return vec2u(look_word(MAT_WATER, 0u, 0.0), 0u);
    }
    if (surface_code == S_SIDEWALK && line != M_NONE) {
        return sidewalk_look(q, line, wall, mpu);
    }
    // A shoulder without a line looks like ground; its nearest way decides whether a step beside it is a wall.
    if (line == M_NONE && shoulder) {
        return vec2u(look_word(MAT_GROUND, CODE_ROAD_SURFACE, 0.0), 0u);
    }
    if (line != M_NONE && code == 0u && surface_code != S_RAILBED) {
        return ground;
    }
    var look = line_look(finish);
    if (surface_code == S_RAILBED) {
        look = vec2u(LOOK_BALLAST, code);
    } else if (surface_code == S_PLATFORM && look.x != LOOK_BALLAST) {
        look = vec2u(LOOK_PAVERS, code);
    }
    let mat = look.x;
    let drawn = look.y;
    let uniform = vec2u(look_word(mat, drawn, FACE_HALF_M) | wall, place_word(FACE_ACROSS_M, 0.0));
    let soil = soil_look(mat);
    if (line == M_NONE || surface_code == S_CORNER_FILL || surface_code == S_PLATFORM) {
        return uniform;
    }
    let jr = m_junction(params.block, label);
    var f: vec3f;
    if (jr == M_NONE) {
        f = line_frame(q, line, mpu);
    } else {
        // Each soil way keeps its tread through the junction: the nearest of all its arms.
        f = m_junction_frame(params.block, jr, vec2f(q), m_arms_with(jr, select(M_ARM_HOST, 0u, soil)), line, mpu).place;
    }
    if (f.x == M_FAR) {
        return uniform;
    }
    // Inside a carriageway its pixels keep the frame within the half-width (corridor.wgsl), so the look draws no
    // verge there; between the corners the frame stays linear, so its lines stay straight.
    let inside = select(PIECE_INSIDE, 0u, soil || shoulder);
    return vec2u(look_word(mat, drawn, f.z) | inside | wall, place_word(f.x, f.y));
}

// A workgroup makes GROUP_CORNERS corners, LANES lanes each: lane 0 the height and the look, lanes 1 to 4 the
// normal's taps, so that a corner's five heights run side by side, not one after another.
const LANES: u32 = 5u;
const GROUP_CORNERS: u32 = 12u;
var<workgroup> corner_heights: array<f32, 64>;

// A dispatch makes the corners from `first` on: a large cell takes a few frames.
@compute @workgroup_size(64)
fn corners(@builtin(local_invocation_index) lane: u32, @builtin(workgroup_id) group: vec3u) {
    let k = (params.first & 0x3fffffffu) + group.x * GROUP_CORNERS + lane / LANES;
    let tap = lane % LANES;
    let terrain_only = (params.first >> 31u) == 1u;
    // The caps pass (CAPS_BIT): item k is a cap, which reads its label at its corner's point.
    let caps_pass = (params.first & CAPS_BIT) != 0u;
    let corner = select(min(k, params.corners - 1u), piece[params.caps + 3u * min(k, max(params.cap_count, 1u) - 1u)], caps_pass);
    let w0 = piece[2u * corner];
    let w1 = piece[2u * corner + 1u];
    // A terrain vertex draws with the terrain's vertex code, as ground.
    let makes = lane < LANES * GROUP_CORNERS && k < select(params.corners, params.cap_count, caps_pass) && (w1 >> 31u) == 0u;
    let q = vec2i(i32(w0 & 0x7ffffu), i32(w1 & 0x7ffffu));
    var label = (w0 >> 19u) | (((w1 >> 19u) & 0xfffu) << 13u);
    // A cap reads its label as storey 0's corner of that label reads it, so a bore's roof stays its plate under the
    // ground as drawn; a cap of the plain terrain reads as the storey's own corners do.
    var storey = params.offsets != 0u;
    if (caps_pass) {
        label = piece[params.caps + 3u * k + 1u];
        storey = label == NO_LABEL;
    }
    if (label == NO_LABEL) {
        label = M_NONE;
    }
    let mpu = h_scale(params.block, vec2f(q));
    // Frame y runs south: the taps east, west, north, south. They stay in the cell: past its border the
    // block holds no records, and a paved label there is the plain terrain.
    let step = i32(max(1.0, round(NORMAL_STEP_M / mpu)));
    var taps = array(vec2i(0), vec2i(step, 0), vec2i(-step, 0), vec2i(0, -step), vec2i(0, step));
    let cell = vec2i(i32(M_CELL));
    // A bore's corner needs no normal: no sun reaches it.
    let bore = makes && !terrain_only && !caps_pass && params.offsets != 0u && capped(k);
    var h = NONE;
    if (makes && (tap == 0u || (!terrain_only && !caps_pass && may_pave(label) && !bore)) && (!terrain_only || label == M_NONE)) {
        h = height_of(clamp(q + taps[tap], vec2i(0), cell), label, storey);
    }
    corner_heights[lane] = h;
    workgroupBarrier();
    if (lane >= LANES * GROUP_CORNERS || k >= select(params.corners, params.cap_count, caps_pass) || tap != 0u) {
        return;
    }
    // A cap's height with its offset, after the caps, for the storey's corners pass.
    if (caps_pass) {
        piece[params.caps + 3u * params.cap_count + k] = bitcast<u32>(select(NONE, h + bitcast<f32>(piece[params.caps + 3u * k + 2u]), known(h)));
        return;
    }
    let o = params.out + 4u * k;
    if (!makes) {
        // A terrain vertex: ground, with its nearest way.
        let way = nearest_way(q, mpu);
        piece[o] = bitcast<u32>(NONE);
        piece[o + 1u] = look_word(MAT_GROUND, 0u, way.y);
        piece[o + 2u] = place_word(way.x, 0.0);
        piece[o + 3u] = 0u;
        return;
    }
    if (terrain_only) {
        if (label == M_NONE) {
            piece[o] = bitcast<u32>(h);
        }
        return;
    }
    var look = corner_look(q, label, mpu);
    // Ground takes the pages' normal (corridor.wgsl); a paved face the function's, over NORMAL_STEP_M each side.
    var n = vec3f(0.0, 0.0, 1.0);
    if ((look.x & 127u) != MAT_GROUND) {
        let low = max(q - vec2i(step), vec2i(0));
        let high = min(q + vec2i(step), cell);
        let metres = vec2f(high - low) * mpu;
        let east = (corner_heights[lane + 1u] - corner_heights[lane + 2u]) / metres.x;
        let north = (corner_heights[lane + 3u] - corner_heights[lane + 4u]) / metres.y;
        if (known(h) && abs(east) < 1e3 && abs(north) < 1e3) {
            n = normalize(vec3f(-east, -north, 1.0));
        }
    } else {
        // Ground: its nearest way, for the ground terms; a shoulder takes that way's wall class.
        let way = nearest_way(q, mpu);
        let side = select(0u, u32(way.z) << SIDE_SHIFT, ((look.x >> 7u) & 15u) == CODE_ROAD_SURFACE);
        let cover = params.offsets == 0u && label != M_NONE && (mw(m_label(params.block, label)) & 0xffu) == M_SURFACE_COVER;
        look = vec2u(look_word(MAT_GROUND, 0u, way.y) | side | select(0u, LOOK_COVER, cover), place_word(way.x, 0.0));
    }
    // A storey's corner: its deck's build and planks, from its label's line.
    if (params.offsets != 0u && label != M_NONE) {
        let line = mw(m_label(params.block, label) + 2u);
        let finish = select(mw(params.block + mw(params.block + 6u) + M_LINE * line + 1u), 0u, line == M_NONE);
        look.x = (look.x & ~(7u << DECK_SHIFT)) | (((finish >> M_BUILD_SHIFT) & 7u) << DECK_SHIFT);
    }
    // Octahedral, as piece.wgsl decodes it.
    let e = n.xy / (abs(n.x) + abs(n.y) + n.z);
    if (params.offsets != 0u && known(h)) {
        h += bitcast<f32>(piece[params.offsets + k]);
    }
    // The least of a storey corner's height and its caps' (mesh/shell.rs's StoreyMesh::corner_height), which the caps
    // pass made; NONE where one is unknown.
    for (var j = select(params.cap_count, first_cap(k), known(h) && params.cap_count != 0u); j < params.cap_count && piece[params.caps + 3u * j] == k; j++) {
        let c = bitcast<f32>(piece[params.caps + 3u * params.cap_count + j]);
        h = select(NONE, min(h, c), known(c) && known(h));
    }
    piece[o] = bitcast<u32>(h);
    piece[o + 1u] = look.x;
    piece[o + 2u] = look.y;
    var last = pack2x16snorm(e);
    if (bore) {
        last = bitcast<u32>(daylight_metres(q, select(mw(m_label(params.block, label) + 2u), M_NONE, label == M_NONE)));
    }
    piece[o + 3u] = last;
}
