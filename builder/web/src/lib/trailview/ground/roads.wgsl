// The paint of roads and paths over the ground, from the feature mask: a port of `roadSurface` and the
// road section of `groundOn` of the prototype. The mask's distance to the road edge is linear across
// it, so the filtered distance keeps the edge sharp to a pixel. look.wgsl includes it, and it reads
// `rule()` and `noise()` of look.wgsl: an include composes each file once.
// #include ../terrain/features.wgsl
// #include look.wgsl

struct Road {
    colour: vec3f,
    // The share of the pixel that the road covers.
    cover: f32,
    // The share of the ground that shows through the road's paint.
    share: f32,
    // On a cut or a fill face of a paved road that is a retaining wall: faded as the paint.
    wall: f32,
    // The share of the ground that a road's or a path's cut or fill shaped: within its reach of the edge.
    shaped: f32,
    // The share of the ground on a road's cut or fill, whose detail runs up its
    // face, and on a path's bench, which lights as its hillside does.
    cuts: f32,
    bench: f32,
}

// The cone of a road's cut or fill (cut and fill 0.85, reach 10 m in roads.json) leaves the road's edge
// at 40° and passes 75° this many metres out.
const WALL_M: f32 = 5.2;
// The reach of a road's cut and fill and of a path's (reach in roads.json). A line narrower than PATH_HALF_M
// is a path: PATH_HALF of bins.ts holds most paths to 1 m, and no road without a `width` tag is narrower
// than 1.25 m.
const SHAPED_M: f32 = 10.0;
const PATH_SHAPED_M: f32 = 3.0;
const PATH_HALF_M: f32 = 1.2;
// Only a road this wide on each side carries lines.
const MARKED_HALF_M: f32 = 2.75;
// A sidewalk's granite kerb: its top, its dark face on the road side and its soft shadow on the road, in metres across.
const KERB_M: f32 = 0.18;
const KERB_FACE_M: f32 = 0.06;
const KERB_SHADOW_M: f32 = 0.35;
// The metres of each side of a way that stay bare: ruts of soil, ruts that grass grows over, a trodden tread.
const RUTS_MEAN_M: f32 = 0.75;
const GRASSED_MEAN_M: f32 = 0.55;
const TREAD_MEAN_M: f32 = 0.36;
// As SURFACES of bins.ts.
const CODE_GRAVEL: u32 = 3u;
const CODE_DIRT: u32 = 4u;
const CODE_GRASS: u32 = 5u;
const CODE_PATH: u32 = 6u;
const CODE_ONEWAY: u32 = 7u;
// Paving lies in square slabs of SLAB_M; their joints show only near the eye.
const SLAB_M: f32 = 0.6;

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

// How far a dash `on` metres long has turned to its share of a line, seen by a pixel `along_w` metres long.
fn dash_mean(on: f32, along_w: f32) -> f32 {
    return smoothstep(0.15 * on, 0.6 * on, along_w);
}

// The share of a pixel `aw` metres long along a line that falls on a dash, `on` metres of each `period`; far
// off, the dashes' share of the paint's line.
fn dashes(along: f32, on: f32, period: f32, phase: f32, aw: f32) -> f32 {
    let x = fract((along + phase) / period) * period;
    let cov = band(x, 0.0, on, aw) + band(x, period, period + on, aw) + band(x, -period, on - period, aw);
    return mix(min(cov, 1.0), on / period, dash_mean(on, aw));
}

// Whether the paint draws this pixel of a way that a corridor draws, at `distance` metres from the eye. The
// corridor discards exactly these pixels, so the two hand over without a pixel drawn twice or left out.
fn paint_draws(distance: f32, pixel: vec2f) -> bool {
    return dither(pixel) < paint_share(distance);
}

// The share of the ground in a way of soil or grass far off, where a pixel spans its ruts or its tread: what a
// corridor shows at the end of its reach (dirt_look and trail_look of roads/looks.wgsl), which the metres of
// each side that stay bare give.
fn ground_mean(code: u32, half: f32, trodden: bool) -> f32 {
    let bare = select(select(TREAD_MEAN_M, RUTS_MEAN_M, code == CODE_DIRT), GRASSED_MEAN_M, code == CODE_GRASS);
    return clamp(1.0 - select(bare, TREAD_MEAN_M, trodden) / half, 0.0, 1.0);
}

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

