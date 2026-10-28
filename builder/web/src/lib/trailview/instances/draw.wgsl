// Draws the instances of one draw: its model's mesh of that tier, once per instance of its list, in the
// vertex colours under the light and the haze of the ground.
// #include ../terrain/view.wgsl
// #include ../light/shadow.wgsl
// #include ../vegetation/phenology.wgsl

struct Instance {
    at: vec4f,
    turn: u32,
    ground: u32,
    look: u32,
    draw: u32,
}

// As Model of emit.wgsl.
struct Model {
    until: vec4f,
    extent: f32,
    radius: f32,
    fade: f32,
    spread: f32,
    crown: f32,
    trunk: f32,
    tie: f32,
    sized: f32,
    sink_low: vec4f,
    sink_high: vec4f,
    luma: f32,
}

@group(0) @binding(1) var<storage, read> lists: array<Instance>;
@group(0) @binding(2) var<storage, read> bases: array<u32>;
@group(0) @binding(3) var<uniform> models: array<Model, 256>;
// The draw of this draw call: x its model × TIERS + its tier.
@group(0) @binding(4) var<uniform> draw: vec4u;
// Per model and phase the colour matrix of its crown in that phase (phases of instances.ts).
@group(0) @binding(5) var<storage, read> tones: array<mat3x3f>;

// Draws per model, as TIERS of emit.wgsl.
const TIERS: u32 = 4u;

// Far crowns darken to this share from FAR_FROM_M to FAR_TO_M, as the crowns of a forest shade each other.
const FAR_TONE: f32 = 0.82;
const FAR_FROM_M: f32 = 150.0;
const FAR_TO_M: f32 = 1500.0;

struct MeshIn {
    @location(0) position: vec4f,
    @location(1) normal: vec4f,
    @location(2) colour: vec4f,
}

// A model's faces that glow (`glowing` of models/flat.ts) shine at night in their own colour this many times over;
// those that rise, as a flame, rise from their foot as the night falls.
const GLOW_GAIN: f32 = 2.5;

struct MeshOut {
    @builtin(position) clip: vec4f,
    @location(0) normal: vec3f,
    @location(1) albedo: vec3f,
    @location(2) @interpolate(flat) look: u32,
    // The haze over the instance: the light that reaches the eye is fog × lit + air.
    @location(3) fog: vec3f,
    @location(4) air: vec3f,
    @location(5) @interpolate(flat) sun: f32,
    @location(6) relative: vec3f,
    // 1 where `sun` holds the objects' shadow too (mesh_vertex), 0 for a tree, which reads it per pixel.
    @location(7) @interpolate(flat) shaded: u32,
    // How much the face glows: 0 to 1.
    @location(8) glow: f32,
}

// 0 by day to 1 at night, as the lamplight of the houses.
fn nightfall() -> f32 {
    return smoothstep(0.0, 0.4, look.ambient.w);
}

// The haze between the eye and a point of render space, as the factor and the term that `atmosphere`
// applies to a colour: per vertex, as an instance is small against the haze's scale.
struct Haze {
    fog: vec3f,
    air: vec3f,
}

fn haze_at(p: vec3f) -> Haze {
    let air = atmosphere(vec3f(0.0), p, frame.eye + p.z);
    return Haze(atmosphere(vec3f(1.0), p, frame.eye + p.z) - air, air);
}

fn linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

// The light that reaches a point of a tree in model units: less low in the crown and inside it, where
// the crown shades itself. 1 for a model that is no tree (crown < 0).
fn crown_shade(p: vec3f, m: Model) -> f32 {
    if (m.crown < 0.0) {
        return 1.0;
    }
    let up = clamp((p.z - m.crown) / (1.0 - m.crown), 0.0, 1.0);
    let out = clamp(length(p.xy) / max(m.spread, 1e-3), 0.0, 1.0);
    return mix(0.55, 1.0, clamp(0.6 * (up + out), 0.0, 1.0));
}

// SNOW of ground/rules.ts, linear.
const SNOW: vec3f = vec3f(0.855, 0.888, 0.922);

