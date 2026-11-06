// The corners of a cell's mesh (pieces.ts), once per cell: each corner's height from the one height function
// (height_at with the corner's label over the plain terrain), its normal from the same function, and the look
// of its label in the corridors' words: material, surface code, half-width, and the metres across and along
// of its line. A ground corner is ground: it shades as the terrain (corridor.wgsl).
// #include ../terrain/drawn-height.wgsl
// #include ../terrain/model.wgsl

// A piece's words: its corners (2 words each, as `mesh/mod.rs` of obc-ground-plan packs them), at `out` the
// corners' height, look word, place word and normal, at `slots` its step slots; as pieces.ts lays them out.
struct Piece {
    // The cell's north-west corner in whole world units.
    origin: vec2i,
    corners: u32,
    slots: u32,
    out: u32,
    steps: u32,
    // The first word of the cell's records block.
    block: u32,
    unused: u32,
}

@group(0) @binding(0) var<storage, read_write> piece: array<u32>;
@group(0) @binding(1) var<uniform> params: Piece;

// As NO_LABEL of the mesh: the plain terrain.
const NO_LABEL: u32 = 33554431u;
// The normal is the function's slope over this many metres each side.
const NORMAL_STEP_M: f32 = 0.1;
// As MAT and LOOK of roads/section.ts, and the surfaces of faces::Surface.
const MAT_GROUND: u32 = 1u;
const LOOK_ASPHALT: u32 = 20u;
const LOOK_MAIN: u32 = 21u;
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


// The plain terrain at q (frame units from the cell's origin): the bicubic of the stored pages that the
// level-18 pages are made of, as the stamp's ground.
fn plain_at(q: vec2i) -> f32 {
    return stored_base(params.origin + q / 16, 18u);
}

fn height_of(q: vec2i, label: u32) -> f32 {
    let t = plain_at(q);
    if (!known(t)) {
        return NONE;
    }
    return height_at(params.block, vec2f(q), label, t, 0u).x;
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

// Metres across from the middle of the nearest of `lines` (signed by side), along it and its half-width, from
// its nearest segment in q's bin; across is M_FAR where none lies there.
fn line_frame(q: vec2i, lines: vec4u, mpu: f32) -> vec3f {
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
        if (all(lines != vec4u(line))) {
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

// The look word and the place word of a corner with a label. A paved face takes its way's look and a frame:
// a way's carriageway, bed and shoulder their line's, a junction's carriageway its one or two through lines',
// and soil its line's. A junction's corner fill and a platform are one surface without edges. Every other face
// is ground.
fn corner_look(q: vec2i, label: u32, mpu: f32) -> vec2u {
    let ground = vec2u(MAT_GROUND, 0u);
    if (label == M_NONE) {
        return ground;
    }
    let lw = m_label(params.block, label);
    let w0 = mw(lw);
    let surface_code = w0 & 0xffu;
    let shoulder = surface_code == S_SHOULDER;
    if (!(m_paved(params.block, label) || shoulder || surface_code == S_CORNER_FILL || surface_code == S_PLATFORM)) {
        return ground;
    }
    let junction = ((w0 >> 8u) & 0xffu) == OWNER_JUNCTION;
    // A junction's region without a material line takes its first through line's.
    let line = select(mw(lw + 2u), mw(lw + 4u), junction && mw(lw + 2u) == M_NONE);
    let lines = params.block + mw(params.block + 6u);
    let finish = select(mw(lines + M_LINE * line + 1u), 0u, line == M_NONE);
    let code = finish & 0xffu;
    // A line without a finish is water's: the water draws its bed.
    if (line != M_NONE && code == 0u && surface_code != S_RAILBED) {
        return ground;
    }
    var half = FACE_HALF_M;
    if (line != M_NONE) {
        half = mf(lines + M_LINE * line + 11u);
    }
    var mat = LOOK_ASPHALT;
    var drawn = code;
    if ((finish & LINE_RAIL) != 0u || code >= 9u || surface_code == S_RAILBED) {
        mat = LOOK_BALLAST;
    } else if (surface_code == S_PLATFORM || code == 8u) {
        mat = LOOK_PAVERS;
    } else if (code == 1u || code == 7u) {
        mat = LOOK_MAIN;
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
    if (junction && !soil) {
        frame = vec4u(mw(lw + 4u), mw(lw + 5u), mw(lw + 6u), mw(lw + 7u));
        // Three through lines or more meet at angles: their nearest changes inside a triangle, and the lines
        // would zigzag across it. The region is one surface then.
        if (frame.z != M_NONE) {
            return uniform;
        }
    }
    var f = line_frame(q, frame, mpu);
    if (f.x == M_FAR) {
        return uniform;
    }
    // Inside a carriageway its pixels keep the frame within the half-width (corridor.wgsl), so the look draws no
    // verge there; between the corners the frame stays linear, so its lines stay straight.
    let inside = select(PIECE_INSIDE, 0u, soil || shoulder);
    return vec2u(look_word(mat, drawn, f.z) | inside, place_word(f.x, f.y));
}

@compute @workgroup_size(64)
fn corners(@builtin(global_invocation_id) id: vec3u) {
    let k = id.x;
    if (k >= params.corners) {
        return;
    }
    let w0 = piece[2u * k];
    let w1 = piece[2u * k + 1u];
    let q = vec2i(i32(w0 & 0x7ffffu), i32(w1 & 0x7ffffu));
    var label = (w0 >> 19u) | (((w1 >> 19u) & 0xfffu) << 13u);
    if (label == NO_LABEL) {
        label = M_NONE;
    }
    let mpu = h_scale(params.block, vec2f(q));
    let h = height_of(q, label);
    // Frame y runs south: north is -y.
    let step = i32(max(1.0, round(NORMAL_STEP_M / mpu)));
    let metres = 2.0 * f32(step) * mpu;
    let east = (height_of(q + vec2i(step, 0), label) - height_of(q - vec2i(step, 0), label)) / metres;
    let north = (height_of(q - vec2i(0, step), label) - height_of(q + vec2i(0, step), label)) / metres;
    var n = normalize(vec3f(-east, -north, 1.0));
    if (!(known(h) && abs(east) < 1e3 && abs(north) < 1e3)) {
        n = vec3f(0.0, 0.0, 1.0);
    }
    let look = corner_look(q, label, mpu);
    // Octahedral, as mesh.wgsl decodes it.
    let e = n.xy / (abs(n.x) + abs(n.y) + n.z);
    let o = params.out + 4u * k;
    piece[o] = bitcast<u32>(h);
    piece[o + 1u] = look.x;
    piece[o + 2u] = look.y;
    piece[o + 3u] = pack2x16snorm(e);
}
