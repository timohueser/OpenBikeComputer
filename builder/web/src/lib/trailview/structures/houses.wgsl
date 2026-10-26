// The houses. `refit` fits the houses in view to the drawn surface and picks their style, in some of
// the frames that move; `fit` culls them and picks their level of detail in every one. The floor F stands
// at most half a storey (a full storey on a steep slope) under the highest ground about the footprint
// and at least `plinth` over the lowest, and the walls reach under the lowest ground, so a house
// never floats and never sinks, at every LOD. The ground under F shows as a plinth band or a basement
// storey. `fit` writes what each house draws with, once: then each template of models/house.ts draws
// its houses from a list, and the outline meshes draw the footprints that no template fits.
// #include ../terrain/drawn-mesh.wgsl
// #include ../terrain/view.wgsl
// #include ../ground/look.wgsl

// As house-records.ts writes it.
struct House {
    // The rectangle's middle in world units; the angle of its long side from east, counter-clockwise.
    at: vec2i,
    angle: f32,
    // Its long side, its short side, and the wall to the eaves (0: the style's storeys), in metres.
    length: f32,
    width: f32,
    wall: f32,
    // Metres per world unit at the house.
    unit: f32,
    // The kind (3 bits), the roof that OSM names (2), the shape (1) at bit 5, a flat roof (1), a seed at bit 16.
    bits: u32,
}

// A house's fit to the ground: the floor F above sea level and the foot of the walls below F (both 0
// until a refit fits the house), its style, and the ground 1 m out from the corners of the footprint
// below F: (-,-), (+,-), (+,+), (-,+) along and across.
struct Fit {
    floor: f32,
    bottom: f32,
    style: u32,
    ground: vec4f,
}

// A fit as the buffer keeps it from frame to frame: the floor, then the bottom (f16) and the style
// (16 bits), then the ground as f16 pairs.
struct Kept {
    floor: f32,
    bottom_style: u32,
    ground: vec2u,
}

fn kept_of(f: Fit) -> Kept {
    return Kept(f.floor, (pack2x16float(vec2f(f.bottom, 0.0)) & 0xffffu) | (f.style << 16u), vec2u(pack2x16float(f.ground.xy), pack2x16float(f.ground.zw)));
}

fn fit_of(k: Kept) -> Fit {
    return Fit(k.floor, unpack2x16float(k.bottom_style).x, k.bottom_style >> 16u, vec4f(unpack2x16float(k.ground.x), unpack2x16float(k.ground.y)));
}

// What a drawn house draws with, from `fit`.
struct Drawn {
    // The middle in LOD space, and the turn and the scale from metres along and across to LOD space.
    centre: vec2f,
    turn: vec2f,
    floor: f32,
    // A church tower's side, else 0.
    side: f32,
    // Half the sides; the wall to the eaves, a storey and the storeys.
    half: vec2f,
    wall: f32,
    storey: f32,
    storeys: f32,
    // The kind (3 bits), the form (2) at bit 3, the timber (2) at bit 5, shutters at bit 7, the front wall (2) at bit 8, the seed at bit 16.
    bits: u32,
    // The ground 1 m out from the corners of the footprint, below F: (-,-), (+,-), (+,+), (-,+) along and across, as f16 pairs.
    ground: vec2u,
    // render, wood, roof, door, shutter, plinth, chimney and balcony, in sRGB.
    colours: array<u32, 8>,
    // The slots of the template as f16 pairs: x at 0, y at Y_SLOTS, z at Z_SLOTS.
    slots: array<u32, 18>,
}

// The slots of a template, as models/house.ts names them: x, y and z in metres about the middle of
// the footprint and above F.
struct Slots {
    x: array<f32, 12>,
    y: array<f32, 8>,
    z: array<f32, 16>,
}

// As `Style` in houses.ts: a style of styles/*.json.
struct Style {
    pitch: vec2f,
    eave: f32,
    verge: f32,
    hip: f32,
    // 0 none, 1 upper, 2 all, 3 frame.
    timber: u32,
    // SHUTTERS, BALCONIES, ONION_DOME.
    flags: u32,
    chimneys: u32,
    storeys: f32,
    plinth: u32,
    chimney: u32,
    // The lengths of the five colour lists, 4 bits each.
    counts: u32,
    // render, wood, roof, door, shutter: four each, as 0xbbggrr.
    colours: array<vec4u, 5>,
}

struct Params {
    // Houses, and the list entries of each draw.
    count: u32,
    capacity: u32,
    // Storeys under the highest ground; a fall of the ground from `steep_from` to `steep_to` storeys buries a full storey.
    burial: f32,
    plinth: f32,
    // Below the lowest ground, the walls go this much deeper.
    margin: f32,
    // A house this many pixels wide or more draws in full, one narrower than `least` not at all.
    full: f32,
    least: f32,
    steep_from: f32,
    steep_to: f32,
    unused: u32,
    // The tiles in `ranges`.
    tiles: u32,
    unused2: f32,
    // Per region: its style, and the style of its cottages and houses; two regions per entry.
    regions: array<vec4u, 64>,
    // The tiles in view: the first record, the records and the first workgroup of `fit` and of `refit` of each.
    ranges: array<vec4u, 256>,
    styles: array<Style, 32>,
}

