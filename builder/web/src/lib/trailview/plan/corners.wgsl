// The corners of a cell's mesh (pieces.ts), once per cell: each corner's height from the one height function
// (height_at with the corner's label over the ground that the water and the structures stamp), its normal from
// the same function, and the look of its label in the corridors' words: material, surface code, half-width,
// and the metres across and along of its line. A ground corner is ground: it shades as the terrain (corridor.wgsl); its words hold its nearest
// way's half-width and the metres past that way's edge, for the ways' ground terms (ground_terms).
// #include ../terrain/drawn-height.wgsl
// #include ../terrain/model.wgsl
// #include ../terrain/stamp-core.wgsl

// A piece's words: its corners (2 words each, as `mesh/mod.rs` of obc-ground-plan packs them), at `out` the
// corners' height, look word, place word and normal, at `slots` its step slots; as pieces.ts lays them out.
struct Piece {
    // The cell's north-west corner in whole world units.
    origin: vec2i,
    corners: u32,
    slots: u32,
    out: u32,
    steps: u32,
    // The first word of the cell's records block, and the first corner of a dispatch; the top bit of
    // `first`: make only the plain terrain's corners, which read the drawn page.
    block: u32,
    first: u32,
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
// As MAT and LOOK of roads/section.ts, and the surfaces of faces::Surface.
const MAT_GROUND: u32 = 1u;
const LOOK_ASPHALT: u32 = 20u;
const LOOK_MAIN: u32 = 21u;
const LOOK_MOTORWAY: u32 = 22u;
const LOOK_CONCRETE: u32 = 23u;
const LOOK_SETT: u32 = 24u;
const LOOK_STRIPS: u32 = 30u;
// As sections::Material.
const MATERIAL_SETT: u32 = 1u;
const MATERIAL_PAVERS: u32 = 2u;
const MATERIAL_CONCRETE: u32 = 3u;
const MATERIAL_STRIPS: u32 = 4u;
const LOOK_PAVERS: u32 = 25u;
const LOOK_GRAVEL: u32 = 26u;
const LOOK_DIRT: u32 = 27u;
const LOOK_TRAIL: u32 = 28u;
const LOOK_BALLAST: u32 = 29u;
const S_SHOULDER: u32 = 3u;
const S_CORNER_FILL: u32 = 5u;
const S_PLATFORM: u32 = 8u;
const OWNER_JUNCTION: u32 = 2u;
// A soil way this narrow on each side is a path: it wears a trail's tread, as `lookOf` of section.ts.
const PATH_HALF_M: f32 = 1.2;
// A face without a frame has no edge in its look: this far across a way this wide on each side, as a junction's faces in section.ts.
const FACE_ACROSS_M: f32 = 2.0;
const FACE_HALF_M: f32 = 12.0;
// As piece.ts, and the band's priority field of a look word, which a piece takes for its frame's clamp.
const HALF_STEP_M: f32 = 0.025;
const PIECE_INSIDE: u32 = 65536u;
const ACROSS_STEP_M: f32 = 0.0078163;
const ALONG_STEPS: f32 = 64.0;
// As RAIL of the records: a line flag above the finish code.
const LINE_RAIL: u32 = 256u;


// The stamps of the water and the structures at a world point, from the level-18 node that holds it.
fn cell_stamp(u: vec2i) -> Stamp {
    let node = u >> vec2u(12u);
    let at = node - (params.origin >> vec2u(12u)) + vec2i(1);
    let entry = stamp_data[at.y * NODES + at.x];
    return stamp_at(18u, node << vec2u(8u), select(NO_DATA, entry - 1u, entry != 0u), u);
}

fn height_of(q: vec2i, label: u32) -> f32 {
    let u = params.origin + q / 16;
    // The plain terrain's rule: the drawn page, which holds the water's bed and the structures' ground as
    // the terrain beside the mesh draws them.
    if (label == M_NONE) {
        return units_height(u, 18u);
    }
    // Under a label, the stamp's ground of the level-18 page: the bicubic of the stored pages after the water and
    // the structures, which bound the height as in model_height of stamp.wgsl.
    let base = stored_base(u, 18u);
    if (!known(base)) {
        return NONE;
    }
    let s = cell_stamp(u);
    let h = height_at(params.block, vec2f(q), label, stamped_ground(base, s), 0u);
    return stamp_bounded(h.x, h.y == H_PAVED, s);
}

// The metres past the core edge of the way whose core lies nearest q, and its half-width: of the lines whose
// heights reach q's bin. M_FAR and FACE_HALF_M where none does. Water has no way. A bin lists every line whose
// heights reach it, and every class's reach is at least a path's, so within a path's reach (ground_terms) the
// answer does not depend on the bin.
fn nearest_way(q: vec2i, mpu: f32) -> vec2f {
    var out = vec2f(M_FAR, FACE_HALF_M);
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
        if ((finish & 0xffu) == 0u && (finish & LINE_RAIL) == 0u) {
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
            out = vec2f(edge, half);
        }
    }
    return out;
}