// The albedo of a model's colour on an instance: a model with a tie takes the hue of the ground under
// it, shaded from root to tip about the ground's own brightness and never much brighter than it, so a
// meadow of tufts reads as the meadow (the prototype's `tie`).
fn tied(colour: vec3f, tint: f32, tie: f32, ground: u32) -> vec3f {
    let own = colour * tint;
    if (tie <= 0.0) {
        return own;
    }
    let g = unpack4x8unorm(ground).rgb;
    return mix(own, g * g * clamp(luma(colour) / 0.3, 0.5, 1.2) * tint, tie);
}

// A forest tree (a model with a crown's mean luma and no tie, on woodland that emit_tree carries) wears
// the colour of the ground under it, which the forest's paint shows past the trees' reach (TREE_REACH_M
// of tree-models.ts). Each point of the crown keeps its own luma against the crown's mean, so a crown
// keeps its light tips. Its phase adds the change that it makes to the tree's own colour, at the same
// scale, so an autumn beech turns orange near the eye; the paint shows the satellite's quarter and not
// the phase, so the change fades out from PHASE_FROM_M to the reach. CROWN_GAIN makes a stand of lit
// crowns, shaded sides and gaps at the reach as bright as the paint past it: measured on the Engelberg
// box at noon in summer.
const CROWN_GAIN: f32 = 2.0;
const PHASE_FROM_M: f32 = 1500.0;
const PHASE_TO_M: f32 = 4500.0;

// The factor of a forest tree's crown on the ground's colour (worn), 0 for any other model.
fn wear_of(m: Model, ground: u32) -> f32 {
    return select(0.0, CROWN_GAIN / m.luma, m.luma > 0.0 && m.tie <= 0.0 && luma(unpack4x8unorm(ground).rgb) > 0.0);
}

// A crown's colour in its phase, `phased`, worn on the ground's colour by `wear` (wear_of) `d` metres
// from the eye; `summer` is its colour in summer.
fn worn(phased: vec3f, summer: vec3f, ground: u32, wear: f32, d: f32) -> vec3f {
    if (wear <= 0.0) {
        return phased;
    }
    let g = unpack4x8unorm(ground).rgb;
    let change = (phased - summer) * (1.0 - smoothstep(PHASE_FROM_M, PHASE_TO_M, d));
    return wear * max(luma(summer) * g * g + luma(g * g) * change, vec3f(0.0));
}

// An albedo under the share of snow of an instance's look, which settles on the faces that turn up.
fn snowed(albedo: vec3f, up: f32, look: u32) -> vec3f {
    return mix(albedo, SNOW, f32((look >> 16u) & 255u) / 255.0 * smoothstep(0.1, 0.5, up));
}

// The share of the sun at an instance's base, from its ground (emit_tree).
fn sun_of(ground: u32) -> f32 {
    return f32(ground >> 26u) / 63.0;
}

// A crown's colour in the phase of its instance (emit_tree).
fn toned(colour: vec3f, model: u32, ground: u32) -> vec3f {
    let phase = (ground >> 24u) & 3u;
    if (phase == PHASE_SUMMER) {
        return colour;
    }
    return max(tones[model * PHASES + phase] * colour, vec3f(0.0));
}

fn far_tone(m: Model, d: f32) -> f32 {
    return select(mix(1.0, FAR_TONE, smoothstep(FAR_FROM_M, FAR_TO_M, d)), 1.0, m.crown < 0.0);
}

// The height of a point of a tree in model units, with its crown's base raised to `lift`: in a dense
// stand the low branches die, so a walker sees trunks under a roof. The crown keeps its width and
// squeezes into the height above `lift`. The trunk, whose vertices carry its top (`top` > 0),
// stretches so that its top meets the point of the crown that it met before: the crown never floats
// above its trunk, and the trunk stands on the ground. A model without a trunk keeps its shape.
fn lifted(z: f32, top: f32, m: Model, lift: f32) -> f32 {
    if (m.trunk == 0.0 || lift <= m.crown) {
        return z;
    }
    let raise = (1.0 - lift) / (1.0 - m.crown);
    if (top > 0.0) {
        return z * max(1.0, (lift + (top - m.crown) * raise) / top);
    }
    return lift + (z - m.crown) * raise;
}