struct DrawOf {
    // The draw's first list entry, and the entries of a list.
    base: u32,
    capacity: u32,
}

@group(0) @binding(1) var<storage, read> houses: array<House>;
@group(0) @binding(2) var<storage, read_write> fits_out: array<Kept>;
// The indirect draws (vertex count, instance count, first vertex, first instance), the count of drawn
// houses, the list of drawn houses of each draw, then each house's entry in `drawn` for the outline
// meshes: `fit` writes them, the vertices read them.
@group(0) @binding(5) var<storage, read_write> draws: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read> lists: array<u32>;
@group(0) @binding(7) var<uniform> draw_of: DrawOf;
@group(0) @binding(8) var<storage, read> template_words: array<vec2u>;
@group(0) @binding(11) var<uniform> params: Params;
@group(0) @binding(12) var<storage, read_write> drawn_out: array<Drawn>;
@group(0) @binding(13) var<storage, read> drawn: array<Drawn>;

const NO_DRAW: u32 = 0xffffffffu;
// The draws: two per template.
const DRAWS: u32 = 8u;
// Lanes per house, and houses per workgroup of `refit`, as in houses.ts.
const LANES: u32 = 9u;
const PER_GROUP: u32 = 7u;
// The word of the drawn count, and the words before the lists.
const COUNT: u32 = 32u;
// Where the y and the z slots start in `Drawn.slots`.
const Y_SLOTS: u32 = 12u;
const Z_SLOTS: u32 = 20u;
const LISTS: u32 = 36u;
// The templates, as TEMPLATES of models/house.ts.
const T_GABLE: u32 = 0u;
const T_FLAT: u32 = 1u;
const T_SPIRE: u32 = 2u;
const T_ONION: u32 = 3u;
// The kinds, as house-records.ts.
const COTTAGE: u32 = 0u;
const HOUSE: u32 = 1u;
const BARN: u32 = 3u;
const BLOCK: u32 = 4u;
const CHURCH: u32 = 5u;
const INDUSTRIAL: u32 = 6u;
// The style of industrial halls, the same in every region: the last of `styles`, as houses.ts writes it.
const INDUSTRIAL_STYLE: u32 = 31u;
const SHUTTERS: u32 = 1u;
const BALCONIES: u32 = 2u;
const ONION_DOME: u32 = 4u;
// Materials, as models/house.ts.
const M_WALL: u32 = 0u;
const M_GABLE: u32 = 1u;
const M_ROOF: u32 = 2u;
const M_EDGE: u32 = 3u;
const M_SOFFIT: u32 = 4u;
const M_CHIMNEY: u32 = 5u;
const M_CAP: u32 = 6u;
const M_BALCONY: u32 = 7u;
const M_TOWER: u32 = 8u;
const M_FLAT: u32 = 9u;
// The colours of `Drawn`.
const C_RENDER: u32 = 0u;
const C_WOOD: u32 = 1u;
const C_ROOF: u32 = 2u;
const C_DOOR: u32 = 3u;
const C_SHUTTER: u32 = 4u;
const C_PLINTH: u32 = 5u;
const C_CHIMNEY: u32 = 6u;
const C_BALCONY: u32 = 7u;

// A kind of the prototype: a storey, the storeys, the window's width and spacing, the door, and the
// sill and the lintel as shares of a storey.
struct Kind {
    storey: f32,
    storeys: f32,
    window: vec2f,
    door: f32,
    opening: vec2f,
}

const KINDS = array<Kind, 7>(
    Kind(2.6, 1.0, vec2f(0.8, 2.2), 0.9, vec2f(0.35, 0.8)),
    Kind(2.7, 2.0, vec2f(1.0, 2.6), 1.0, vec2f(0.35, 0.8)),
    Kind(2.8, 2.0, vec2f(0.9, 2.2), 1.2, vec2f(0.35, 0.8)),
    Kind(4.2, 1.0, vec2f(0.7, 6.0), 3.0, vec2f(0.55, 0.75)),
    Kind(3.0, 4.0, vec2f(1.4, 2.8), 1.6, vec2f(0.35, 0.8)),
    Kind(6.5, 1.0, vec2f(1.2, 3.6), 1.8, vec2f(0.3, 0.75)),
    // A hall: a band of wide windows under the eaves and a loading door.
    Kind(7.0, 1.0, vec2f(2.4, 6.0), 4.0, vec2f(0.62, 0.82)),
);

const GLASS = vec3f(0.212, 0.259, 0.298);
const FLAT_ROOF = vec3f(0.478, 0.463, 0.431);
const PLINTH_M: f32 = 0.35;
const BEAM_M: f32 = 0.2;
const BALCONY_M: f32 = 0.9;
// A roof rises at most this much above the eaves.
const ROOF_M: f32 = 7.0;
// At night this share of the windows shines in this colour, and this share of the houses stays dark.
const LIT: f32 = 0.35;
const LAMPLIGHT = vec3f(1.0, 0.62, 0.3);
const DARK_HOUSES: f32 = 0.3;
// Lamplight carries this many times as far as the haze lets daylight colour through.
const LAMP_REACH: f32 = 3.0;

