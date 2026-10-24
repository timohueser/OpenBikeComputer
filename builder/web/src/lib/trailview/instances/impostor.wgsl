// Impostors: a model seen from FRAMES × FRAMES directions over the upper half of the sphere
// (hemi-octahedral), baked into one layer of an atlas (impostor-bake.wgsl), and drawn far away as one
// quad that faces the eye and shows the frame of the nearest direction. The atlas holds the colour and
// the cover, and the normal in the model's frame, so the light of the frame lights an impostor.
// #include draw.wgsl
// #include hemi.wgsl

@group(1) @binding(0) var atlas_albedo: texture_2d_array<f32>;
@group(1) @binding(1) var atlas_normal: texture_2d_array<f32>;
@group(1) @binding(2) var atlas_sampler: sampler;
// Per model its layer of the atlas.
@group(1) @binding(3) var<uniform> slots: array<vec4u, 64>;
// Per model and frame the outline that holds the model in the frame and its neighbours (outlines of
// instances.ts): its bottom and top in the frame, and its half widths there.
@group(1) @binding(4) var<storage, read> outlines: array<vec4f>;

// Texels along a frame's side, and the atlas's mip levels: a frame of 32 texels down to 1.
const FRAME_TEXELS: f32 = 32.0;
const ATLAS_LEVELS: f32 = 6.0;

struct ImpostorOut {
    @builtin(position) clip: vec4f,
    @location(0) fog: vec3f,
    @location(7) air: vec3f,
    // The point of the quad in the frame, -1 to 1.
    @location(1) corner: vec2f,
    // The direction toward the eye in the model's frame, and its turn (cos, sin).
    @location(2) toward: vec3f,
    @location(3) @interpolate(flat) turn: vec2f,
    @location(4) @interpolate(flat) look: u32,
    @location(5) @interpolate(flat) slot: u32,
    @location(6) tint: f32,
    // The atlas's mip level where a texel spans a render pixel.
    @location(8) @interpolate(flat) level: f32,
    // The instance's ground and its model's tie (draw.wgsl).
    @location(9) @interpolate(flat) ground: u32,
    @location(10) @interpolate(flat) tie: f32,
}

@vertex
fn impostor_vertex(v: MeshIn, @builtin(instance_index) instance: u32) -> ImpostorOut {
    let i = lists[bases[draw.x] + instance];
    let model = draw.x / TIERS;
    let m = models[model];
    let turn = unpack2x16float(i.turn);
    let width = length(turn);
    let t = turn / width;
    let centre = i.at.xyz + vec3f(0.0, 0.0, 0.5 * i.at.w);
    let d = -normalize(centre);
    // The direction toward the eye in the model's frame: the turn undone.
    let toward = vec3f(t.x * d.x + t.y * d.y, -t.y * d.x + t.x * d.y, d.z);
    let axes = view_axes(d);
    let r = m.radius * i.at.w;
    let far = length(centre);
    // A frame spans the sphere's diameter.
    let texels = FRAME_TEXELS * far * frame.pixel / (look.screen.z * 2.0 * r);
    let level = clamp(log2(max(texels, 1.0)), 0.0, ATLAS_LEVELS - 1.0);
    // The quad holds only the part of the frame that the model covers: the outline of the frame that
    // holds the direction, which holds its neighbours too, as the fragments pick them, and a texel of
    // the level about it.
    let margin = exp2(ceil(level)) * 2.0 / FRAME_TEXELS;
    let at = clamp(vec2i(floor(hemi_encode(toward) * FRAMES)), vec2i(0), vec2i(i32(FRAMES) - 1));
    let outline = outlines[model * u32(FRAMES * FRAMES) + u32(at.y * i32(FRAMES) + at.x)];
    let up = 0.5 * v.position.y + 0.5;
    let corner = clamp(vec2f(v.position.x * (mix(outline.z, outline.w, up) + margin), mix(outline.x, outline.y, up) + v.position.y * margin), vec2f(-1.0), vec2f(1.0));
    // A tree widened by `width` is as wide seen from above as from the side.
    let p = centre + r * (corner.x * width * axes[0] + corner.y * mix(1.0, width, abs(d.z)) * axes[1]);
    var out: ImpostorOut;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    let haze = haze_at(centre);
    out.fog = haze.fog;
    out.air = haze.air;
    out.corner = corner;
    out.toward = toward;
    out.turn = t;
    out.look = i.look;
    out.slot = slots[model / 4u][model % 4u];
    out.ground = i.ground;
    out.tie = m.tie;
    out.tint = (0.75 + 0.5 * f32(i.look & 255u) / 255.0) * far_tone(m, far);
    out.level = level;
    return out;
}

@fragment
fn impostor_fragment(in: ImpostorOut) -> @location(0) vec4f {
    if (!kept(in.clip.xy, in.look)) {
        discard;
    }
    // The frame of the nearest direction, blended with its neighbours by a dither that the resolve averages.
    let at = clamp(floor(hemi_encode(in.toward) * FRAMES + vec2f(dither(in.clip.xy + vec2f(13.0, 59.0)), dither(in.clip.xy + vec2f(71.0, 5.0))) - 0.5), vec2f(0.0), vec2f(FRAMES - 1.0));
    let uv = (at + 0.5 + vec2f(0.5 * in.corner.x, -0.5 * in.corner.y)) / FRAMES;
    let albedo = textureSampleLevel(atlas_albedo, atlas_sampler, uv, in.slot, in.level);
    // The cover of a coarser level is a share, which the dither keeps on average.
    if (albedo.a <= dither(in.clip.xy + vec2f(29.0, 43.0))) {
        discard;
    }
    let local = textureSampleLevel(atlas_normal, atlas_sampler, uv, in.slot, in.level).xyz / albedo.a * 2.0 - 1.0;
    let n = normalize(vec3f(in.turn.x * local.x - in.turn.y * local.y, in.turn.y * local.x + in.turn.x * local.y, local.z));
    return vec4f(in.fog * light_up(snowed(tied(albedo.rgb / albedo.a, in.tint, in.tie, in.ground), n.z, in.look), n, 1.0, 1.0, look.horizon.w) + in.air, 1.0);
}
