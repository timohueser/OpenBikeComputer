// #include unknown.wgsl
// The terrain mesh: one grid of 64 × 64 cells, instanced per node. A vertex morphs toward the next
// coarser grid by its node's level − LOD, as `grid_vertex` gives it, and drops with the curvature.
// #include drawn-mesh.wgsl
// #include ../ground/look.wgsl

struct Frame {
    // Camera-relative world (x east, y north, z up) to clip space; reversed Z, infinite far plane.
    view_proj: mat4x4f,
    // xyz: the direction to the sun. w: the sunlight.
    sun: vec4f,
    // The eye's height above sea level.
    eye: f32,
    // Metres of drop per square metre of distance: Earth curvature with refraction.
    curvature: f32,
    // The index of the debug view: 0 off, 1 normals, 2 LOD, 3 cracks, 4 surface, MASK_VIEW mask; look.wgsl draws the others.
    debug: u32,
}

struct Node {
    // The node's north-west corner in whole units about the anchor.
    origin: vec2i,
    level: u32,
    unused: u32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> nodes: array<Node>;
// The boxes of the tunnels' portals near the eye: x the count; then per box its facade's middle in units
// about the anchor and its way into the tunnel; in metres its depth, its half-width, the heights of its
// roof's top and of its walls' foot at the facade; and the rise of both per metre in.
@group(0) @binding(2) var<storage, read> holes: array<vec4f>;

// The nodes with a tunnel's portal draw no terrain in its box between its walls' foot and its roof: the
// bore shows through.
override HOLES: bool = false;

// Metres from a point of the terrain, `u` units about the anchor at height `h`, to the nearest portal
// box, negative inside. The distance is convex, so its interpolation over a triangle never clips the
// terrain outside a box: beside it, behind it, on the hill over its roof or on the slope under it.
fn hole_distance(u: vec2f, h: f32) -> f32 {
    var d = 1e6;
    for (var i = 0u; i < u32(holes[0].x); i++) {
        let a = holes[1u + 3u * i];
        let b = holes[2u + 3u * i];
        let rise = holes[3u + 3u * i].x;
        let p = (u - a.xy) * surface.unit;
        let along = dot(p, a.zw);
        let across = abs(a.z * p.y - a.w * p.x);
        let height = h - rise * along;
        d = min(d, max(max(max(-along, along - b.x), across - b.y), max(height - b.z, b.w - height)));
    }
    return d;
}

const CELLS = 64u;

@vertex
fn vertex_main(@builtin(vertex_index) index: u32, @builtin(instance_index) instance: u32) -> Varyings {
    let node = nodes[instance];
    let grid = vec2i(vec2u(index % (CELLS + 1u), index / (CELLS + 1u)));
    let v = grid_vertex(node.origin + grid * (1 << (24u - node.level)), node.level, true);
    // A vertex of unknown height stands on the ground under the eye; `mark_known` drops each triangle that has one.
    var p = vec3f(to_render(v.s), select(-surface.above, v.h - frame.eye, known(v.h)));
    p.z -= frame.curvature * dot(p.xy, p.xy);
    var out: Varyings;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.s = v.s;
    out.relative = p;
    mark_known(&out, known(v.h));
    mark_hole(&out, select(1e6, hole_distance(units_of(v.s), v.h), HOLES));
    out.h = v.h;
    out.level = node.level;
    return out;
}

// The debug view of the mask, as `mask` in DEBUG_VIEWS of view.ts.
const MASK_VIEW: u32 = 10u;

// The road surfaces of the mask, by code: main, road, gravel, dirt, grass, path, oneway, paving, then the
// tracks in purple: standard and narrow gauge, each without and with a rack.
const SURFACE_COLOURS = array<vec3f, 12>(vec3f(0.2, 0.2, 0.22), vec3f(0.35, 0.35, 0.37), vec3f(0.62, 0.55, 0.42),
    vec3f(0.5, 0.36, 0.22), vec3f(0.45, 0.6, 0.3), vec3f(0.85, 0.75, 0.55), vec3f(0.16, 0.22, 0.34), vec3f(0.75, 0.74, 0.7),
    vec3f(0.45, 0.2, 0.55), vec3f(0.62, 0.35, 0.75), vec3f(0.3, 0.1, 0.4), vec3f(0.5, 0.25, 0.6));

// The decoded features, flat on grey terrain: roads by surface, sidewalks as paving, cut and fill faces
// orange, conflicts red, fords cyan.
fn mask_view(s: vec2f, grey: vec3f) -> vec3f {
    let mask = mask_at(s, u32(ceil(lod(s))), false);
    var colour = grey;
    if (mask.level == 0u) {
        return colour;
    }
    if (mask.kind.x != 0u && mask.edge < 0.0) {
        colour = select(SURFACE_COLOURS[mask.kind.x - 1u], vec3f(0.2, 0.85, 0.9), (mask.kind.y & FORDED) != 0u);
    } else if (mask.kind.x != 0u && mask.edge < mask.walk) {
        colour = SURFACE_COLOURS[PAVING_CODE - 1u];
    }
    colour = mix(colour, vec3f(0.95, 0.55, 0.1), 0.7 * mask.face);
    if ((mask.kind.y & CONFLICT) != 0u) {
        colour = vec3f(0.9, 0.1, 0.1);
    }
    return colour;
}

fn hue(level: f32) -> vec3f {
    let h = fract(level * 0.29 + vec3f(0.0, 2.0 / 3.0, 1.0 / 3.0));
    return clamp(abs(h * 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
}

@fragment
fn fragment_main(in: Fragment) -> @location(0) vec4f {
    drop_unknown(in);
    if (frame.debug == 0u || (frame.debug > 4u && frame.debug != MASK_VIEW)) {
        return vec4f(ground_colour(in.s, in.relative, in.h, in.level), 1.0);
    }
    let n = render_normal(drawn_normal(in.s));
    let light = max(dot(n, frame.sun.xyz), 0.0) * frame.sun.w + 0.3;
    if (frame.debug == 1u) {
        return vec4f(n * 0.5 + 0.5, 1.0);
    }
    if (frame.debug == 2u) {
        // A hue per level, darker through the morph band toward the next coarser level.
        let lambda = lod(in.s);
        return vec4f(hue(ceil(lambda)) * (1.0 - 0.6 * (ceil(lambda) - lambda)) * (0.5 + 0.5 * light), 1.0);
    }
    if (frame.debug == 3u) {
        // Grey only: the red clear colour shows through a crack and nowhere else.
        return vec4f(vec3f(0.25 + 0.5 * light / 1.3), 1.0);
    }
    if (frame.debug == 4u) {
        // Red where drawnHeight leaves the mesh by 5 cm, grey elsewhere. The rounding of the float32
        // position about the eye grows with the distance, so a far pixel on a steep slope can show red.
        let error = abs(drawn_height(in.s) - in.h);
        return select(vec4f(vec3f(0.25 + 0.5 * light / 1.3), 1.0), vec4f(1.0, 0.0, 0.0, 1.0), !(error < 0.05));
    }
    return vec4f(mask_view(in.s, vec3f(0.25 + 0.5 * light / 1.3)), 1.0);
}