// A town's paving at p, seen by a pixel w metres wide: slabs on the world's axes, each its own tone, in
// faint joints that fade to their share as a pixel grows past them.
fn paving(region: u32, p: vec2f, w: f32) -> vec3f {
    var c = rule(region, slot_roads() + i32(PAVING_CODE) - 1).rgb * (0.95 + 0.1 * noise(p / 5.0));
    let near = 1.0 - smoothstep(0.02, 0.08, w);
    if (near > 0.0) {
        let slab = p / SLAB_M;
        let joint = (0.5 - max(abs(fract(slab.x) - 0.5), abs(fract(slab.y) - 0.5))) * SLAB_M;
        c *= 1.0 + near * (0.08 * (hash(floor(slab)) - 0.5) - 0.15 * band(joint, -0.008, 0.008, w));
    }
    return c;
}

// A road's surface with its sidewalk, `walk` metres wide past its edge, at `edge` metres out from the edge:
// the paving, a granite kerb at the edge with its dark face on the road side, and its shadow on the road.
fn kerbed(region: u32, road: vec3f, edge: f32, walk: f32, p: vec2f, w: f32, cross_w: f32) -> vec3f {
    let kerb = rule(region, slot_kerb()).rgb;
    var c = road * (1.0 - 0.3 * clamp(1.0 + edge / KERB_SHADOW_M, 0.0, 1.0) * band(edge, -KERB_SHADOW_M, 0.0, cross_w));
    c = mix(c, 0.4 * kerb, band(edge, -KERB_FACE_M, 0.0, cross_w));
    c = mix(c, paving(region, p, w), band(edge, KERB_M, walk, cross_w));
    return mix(c, kerb * (0.94 + 0.12 * noise(p / 0.9)), band(edge, 0.0, KERB_M, cross_w));
}