// A vertex of an instance in render space; a face that rises stands `rise` of its height.
fn placed(v: MeshIn, i: Instance, m: Model, rise: f32) -> vec3f {
    let turn = unpack2x16float(i.turn);
    let model = v.position.xyz * m.extent;
    let local = vec3f(model.xy, lifted(model.z, v.position.w, m, f32((i.look >> 8u) & 255u) / 255.0) * mix(1.0, rise, v.normal.w));
    return i.at.xyz + i.at.w * vec3f(turn.x * local.x - turn.y * local.y, turn.y * local.x + turn.x * local.y, local.z);
}

// A vertex of a caster in the shadow map (light/shadow.ts), whose frame holds the sun's view. A flame casts no shadow.
@vertex
fn caster_vertex(v: MeshIn, @builtin(instance_index) instance: u32) -> @builtin(position) vec4f {
    return frame.view_proj * vec4f(placed(v, lists[bases[draw.x] + instance], models[draw.x / TIERS], 0.0), 1.0);
}

@vertex
fn mesh_vertex(v: MeshIn, @builtin(instance_index) instance: u32) -> MeshOut {
    let i = lists[bases[draw.x] + instance];
    let m = models[draw.x / TIERS];
    let model = v.position.xyz * m.extent;
    let p = placed(v, i, m, nightfall());
    let t = normalize(unpack2x16float(i.turn));
    var out: MeshOut;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.relative = p;
    let haze = haze_at(p);
    out.fog = haze.fog;
    out.air = haze.air;
    out.normal = vec3f(t.x * v.normal.x - t.y * v.normal.y, t.y * v.normal.x + t.x * v.normal.y, v.normal.z);
    // The snow last, as on an impostor, whose atlas holds the shaded crown.
    let tint = 0.75 + 0.5 * f32(i.look & 255u) / 255.0;
    // The trunk keeps its bark in every phase.
    let summer = linear(v.colour.rgb);
    let crown = worn(toned(summer, draw.x / TIERS, i.ground), summer, i.ground, wear_of(m, i.ground), length(p));
    let colour = select(crown, summer, v.position.w > 0.0);
    out.albedo = snowed(tied(colour, tint, m.tie, i.ground) * crown_shade(model, m) * far_tone(m, length(p)), v.normal.z, i.look);
    out.look = i.look;
    out.glow = (1.0 - v.colour.a) * nightfall();
    out.sun = sun_of(i.ground);
    // A plant, a rock or an animal is small: one read of the objects' shadow at its middle, lifted out
    // of its own caster, shades it whole, for less than a read per pixel of its many small faces.
    if (m.crown < 0.0) {
        out.sun = sun_share(out.sun, i.at.xyz + vec3f(0.0, 0.0, 0.5 * i.at.w), vec3f(0.0, 0.0, 1.0), i.at.w);
        out.shaded = 1u;
    }
    return out;
}

// Whether a pixel of an instance draws: the share `appear` of its pixels, by the dither.
fn kept(pixel: vec2f, look: u32) -> bool {
    return dither(pixel + vec2f(37.0, 11.0)) < f32(look >> 24u) / 255.0;
}

@fragment
fn mesh_fragment(in: MeshOut) -> @location(0) vec4f {
    if (!kept(in.clip.xy, in.look)) {
        discard;
    }
    return mesh_lit(in);
}

// The near tiers, whose instances are whole (WHOLE_TIERS of instances.ts): without a discard, a tiled
// GPU hides what they cover before it shades it.
@fragment
fn mesh_fragment_whole(in: MeshOut) -> @location(0) vec4f {
    return mesh_lit(in);
}

fn mesh_lit(in: MeshOut) -> vec4f {
    let n = normalize(in.normal);
    var sun = in.sun;
    if (in.shaded == 0u) {
        sun = crown_sun_share(sun, in.relative, n);
    }
    return vec4f(in.fog * light_up(in.albedo, n, sun, 1.0, look.horizon.w) + in.air + in.albedo * GLOW_GAIN * in.glow, 1.0);
}