fn seeded(a: u32, b: u32) -> f32 {
    var h = a * 747796405u + b * 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return f32((h >> 22u) ^ h) / 4294967296.0;
}

fn kind_of(h: House) -> u32 {
    return h.bits & 7u;
}

// The kind table by a switch: an array indexed at run time would leave the registers.
fn kind_table(kind: u32) -> Kind {
    switch kind {
        case 0u: { return KINDS[0]; }
        case 1u: { return KINDS[1]; }
        case 2u: { return KINDS[2]; }
        case 3u: { return KINDS[3]; }
        case 4u: { return KINDS[4]; }
        case 5u: { return KINDS[5]; }
        default: { return KINDS[6]; }
    }
}

// A colour of list `list` of style `style`, by the house's seed, in sRGB.
fn pick(style: u32, list: u32, seed: u32) -> vec3f {
    let n = max(1u, (params.styles[style].counts >> (4u * list)) & 15u);
    return unpack4x8unorm(params.styles[style].colours[list][min(u32(seeded(seed, list) * f32(n)), n - 1u)]).rgb;
}

// What a house's style and footprint make of it.
struct Shape {
    kind: u32,
    form: u32,
    // Along and across: half its sides; the wall to the eaves, a storey and the storeys.
    half: vec2f,
    wall: f32,
    storey: f32,
    storeys: f32,
    pitch: f32,
    hip: f32,
    timber: u32,
    shutters: bool,
    chimneys: u32,
    balconies: bool,
}

fn shape_of(h: House, index: u32) -> Shape {
    let style = &params.styles[index];
    var s: Shape;
    s.kind = kind_of(h);
    let k = kind_table(s.kind);
    let seed = h.bits >> 16u;
    let church = s.kind == CHURCH;
    let dwelling = s.kind == COTTAGE || s.kind == HOUSE;
    s.half = 0.5 * vec2f(h.length, h.width);
    // OSM's height and levels of a church describe its tower, so the nave's walls follow its width.
    s.wall = select(select(k.storey * select(k.storeys, (*style).storeys, dwelling && (*style).storeys > 0.0), h.wall, h.wall > 0.0), clamp(0.6 * h.width, 3.0, 14.0), church);
    s.storeys = max(1.0, round(s.wall / k.storey));
    s.storey = s.wall / s.storeys;
    s.pitch = mix((*style).pitch.x, (*style).pitch.y, seeded(seed, 7u));
    let roof = (h.bits >> 3u) & 3u;
    s.hip = select(select(select(0.5, 1.0, roof == 2u), 0.0, roof == 1u), (*style).hip, roof == 0u);
    s.timber = select(select((*style).timber, 2u, s.kind == BARN && (*style).timber != 0u), 0u, s.kind == BLOCK || church);
    s.shutters = ((*style).flags & SHUTTERS) != 0u && s.kind != BARN && !church;
    s.chimneys = select((*style).chimneys, 0u, s.kind == BARN || church);
    s.balconies = ((*style).flags & BALCONIES) != 0u && s.kind != BARN && !church;
    // A flat roof flattens any kind but a church, which keeps its tower.
    let flat = ((h.bits >> 6u) & 1u) == 1u;
    s.form = select(select(T_GABLE, T_FLAT, flat), select(T_SPIRE, T_ONION, ((*style).flags & ONION_DOME) != 0u), church);
    return s;
}

// A church's tower: its side, and its middle along the footprint.
fn tower_of(s: Shape) -> vec2f {
    let side = min(1.2 * s.half.y, 2.0 * s.half.x / 3.0);
    return vec2f(side, -s.half.x + 0.5 * side);
}

