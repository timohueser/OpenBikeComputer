// The layout of a cell's records block (the crate's `faces::records`) and the reads of its words. A
// block is a word offset into model_words; its offsets are relative to it.
// #include model-words.wgsl

const MODEL_ROW: u32 = 4096u;
const M_BINS: i32 = 64;
const M_BIN: f32 = 4096.0;
const M_CELL: f32 = 262144.0;
const M_CAND: u32 = 11u;
const M_LABEL: u32 = 12u;
const M_LINE: u32 = 18u;
const M_SAMPLE: u32 = 12u;
const M_DISC: u32 = 5u;
const M_BLEND: u32 = 6u;
const M_HEADER: u32 = 4u;
const M_NONE: u32 = 0xffffffffu;
const M_FAR: f32 = 1e9;
// A junction record, as records::JR: node x, y; the arm count and the graph's junction; then M_JR_ARM words per arm:
// its line with its flags (a host, a stop arm), its node and last sample, its direction at the node.
const M_JUNCTION: u32 = 2u;
const M_JR: u32 = 3u;
const M_JR_ARM: u32 = 3u;
const M_ARM_HOST: u32 = 0x10000u;
const M_ARM_STOP: u32 = 0x20000u;
// Line flags of the finish word, as records.rs: a rail, a way that wears lines, one that looks as a motorway, a
// wall class, a deck's ceiling line (after every line of storey 0), line water, a path, and the shift of its top's
// material (sections::Material).
const M_RAIL: u32 = 256u;
const M_MARKED: u32 = 512u;
const M_MOTORWAY: u32 = 1024u;
const M_WALL: u32 = 2048u;
const M_CEILING: u32 = 4096u;
const M_WATER: u32 = 8192u;
const M_PATH: u32 = 16384u;
const M_MATERIAL_SHIFT: u32 = 16u;
// Candidate kinds and the surfaces of the slope, line water, its bank and a culvert's zone, as records.rs and
// faces::Surface.
const M_BAND: u32 = 3u;
const M_FILL: u32 = 4u;
const M_SURFACE_SLOPE: u32 = 1u;
const M_SURFACE_WATER: u32 = 9u;
const M_SURFACE_BANK: u32 = 10u;
const M_SURFACE_ZONE: u32 = 11u;
// As MARGIN_M of records.rs: a bin lists every candidate edge within this of its square.
const M_MARGIN: f32 = 3.0;

fn mw(i: u32) -> u32 {
    return textureLoad(model_words, vec2i(i32(i % MODEL_ROW), i32(i / MODEL_ROW)), 0).r;
}

fn mf(i: u32) -> f32 {
    return bitcast<f32>(mw(i));
}

fn mi(i: u32) -> f32 {
    return f32(bitcast<i32>(mw(i)));
}

fn m_cross(a: vec2f, b: vec2f) -> f32 {
    return a.x * b.y - a.y * b.x;
}

fn m_cand(block: u32, ci: u32) -> u32 {
    return block + mw(block + 4u) + ci * M_CAND;
}

fn m_vertex(at: u32) -> vec2i {
    return vec2i(bitcast<i32>(mw(at)), bitcast<i32>(mw(at + 1u)));
}

// Metres per frame unit at the point (head word 10 is the cell's zoom-16 row): two cells that share
// a point read the same scale.
fn h_scale(block: u32, q: vec2f) -> f32 {
    let v = f32(mw(block + 10u)) / 65536.0 + q.y / 17179869184.0;
    return 40075016.686 / 17179869184.0 / cosh(3.14159265 * (1.0 - 2.0 * v));
}

fn m_label(block: u32, id: u32) -> u32 {
    return block + mw(block + 5u) + id * M_LABEL;
}

// The junction record of label `id`, M_NONE for a label that no junction owns.
fn m_junction(block: u32, id: u32) -> u32 {
    if (id == M_NONE) {
        return M_NONE;
    }
    let l = m_label(block, id);
    if (((mw(l) >> 8u) & 0xffu) != M_JUNCTION) {
        return M_NONE;
    }
    return block + mw(block + mw(block + 12u) + mw(l + 1u));
}

