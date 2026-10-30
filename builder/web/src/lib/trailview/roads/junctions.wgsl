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

// The albedo and the ground's share of a junction face: `road` is metres across and the half-width, `p`
// the place in metres, `w` a pixel's width.
fn junction_surface(mat: u32, region: u32, code: u32, road: vec2f, along: f32, p: vec2f, w: f32, along_w: f32) -> vec4f {
    // The carriageway's own surface, without its lines.
    let base = road_surface(region, code, 1.0, 0.0, 0.0, along, p, w, w, along_w);
    var c = base.rgb;
    // Worn paint: thinner where the wheels run.
    let wear = 0.82 + 0.18 * noise(p / 0.7);
    if (mat == MAT_ZEBRA) {
        let x = road.x;
        let stripe = band(abs(fract(x + 0.5) - 0.5), -0.25, 0.25, w) * band(road.y - abs(x), ZEBRA_CLEAR_M, 100.0, w);
        c = mix(c, ZEBRA_YELLOW * (0.9 + 0.1 * noise(p / 0.3)), stripe * wear);
    } else if (mat == MAT_GIVE_WAY) {
        let line = rule(region, slot_road_line()).rgb;
        let dash = band(abs(fract(road.x + 0.25) - 0.5), -0.25, 0.25, w) * band(road.y - abs(road.x), 0.15, 100.0, w);
        c = mix(c, line, dash * wear);
    }
    return vec4f(c, base.a);
}