// The first word and the count of the centre-line segments of q's bin.
fn bin_segments(q: vec2i) -> vec2u {
    let head = m_bin_head(params.block, vec2f(q));
    if (head == 0u) {
        return vec2u(0u);
    }
    var at = params.block + mw(head);
    for (var item = 0u; item < (mw(head + 1u) >> 16u); item++) {
        at = m_skip_item(at);
    }
    return vec2u(at, mw(head + 1u) & 0xffffu);
}

// Metres across from the middle of the nearest of `lines` and `others` (signed by side), along it and its half-width, from
// its nearest segment in q's bin; across is M_FAR where none lies there.
fn line_frame(q: vec2i, lines: vec4u, others: vec4u, mpu: f32) -> vec3f {
    let block = params.block;
    let segs = bin_segments(q);
    let samples = block + mw(block + 7u);
    let table = block + mw(block + 6u);
    let qf = vec2f(q);
    var best = M_FAR;
    var out = vec3f(M_FAR, 0.0, FACE_HALF_M);
    for (var k = 0u; k < segs.y; k++) {
        let s = mw(segs.x + k);
        let line = s & 0xffffu;
        if (all(lines != vec4u(line)) && all(others != vec4u(line))) {
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

fn look_word(mat: u32, code: u32, half: f32) -> u32 {
    return mat | (code << 7u) | (min(511u, u32(round(max(half, 0.0) / HALF_STEP_M))) << 19u);
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
    return m_paved(params.block, label) || surface_code == S_SHOULDER || surface_code == S_CORNER_FILL || surface_code == S_PLATFORM;
}

// The look word and the place word of a corner with a label. A paved face takes its way's look and a frame:
// a way's carriageway, bed and shoulder their line's, a junction's carriageway its one or two through lines',
// and soil its line's, in a junction the nearest arm's. A junction's corner fill and a platform are one surface without edges. Every other face
// is ground.
fn corner_look(q: vec2i, label: u32, mpu: f32) -> vec2u {
    let ground = vec2u(MAT_GROUND, 0u);
    if (!may_pave(label)) {
        return ground;
    }
    let lw = m_label(params.block, label);
    let w0 = mw(lw);
    let surface_code = w0 & 0xffu;
    let shoulder = surface_code == S_SHOULDER;
    // In a stop arm's mouth a junction's face wears the arm's own look and frame (m_mouth).
    let segs = bin_segments(q);
    let mouth = m_mouth(params.block, label, q, segs.x, segs.y, mpu);
    let junction = ((w0 >> 8u) & 0xffu) == OWNER_JUNCTION && mouth == M_NONE;
    // A junction's region without a material line takes its first through line's.
    var line = select(mw(lw + 2u), mw(lw + 4u), junction && mw(lw + 2u) == M_NONE);
    if (mouth != M_NONE) {
        line = mouth;
    }
    let lines = params.block + mw(params.block + 6u);
    let finish = select(mw(lines + M_LINE * line + 1u), 0u, line == M_NONE);
    let code = finish & 0xffu;
    // A line without a finish is water's: the water draws its bed. A shoulder without a line is ground.
    if ((line != M_NONE && code == 0u && surface_code != S_RAILBED) || (line == M_NONE && shoulder)) {
        return ground;
    }
    var half = FACE_HALF_M;
    if (line != M_NONE) {
        half = mf(lines + M_LINE * line + 11u);
    }
    var mat = LOOK_ASPHALT;
    var drawn = code;
    let material = (finish >> M_MATERIAL_SHIFT) & 0xffu;
    if ((finish & LINE_RAIL) != 0u || code >= 9u || surface_code == S_RAILBED) {
        mat = LOOK_BALLAST;
    } else if (surface_code == S_PLATFORM || code == 8u || material == MATERIAL_PAVERS) {
        mat = LOOK_PAVERS;
    } else if (material == MATERIAL_SETT) {
        mat = LOOK_SETT;
    } else if (material == MATERIAL_CONCRETE || material == MATERIAL_STRIPS) {
        mat = select(LOOK_CONCRETE, LOOK_STRIPS, material == MATERIAL_STRIPS);
    } else if ((finish & M_MARKED) != 0u) {
        mat = select(LOOK_MAIN, LOOK_MOTORWAY, (finish & M_MOTORWAY) != 0u);
    } else if (code == 3u) {
        mat = LOOK_GRAVEL;
    } else if (code == 4u || code == 5u) {
        mat = select(LOOK_DIRT, LOOK_TRAIL, half < PATH_HALF_M);
    } else if (code == 6u) {
        mat = LOOK_TRAIL;
    } else {
        drawn = 2u;
    }
    let uniform = vec2u(look_word(mat, drawn, FACE_HALF_M), place_word(FACE_ACROSS_M, 0.0));
    let soil = mat == LOOK_DIRT || mat == LOOK_TRAIL;
    if (line == M_NONE || surface_code == S_CORNER_FILL || surface_code == S_PLATFORM) {
        return uniform;
    }
    var frame = vec4u(line);
    var others = vec4u(line);
    if (junction) {
        frame = vec4u(mw(lw + 4u), mw(lw + 5u), mw(lw + 6u), mw(lw + 7u));
        // Each soil way keeps its tread through the junction: the nearest of all its arms.
        if (soil) {
            others = vec4u(m_other(lw + 8u), m_other(lw + 9u), m_other(lw + 10u), m_other(lw + 11u));
        } else if (frame.z != M_NONE) {
            // Three through lines or more meet at angles: their nearest changes inside a triangle, and the
            // lines would zigzag across it. The region is one surface then.
            return uniform;
        }
    }
    var f = line_frame(q, frame, others, mpu);
    if (f.x == M_FAR) {
        return uniform;
    }
    // Inside a carriageway its pixels keep the frame within the half-width (corridor.wgsl), so the look draws no
    // verge there; between the corners the frame stays linear, so its lines stay straight.
    let inside = select(PIECE_INSIDE, 0u, soil || shoulder);
    return vec2u(look_word(mat, drawn, f.z) | inside, place_word(f.x, f.y));
}

// A workgroup makes GROUP_CORNERS corners, LANES lanes each: lane 0 the height and the look, lanes 1 to 4 the
// normal's taps, so that a corner's five heights run side by side, not one after another.
const LANES: u32 = 5u;
const GROUP_CORNERS: u32 = 12u;
var<workgroup> corner_heights: array<f32, 64>;

// A dispatch makes the corners from `first` on: a large cell takes a few frames.
@compute @workgroup_size(64)
fn corners(@builtin(local_invocation_index) lane: u32, @builtin(workgroup_id) group: vec3u) {
    let k = (params.first & 0x7fffffffu) + group.x * GROUP_CORNERS + lane / LANES;
    let tap = lane % LANES;
    let terrain_only = (params.first >> 31u) == 1u;
    let w0 = piece[2u * min(k, params.corners - 1u)];
    let w1 = piece[2u * min(k, params.corners - 1u) + 1u];
    // A terrain vertex draws with the terrain's vertex code, as ground.
    let makes = lane < LANES * GROUP_CORNERS && k < params.corners && (w1 >> 31u) == 0u;
    let q = vec2i(i32(w0 & 0x7ffffu), i32(w1 & 0x7ffffu));
    var label = (w0 >> 19u) | (((w1 >> 19u) & 0xfffu) << 13u);
    if (label == NO_LABEL) {
        label = M_NONE;
    }
    let mpu = h_scale(params.block, vec2f(q));
    // Frame y runs south: the taps east, west, north, south. They stay in the cell: past its border the
    // block holds no records, and a paved label there is the plain terrain.
    let step = i32(max(1.0, round(NORMAL_STEP_M / mpu)));
    var taps = array(vec2i(0), vec2i(step, 0), vec2i(-step, 0), vec2i(0, -step), vec2i(0, step));
    let cell = vec2i(i32(M_CELL));
    var h = NONE;
    if (makes && (tap == 0u || (!terrain_only && may_pave(label))) && (!terrain_only || label == M_NONE)) {
        h = height_of(clamp(q + taps[tap], vec2i(0), cell), label);
    }
    corner_heights[lane] = h;
    workgroupBarrier();
    if (lane >= LANES * GROUP_CORNERS || k >= params.corners || tap != 0u) {
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
        // Ground: its nearest way, for the ground terms.
        let way = nearest_way(q, mpu);
        look = vec2u(look_word(MAT_GROUND, 0u, way.y), place_word(way.x, 0.0));
    }
    // Octahedral, as piece.wgsl decodes it.
    let e = n.xy / (abs(n.x) + abs(n.y) + n.z);
    piece[o] = bitcast<u32>(h);
    piece[o + 1u] = look.x;
    piece[o + 2u] = look.y;
    piece[o + 3u] = pack2x16snorm(e);
}
