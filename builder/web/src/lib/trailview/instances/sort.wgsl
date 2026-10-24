// Sorts the staged instances of a frame into one list per draw: `count_draws` counts the instances of
// each draw, `place_draws` gives each draw its place in the lists and its indirect arguments, then
// `scatter` moves each staged instance to its draw's list.

struct Instance {
    at: vec4f,
    turn: u32,
    ground: u32,
    look: u32,
    draw: u32,
}

struct Tally {
    staged: u32,
    dropped: u32,
    unused: vec2u,
    draws: array<atomic<u32>, 1024>,
}

// The mesh of a draw, as instances.ts writes it.
struct Mesh {
    index_count: u32,
    first_index: u32,
    base_vertex: i32,
    unused: u32,
}

// drawIndexedIndirect's arguments.
struct Args {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

const DRAWS: u32 = 1024u;

@group(0) @binding(0) var<storage, read_write> tally: Tally;
@group(0) @binding(1) var<storage, read> meshes: array<Mesh, 1024>;
@group(0) @binding(2) var<storage, read_write> args: array<Args, 1024>;
// The first list entry of each draw, then the cursor of each draw for `scatter`.
@group(0) @binding(3) var<storage, read_write> bases: array<atomic<u32>, 2048>;
@group(0) @binding(4) var<storage, read> staged: array<Instance>;
@group(0) @binding(5) var<storage, read_write> lists: array<Instance>;

var<workgroup> counts: array<atomic<u32>, 1024>;
var<workgroup> staged_n: u32;

// The lanes of `count_draws` and `scatter`, as SORT_LANES of instances.ts: each takes every SORT_LANES-th staged instance.
const LANES: u32 = 16384u;

// A workgroup's histogram of draws, then one global add per draw that it holds.
@compute @workgroup_size(64)
fn count_draws(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_index) lane: u32) {
    if (lane == 0u) {
        staged_n = min(tally.staged, arrayLength(&staged));
    }
    let staged_count = workgroupUniformLoad(&staged_n);
    for (var d = lane; d < DRAWS; d += 64u) {
        atomicStore(&counts[d], 0u);
    }
    workgroupBarrier();
    for (var k = id.x; k < staged_count; k += LANES) {
        atomicAdd(&counts[staged[k].draw], 1u);
    }
    workgroupBarrier();
    for (var d = lane; d < DRAWS; d += 64u) {
        let n = atomicLoad(&counts[d]);
        if (n > 0u) {
            atomicAdd(&tally.draws[d], n);
        }
    }
}

var<workgroup> sums: array<u32, 256>;

// Draws per lane of `place_draws`: DRAWS over its 256 lanes.
const PER_LANE: u32 = DRAWS / 256u;

// The first list entry of each draw, a scan over the draws, PER_LANE per lane; and the indirect arguments.
@compute @workgroup_size(256)
fn place_draws(@builtin(local_invocation_index) lane: u32) {
    var own = 0u;
    for (var k = 0u; k < PER_LANE; k++) {
        own += atomicLoad(&tally.draws[PER_LANE * lane + k]);
    }
    sums[lane] = own;
    for (var step = 1u; step < 256u; step <<= 1u) {
        workgroupBarrier();
        let add = select(0u, sums[max(lane, step) - step], lane >= step);
        workgroupBarrier();
        sums[lane] += add;
    }
    var at = sums[lane] - own;
    for (var k = 0u; k < PER_LANE; k++) {
        let d = PER_LANE * lane + k;
        let n = atomicLoad(&tally.draws[d]);
        atomicStore(&bases[d], at);
        at += n;
        atomicStore(&bases[DRAWS + d], 0u);
        let mesh = meshes[d];
        args[d] = Args(mesh.index_count, select(0u, n, mesh.index_count > 0u), mesh.first_index, mesh.base_vertex, 0u);
    }
}

@compute @workgroup_size(64)
fn scatter(@builtin(global_invocation_id) id: vec3u) {
    for (var k = id.x; k < min(tally.staged, arrayLength(&staged)); k += LANES) {
        let instance = staged[k];
        lists[atomicLoad(&bases[instance.draw]) + atomicAdd(&bases[DRAWS + instance.draw], 1u)] = instance;
    }
}
