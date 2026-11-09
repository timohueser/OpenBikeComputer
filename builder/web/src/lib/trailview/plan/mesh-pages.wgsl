// Spike 2 (never merges, `pages=mesh`): a computed piece's triangles drawn top-down into one page of level 17 or 18,
// and the lookup at the page's texel centres (the stamp's model_at and height_at), for the diff against the page
// that the stamp pass wrote. Nothing here reaches the frame.
// #include ../terrain/drawn-height.wgsl
// #include ../terrain/stamp-core.wgsl
// #include ../terrain/model.wgsl

struct Job {
    // The cell's north-west corner and the page's first texel at its level, as the stamp's job.first.
    cell: vec2i,
    first: vec2i,
    level: u32,
    // The first word of the corners' height words in `piece`: 2 × corners.
    out: u32,
    pad: vec2u,
}

@group(0) @binding(0) var<storage, read> piece: array<u32>;
@group(0) @binding(2) var<uniform> job: Job;
@group(0) @binding(3) var<storage, read_write> looked: array<vec4u>;

const NO_LABEL: u32 = 33554431u;
const PAINT_TEXELS: f32 = 2.0;

// The lookup per texel: the stamp's label and height over the plain terrain (no water or structure stamp), the
// label's rule and surface, and the plain terrain.
@compute @workgroup_size(8, 8)
fn lookup(@builtin(global_invocation_id) id: vec3u) {
    let t = job.first + vec2i(id.xy);
    let shift = 22u - job.level;
    let p = (t << vec2u(shift)) + vec2i(1 << (shift - 1u));
    let mp = model_at(p, PAINT_TEXELS * unit_metres(p.y) * f32(1 << shift));
    let plain = stored_base(p, job.level);
    var h = plain;
    var kind = 0u;
    if (mp.block != 0u && known(plain)) {
        let hk = height_at(mp.block, vec2f(mp.q), mp.label, plain, mp.heights);
        h = hk.x;
        kind = u32(hk.y);
    }
    var words = 0u;
    if (mp.block != 0u && mp.label != M_NONE) {
        let w = mw(m_label(mp.block, mp.label));
        words = (w >> 24u) | ((w & 0xffu) << 8u);
    }
    looked[id.y * 256u + id.x] = vec4u(bitcast<u32>(h), mp.label, words | (kind << 16u), bitcast<u32>(plain));
}

struct RasterOut {
    @builtin(position) clip: vec4f,
    @location(0) h: f32,
    // The corner's height less its plain terrain.
    @location(1) offset: f32,
    @location(2) @interpolate(flat) label: u32,
    // Over 0 where a corner of the triangle has no height.
    @location(3) bad: f32,
}

// A corner at its place in the page: x and y in texels from the page's north-west corner.
fn raster_corner(k: u32) -> RasterOut {
    let w0 = piece[2u * k];
    let w1 = piece[2u * k + 1u];
    let q = vec2i(i32(w0 & 0x7ffffu), i32(w1 & 0x7ffffu));
    let shift = 22u - job.level;
    let local = vec2f(job.cell - (job.first << vec2u(shift))) + vec2f(q) / 16.0;
    let t = local / f32(1 << shift);
    let u = job.cell + q / 16;
    var h = bitcast<f32>(piece[job.out + 4u * k]);
    // A terrain vertex stands at the drawn page's height, as the terrain's vertex at its level-18 grid point.
    if ((w1 >> 31u) == 1u) {
        h = units_height(u, 18u);
    }
    var out: RasterOut;
    out.clip = vec4f(t.x / 128.0 - 1.0, 1.0 - t.y / 128.0, 0.5, 1.0);
    out.h = h;
    let plain = stored_base(u, 18u);
    out.offset = h - plain;
    out.bad = select(0.0, 1.0, !known(h) || !known(plain));
    out.label = (w0 >> 19u) | (((w1 >> 19u) & 0xfffu) << 13u);
    return out;
}

@vertex
fn raster_vertex(@builtin(vertex_index) k: u32) -> RasterOut {
    return raster_corner(k);
}

// A step slot's two triangles on its four corners a_p, a_q, b_q, b_p, as slot_vertex of piece.wgsl.
@vertex
fn slot_raster(@builtin(vertex_index) index: u32) -> RasterOut {
    // job.pad.x: the first word of the slots, 6 × corners.
    let w = job.pad.x + 4u * (index / 6u);
    let order = array(0u, 1u, 2u, 0u, 2u, 3u);
    return raster_corner(piece[w + order[index % 6u]]);
}

// Height bits, height from the offset over the page's plain terrain, the label (NO_LABEL as M_NONE), 1.
@fragment
fn raster_fragment(v: RasterOut) -> @location(0) vec4u {
    let t = vec2i(floor(v.clip.xy));
    let shift = 22u - job.level;
    let p = ((job.first + t) << vec2u(shift)) + vec2i(1 << (shift - 1u));
    let plain = stored_base(p, job.level);
    return vec4u(bitcast<u32>(v.h), bitcast<u32>(plain + v.offset), select(v.label, M_NONE, v.label == NO_LABEL), select(1u, 2u, v.bad != 0.0));
}
