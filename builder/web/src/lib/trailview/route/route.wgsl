// The planned route: a ribbon LIFT_M above the drawn surface through the points of route.ts, drawn after
// the terrain. Near, it lies on the ground BODY_M to each side, its edges draped on the surface; where
// lying would show it thinner than BODY_PIXELS it turns to face the eye. A crisp body with a lighter middle
// covers the ground, and a narrow glow beyond its edges adds light. Its depth moves toward the eye by
// PULL_M and PULL_PIXELS, so that the ground between two points does not hide it. It is not lit.
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

// As POINT_WORDS of route.ts.
struct Point {
    at: vec2i,
    // Metres along the route.
    along: f32,
    // The first and the last point of the grade window about the point.
    behind: u32,
    ahead: u32,
}

// As `styleOf` of route.ts: colours in linear light.
struct Style {
    // w: 1 when the line takes the colour of its grade band.
    line: vec4f,
    // The grade thresholds in percent, as gradeThresholds of grade-data.ts.
    steps: array<vec4f, 2>,
    // As gradeBands of grade-data.ts: the steepest descent first, then flat, then the climbs, then unknown.
    bands: array<vec4f, 12>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> points: array<Point>;
@group(0) @binding(2) var<uniform> style: Style;

const LIFT_M: f32 = 0.3;
// The half-width of the body and the width of the glow beyond it, in metres and at least in output pixels.
const BODY_M: f32 = 1.0;
const GLOW_M: f32 = 0.5;
const BODY_PIXELS: f32 = 1.5;
const GLOW_PIXELS: f32 = 2.0;
// The body's light over the line's colour: LIGHTER more in the middle, DARKER less at the edge.
const BODY_GAIN: f32 = 1.2;
const LIGHTER: f32 = 0.6;
const DARKER: f32 = 0.35;
const OPACITY: f32 = 0.95;
const GLOW_GAIN: f32 = 0.8;
const PULL_M: f32 = 0.5;
const PULL_PIXELS: f32 = 4.0;
// A grade window shorter than this has no grade, as in profileGrades of grade-data.ts.
const LEAST_WINDOW_M: f32 = 20.0;
const UNKNOWN_BAND: u32 = 11u;

struct RouteOut {
    @builtin(position) clip: vec4f,
    // The side from −1 to 1, and the share of it that the body fills.
    @location(0) side: f32,
    @location(1) body: f32,
    @location(2) colour: vec3f,
    // 1 where the surface under the vertex is known.
    @location(3) kept: f32,
}

fn centre(i: u32) -> vec2f {
    return lod_space(vec2f(points[i].at - surface.anchor));
}

// The ground under a point of LOD space.
fn ground(s: vec2f) -> f32 {
    return stand_height(s, surface.anchor + vec2i(round(units_of(s))));
}

fn direction(d: vec2f, fallback: vec2f) -> vec2f {
    return select(fallback, normalize(d), dot(d, d) > 1e-12);
}

// As gradeBand of grade-data.ts.
fn band(grade: f32) -> u32 {
    let magnitude = round(abs(grade) * 10.0) / 10.0;
    var level = 0u;
    while (level < 5u && magnitude >= style.steps[level / 4u][level % 4u]) {
        level++;
    }
    return select(5u + level, 5u - level, grade < 0.0);
}

fn colour_of(p: Point) -> vec3f {
    if (style.line.w < 0.5) {
        return style.line.rgb;
    }
    let window = points[p.ahead].along - points[p.behind].along;
    let low = ground(centre(p.behind));
    let high = ground(centre(p.ahead));
    if (window < LEAST_WINDOW_M || !known(low) || !known(high)) {
        return style.bands[UNKNOWN_BAND].rgb;
    }
    return style.bands[band((high - low) / window * 100.0)].rgb;
}

fn eye_space(s: vec2f, h: f32) -> vec3f {
    let r = to_render(s);
    return vec3f(r, h + LIFT_M - frame.eye - frame.curvature * dot(r, r));
}

@vertex
fn route_vertex(@builtin(vertex_index) v: u32) -> RouteOut {
    let n = arrayLength(&points);
    let i = v >> 1u;
    let side = f32(v & 1u) * 2.0 - 1.0;
    let s = centre(i);
    let back = direction(s - centre(max(i, 1u) - 1u), vec2f(0.0, 1.0));
    let tangent = direction(back + direction(centre(min(i + 1u, n - 1u)) - s, back), back);
    let left = vec3f(-tangent.y, tangent.x, 0.0);
    let h = ground(s);
    let c = eye_space(s, select(frame.eye - LIFT_M - surface.above, h, known(h)));
    let distance = max(length(c), 1e-6);
    // Metres of one output pixel at the point.
    let metre = frame.pixel * distance;
    let body = max(BODY_M, BODY_PIXELS * metre);
    let half = body + max(GLOW_M, GLOW_PIXELS * metre);
    // The half-width in pixels that the body shows when it lies on the ground.
    let lying_pixels = BODY_M * length(cross(left, c)) / (distance * metre);
    let lying = 1.0 - smoothstep(1.0, 2.0, BODY_PIXELS / max(lying_pixels, 1e-6));
    let foot = s + left.xy * side * half;
    let foot_h = ground(foot);
    let lie = eye_space(foot, select(h, foot_h, known(foot_h)));
    // Across the line as the eye sees it, on the side of left; a line that runs straight at the eye turns flat.
    let facing = cross(vec3f(tangent, 0.0), c) * select(1.0, -1.0, c.z > 0.0);
    let across = select(left, normalize(facing), dot(facing, facing) > 1e-6 * distance * distance);
    var p = mix(c + across * side * half, lie, lying);
    let reach = length(p);
    p *= max(reach - PULL_M - PULL_PIXELS * frame.pixel * reach, 0.5 * reach) / max(reach, 1e-6);
    var out: RouteOut;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.side = side;
    out.body = body / half;
    out.colour = colour_of(points[i]);
    // The crack and surface views show the terrain alone.
    out.kept = select(0.0, 1.0, known(h) && frame.debug != 3u && frame.debug != 4u);
    return out;
}

// Premultiplied: the body covers what lies behind it, the glow adds to it.
@fragment
fn route_fragment(in: RouteOut) -> @location(0) vec4f {
    let t = abs(in.side);
    let aa = max(fwidth(t), 1e-6);
    if (in.kept < 0.999) {
        discard;
    }
    let cover = clamp((in.body - t) / aa + 0.5, 0.0, 1.0);
    let inner = min(t / in.body, 1.0);
    let shade = BODY_GAIN * mix(1.0 + LIGHTER, 1.0 - DARKER, inner * inner);
    let outer = clamp((t - in.body) / max(1.0 - in.body, 1e-6), 0.0, 1.0);
    let glow = GLOW_GAIN * (1.0 - outer) * (1.0 - outer);
    let light = in.colour * (shade * cover * OPACITY + glow * (1.0 - cover));
    return vec4f(light, cover * OPACITY);
}
