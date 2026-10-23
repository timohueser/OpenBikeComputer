// The paint of roads and paths over the ground, from the feature mask: a port of `roadSurface` and the
// road section of `groundOn` of the prototype. The mask's distance to the road edge is linear across
// it, so the filtered distance keeps the edge sharp to a pixel. look.wgsl includes it, and it reads
// `rule()` and `noise()` of look.wgsl: an include composes each file once.
// #include ../terrain/features.wgsl
// #include look.wgsl

// The terrain draws the nodes of the levels with masks with the paint, the others without it: the
// compiler drops the paint there, and with it the registers that it takes from every pixel.
override PAINT: bool = true;

struct Road {
    colour: vec3f,
    // The share of the pixel that the road covers.
    cover: f32,
    // The share of the ground that shows through the road's paint.
    share: f32,
    // On a cut or a fill face of a paved road that is a retaining wall: faded as the paint.
    wall: f32,
}

// The cone of a road's cut or fill (cut and fill 0.85, reach 10 m in roads.json) leaves the road's edge
// at 40° and passes 75° this many metres out.
const WALL_M: f32 = 5.2;

// The tracks, from RAIL_CODE on as SURFACES of bins.ts: standard and narrow gauge, each without and with
// a rack. A track painted where nothing counts its sleepers, on a deck or in a bore, takes NO_SLEEPERS
// and shows their share.
const NO_SLEEPERS: f32 = -1e9;
// Linear colours: ballast, concrete sleepers, a rail's worn head, its rusty side and a rack.
const BALLAST: vec3f = vec3f(0.23, 0.21, 0.18);
const SLEEPER: vec3f = vec3f(0.3, 0.285, 0.255);
const RAIL_HEAD: vec3f = vec3f(0.5, 0.505, 0.51);
const RAIL_SIDE: vec3f = vec3f(0.09, 0.06, 0.045);
const RACK: vec3f = vec3f(0.06, 0.055, 0.05);

// A track `across` metres from its middle, its sleepers counted `sleepers` from its start, seen by a
// pixel w metres wide that spans `cross_w` metres across it and `along_w` along it: ballast, sleepers
// 0.26 m wide that fade to their share of the ballast as a pixel grows past them along the track, and the
// rails, with a rack between them.
fn track_surface(code: u32, across: f32, sleepers: f32, p: vec2f, w: f32, cross_w: f32, along_w: f32) -> vec3f {
    let narrow = ((code - RAIL_CODE) & 1u) != 0u;
    // The middle of a rail's head, and the half-length of a sleeper.
    let gauge = select(0.753, 0.53, narrow);
    let reach = select(1.3, 0.95, narrow);
    let near = 1.0 - smoothstep(0.05, 0.25, w);
    var c = BALLAST * (0.9 + 0.12 * noise(p / 2.5) + 0.2 * (noise(p / 0.09) - 0.5) * near);
    let off = abs(fract(sleepers + 0.5) - 0.5) * SLEEPER_SPACING_M;
    let share = 0.26 / SLEEPER_SPACING_M;
    let lying = select(mix(band(off, -0.13, 0.13, along_w), share, smoothstep(0.25, 0.55, along_w)), share, sleepers <= NO_SLEEPERS);
    c = mix(c, SLEEPER, lying * band(across, -reach, reach, cross_w));
    let rail = abs(across - gauge);
    c = mix(c, RAIL_SIDE, band(rail, -0.07, 0.07, cross_w));
    c = mix(c, RAIL_HEAD, band(rail, -0.036, 0.036, cross_w));
    if (code >= RAIL_CODE + 2u) {
        c = mix(c, RACK, band(across, -0.06, 0.06, cross_w));
    }
    return c;
}

// The share of a pixel w metres wide, centred on x, that falls inside [a, b].
fn band(x: f32, a: f32, b: f32, w: f32) -> f32 {
    return clamp((min(x + 0.5 * w, b) - max(x - 0.5 * w, a)) / w, 0.0, 1.0);
}

// A road's surface at `inside` metres in from its edge and `across` metres from its middle, `half`
// metres wide on each side, seen by a pixel w metres wide that spans `cross_w` metres across the road, with the share of the ground beside it that shows through. Paved main roads
// keep a line along each edge and one in the middle; lanes and tracks wear two wheel ruts, grass and
// dirt ones with grass between; gravel lies in grains. Codes as SURFACES of bins.ts: main, road,
// gravel, dirt, grass, path, and oneway: a one-way main road, no line in its middle; then the tracks.
fn road_surface(region: u32, code: u32, inside: f32, across: f32, half: f32, sleepers: f32, p: vec2f, w: f32, cross_w: f32, along_w: f32) -> vec4f {
    if (code >= RAIL_CODE) {
        return vec4f(track_surface(code, across, sleepers, p, w, cross_w, along_w), 0.0);
    }
    let oneway = code == 7u;
    var c = rule(region, slot_roads() + i32(select(code, 1u, oneway)) - 1).rgb;
    // Patched asphalt, compacted gravel: broad tone, too faint to alias far off, and a grain near by.
    c *= 0.94 + 0.1 * noise(p / 4.0) + 0.06 * (noise(p / 0.35) - 0.5) * (1.0 - smoothstep(0.05, 0.3, w));
    // Only roads of two full lanes carry lines. The middle one is solid: the mask holds no metres
    // along the road for dashes.
    if ((code == 1u || oneway) && half >= 3.5) {
        let line = rule(region, slot_road_line()).rgb;
        c = mix(c, line, max(band(inside, 0.25, 0.37, cross_w), select(band(across, -0.06, 0.06, cross_w), 0.0, oneway)));
    }
    if (code == 3u) {
        c *= 0.93 + 0.14 * mix(0.5, noise(p / 0.18), 1.0 - smoothstep(0.05, 0.25, w));
    }
    let ruts = band(across, 0.55, 1.05, cross_w);
    // One car wide.
    let lane = half < 2.6;
    if ((code == 2u && lane) || code == 3u) {
        c *= 1.0 - 0.08 * ruts;
    }
    if (code == 4u) {
        return vec4f(c * (1.0 - 0.18 * ruts), band(across, -1.0, 0.3 + 0.12 * noise(p / 0.6), cross_w) * 0.85);
    }
    // Grass with the dirt of the ruts in it.
    if (code == 5u) {
        let r = ruts * 0.8;
        return vec4f((0.35 * (1.0 - r) * c + r * rule(region, slot_roads() + 3).rgb) / (0.35 + 0.65 * r), 0.65 * (1.0 - r));
    }
    return vec4f(c, 0.0);
}

