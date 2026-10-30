// PROTOTYPE road system (a study; never merges). The paint of the junctions' faces (roads/junctions.ts):
// the zebras, the give-way lines and the corners' asphalt. corridor.wgsl calls junction_surface for the
// materials from MAT_JUNCTION_FIRST on.
// #include ../ground/roads.wgsl

// Materials, as JMAT of junctions.ts.
const MAT_JUNCTION_FIRST: u32 = 60u;
const MAT_JUNCTION_LAST: u32 = 79u;
const MAT_ZEBRA: u32 = 60u;
const MAT_GIVE_WAY: u32 = 61u;
const MAT_FILLET: u32 = 62u;
// A Swiss zebra's yellow, linear; its stripes 0.5 m wide every metre across, clear of the edges by this much.
const ZEBRA_YELLOW: vec3f = vec3f(0.78, 0.52, 0.015);
const ZEBRA_CLEAR_M: f32 = 0.35;
// The give-way line and its dashes, then the outlined triangle behind it in the approaching lane, pointing at the
// approaching driver: metres back from the line's near edge, its base and its length.
const GIVE_WAY_M: f32 = 0.3;
const GIVE_WAY_DASH_M: f32 = 0.5;
const TRIANGLE_BACK_M: f32 = 1.2;
const TRIANGLE_BASE_M: f32 = 1.2;
const TRIANGLE_LENGTH_M: f32 = 2.6;
const TRIANGLE_STROKE_M: f32 = 0.15;

// The albedo and the ground's share of a junction face: `param` is the surface code and, from bit 4 on, the
// face's flags; `road` is metres across and the half-width, `p` the place in metres, `w` a pixel's width.
fn junction_surface(mat: u32, region: u32, param: u32, road: vec2f, along: f32, p: vec2f, w: f32, along_w: f32) -> vec4f {
    let code = param & 15u;
    // The carriageway's own surface, without its lines.
    let base = road_surface(region, code, 1.0, 0.0, 0.0, along, p, w, w, along_w);
    var c = base.rgb;
    // Worn paint: thinner where the wheels run.
    let wear = 0.82 + 0.18 * noise(p / 0.7);
    if (mat == MAT_ZEBRA) {
        // A zebra's faces carry their metres across its straight middle line in `along`.
        let stripe = band(abs(fract(along + 0.5) - 0.5), -0.25, 0.25, w) * band(road.y - abs(road.x), ZEBRA_CLEAR_M, 100.0, w);
        c = mix(c, ZEBRA_YELLOW * (0.9 + 0.1 * noise(p / 0.3)), stripe * wear);
    } else if (mat == MAT_GIVE_WAY) {
        // `along` is metres back from the line. The approaching lane lies on the line's left (bit 4: its right); bit 5 draws the triangle.
        let line = rule(region, slot_road_line()).rgb;
        let dash = band(abs(fract(road.x / (2.0 * GIVE_WAY_DASH_M) + 0.25) - 0.5), -0.25, 0.25, w) * band(road.y - abs(road.x), 0.15, 100.0, w) * band(along, 0.0, GIVE_WAY_M, w);
        let lane = select(road.x, -road.x, ((param >> 4u) & 1u) != 0u) - 0.5 * road.y;
        let u = along - GIVE_WAY_M - TRIANGLE_BACK_M;
        let half = 0.5 * TRIANGLE_BASE_M * (1.0 - u / TRIANGLE_LENGTH_M);
        let d = min(u, (half - abs(lane)) * 0.97);
        let triangle = select(0.0, band(d, 0.0, TRIANGLE_STROKE_M, w), ((param >> 5u) & 1u) != 0u && u < TRIANGLE_LENGTH_M);
        c = mix(c, line, max(dash, triangle) * wear);
    }
    return vec4f(c, base.a);
}
