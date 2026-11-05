// The debug draw of the ground plan over the canvas, after the resolve (plan/draw.ts): the lines and
// fills of plan/debug.ts, LIFT_M above the drawn surface, in the debug tool's sRGB colours. A primitive
// with a vertex of unknown height draws nothing. A fragment more than HIDDEN_M behind the scene's depth
// is hidden: a hill hides the plan behind it, but no house or tree hides it from above.
// #include ../terrain/drawn-mesh.wgsl

// As Frame of terrain.wgsl, which shares its buffer.
struct Frame {
    view_proj: mat4x4f,
    sun: vec4f,
    eye: f32,
    curvature: f32,
    debug: u32,
    pixel: f32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
// xy: render pixels per canvas pixel.
@group(0) @binding(1) var<uniform> ratio: vec4f;
// The scene's depth in its top left, reversed Z: depth = NEAR / distance along the view.
@group(2) @binding(0) var scene_depth: texture_depth_2d;

const LIFT_M: f32 = 0.05;
const HIDDEN_M: f32 = 50.0;
// As NEAR of frame/camera.ts.
const NEAR: f32 = 0.1;

struct PlanIn {
    // World units.
    @location(0) at: vec2i,
    // sRGB with alpha.
    @location(1) colour: vec4f,
}

struct PlanOut {
    @builtin(position) clip: vec4f,
    // Premultiplied.
    @location(0) colour: vec4f,
    // 1 where the surface under the vertex is known.
    @location(1) known: f32,
    // The distance along the view.
    @location(2) along: f32,
}

@vertex
fn plan_vertex(v: PlanIn) -> PlanOut {
    let s = lod_space(vec2f(v.at - surface.anchor));
    let h = stand_height(s, v.at);
    let r = to_render(s);
    // A vertex of unknown height takes the eye's ground, as the route's does.
    let z = select(-surface.above, h + LIFT_M - frame.eye, known(h)) - frame.curvature * dot(r, r);
    var out: PlanOut;
    out.clip = frame.view_proj * vec4f(r, z, 1.0);
    out.colour = vec4f(v.colour.rgb * v.colour.a, v.colour.a);
    out.known = select(0.0, 1.0, known(h));
    out.along = out.clip.w;
    return out;
}

@fragment
fn plan_fragment(in: PlanOut) -> @location(0) vec4f {
    if (in.known < 0.999) {
        discard;
    }
    let depth = textureLoad(scene_depth, vec2i(floor(in.clip.xy * ratio.xy)), 0);
    if (depth > 0.0 && in.along > NEAR / depth + HIDDEN_M) {
        discard;
    }
    return in.colour;
}