// A retaining wall of `stone`, `along` metres along its face and `altitude` up it, seen by a pixel w
// metres wide: courses 0.42 m high of blocks 0.9 m long, each its own tone, in dark joints.
fn masonry(stone: vec3f, along: f32, altitude: f32, w: f32) -> vec3f {
    let course = altitude / 0.42;
    let row = floor(course);
    let block = along / 0.9 + 0.5 * row + 0.3 * hash(vec2f(row, 7.0));
    let near = 1.0 - smoothstep(0.3, 1.0, w);
    let c = stone * (1.0 + 0.3 * (hash(vec2f(floor(block), row)) - 0.5) * near);
    let bed = abs(fract(course + 0.5) - 0.5) * 0.42;
    let head = abs(fract(block + 0.5) - 0.5) * 0.9;
    let joint = max(band(bed, -0.025, 0.025, w), band(head, -0.025, 0.025, w));
    // Far off, the joints are a share of the tone.
    return c * mix(0.93, 1.0 - 0.45 * joint, near);
}

// The road at a point of the ground: `s` in LOD space with its screen derivatives `ds`, `p` in world
// metres, a pixel `w` metres wide, `distance` metres from the eye. A road narrower than a pixel draws one pixel wide, as
// faint as its share of it. The paint fades out before the reach of the mask pool and in the outer
// half of the morph band of the coarsest level with masks.
fn road_at(s: vec2f, ds: mat2x2f, p: vec2f, w: f32, distance: f32, region: u32) -> Road {
    if (!PAINT) {
        return Road(vec3f(0.0), 0.0, 0.0, 0.0);
    }
    let level = lod(s);
    let reach = (1.0 - smoothstep(0.75 * masks.reach, masks.reach, distance)) * smoothstep(f32(MASK_LEVEL) - 1.0, f32(MASK_LEVEL) - 0.5, level);
    if (reach <= 0.0) {
        return Road(vec3f(0.0), 0.0, 0.0, 0.0);
    }
    let mask = mask_at(s, u32(ceil(level)), true);
    if (mask.level == 0u || mask.kind.x == 0u) {
        return Road(vec3f(0.0), 0.0, 0.0, 0.0);
    }
    let code = mask.kind.x;
    let paved = code <= 2u || code == 7u;
    var wall = select(0.0, reach * mask.shape.w, paved || code >= RAIL_CODE);
    // A face is a wall from the road's edge out where it is still a face WALL_M out, where its cone is
    // steeper than 75°; elsewhere the slope stays ground.
    if (wall > 0.0) {
        let out = mask_at(s + mask.away * (WALL_M - mask_metres(mask.shape.x)), mask.level, false);
        wall *= out.shape.w;
    }
    var edge = mask_metres(mask.shape.x);
    if (edge > w + 0.5) {
        return Road(vec3f(0.0), 0.0, 0.0, wall);
    }
    let across = abs(mask.across);
    let half = max(mask.half, 0.05);
    // The pixel's span across the road: far off, a pixel is long along the view and narrow across it,
    // so the lines of a road that runs away from the eye stay sharp.
    let span = length(vec2f(dot(ds[0], mask.away), dot(ds[1], mask.away)));
    let cross_w = select(w, max(span, 1e-3), dot(mask.away, mask.away) > 0.0);
    let ahead = vec2f(-mask.away.y, mask.away.x);
    let along_w = select(w, max(length(vec2f(dot(ds[0], ahead), dot(ds[1], ahead))), 1e-3), dot(mask.away, mask.away) > 0.0);
    let surface = road_surface(region, code, max(-edge, 0.0), across, half, mask.sleepers, p, w, cross_w, along_w);
    // Unpaved edges fray into the ground.
    if (!paved) {
        edge += 0.36 * (0.7 * noise(p / 1.1) + 0.3 * noise(p / 0.35) - 0.5);
    }
    let thin = max(0.0, 0.5 * cross_w - half);
    let cover = (1.0 - smoothstep(-cross_w, cross_w, edge - thin)) * min(1.0, 2.0 * half / cross_w + 0.35) * reach;
    return Road(surface.rgb, cover, surface.w, wall);
}
