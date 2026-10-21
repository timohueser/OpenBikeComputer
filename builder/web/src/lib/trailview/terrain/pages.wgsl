// Per-page compute: the heights of synthesized pages, and the normals of every page.
// #include drawn-height.wgsl

struct Job {
    // The node's first texel at its level.
    texel: vec2i,
    level: u32,
    // Synthesis: the level of the deepest stored ancestor.
    stored: u32,
    // Metres per texel at the node's latitude.
    metres: f32,
    slot: u32,
    // 1 when the stamp pass takes the page next: it writes the extent of the stamped heights.
    stamped: u32,
    unused: u32,
}

@group(0) @binding(0) var<storage, read> jobs: array<Job>;
// Per job, one page: float32 bits of the heights before their 16-bit codes, or the normals of two texels per word.
@group(0) @binding(1) var<storage, read_write> out: array<u32>;
// Per job, the least and the greatest height as ordered bits, for the offset and the scale of its page.
@group(0) @binding(2) var<storage, read_write> extent: array<atomic<u32>>;

// The float32 bits of an unknown height in `out`, before its 16-bit codes.
const UNKNOWN32: u32 = 0xffffffffu;

// Catmull-Rom weights of the four texels about a part t.
fn spline(t: f32) -> vec4f {
    return 0.5 * vec4f(-t + 2.0 * t * t - t * t * t, 2.0 - 5.0 * t * t + 3.0 * t * t * t, t + 4.0 * t * t - 3.0 * t * t * t, -t * t + t * t * t);
}

// The bicubic height of the finest stored level up to `top` that knows all 16 texels about a point.
fn stored_base(u: vec2i, top: u32) -> f32 {
    for (var level = top; level >= COARSEST; level--) {
        let t = texel_of_units(u, level);
        let wx = spline(t.part.x);
        let wy = spline(t.part.y);
        var sum = 0.0;
        var complete = true;
        for (var j = 0; j < 4 && complete; j++) {
            for (var i = 0; i < 4; i++) {
                let h = texel_height(level, t.whole + vec2i(i - 1, j - 1), true);
                if (!known(h)) {
                    complete = false;
                    break;
                }
                sum += h * wx[i] * wy[j];
            }
        }
        if (complete) {
            return sum;
        }
    }
    return NONE;
}

// The ordered bits of a height, so that unsigned atomics take the least and the greatest.
fn ordered(h: f32) -> u32 {
    let b = bitcast<u32>(h);
    return select(b | 0x80000000u, ~b, (b & 0x80000000u) != 0u);
}

// A synthesized page: bicubic from the stored ancestor, never from a synthesized level.
@compute @workgroup_size(8, 8)
fn synthesize(@builtin(global_invocation_id) id: vec3u) {
    let job = jobs[id.z];
    let shift = 22u - job.level;
    let u = ((job.texel + vec2i(id.xy)) << vec2u(shift)) + vec2i(1 << (shift - 1u));
    let h = stored_base(u, job.stored);
    let at = id.z * 65536u + id.y * 256u + id.x;
    if (!known(h)) {
        out[at] = UNKNOWN32;
        return;
    }
    out[at] = bitcast<u32>(h);
    if (job.stamped == 0u) {
        atomicMin(&extent[2u * id.z], ordered(h));
        atomicMax(&extent[2u * id.z + 1u], ordered(h));
    }
}

// The height of a neighbour texel, or of the coarser levels at its centre, or else of the centre texel.
fn neighbour(level: u32, t: vec2i, centre: f32) -> f32 {
    var h = texel_height(level, t, false);
    if (!known(h) && level > COARSEST) {
        let shift = 22u - level;
        h = units_height((t << vec2u(shift)) + vec2i(1 << (shift - 1u)), level - 1u);
    }
    return select(centre, h, known(h));
}

// The x east and y north of a texel's unit normal; −1, −1 where its height is unknown.
fn texel_normal_of(level: u32, t: vec2i, metres: f32) -> vec2f {
    let c = texel_height(level, t, false);
    if (!known(c)) {
        return vec2f(-1.0);
    }
    let east = neighbour(level, t + vec2i(1, 0), c) - neighbour(level, t - vec2i(1, 0), c);
    let north = neighbour(level, t - vec2i(0, 1), c) - neighbour(level, t + vec2i(0, 1), c);
    return normalize(vec3f(-east, -north, 2.0 * metres)).xy;
}

// The normals of a page, two texels per invocation, as rg8snorm.
@compute @workgroup_size(8, 8)
fn normals_of(@builtin(global_invocation_id) id: vec3u) {
    let job = jobs[id.z];
    let t = job.texel + vec2i(2 * i32(id.x), i32(id.y));
    let a = texel_normal_of(job.level, t, job.metres);
    let b = texel_normal_of(job.level, t + vec2i(1, 0), job.metres);
    out[id.z * 32768u + id.y * 128u + id.x] = pack4x8snorm(vec4f(a, b));
}
