// r16 pages: the synthesized heights of each job as 16-bit codes, with the offset and the scale of its page.

struct Job {
    texel: vec2i,
    level: u32,
    stored: u32,
    metres: f32,
    slot: u32,
    unused: vec2u,
}

@group(0) @binding(0) var<storage, read> jobs: array<Job>;
// Per job, the float32 bits of a page's heights, and the ordered bits of its least and greatest height.
@group(0) @binding(1) var<storage, read> heights: array<u32>;
@group(0) @binding(2) var<storage, read> extent: array<u32>;
// Per job, two codes per word.
@group(0) @binding(3) var<storage, read_write> codes: array<u32>;
// Per page: the height of code 0, and metres per code.
@group(0) @binding(4) var<storage, read_write> scales: array<vec2f>;

const UNKNOWN32: u32 = 0xffffffffu;
const UNKNOWN16: u32 = 0xffffu;

fn height(ordered: u32) -> f32 {
    return bitcast<f32>(select(~ordered, ordered & 0x7fffffffu, (ordered & 0x80000000u) != 0u));
}

fn code(raw: u32, offset: f32, scale: f32) -> u32 {
    if (raw == UNKNOWN32) {
        return UNKNOWN16;
    }
    return u32(clamp(round((bitcast<f32>(raw) - offset) / scale), 0.0, f32(UNKNOWN16 - 1u)));
}

@compute @workgroup_size(8, 8)
fn quantize(@builtin(global_invocation_id) id: vec3u) {
    let least = extent[2u * id.z];
    let most = extent[2u * id.z + 1u];
    var offset = 0.0;
    var scale = 1.0;
    if (least <= most) {
        offset = height(least);
        scale = max((height(most) - offset) / f32(UNKNOWN16 - 1u), 1e-4);
    }
    let at = id.z * 65536u + id.y * 256u + 2u * id.x;
    codes[id.z * 32768u + id.y * 128u + id.x] = code(heights[at], offset, scale) | (code(heights[at + 1u], offset, scale) << 16u);
    if (all(id.xy == vec2u(0u))) {
        scales[jobs[id.z].slot] = vec2f(offset, scale);
    }
}
