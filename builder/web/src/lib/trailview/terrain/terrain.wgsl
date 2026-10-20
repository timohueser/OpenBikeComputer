// The naive terrain: one mesh per Terrarium tile, lit by the sun and hazed by distance.

struct Frame {
    // Camera-relative world (x east, y north, z up) to clip space; reversed Z, infinite far plane.
    view_proj: mat4x4f,
    // xyz: the direction to the sun. w: the sunlight.
    sun: vec4f,
    // rgb: the horizon. w: the visibility at sea level in metres.
    sky: vec4f,
    // The eye's height above sea level.
    eye: f32,
    // Metres of drop per square metre of distance: Earth curvature with refraction.
    curvature: f32,
    // The index of the debug view: 0 off, 1 normals, 2 zoom of the tile.
    debug: u32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
// Per draw: xyz the tile centre relative to the eye, w the zoom of the tile.
@group(0) @binding(1) var<uniform> tiles: array<vec4f, 1024>;

struct Vertex {
    // Metres from the tile centre; z is the height above sea level.
    @location(0) offset: vec3f,
    // xyz: the normal. w: 1 where the height is known.
    @location(1) normal: vec4f,
}

struct Varyings {
    @builtin(position) clip: vec4f,
    @location(0) normal: vec3f,
    @location(1) known: f32,
    @location(2) relative: vec3f,
    @location(3) @interpolate(flat) zoom: f32,
}

// The height of the haze layer: its density falls by e every 2.5 km.
const HAZE_HEIGHT = 2500.0;

@vertex
fn vertex_main(v: Vertex, @builtin(instance_index) draw: u32) -> Varyings {
    let tile = tiles[draw];
    var p = tile.xyz + v.offset;
    p.z -= frame.curvature * dot(p.xy, p.xy);
    var out: Varyings;
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.normal = v.normal.xyz;
    out.known = v.normal.w;
    out.relative = p;
    out.zoom = tile.w;
    return out;
}

// The share of the colour that the haze hides between the eye and a point, for haze that thins with height.
fn haze(relative: vec3f) -> f32 {
    let distance = length(relative);
    let low = frame.eye / HAZE_HEIGHT;
    let rise = relative.z / HAZE_HEIGHT;
    // The mean density along the ray; a level ray has the density of its height.
    let mean = select((exp(-low) - exp(-low - rise)) / rise, exp(-low), abs(rise) < 1e-3);
    return 1.0 - exp(-distance / frame.sky.w * mean);
}

@fragment
fn fragment_main(in: Varyings) -> @location(0) vec4f {
    if (in.known < 0.99) {
        discard;
    }
    let n = normalize(in.normal);
    if (frame.debug == 1u) {
        return vec4f(n * 0.5 + 0.5, 1.0);
    }
    if (frame.debug == 2u) {
        // A hue per zoom, so that neighbouring zooms differ.
        let hue = fract(in.zoom * 0.29 + vec3f(0.0, 2.0 / 3.0, 1.0 / 3.0));
        return vec4f(clamp(abs(hue * 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0)), 1.0);
    }
    let rock = smoothstep(0.75, 0.55, n.z);
    let albedo = mix(vec3f(0.36, 0.42, 0.27), vec3f(0.52, 0.5, 0.47), rock);
    let light = max(dot(n, frame.sun.xyz), 0.0) * frame.sun.w + 0.3;
    return vec4f(mix(albedo * light, frame.sky.rgb, haze(in.relative)), 1.0);
}