fn slots_of(s: Shape, index: u32, bottom: f32) -> Slots {
    let eave_m = params.styles[index].eave;
    let verge = params.styles[index].verge;
    var o: Slots;
    // A wide hall's roof keeps a low pitch, and a style of 0° a near-flat one.
    let t = max(min(tan(radians(s.pitch)), ROOF_M / s.half.y), 0.05);
    let w = s.half;
    o.z[0] = bottom;
    o.z[1] = s.wall;
    if (s.form == T_FLAT) {
        o.x[1] = w.x;
        o.x[2] = w.x + 0.2;
        o.y[1] = w.y;
        o.y[2] = w.y + 0.2;
        return o;
    }
    let eave = s.wall - eave_m * t;
    let ridge = s.wall + w.y * t;
    if (s.form == T_GABLE) {
        let y = w.y + eave_m;
        let end = w.x + verge;
        // The hipped ends rise at the roof's own pitch from the hip line to the shortened ridge.
        let hip_z = ridge - s.hip * (ridge - eave);
        let ridge_x = max(0.0, end - s.hip * y);
        let top_z = min(ridge, hip_z + verge * t);
        let chimney = min(w.x - 0.6, max(0.0, ridge_x - 0.5));
        let c1 = select(0.0, 1.0, s.chimneys >= 1u);
        let c2 = select(0.0, 1.0, s.chimneys == 2u);
        let b1 = select(0.0, 1.0, s.balconies && s.storeys > 1.5);
        let b2 = select(0.0, 1.0, s.balconies && s.storeys > 2.5);
        o.x = array<f32, 12>(0.0, w.x, end, ridge_x, chimney - 0.35 * c1, chimney + 0.35 * c1, chimney - 0.35 * c2, chimney + 0.35 * c2, 0.6 * w.x * b1, 0.6 * w.x * b2, 0.0, 0.0);
        o.y = array<f32, 8>(0.0, w.y, y, s.hip * y, (ridge - top_z) / t, 0.275, w.y + BALCONY_M, 0.0);
        o.z = array<f32, 16>(bottom, s.wall, eave, ridge, hip_z, top_z, ridge - 1.0, ridge - 1.0 + 2.2 * c1, ridge - 1.0 + 2.2 * c2,
            s.storey - 0.15, s.storey - 0.15 + 1.15 * b1, 2.0 * s.storey - 0.15, 2.0 * s.storey - 0.15 + 1.15 * b2, 0.0, 0.0, 0.0);
        return o;
    }
    // A church: the tower at the -x end, the nave filling the rest, and a spire as steep as the roof allows or an onion dome.
    let tower = tower_of(s);
    let spire = tan(radians(clamp(1.7 * s.pitch, 40.0, 82.0)));
    let e = select(min(0.3, 0.4 / spire), 0.3, s.form == T_ONION);
    let nave_w = -w.x + tower.x;
    let top = ridge + tower.x;
    o.x = array<f32, 12>(0.0, nave_w, w.x, nave_w - verge, w.x + verge, -w.x, tower.y, tower.y - 0.5 * tower.x - e, tower.y + 0.5 * tower.x + e, 0.0, 0.0, 0.0);
    o.y = array<f32, 8>(0.0, w.y, w.y + eave_m, 0.5 * tower.x, 0.5 * tower.x + e, 0.0, 0.0, 0.0);
    o.z = array<f32, 16>(bottom, s.wall, eave, ridge, top, top - e * spire, top + 0.5 * tower.x * spire, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    return o;
}

// A point about the house's middle in metres, along and across, in LOD space.
fn house_space(h: House, p: vec2f) -> vec2f {
    let c = cos(h.angle);
    let s = sin(h.angle);
    let centre = lod_space(vec2f(h.at - surface.anchor));
    return centre + vec2f(c * p.x - s * p.y, s * p.x + c * p.y) * (surface.unit / h.unit);
}

// The tile of workgroup g of `fit` (ranges' z) or of `refit` (w): the last whose first workgroup is at most g.
fn tile_of(g: u32, refitting: bool) -> vec4u {
    var first = 0u;
    var past = params.tiles;
    while (past - first > 1u) {
        let middle = (first + past) / 2u;
        if (select(params.ranges[middle].z, params.ranges[middle].w, refitting) <= g) {
            first = middle;
        } else {
            past = middle;
        }
    }
    return params.ranges[first];
}

// Whether a house of the frame can show: wide enough on screen, in the frustum, standing near `floor`.
struct Seen {
    centre: vec2f,
    pixels: f32,
    seen: bool,
}

fn seen_at(h: House, floor: f32) -> Seen {
    let centre = house_space(h, vec2f(0.0));
    let radius = 0.5 * length(vec2f(h.length, h.width)) + 2.0;
    // A bound of the height: without OSM's, a kind's walls reach 14 m at most; a church's tower and
    // spire rise at most five tower sides over its roof.
    let height = max(h.wall, 14.0) + ROOF_M + select(0.0, 5.0 * min(0.6 * h.width, h.length / 3.0), kind_of(h) == CHURCH);
    let p = render_at(centre, floor + 0.5 * height);
    let pixels = 2.0 * radius / (length(p) * frame.pixel);
    return Seen(centre, pixels, pixels >= params.least && visible(p, radius + 0.5 * height));
}

// The ground samples of a workgroup of `refit`: LANES threads sample a house's ground at once, for a
// thread's samples one after another make a long chain of reads, and the frame waits for the longest.
var<workgroup> sampled: array<f32, 64>;

// Where lane k samples the ground of a house, in metres along and across: the middle, the corners
// of the footprint 1 m wider, then the middles of its sides.
fn sample_point(h: House, lane: u32) -> vec2f {
    let half = 0.5 * vec2f(h.length, h.width) + 1.0;
    let corners = array<vec2f, 5>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0), vec2f(-1.0, -1.0));
    let c = (lane - 1u) % 4u;
    return select(select(0.5 * (corners[c] + corners[c + 1u]), corners[c], lane <= 4u) * half, vec2f(0.0), lane == 0u);
}