// A road's surface at `inside` metres in from its edge and `across` metres from its middle, `half`
// metres wide on each side, seen by a pixel w metres wide that spans `cross_w` metres across the road, with the share of the ground beside it that shows through. Paved main roads
// keep a line along each edge and one in the middle; asphalt wears no ruts, gravel, dirt and grass
// tracks wear two, the dirt and grass ones with grass between; gravel lies in grains. Codes as SURFACES of bins.ts: main, road,
// gravel, dirt, grass, path, oneway: a one-way main road, no line in its middle; paving; then the tracks.
fn road_surface(region: u32, code: u32, inside: f32, across: f32, half: f32, sleepers: f32, p: vec2f, w: f32, cross_w: f32, along_w: f32, marked: bool, edged: f32) -> vec4f {
    if (code >= RAIL_CODE) {
        return vec4f(track_surface(code, across, sleepers, p, w, cross_w, along_w), 0.0);
    }
    if (code == PAVING_CODE) {
        return vec4f(paving(region, p, w), 0.0);
    }
    let oneway = code == CODE_ONEWAY;
    var c = rule(region, slot_roads() + i32(code) - 1).rgb;
    // Patched asphalt, compacted gravel: broad tone, too faint to alias far off, and a grain near by.
    c *= 0.94 + 0.1 * noise(p / 4.0) + 0.06 * (noise(p / 0.35) - 0.5) * (1.0 - smoothstep(0.05, 0.3, w));
    // The mask holds no metres along the road for dashes. The paint takes the corridors' lines, so the two hand
    // over unseen: `marked`, their widths, and far off the share of their dashes. A deck or a bore has the metres
    // in `sleepers`, and the dashes themselves.
    if (marked && half >= MARKED_HALF_M) {
        let line = rule(region, slot_road_line()).rgb;
        let edge = band(inside, 0.2, 0.35, cross_w) * edged;
        let form = rule(region, slot_dashes());
        let dash = select(mix(1.0, form.x / form.y, dash_mean(form.x, along_w)), dashes(sleepers, form.x, form.y, 0.0, along_w), sleepers > NO_SLEEPERS);
        c = mix(c, line, max(edge, select(band(across, -0.06, 0.06, cross_w) * dash, 0.0, oneway)));
    }
    let ruts = band(across, 0.55, 1.05, cross_w);
    if (code == CODE_GRAVEL) {
        c *= (0.93 + 0.14 * mix(0.5, noise(p / 0.18), 1.0 - smoothstep(0.05, 0.25, w))) * (1.0 - 0.08 * ruts);
    }
    if (code == CODE_DIRT) {
        return vec4f(c * (1.0 - 0.18 * ruts), band(across, -1.0, 0.3 + 0.12 * noise(p / 0.6), cross_w) * 0.85);
    }
    // Grass with the dirt of the ruts in it.
    if (code == CODE_GRASS) {
        let r = ruts * 0.8;
        return vec4f((0.35 * (1.0 - r) * c + r * rule(region, slot_roads() + i32(CODE_DIRT) - 1).rgb) / (0.35 + 0.65 * r), 0.65 * (1.0 - r));
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
// half of the morph band of the coarsest level with masks. `pixel` is the pixel on the screen, `flat` the z of the
// normal of the drawn triangle.
fn road_at(s: vec2f, ds: mat2x2f, p: vec2f, w: f32, distance: f32, region: u32, pixel: vec2f, flat: f32) -> Road {
    if (!DETAIL) {
        return Road(vec3f(0.0), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    // Probe 2048: the look evaluated once per page (the mask); with 4096, per fragment nearer than the mesh range.
    if (MODEL && ((MODEL_PROBE & 2048u) == 0u || ((MODEL_PROBE & 4096u) != 0u && distance < 560.0))) {
        return model_road(s, ds, p, w, distance, region, flat);
    }
    let level = lod(s);
    let reach = (1.0 - smoothstep(0.75 * masks.reach, masks.reach, distance)) * smoothstep(f32(MASK_LEVEL) - 1.0, f32(MASK_LEVEL) - 0.5, level);
    if (reach <= 0.0) {
        return Road(vec3f(0.0), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    let mask = mask_at(s, u32(ceil(level)), true);
    if (mask.level == 0u || mask.kind.x == 0u) {
        return Road(vec3f(0.0), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    let code = mask.kind.x;
    let paved = code <= 2u || code == 7u || code == PAVING_CODE;
    // The paint and the road's cut and fill reach past its sidewalk on this side.
    let walk = mask.walk;
    // A wall stands under a road's or a track's paint, never under a footway's paving. Inside
    // the corridors' reach its masonry fades out, or a mesh wall shows a translucent ghost.
    var wall = select(0.0, reach * mask.face * paint_share(distance), code <= 2u || code == 7u || code >= RAIL_CODE);
    // A face is a wall from the road's edge out where it is still a face WALL_M out, where its cone is
    // steeper than 75°; elsewhere the slope stays ground.
    if (wall > 0.0) {
        let out = mask_at(s + mask.away * (WALL_M + walk - mask.edge), mask.level, false);
        wall *= out.face;
    }
    var edge = mask.edge;
    let half = max(mask.half, 0.05);
    let shaped_m = select(SHAPED_M, PATH_SHAPED_M, half < PATH_HALF_M);
    let shaped = reach * (1.0 - smoothstep(0.75 * shaped_m, shaped_m, edge - walk));
    let cuts = select(0.0, shaped, half >= PATH_HALF_M);
    // The bench reaches a metre past the edge: the band and a texel of the heights about it.
    let bench = select(0.0, reach * (1.0 - smoothstep(0.75, 1.25, edge)), half < PATH_HALF_M);
    if (edge - walk > w + 0.5) {
        return Road(vec3f(0.0), 0.0, 0.0, wall, shaped, cuts, bench);
    }
    let across = abs(mask.across);
    // The pixel's span across the road: far off, a pixel is long along the view and narrow across it,
    // so the lines of a road that runs away from the eye stay sharp.
    let known = dot(mask.axis, mask.axis) > 0.0;
    let cross_w = select(w, max(length(vec2f(dot(ds[0], mask.axis), dot(ds[1], mask.axis))), 1e-3), known);
    let ahead = vec2f(-mask.axis.y, mask.axis.x);
    let along_w = select(w, max(length(vec2f(dot(ds[0], ahead), dot(ds[1], ahead))), 1e-3), known);
    var surface = road_surface(region, code, max(-edge, 0.0), across, half, select(NO_SLEEPERS, mask.sleepers, code >= RAIL_CODE), p, w, cross_w, along_w, mask.marked, 1.0);
    if (code >= CODE_DIRT && code <= CODE_PATH) {
        surface.w = mix(surface.w, ground_mean(code, half, mask.trodden), smoothstep(0.1, 0.4, cross_w));
    }
    if (walk > 0.0) {
        surface = vec4f(kerbed(region, surface.rgb, edge, walk, p, w, cross_w), surface.w);
    }
    // Unpaved edges fray into the ground.
    if (!paved) {
        edge += 0.36 * (0.7 * noise(p / 1.1) + 0.3 * noise(p / 0.35) - 0.5);
    }
    let thin = max(0.0, 0.5 * cross_w - half);
    // A corridor draws an unpaved way, and the ground shows through it: its paint keeps only the pixels that the
    // corridor leaves it. A paved way keeps its paint: where the stamped ground lies over its corridor, the paint
    // is the way.
    let corridor = code >= CODE_GRAVEL && code <= CODE_PATH && !paint_draws(distance, pixel);
    // A triangle that rises over a sidewalk's edge would carry its paint up the cut: a corridor draws the sidewalk.
    let riding = edge > 0.0 && flat < 0.86 && !paint_draws(distance, pixel);
    let cover = select(1.0, 0.0, corridor || riding) * (1.0 - smoothstep(-cross_w, cross_w, edge - walk - thin)) * min(1.0, 2.0 * half / cross_w + 0.35) * reach;
    return Road(surface.rgb, cover, surface.w, wall, shaped, cuts, bench);
}

// The flat colour of a label in the model's debug view, as the crate's `debug model` pictures draw it.
fn model_key(block: u32, id: u32) -> vec3f {
    let surface = model_surface(block, id);
    let finish = model_finish(block, id) & 0xffu;
    var c = vec3f(236.0, 236.0, 228.0);
    if (surface == 1u) {
        c = vec3f(205.0, 196.0, 140.0);
    } else if (surface == 2u) {
        c = vec3f(120.0, 100.0, 90.0);
        if (finish == 1u || finish == 7u) { c = vec3f(95.0, 95.0, 102.0); }
        if (finish == 2u) { c = vec3f(125.0, 125.0, 130.0); }
        if (finish == 3u) { c = vec3f(170.0, 150.0, 115.0); }
        if (finish == 4u || finish == 5u) { c = vec3f(150.0, 120.0, 80.0); }
        if (finish == 6u) { c = vec3f(190.0, 150.0, 110.0); }
        if (finish == 8u) { c = vec3f(160.0, 150.0, 150.0); }
        if (model_owner_kind(block, id) == 2u) { c = c * 0.5 + vec3f(70.0, 30.0, 95.0); }
    } else if (surface == 3u) {
        c = vec3f(180.0, 174.0, 158.0);
    } else if (surface == 4u) {
        c = vec3f(140.0, 185.0, 110.0);
    } else if (surface == 6u) {
        c = vec3f(210.0, 60.0, 60.0);
    } else if (surface == 7u) {
        c = vec3f(125.0, 95.0, 75.0);
    }
    return pow(c / 255.0, vec3f(2.2));
}

// Whether a label is paved: a carriageway or a crossing, which the paint draws.
fn model_paved(block: u32, id: u32) -> bool {
    let surface = model_surface(block, id);
    return surface == S_CARRIAGEWAY || surface == S_CROSSING || surface == S_RAILBED;
}

// The surface of a label at a point of its material line, as road_surface draws a way.
fn model_surface_look(block: u32, id: u32, mp: ModelPoint, p: vec2f, w: f32, cross_w: f32, along_w: f32, region: u32) -> vec4f {
    let flags = model_finish(block, id);
    // One asphalt look: a road of a lesser class (code 2) paints as a main road (code 1), so a class
    // change makes no seam; only a change of material does.
    var code = select(flags & 0xffu, 1u, (flags & 0xffu) == 2u);
    if (model_surface(block, id) == S_RAILBED && code < RAIL_CODE) {
        code = RAIL_CODE;
    }
    let marked = (flags & 256u) != 0u;
    let along = select(mp.along, mp.along / SLEEPER_SPACING_M, code >= RAIL_CODE);
    // The edge line runs along the paved outline where it is the label's own way's edge (its hosts'), and
    // along a round only where both arms that the round joins wear lines. So a host's line stops at a mouth,
    // as tangent as the round, and a guest's stub inside the junction wears none.
    let on_round = 1.0 - smoothstep(0.03, 0.12, mp.d_corner - mp.d_paved);
    let on_edge = 1.0 - smoothstep(0.05, 0.25, abs(max(-mp.d_paved, 0.0) - (mp.half - abs(mp.across))));
    let edged = max(on_round * select(0.0, 1.0, mp.corner_lined), on_edge * (1.0 - on_round));
    var surface = road_surface(region, code, max(-mp.d_paved, 0.0), abs(mp.across), max(mp.half, 0.05), along, p, w, cross_w, along_w, marked, edged);
    if (code >= CODE_DIRT && code <= CODE_PATH) {
        surface.w = mix(surface.w, ground_mean(code, mp.half, false), smoothstep(0.1, 0.4, cross_w));
    }
    return surface;
}

// The road of the one road model at a point: the label of the least key among GP3's candidates there,
// evaluated in this fragment, and its paint from the distances to the paved outline and to the label's
// boundary, so no edge is a texel's. Walls and the shaped ground still come from the far stamp's mask.
fn model_road(s: vec2f, ds: mat2x2f, p: vec2f, w: f32, distance: f32, region: u32, flat: f32) -> Road {
    var wall = 0.0;
    var shaped = 0.0;
    var cuts = 0.0;
    let level = lod(s);
    let mask = mask_at(s, u32(ceil(level)), true);
    if (mask.level != 0u && mask.kind.x != 0u) {
        let code = mask.kind.x;
        wall = select(0.0, mask.face, code <= 2u || code == 7u || code >= RAIL_CODE);
        if (wall > 0.0) {
            let out = mask_at(s + mask.away * (WALL_M - mask.edge), mask.level, false);
            wall *= out.face;
        }
        // The mask holds the distance to the model's core; its slopes reach the class's reach and round
        // into the terrain over a few metres more, all of it shaped ground.
        let shaped_m = select(SHAPED_M, PATH_SHAPED_M, mask.half < PATH_HALF_M);
        shaped = 1.0 - smoothstep(shaped_m, min(shaped_m + 4.0, 12.5), mask.edge);
        cuts = select(0.0, shaped, mask.half >= PATH_HALF_M);
    }
    if ((MODEL_PROBE & 1u) != 0u) {
        wall = 0.0;
    }
    if ((MODEL_PROBE & 32u) != 0u) {
        return Road(vec3f(0.0), 0.0, 0.0, wall, shaped, cuts, 0.0);
    }
    // The stamp's texels hold the distance to the core's outline: a fragment that the paint cannot reach
    // reads no record, and its shaped ground comes from that distance.
    // Conservative: the core contains the paved area, and a bilinear distance is off by at most half a
    // texel's diagonal (under 1.2 m on the mask levels), inside the 2 m margin. Probe 8192 turns it off.
    if ((MODEL_PROBE & 8192u) == 0u && mask.level != 0u && (mask.kind.x == 0u || mask.edge > 2.0 * w + 2.0)) {
        return Road(vec3f(0.0), 0.0, 0.0, wall, shaped, cuts, 0.0);
    }
    // Probes of the cost: 64 no record past the masks' reach, 128 none outside the core.
    // 256: none nearer than the mesh range, where GP9's pieces draw the ground and hide the terrain.
    if (((MODEL_PROBE & 64u) != 0u && mask.level == 0u) || ((MODEL_PROBE & 128u) != 0u && mask.edge > 0.0) || ((MODEL_PROBE & 256u) != 0u && distance < 560.0)) {
        return Road(vec3f(0.0), 0.0, 0.0, wall, shaped, cuts, 0.0);
    }
    let mp = model_at_s(s, max(1.5 * w, 0.05));
    if (mp.block == 0u) {
        return Road(vec3f(0.0), 0.0, 0.0, wall, shaped, cuts, 0.0);
    }
    if (MODEL_VIEW == 5u) {
        // Which fragments read records (blue), have a candidate in their bin (red), and lie where the paint
        // can reach (green: within 2 px + 1 m of the paved area).
        let reach = mp.d_paved < 2.0 * w + 1.0;
        return Road(vec3f(select(0.0, 1.0, mp.d_paved < M_FAR || mp.label != M_NONE), select(0.0, 1.0, reach), 1.0), 1.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    if ((MODEL_PROBE & 8u) != 0u || MODEL_VIEW >= 2u) {
        // The work of the fragment: red the words of its bin's items over 1,024, green its items over 64.
        // On or off per threshold, so the screen's tone curve keeps them: view 2 words > 256 and items
        // > 16, view 3 words > 512 and items > 32, view 4 words > 128 and items > 8.
        let k = select(select(1.0, 0.5, MODEL_VIEW == 4u), 2.0, MODEL_VIEW == 3u);
        return Road(vec3f(select(0.0, 1.0, mp.across > 256.0 * k), select(0.0, 1.0, mp.d_label > 16.0 * k), 1.0), 1.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    // The ground that the slopes shape: the slope layer, from its outline's distance, so the cliff light
    // and the cut-face look switch along a smooth line, never per texel or per facet.

    if (MODEL_VIEW == 1u) {
        let edge = 1.0 - smoothstep(0.0, w, mp.d_label);
        return Road(mix(model_key(mp.block, mp.label), model_key(mp.block, mp.alt), 0.5 * edge), 1.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    // The label that the paint draws: the point's own, else the paved one across the nearest change.
    var id = mp.label;
    if (!model_paved(mp.block, id)) {
        id = select(M_NONE, mp.alt, mp.alt != M_NONE && model_paved(mp.block, mp.alt));
    }
    if (id == M_NONE || mp.d_paved > 2.0 * w + 1.0) {
        return Road(vec3f(0.0), 0.0, 0.0, wall, shaped, cuts, 0.0);
    }
    // The pixel's span across the way and along it; LOD space has y north, the frame y south.
    let axis = vec2f(mp.axis.x, -mp.axis.y);
    let ahead = vec2f(-axis.y, axis.x);
    let cross_w = max(length(vec2f(dot(ds[0], axis), dot(ds[1], axis))), 1e-3);
    let along_w = max(length(vec2f(dot(ds[0], ahead), dot(ds[1], ahead))), 1e-3);
    var surface = model_surface_look(mp.block, id, mp, p, w, cross_w, along_w, region);
    // A change of finish inside the paved area blends over the pixel, from the distance to it.
    if (mp.alt != M_NONE && mp.alt != id && model_paved(mp.block, mp.alt) && mp.d_label < w) {
        let other = model_surface_look(mp.block, mp.alt, mp, p, w, cross_w, along_w, region);
        surface = mix(surface, other, 0.5 - 0.5 * clamp(mp.d_label / max(0.5 * w, 1e-3), 0.0, 1.0));
    }
    var edge = mp.d_paved;
    let code = model_finish(mp.block, id) & 0xffu;
    let paved = code <= 2u || code == 7u || code == PAVING_CODE;
    if (!paved) {
        edge += 0.36 * (0.7 * noise(p / 1.1) + 0.3 * noise(p / 0.35) - 0.5);
    }
    let thin = max(0.0, 0.5 * cross_w - mp.half);
    let cover = (1.0 - smoothstep(-0.5 * cross_w, 0.5 * cross_w, edge - thin)) * min(1.0, 2.0 * mp.half / cross_w + 0.35);
    return Road(surface.rgb, cover, surface.w, wall, shaped, cuts, 0.0);
}