// The first word of arm k of junction record jr.
fn m_arm(jr: u32, k: u32) -> u32 {
    return jr + M_JR + M_JR_ARM * k;
}

// A line and a point's place in its frame: metres across from its middle (signed by side), along it, and its
// half-width.
struct ArmFrame {
    line: u32,
    place: vec3f,
}

// The arms of junction record jr whose flags hold `flags`, a bit each.
fn m_arms_with(jr: u32, flags: u32) -> u32 {
    var arms = 0u;
    for (var k = 0u; k < min(mw(jr + 2u) & 0xffu, 32u); k++) {
        arms |= select(0u, 1u << k, (mw(m_arm(jr, k)) & flags) == flags);
    }
    return arms;
}

// The nearest of the lines of a junction's `arms` (a bit each) and line `also`, and q's place in its frame, from their
// segments in q's bin, which lists every segment of them that can be nearest to a point of the bin. Across is M_FAR
// for none.
fn m_junction_frame(block: u32, jr: u32, q: vec2f, arms: u32, also: u32, mpu: f32) -> ArmFrame {
    let segs = m_bin_segments(block, q);
    let samples = block + mw(block + 7u);
    let lines = block + mw(block + 6u);
    var best = M_FAR;
    var out = ArmFrame(M_NONE, vec3f(M_FAR, 0.0, 0.0));
    for (var i = 0u; i < segs.y; i++) {
        let s = mw(segs.x + i);
        let line = s & 0xffffu;
        var take = line == also;
        for (var k = 0u; k < min(mw(jr + 2u) & 0xffu, 32u); k++) {
            take = take || (((arms >> k) & 1u) == 1u && (mw(m_arm(jr, k)) & 0xffffu) == line);
        }
        if (!take) {
            continue;
        }
        let wa = samples + M_SAMPLE * (mw(lines + M_LINE * line + 2u) + (s >> 16u));
        let wb = wa + M_SAMPLE;
        let a = vec2f(m_vertex(wa));
        let u = vec2f(m_vertex(wb)) - a;
        let t = clamp(dot(q - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
        let d = length(q - a - t * u);
        if (d < best) {
            best = d;
            out = ArmFrame(line, vec3f(sign(m_cross(u, q - a)) * d * mpu, mix(mf(wa + 3u), mf(wb + 3u), t), mix(mf(wa + 2u), mf(wb + 2u), t) * mpu));
        }
    }
    return out;
}

// The finish word of a line, 0 for none.
fn m_line_finish(block: u32, line: u32) -> u32 {
    return select(mw(block + mw(block + 6u) + M_LINE * line + 1u), 0u, line == M_NONE);
}

fn m_extras(block: u32) -> u32 {
    return block + mw(block + 8u);
}

// The header of q's bin (the closed cell square, as `find`), 0 outside the cell or for an empty bin.
fn m_bin_head(block: u32, q: vec2f) -> u32 {
    if (any(q < vec2f(0.0)) || any(q > vec2f(M_CELL))) {
        return 0u;
    }
    let bin = min(vec2i(floor(q / M_BIN)), vec2i(M_BINS - 1));
    let head = block + mw(block + 9u) + M_HEADER * u32(bin.y * M_BINS + bin.x);
    return select(head, 0u, mw(head) == 0u);
}

// The word after the candidate item at `at`.
fn m_skip_item(at: u32) -> u32 {
    var next = at + 2u;
    for (var r = 0u; r < mw(at + 1u); r++) {
        next += 1u + 2u * mw(next);
    }
    return next;
}

// The first word and the count of the centre-line segments of q's bin.
fn m_bin_segments(block: u32, q: vec2f) -> vec2u {
    let head = m_bin_head(block, q);
    if (head == 0u) {
        return vec2u(0u);
    }
    return vec2u(block + mw(head) + (mw(head + 3u) >> 16u), mw(head + 1u) & 0xffffu);
}