// Fits the houses in view to the drawn surface and picks their style. It runs in the frames that
// `houses.ts` names, before `fit`; between them, a house keeps its fit.
@compute @workgroup_size(64)
fn refit(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) local: u32) {
    let range = tile_of(group.x, true);
    let lane = local % LANES;
    let nth = (group.x - range.w) * PER_GROUP + local / LANES;
    let i = range.x + nth;
    // Every lane of a house decides alike; lane 0 writes its fit after the barrier.
    let valid = local < PER_GROUP * LANES && nth < range.y && i < params.count;
    var h: House;
    var last: Fit;
    if (valid) {
        h = houses[i];
        last = fit_of(fits_out[i]);
    }
    // A house that has no fit yet stands at its middle for the culling.
    let remembered = last.floor != 0.0 || last.bottom != 0.0;
    var middle = last.floor;
    if (valid && !remembered) {
        middle = plain_height(house_space(h, vec2f(0.0)));
    }
    let view = seen_at(h, middle);
    let refitting = valid && known(middle) && view.seen;
    let large = max(h.length, h.width) > 20.0;
    var height = NONE;
    if (refitting && (lane <= 4u || large)) {
        height = plain_height(house_space(h, sample_point(h, lane)));
    }
    sampled[local] = height;
    workgroupBarrier();
    if (!refitting || lane != 0u) {
        return;
    }
    var low = 1e30;
    var high = -1e30;
    var all_known = true;
    for (var k = 0u; k < select(5u, LANES, large); k++) {
        let r = sampled[local + k];
        all_known = all_known && known(r);
        low = min(low, r);
        high = max(high, r);
    }
    if (!all_known) {
        return;
    }
    var out = last;
    let region = min(region_at(view.centre), 127u);
    let styles_of = params.regions[region / 2u];
    let regional = select(styles_of.xy, styles_of.zw, region % 2u == 1u);
    let kind = kind_of(h);
    out.style = select(min(select(regional.x, regional.y, kind == COTTAGE || kind == HOUSE), INDUSTRIAL_STYLE - 1u), INDUSTRIAL_STYLE, kind == INDUSTRIAL);
    let storey = shape_of(h, out.style).storey;
    // Half a storey stands in the ground uphill, a full one on a slope that would show two storeys or more
    // of basement downhill; on flat ground the floor stands over all of it.
    let fall = (high - low) / storey;
    let burial = min(storey * mix(params.burial, 1.0, smoothstep(params.steep_from, params.steep_to, fall)), high - low);
    out.floor = max(high - burial, low + params.plinth);
    out.bottom = low - out.floor - params.margin;
    out.ground = vec4f(sampled[local + 1u], sampled[local + 2u], sampled[local + 3u], sampled[local + 4u]) - out.floor;
    fits_out[i] = kept_of(out);
}

