// A path across a slope as a level ledge. Its tread and its cut bank are narrower than a mesh cell, and the
// bank is narrower than a texel: the heights draw the bank as a staircase of texels along the path, and
// the mesh as a slope across a cell, which look.wgsl would light as cliff. The mask knows them to the
// centimetre: the tread shades level across the path, the bank above it as a cut face of bare earth with
// a contact shadow at its foot, the hillside past it as the hillside further up, and the edge below the
// tread shows a lighter lip. look.wgsl includes it.
// #include roads.wgsl

// The ground rises across the path by this much per metre where the bank and the lip begin to show, and
// where they show fully.
const LEDGE_RISE: vec2f = vec2f(0.15, 0.35);
// As `cut` of the paths in roads.json: the rise per metre of the bank's face.
const BANK_CUT: f32 = 2.1445;
// The bank is as high as the hillside rises over the path's half-width, no more than BANK_MAX_M; its earth
// starts under the fray of the paint's edge (0.18 m in road_at). The heights' staircase reaches up to
// STAIR_TEXELS past the bank: there, and to tell the side where the ground rises, the hillside shades as
// it does past the staircase.
const BANK_MAX_M: f32 = 0.3;
const FRAY_M: f32 = 0.18;
const STAIR_TEXELS: f32 = 3.0;
// The contact shadow and the lip reach these metres in from the edge.
const FOOT_M: f32 = 0.3;
const LIP_M: f32 = 0.15;
// The bank's earth, its shadow and the lip are details: they fade between these metres from the eye.
const LEDGE_NEAR: vec2f = vec2f(40.0, 120.0);

struct Ledge {
    // The ground's normal in render space, and the share of the pixel that the tread, the bank and the
    // hillside past it take.
    normal: vec3f,
    zone: f32,
    // The share of the bank's earth, and the factor of the albedo for the shadow and the lip.
    earth: f32,
    tint: f32,
}

// The ledge of a path at a point of the ground, as road_at reads the mask, where `n` is the ground's normal.
fn ledge_at(s: vec2f, ds: mat2x2f, n: vec3f, distance: f32) -> Ledge {
    let none = Ledge(n, 0.0, 0.0, 1.0);
    if (!DETAIL) {
        return none;
    }
    let level = lod(s);
    let reach = (1.0 - smoothstep(0.75 * masks.reach, masks.reach, distance)) * smoothstep(f32(MASK_LEVEL) - 1.0, f32(MASK_LEVEL) - 0.5, level);
    if (reach <= 0.0) {
        return none;
    }
    let mask = mask_at(s, u32(ceil(level)), true);
    let past = BANK_MAX_M + STAIR_TEXELS * f32(1 << (22u - u32(ceil(level)))) * surface.unit;
    if (mask.level == 0u || mask.kind.x == 0u || mask.kind.x >= RAIL_CODE || mask.half >= PATH_HALF_M || mask.edge > past || dot(mask.axis, mask.axis) == 0.0) {
        return none;
    }
    let across = render_normal(vec3f(mask.axis, 0.0));
    let cross_w = max(length(vec2f(dot(ds[0], mask.axis), dot(ds[1], mask.axis))), 1e-3);
    // The hillside past the edge on the pixel's side, and how steeply it rises away from the path.
    let hillside = render_normal(drawn_normal(s + mask.away * (past - mask.edge)));
    let out = sign(mask.across) * across;
    let rise = -dot(hillside.xy, out.xy) / max(hillside.z, 0.1);
    let up = reach * smoothstep(LEDGE_RISE.x, LEDGE_RISE.y, rise);
    let down = reach * smoothstep(LEDGE_RISE.x, LEDGE_RISE.y, -rise);
    let bank = clamp(rise * mask.half, 0.0, BANK_MAX_M);
    let tread = reach * band(mask.across, -mask.half, mask.half, cross_w);
    var normal = mix(n, normalize(n - across * dot(n, across)), tread);
    normal = mix(normal, hillside, up * band(mask.edge, bank, past, cross_w));
    normal = normalize(mix(normal, normalize(vec3f(-BANK_CUT * out.xy, 1.0)), 0.5 * up * band(mask.edge, 0.0, bank, cross_w)));
    let near = 1.0 - smoothstep(LEDGE_NEAR.x, LEDGE_NEAR.y, distance);
    let earth = 0.6 * near * up * band(mask.edge, -FRAY_M, bank, cross_w);
    let shadow = clamp(1.0 + mask.edge / FOOT_M, 0.0, 1.0) * band(mask.edge, -FOOT_M, 0.0, cross_w);
    let tint = 1.0 + near * (0.12 * down * band(mask.edge, -LIP_M, 0.0, cross_w) - 0.3 * up * shadow);
    return Ledge(normal, max(tread, up * band(mask.edge, 0.0, past, cross_w)), earth, tint);
}
