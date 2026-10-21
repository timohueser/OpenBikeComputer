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
// Per job, one page: float32 bits of the heights, or the normals of two texels per word.
@group(0) @binding(1) var<storage, read_write> out: array<u32>;
// Per job, the least and the greatest height as ordered bits, for r16 pages.
@group(0) @binding(2) var<storage, read_write> extent: array<atomic<u32>>;

override EROSION: bool = false;

// The amplitude of a micro-relief octave at slope 1, as a share of its cell.
const RELIEF: f32 = 0.08;
const TAU: f32 = 6.2831853;

// Catmull-Rom weights of the four texels about a part t, and their slopes.
fn spline(t: f32) -> vec4f {
    return 0.5 * vec4f(-t + 2.0 * t * t - t * t * t, 2.0 - 5.0 * t * t + 3.0 * t * t * t, t + 4.0 * t * t - 3.0 * t * t * t, -t * t + t * t * t);
}

fn spline_slope(t: f32) -> vec4f {
    return 0.5 * vec4f(-1.0 + 4.0 * t - 3.0 * t * t, -10.0 * t + 9.0 * t * t, 1.0 + 8.0 * t - 9.0 * t * t, -2.0 * t + 3.0 * t * t);
}

// The bicubic height of the finest stored level up to `top` that knows all 16 texels about a point,
// with its slope east and south per texel of that level, and the level.
fn stored_base(u: vec2i, top: u32) -> vec4f {
    for (var level = top; level >= COARSEST; level--) {
        let t = texel_of_units(u, level);
        let wx = spline(t.part.x);
        let wy = spline(t.part.y);
        let sx = spline_slope(t.part.x);
        let sy = spline_slope(t.part.y);
        var sum = vec3f(0.0);
        var complete = true;
        for (var j = 0; j < 4 && complete; j++) {
            for (var i = 0; i < 4; i++) {
                let h = texel_height(level, t.whole + vec2i(i - 1, j - 1), true);
                if (!known(h)) {
                    complete = false;
                    break;
                }
                sum += h * vec3f(wx[i] * wy[j], sx[i] * wy[j], wx[i] * sy[j]);
            }
        }
        if (complete) {
            return vec4f(sum, f32(level));
        }
    }
    return vec4f(NONE, 0.0, 0.0, 0.0);
}

fn hash(c: vec2i) -> vec2f {
    var v = vec2u(c) * vec2u(1597334673u, 3812015801u);
    v = (v.x ^ v.y) * vec2u(1597334673u, 3812015801u);
    return vec2f(v >> vec2u(8u)) / 16777216.0;
}

// One octave of gullies after Johansen's erosion filter: from each jittered cell point about a
// position, a wave that runs down the slope, so its crests are gullies. `along` is the unit contour
// direction. The value is in [−1, 1]; its slope is per cell.
fn gullies(cell: vec2i, part: vec2f, along: vec2f) -> vec3f {
    var sum = vec3f(0.0);
    var weight = 0.0;
    for (var j = -1; j <= 2; j++) {
        for (var i = -1; i <= 2; i++) {
            let o = vec2i(i, j);
            let p = part - vec2f(o) - 0.5 * hash(cell + o);
            let w = exp(-2.0 * dot(p, p));
            let phase = TAU * dot(p, along);
            sum += w * vec3f(cos(phase), -sin(phase) * TAU * along);
            weight += w;
        }
    }
    return sum / weight;
}

// The ordered bits of a height, so that unsigned atomics take the least and the greatest.
fn ordered(h: f32) -> u32 {
    let b = bitcast<u32>(h);
    return select(b | 0x80000000u, ~b, (b & 0x80000000u) != 0u);
}

// A synthesized page: bicubic from the stored ancestor, never from a synthesized level, then the
// micro-relief of each level below the stored one.
@compute @workgroup_size(8, 8)
fn synthesize(@builtin(global_invocation_id) id: vec3u) {
    let job = jobs[id.z];
    let shift = 22u - job.level;
    let u = ((job.texel + vec2i(id.xy)) << vec2u(shift)) + vec2i(1 << (shift - 1u));
    let base = stored_base(u, job.stored);
    let at = id.z * 65536u + id.y * 256u + id.x;
    if (!known(base.x)) {
        out[at] = UNKNOWN32;
        return;
    }
    var h = base.x;
    var slope = base.yz / (job.metres * exp2(f32(job.level) - base.w));
    if (EROSION) {
        for (var level = u32(base.w) + 1u; level <= job.level; level++) {
            // Cells of 16 texels of this level.
            let cell_shift = 26u - level;
            let cell = u >> vec2u(cell_shift);
            let part = vec2f(u & vec2i((1 << cell_shift) - 1)) / f32(1 << cell_shift);
            let size = 16.0 * job.metres * exp2(f32(job.level - level));
            let steep = length(slope);
            let along = select(vec2f(0.0), vec2f(-slope.y, slope.x) / steep, steep > 1e-4);
            let g = gullies(cell, part, along);
            let amplitude = RELIEF * size * min(steep, 1.0);
            h += amplitude * g.x;
            slope += amplitude * g.yz / size;
        }
    }
    out[at] = bitcast<u32>(h);
    if (R16 && job.stamped == 0u) {
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
