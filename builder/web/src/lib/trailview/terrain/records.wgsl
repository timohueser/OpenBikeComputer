// The layout of a cell's records block (the crate's `faces::records`) and the reads of its words. A
// block is a word offset into model_words; its offsets are relative to it.
// #include model-words.wgsl

const MODEL_ROW: u32 = 4096u;
const M_BINS: i32 = 64;
const M_BIN: f32 = 4096.0;
const M_CELL: f32 = 262144.0;
const M_CAND: u32 = 11u;
const M_LABEL: u32 = 12u;
const M_LINE: u32 = 16u;
const M_SAMPLE: u32 = 12u;
const M_DISC: u32 = 5u;
const M_BLEND: u32 = 6u;
const M_HEADER: u32 = 4u;
const M_NONE: u32 = 0xffffffffu;
// On a junction label's other arm word: an unpaved arm that stops at the junction's host, with its corner radius in
// centimetres above its line, as records::STOP_ARM.
const STOP_ARM: u32 = 0x80000000u;
// Line flags of the finish word, as records.rs: a rail, a way that wears lines, one that looks as a motorway, a
// wall class, and the shift of its top's material (sections::Material).
const M_RAIL: u32 = 256u;
const M_MARKED: u32 = 512u;
const M_MOTORWAY: u32 = 1024u;
const M_WALL: u32 = 2048u;
const M_MATERIAL_SHIFT: u32 = 16u;
// Candidate kinds and the slope surface, as records.rs and faces::Surface.
const M_BAND: u32 = 3u;
const M_FILL: u32 = 4u;
const M_SURFACE_SLOPE: u32 = 1u;
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

// The line of a junction label's other arm word, as records::arm_line.
fn m_arm_line(x: u32) -> u32 {
    return select(x & 0xffffu, M_NONE, x == M_NONE);
}

// A junction label's other arm line at word w.
fn m_other(w: u32) -> u32 {
    return m_arm_line(mw(w));
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
