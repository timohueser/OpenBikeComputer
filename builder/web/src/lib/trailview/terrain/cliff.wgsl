// The cliff share of the pages with masks on the plan path (cliff_at of features.wgsl): per texel, how far
// ground_colour turns the ground's normal toward the coarse one. The plan's heights hold creases and a wall's step
// sharper than a texel, and the texel normals beside them change with the crease's phase from texel to texel: the
// turn's key (cliff_of, or a normal that turns past what the texels resolve) then flips per texel along the crease.
// Each texel takes the key of the 3 × 3 texels about it, filtered, where the plan moved the ground within two texels;
// elsewhere UNSHAPED, and the look keeps its own key. The pass makes a page once when its heights or its mask
// change, and the texels facing a neighbour of the same level when that neighbour's do (cliff.ts).
// #include ../ground/look.wgsl

struct CliffJob {
    // The node's first texel at its level.
    first: vec2i,
    level: u32,
    mask: u32,
    // The texels of the page to make, from lo to hi (not included); x in whole packed texels of four.
    lo: vec2u,
    hi: vec2u,
}

@group(0) @binding(0) var<storage, read> cliff_jobs: array<CliffJob>;
// As cliff_texels of features.wgsl: four texels of a row in one texel.
@group(0) @binding(1) var cliff_out: texture_storage_2d_array<rgba8unorm, write>;

// A workgroup's tile of 8 × 8 texels: the heights and the moves two texels about it, the keys one texel about it.
var<workgroup> tile_heights: array<f32, 144>;
var<workgroup> tile_moved: array<f32, 144>;
var<workgroup> tile_keys: array<f32, 100>;
var<workgroup> tile_shares: array<f32, 64>;

// The workgroups of a job's dispatch row: each makes every 32nd tile of the job's texels.
const ROW: u32 = 32u;
// A normal that turns more than this from one texel to the next is all cliff: the texels do not resolve it.
const UNRESOLVED: vec2f = vec2f(0.15, 0.4);

@compute @workgroup_size(8, 8)
fn cliff(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let job = cliff_jobs[group.z];
    let low = job.lo / 8u;
    let tiles = (job.hi + 7u) / 8u - low;
    for (var k = group.x; k < tiles.x * tiles.y; k += ROW) {
        cliff_tile(job, (low + vec2u(k % tiles.x, k / tiles.x)) * 8u, lane);
    }
}

fn cliff_tile(job: CliffJob, corner: vec2u, lane: u32) {
    let origin = job.first + vec2i(corner) - 2;
    for (var k = lane; k < 144u; k += 64u) {
        let t = origin + vec2i(i32(k % 12u), i32(k / 12u));
        tile_heights[k] = texel_height(job.level, t, false);
        let slot = mask_of(job.level, t);
        var moved = 0.0;
        if (slot != 0u) {
            moved = moved_texel(slot - 1u, t);
        }
        tile_moved[k] = moved;
    }
    workgroupBarrier();
    let metres = f32(1 << (22u - job.level)) * surface.unit;
    for (var k = lane; k < 100u; k += 64u) {
        tile_keys[k] = texel_key((k / 10u + 1u) * 12u + k % 10u + 1u, metres);
    }
    workgroupBarrier();
    let own = vec2u(lane % 8u, lane / 8u);
    var sum = 0.0;
    var weight = 0.0;
    for (var j = 0u; j < 3u; j++) {
        for (var i = 0u; i < 3u; i++) {
            let key = tile_keys[(own.y + j) * 10u + own.x + i];
            // The weights 1 2 1 across and up.
            let w = select(0.0, f32((1u + i % 2u) * (1u + j % 2u)), key >= 0.0);
            sum += w * key;
            weight += w;
        }
    }
    var shaped = false;
    for (var j = 0u; j < 5u; j++) {
        for (var i = 0u; i < 5u; i++) {
            shaped = shaped || tile_moved[(own.y + j) * 12u + own.x + i] >= 0.5 * MOVED_M;
        }
    }
    tile_shares[lane] = select(UNSHAPED, round(SHARE_STEPS * sum / max(weight, 1.0)) / 255.0, shaped && weight > 0.0);
    workgroupBarrier();
    let t = corner + own;
    if (own.x % 4u == 0u && all(t >= job.lo) && all(t < job.hi)) {
        let at = in_layer(job.mask, job.first + vec2i(t));
        textureStore(cliff_out, vec2i(at.x >> 2u, at.y), job.mask >> 6u, vec4f(tile_shares[lane], tile_shares[lane + 1u], tile_shares[lane + 2u], tile_shares[lane + 3u]));
    }
    workgroupBarrier();
}

// The turn's key of a texel of the tile, at `at` in tile_heights: how far it is cliff (cliff_of, without the spread
// that the filter takes over), or all of it where its normal turns more than the texels resolve; -1 where its height is
// unknown. Its normal and turn are those of level_normal at the texel: an unknown neighbour continues the slope through
// it.
fn texel_key(at: u32, metres: f32) -> f32 {
    let h = tile_heights[at];
    if (!known(h)) {
        return -1.0;
    }
    let we = continued(h, tile_heights[at - 1u], tile_heights[at + 1u]);
    let ns = continued(h, tile_heights[at - 12u], tile_heights[at + 12u]);
    let rise = vec2f(we.y - we.x, ns.x - ns.y);
    let bend = vec2f(we.x + we.y, ns.x + ns.y) - 2.0 * h;
    let across = dot(rise, rise) + 4.0 * metres * metres;
    let z = 2.0 * metres * inverseSqrt(across);
    let turn = length(bend) * 4.0 * metres / across;
    return max(cliff_of(z, 0.0), smoothstep(UNRESOLVED.x, UNRESOLVED.y, turn));
}

// The heights before and after a texel of height h: an unknown one continues the slope through the texel, level
// where both are unknown.
fn continued(h: f32, before: f32, after: f32) -> vec2f {
    let b = select(select(h, 2.0 * h - after, known(after)), before, known(before));
    return vec2f(b, select(2.0 * h - b, after, known(after)));
}
