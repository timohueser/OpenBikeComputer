// Material fields: the land is cut into fields, Voronoi cells about FIELD across, each with one row
// direction and row spacing of its own. The ground paints the rows of the materials whose `rows`
// (materials.json) say how they are planted (look.wgsl), and the plantings stand on the same rows
// (vegetation/rows.wgsl). A field is whole world units and its hashes, so both agree at every distance.
// #include ../terrain/drawn-height.wgsl

// Units along a field cell: about 105 m at 47° N. The site of a cell keeps to the middle of it, so
// the site nearest a point is among the four cells about it all but a few times; both users look there only.
const FIELD_SHIFT: u32 = 12u;
const FIELD: i32 = 4096;
// Rows keep this far from their field's edge, as a headland.
const HEADLAND_M: f32 = 3.0;

fn field_bits(c: vec2i, salt: u32) -> u32 {
    var h = (bitcast<u32>(c.x) * 0x8da6b343u) ^ (bitcast<u32>(c.y) * 0xd8163841u) ^ (salt * 0xcb1ab31fu);
    h = (h ^ (h >> 16u)) * 0x7feb352du;
    h = (h ^ (h >> 15u)) * 0x846ca68bu;
    return h ^ (h >> 16u);
}

fn field_hash(c: vec2i, salt: u32) -> f32 {
    return f32(field_bits(c, salt) >> 8u) / 16777216.0;
}

// The site of field cell c in whole world units.
fn field_site(c: vec2i) -> vec2i {
    let b = field_bits(c, 0u);
    return (c << vec2u(FIELD_SHIFT)) + vec2i(vec2f(0.15) * f32(FIELD) + 0.7 * f32(FIELD) * vec2f(f32(b & 4095u), f32((b >> 12u) & 4095u)) / 4096.0);
}

// The first of the 2 × 2 field cells whose sites a point at `u` units about the anchor looks at.
fn field_cells(u: vec2f) -> vec2i {
    let low = vec2f(surface.anchor & vec2i(FIELD - 1));
    return (surface.anchor >> vec2u(FIELD_SHIFT)) + vec2i(floor((low + u) / f32(FIELD) - 0.5));
}

// The field of a point at `u` units about the anchor among the 2 × 2 cells from `first`: its cell,
// and how far the point lies inside it, in units.
struct FieldAt {
    cell: vec2i,
    edge: f32,
}

fn field_of(first: vec2i, u: vec2f) -> FieldAt {
    var d1 = 1e30;
    var d2 = 1e30;
    var cell = first;
    for (var k = 0; k < 4; k++) {
        let c = first + vec2i(k & 1, k >> 1);
        let d = length(u - vec2f(field_site(c) - surface.anchor));
        if (d < d1) {
            d2 = d1;
            d1 = d;
            cell = c;
        } else if (d < d2) {
            d2 = d;
        }
    }
    return FieldAt(cell, 0.5 * (d2 - d1));
}

// The rows of a field of a material of `rows` (across and along in metres, each a range): the row
// direction in world units (y south), the metres along and across the rows, and the metres per unit
// at its site, so a field keeps its rows wherever the eye is.
struct FieldRows {
    along: vec2f,
    pitch: vec2f,
    unit: f32,
}

fn field_rows(c: vec2i, rows: vec4f) -> FieldRows {
    let a = 3.14159265 * field_hash(c, 2u);
    // Mercator: the metres per unit (at the equator, as anchorAt of frame/space.ts) fall with the
    // cosine of the latitude, 1 / cosh of the world y.
    let y = 3.14159265 * (1.0 - f32(field_site(c).y) / 536870912.0);
    let unit = 0.037281 / cosh(y);
    return FieldRows(vec2f(cos(a), sin(a)), vec2f(mix(rows.z, rows.w, field_hash(c, 4u)), mix(rows.x, rows.y, field_hash(c, 3u))), unit);
}