// Culls each house of the tiles in view at its fit, picks its level of detail, and writes what it
// draws with: one thread per house, in every frame that moves.
@compute @workgroup_size(64)
fn fit(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) local: u32) {
    let range = tile_of(group.x, false);
    let nth = (group.x - range.z) * 64u + local;
    let i = range.x + nth;
    if (nth >= range.y || i >= params.count) {
        return;
    }
    let h = houses[i];
    let out = fit_of(fits_out[i]);
    let outline = ((h.bits >> 5u) & 1u) == 1u;
    let at = LISTS + DRAWS * params.capacity + i;
    if (outline) {
        atomicStore(&draws[at], NO_DRAW);
    }
    let view = seen_at(h, out.floor);
    if ((out.floor == 0.0 && out.bottom == 0.0) || !view.seen) {
        return;
    }
    let kind = kind_of(h);
    let s = shape_of(h, out.style);
    let v = atomicAdd(&draws[COUNT], 1u);
    if (v >= arrayLength(&drawn_out)) {
        return;
    }
    let draw = 2u * s.form + select(1u, 0u, view.pixels >= params.full);
    var slot = 0u;
    if (!outline) {
        slot = atomicAdd(&draws[4u * draw + 1u], 1u);
        if (slot >= params.capacity) {
            return;
        }
    }
    var d: Drawn;
    d.centre = view.centre;
    d.turn = vec2f(cos(h.angle), sin(h.angle)) * (surface.unit / h.unit);
    d.floor = out.floor;
    d.side = select(0.0, tower_of(s).x, kind == CHURCH);
    d.half = s.half;
    d.wall = s.wall;
    d.storey = s.storey;
    d.storeys = s.storeys;
    let below = out.ground;
    d.ground = vec2u(pack2x16float(below.xy), pack2x16float(below.zw));
    // The door is in the wall whose ground lies nearest the floor.
    let mids = abs(0.5 * (below + below.yzwx));
    let nearest = min(min(mids.x, mids.y), min(mids.z, mids.w));
    let front = select(select(select(3u, 2u, mids.z == nearest), 1u, mids.y == nearest), 0u, mids.x == nearest);
    d.bits = s.kind | (s.form << 3u) | (s.timber << 5u) | (u32(s.shutters) << 7u) | (front << 8u) | (h.bits & 0xffff0000u);
    let seed = h.bits >> 16u;
    let wood = pick(out.style, 1u, seed);
    let shutter = select(pick(out.style, 4u, seed), wood, s.timber == 3u);
    d.colours = array<u32, 8>(
        pack4x8unorm(vec4f(pick(out.style, 0u, seed), 1.0)), pack4x8unorm(vec4f(wood, 1.0)),
        pack4x8unorm(vec4f(select(pick(out.style, 2u, seed), FLAT_ROOF, s.form == T_FLAT), 1.0)),
        pack4x8unorm(vec4f(select(pick(out.style, 3u, seed), wood * 0.55, s.kind == BARN), 1.0)), pack4x8unorm(vec4f(shutter, 1.0)),
        params.styles[out.style].plinth, params.styles[out.style].chimney, pack4x8unorm(vec4f(shutter, 1.0)));
    let o = slots_of(s, out.style, out.bottom);
    let x = o.x;
    let y = o.y;
    let z = o.z;
    d.slots = array<u32, 18>(
        pack2x16float(vec2f(x[0], x[1])), pack2x16float(vec2f(x[2], x[3])), pack2x16float(vec2f(x[4], x[5])),
        pack2x16float(vec2f(x[6], x[7])), pack2x16float(vec2f(x[8], x[9])), pack2x16float(vec2f(x[10], x[11])),
        pack2x16float(vec2f(y[0], y[1])), pack2x16float(vec2f(y[2], y[3])), pack2x16float(vec2f(y[4], y[5])), pack2x16float(vec2f(y[6], y[7])),
        pack2x16float(vec2f(z[0], z[1])), pack2x16float(vec2f(z[2], z[3])), pack2x16float(vec2f(z[4], z[5])), pack2x16float(vec2f(z[6], z[7])),
        pack2x16float(vec2f(z[8], z[9])), pack2x16float(vec2f(z[10], z[11])), pack2x16float(vec2f(z[12], z[13])), pack2x16float(vec2f(z[14], z[15])));
    drawn_out[v] = d;
    atomicStore(&draws[select(LISTS + draw * params.capacity + slot, at, outline)], v);
}

struct HouseOut {
    @builtin(position) clip: vec4f,
    @location(0) relative: vec3f,
    @location(1) altitude: f32,
    // Metres along, across and above F.
    @location(2) local: vec3f,
    // The drawn house, and the material and the wall of the face.
    @location(3) @interpolate(flat) house: u32,
    @location(4) @interpolate(flat) face: u32,
    // The metres along an outline's walls.
    @location(5) along: f32,
    // The share of the sun that the horizons let through.
    @location(6) sun: f32,
}

// Slot k of a drawn house.
fn slot(v: u32, k: u32) -> f32 {
    return unpack2x16float(drawn[v].slots[k / 2u])[k % 2u];
}

fn house_vertex(v: u32, s: vec2f, local: vec3f, face: u32, along: f32) -> HouseOut {
    var out: HouseOut;
    let z = drawn[v].floor + local.z;
    let p = render_at(s, z);
    out.clip = frame.view_proj * vec4f(p, 1.0);
    out.relative = p;
    out.altitude = z;
    out.local = local;
    out.house = v;
    out.face = face;
    out.along = along;
    out.sun = sun_at(s);
    return out;
}

// A vertex of a drawn house: the house, its point in LOD space, its metres about the footprint's middle
// and above F, and its face.
struct Placed {
    v: u32,
    s: vec2f,
    local: vec3f,
    face: u32,
}

fn clip_of(p: Placed) -> vec4f {
    return frame.view_proj * vec4f(render_at(p.s, drawn[p.v].floor + p.local.z), 1.0);
}

@vertex
fn template_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> HouseOut {
    let p = template_at(vertex, instance);
    return house_vertex(p.v, p.s, p.local, p.face, 0.0);
}

// A vertex of a caster in the shadow map (light/shadow.ts), whose frame holds the sun's view.
@vertex
fn template_caster(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> @builtin(position) vec4f {
    return clip_of(template_at(vertex, instance));
}

fn template_at(vertex: u32, instance: u32) -> Placed {
    let v = lists[LISTS + draw_of.base + instance];
    let word = template_words[vertex];
    let w = word.x;
    var local = vec3f(slot(v, w & 15u), slot(v, Y_SLOTS + ((w >> 4u) & 15u)), slot(v, Z_SLOTS + ((w >> 8u) & 15u)));
    local.x *= select(1.0, -1.0, (w & 0x1000u) != 0u);
    local.y *= select(1.0, -1.0, (w & 0x2000u) != 0u);
    if ((w & 0x4000u) != 0u) {
        // A vertex of the onion dome, on its ring about the tower: radius and height as shares of the
        // tower's half side, in hundredths and fiftieths.
        let ring = vec2f(f32((word.y >> 16u) & 255u) / 100.0, f32(word.y >> 24u) / 50.0) * 0.5 * drawn[v].side;
        let turn = f32((w >> 26u) & 7u) * 0.25 * 3.14159265;
        local = vec3f(local.x + ring.x * cos(turn), ring.x * sin(turn), local.z + 0.3 + ring.y);
    }
    local.z += f32(bitcast<i32>(word.y << 16u) >> 16u) * 0.01;
    let t = drawn[v].turn;
    return Placed(v, drawn[v].centre + vec2f(t.x * local.x - t.y * local.y, t.y * local.x + t.x * local.y), local, (w >> 15u) & 255u);
}

struct OutlineIn {
    @location(0) at: vec2i,
    // The house, 1 << 31 at the top of the wall, 1 << 30 on the roof.
    @location(1) bits: u32,
    @location(2) along: f32,
}

// Past the clip volume: a vertex of an outline whose house does not draw.
const UNDRAWN: vec4f = vec4f(2.0, 2.0, 2.0, 1.0);

@vertex
fn outline_vertex(o: OutlineIn) -> HouseOut {
    let p = outline_at(o);
    if (p.v == NO_DRAW) {
        var out: HouseOut;
        out.clip = UNDRAWN;
        return out;
    }
    return house_vertex(p.v, p.s, p.local, p.face, o.along);
}

@vertex
fn outline_caster(o: OutlineIn) -> @builtin(position) vec4f {
    let p = outline_at(o);
    if (p.v == NO_DRAW) {
        return UNDRAWN;
    }
    return clip_of(p);
}

fn outline_at(o: OutlineIn) -> Placed {
    let v = lists[LISTS + DRAWS * draw_of.capacity + (o.bits & 0x3fffffffu)];
    if (v == NO_DRAW) {
        return Placed(v, vec2f(0.0), vec3f(0.0), 0u);
    }
    let local = vec3f(0.0, 0.0, select(slot(v, Z_SLOTS), drawn[v].wall, (o.bits >> 31u) == 1u));
    return Placed(v, lod_space(vec2f(o.at - surface.anchor)), local, select(M_WALL, M_FLAT, ((o.bits >> 30u) & 1u) == 1u) | (4u << 4u));
}

fn srgb(c: vec3f) -> vec3f {
    return c * c;
}

fn colour_of(d: Drawn, k: u32) -> vec3f {
    return unpack4x8unorm(d.colours[k]).rgb;
}

// The paint of wall `wall`, `len` metres long, at `u` metres along it and `z` above F, over ground
// `ground` below F; `w` is the size of a pixel in metres, and `first` the number of the wall's first
// window. Windows and doors are cells of a grid, as in the prototype. Alpha: how much the cell shines at night.
fn facade(d: Drawn, wall: u32, first: u32, u: f32, len: f32, z: f32, front: bool, ground: f32, w: f32) -> vec4f {
    let kind = d.bits & 7u;
    let timber = (d.bits >> 5u) & 3u;
    let k = kind_table(kind);
    let wood = colour_of(d, C_WOOD);
    let plinth = colour_of(d, C_PLINTH);
    let n = max(1.0, floor(len / k.window.y));
    let column = clamp(floor(u / len * n), 0.0, n - 1.0);
    let centre = len * (column + 0.5) / n;
    let seed = d.bits >> 16u;
    let lived = kind != BARN && kind != INDUSTRIAL && seeded(seed, 99u) >= DARK_HOUSES;
    // Whether the window of the pixel's cell shines, and the share of its cell that it fills: far off,
    // a pixel shows one window's light at that share, so a far village is lit dots of the same light.
    let storey = clamp(floor(z / d.storey), 0.0, d.storeys - 1.0);
    let lit = select(0.0, 1.0, lived && z >= 0.0 && seeded(seed ^ (wall << 20u) ^ (u32(storey) << 24u), first + u32(column)) < LIT);
    let share = k.window.x / k.window.y * (k.opening.y - k.opening.x);
    let off = abs(u - centre);
    let ww = 0.5 * k.window.x;
    var base = select(colour_of(d, C_RENDER), wood, timber == 2u || (timber == 1u && z > d.storey));
    if (timber == 3u && d.storeys > 1.5 && z > d.storey) {
        let post = abs(u - round(u * n / len) * len / n) < 0.5 * BEAM_M;
        let beam = fract(z / d.storey) * d.storey < BEAM_M || z > d.wall - BEAM_M;
        base = select(base, wood, post || beam);
    }
    // Far off, the cells are smaller than a pixel: their mean.
    let glass = select(0.0, share, z >= 0.0);
    let mean = vec4f(mix(select(base, plinth, z < PLINTH_M), GLASS, glass), lit * share);
    if (w > 0.5) {
        return mean;
    }
    var colour = base;
    var glow = 0.0;
    if (z < 0.0) {
        // A basement storey under the floor shows small windows where its wall stands clear of the ground.
        colour = plinth;
        let below = -z / d.storey;
        let band = fract(below);
        let sill = -(floor(below) + 0.45) * d.storey;
        if (-ground > 0.8 * d.storey && band > 0.15 && band < 0.4 && off < 0.6 * ww && sill > ground + 0.2) {
            colour = GLASS;
        }
    } else {
        // A window opens where its sill stands clear of the ground: a buried storey shows its wall.
        let sill = (storey + k.opening.x) * d.storey;
        let open = z > sill && z < (storey + k.opening.y) * d.storey && sill > ground + 0.1;
        let shutter_half = ww + select(0.0, 0.9 * ww, ((d.bits >> 7u) & 1u) == 1u);
        if (front && abs(u - 0.5 * len) < 0.5 * k.door && z < k.opening.y * d.storey) {
            colour = colour_of(d, C_DOOR);
        } else if (open && off < ww) {
            colour = GLASS;
            glow = lit;
        } else if (open && off < shutter_half) {
            colour = colour_of(d, C_SHUTTER);
        } else if (z < PLINTH_M) {
            colour = plinth;
        }
    }
    return mix(vec4f(colour, glow), mean, smoothstep(0.15, 0.5, w));
}

// A church tower's face: plinth, wall, the belfry and the wall over it, in three columns.
fn tower_face(u: f32, side: f32, z: f32, top: f32, wall: vec3f, plinth: vec3f) -> vec3f {
    let middle = u > 0.25 * side && u < 0.75 * side;
    if (z < PLINTH_M) {
        return plinth;
    }
    return select(wall, GLASS, middle && z > top - 2.0 * side / 3.0 && z < top - side / 6.0);
}

@fragment
fn house_fragment(in: HouseOut) -> @location(0) vec4f {
    // The crack and surface views show the terrain alone.
    if (frame.debug == 3u || frame.debug == 4u) {
        discard;
    }
    let d = drawn[in.house];
    let material = in.face & 15u;
    let wall = (in.face >> 4u) & 7u;
    var n = normalize(cross(dpdx(in.relative), dpdy(in.relative)));
    n *= -sign(dot(n, in.relative));
    let w = max(length(dpdx(in.relative)), length(dpdy(in.relative)));
    let timber = (d.bits >> 5u) & 3u;
    let x = in.local.x;
    let y = in.local.y;
    var colour = colour_of(d, C_RENDER);
    var glow = 0.0;
    switch material {
        case M_WALL: {
            var u = in.along;
            var len = 1e9;
            var ground = -0.5;
            var first = 0u;
            if (wall < 4u) {
                // A church's nave starts past its tower.
                let lo = -d.half.x + d.side;
                let along = vec4f(x - lo, y + d.half.y, d.half.x - x, d.half.y - y);
                let lengths = vec4f(d.half.x - lo, 2.0 * d.half.y, d.half.x - lo, 2.0 * d.half.y);
                u = along[wall];
                len = lengths[wall];
                let below = vec4f(unpack2x16float(d.ground.x), unpack2x16float(d.ground.y));
                ground = mix(below[wall], below[(wall + 1u) % 4u], clamp(u / len, 0.0, 1.0));
            } else {
                // An outline: its windows repeat along it.
                let spacing = kind_table(d.bits & 7u).window.y;
                len = spacing;
                first = u32(floor(in.along / spacing));
                u = in.along - f32(first) * spacing;
            }
            let painted = facade(d, wall, first, u, len, in.local.z, wall == ((d.bits >> 8u) & 3u), ground, w);
            colour = painted.rgb;
            glow = painted.a;
        }
        case M_GABLE: {
            colour = select(colour, colour_of(d, C_WOOD), timber == 1u || timber == 2u);
        }
        case M_ROOF: {
            colour = colour_of(d, C_ROOF);
        }
        case M_EDGE: {
            colour = colour_of(d, C_ROOF) * 0.8;
        }
        case M_SOFFIT: {
            colour = colour_of(d, C_WOOD) * 0.75;
        }
        case M_CHIMNEY: {
            colour = colour_of(d, C_CHIMNEY);
        }
        case M_CAP: {
            colour = colour_of(d, C_PLINTH);
        }
        case M_BALCONY: {
            colour = colour_of(d, C_BALCONY);
        }
        case M_TOWER: {
            let side = d.side;
            let along = vec4f(x + d.half.x, y + 0.5 * side, -d.half.x + side - x, 0.5 * side - y);
            colour = tower_face(along[wall], side, in.local.z, slot(in.house, Z_SLOTS + 4u), colour, colour_of(d, C_PLINTH));
        }
        default: {
            colour = FLAT_ROOF;
        }
    }
    let lit = light_up(srgb(colour), n, sun_share(in.sun, in.relative, n, LIFT_SOLID), 1.0, look.horizon.w);
    // Lamplight after the haze, which would turn it blue at night: it fades with the distance instead.
    let lamps = LAMPLIGHT * glow * smoothstep(0.0, 0.4, look.ambient.w) * exp(-length(in.relative) / (LAMP_REACH * look.haze.x));
    return vec4f(atmosphere(lit, in.relative, in.altitude) + lamps, 1.0);
}
