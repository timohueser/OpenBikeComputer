// The stamps: per texel, the interval [lo, hi] of each feature near it, intersected in priority
// buckets that combine from the top, then clamp(base, lo, hi). The result does not depend on the
// order of the features, and neighbouring nodes get the same intervals where their data overlap.
// stamp.wgsl stamps pages with it and audit.wgsl checks against it. Include it with
// `// #include stamp-core.wgsl`; bins.ts packs the data and the profiles, and mirrors the gates of `road_shows`
// and `path_shows`. The records and the paint are in paint-core.wgsl.
// #include paint-core.wgsl

// The buckets, from the top.
const WATER_CORE: u32 = 0u;
// Nothing on the ground rises through the road of a deck.
const DECK: u32 = 1u;
const ROAD_CORE: u32 = 2u;
// A junction's corner yields to the roads, whose own heights hold their carriageways and sidewalks.
const CORNER_CORE: u32 = 3u;
// A tunnel's road into its portal yields to every road over it, as under a road on an embankment.
const PORTAL_CORE: u32 = 4u;
// A path's tread outranks the slopes of the roads beside it.
const PATH_CORE: u32 = 5u;
const BANKS: u32 = 6u;
// The slopes of the roads and the paths.
const CONES: u32 = 7u;
// The ground under a deck yields to every road beside it and under it.
const TROUGH: u32 = 8u;
const COVER: u32 = 9u;
const BUCKETS: u32 = 10u;
// As KERB_M of roads.wgsl: a corner's slab keeps its kerb where its sidewalk narrows to nothing.
const CORNER_KERB_M: f32 = 0.18;

// Near a junction, where the cores of its roads overlap, they hold the middle of their heights where
// these disagree; the reach of a junction is at most JUNCTION_CAP.
const JUNCTION_CAP: f32 = 15.0;
// A lake's bed falls from its shore at SHORE_SLOPE to BED_DEPTH. On the levels that can hold it, the
// terrain about the lake stays under a ramp that rises from the shore at SHORE_SLOPE over SHORE_CELLS
// mesh cells, then widens until it stops binding at the buffer. So every mesh triangle across a shore
// crosses the level at the shore, and the lake's outline is the water's edge.
const SHORE_SLOPE: f32 = 0.5;
const SHORE_CELLS: f32 = 1.5;
const BED_DEPTH: f32 = 4.0;
// The least widening that a level must fit past the ramp.
const SHORE_EASE: f32 = 4.0;
// Water lies LIFT_M plus LIFT of a mesh cell above the terrain that it meets, so that the mesh between
// the vertices of a ribbon, and the terrain at a lake's shore, stay under it.
const LIFT_M: f32 = 0.03;
const LIFT: f32 = 0.02;
// A bridge rests on its abutments for ABUTMENT_CELLS mesh cells from an end that meets the ground: the
// road's core runs on under the deck, so no mesh triangle there reaches the trough. Past them the
// ground stays CLEAR_M under the deck's underside, out to TROUGH_M past the deck's edge, and widens
// from there at 45° until it stops binding TROUGH_REACH_M out.
const CLEAR_M: f32 = 1.0;
const TROUGH_M: f32 = 1.0;
const TROUGH_REACH_M: f32 = 15.0;
// The ground under a deck's road stays at least DECK_GAP_M under it, whatever road beside it holds the ground.
const DECK_GAP_M: f32 = 0.1;
// A tunnel runs on the ground into its portal for PORTAL_CELLS mesh cells from an end that meets the
// ground. Past them the ground stays COVER_M over its bore, out to BORE_WALL_M past the road's edge, and
// falls from there at 45° until it stops binding COVER_REACH_M out.
const COVER_M: f32 = 1.0;
const BORE_WALL_M: f32 = 1.0;
const COVER_REACH_M: f32 = 15.0;
// A gallery's wall and pillars stand GALLERY_BACK_M past its road's edge. Behind them the hill holds the
// top of its roof, less toward its ends, and falls from there at GALLERY_FILL to the hillside and to
// its ends, until it stops binding
// GALLERY_REACH_M out; on the valley side the ground falls away, cut at GALLERY_VALLEY_CUT at most.
const GALLERY_BACK_M: f32 = 1.0;
const GALLERY_FILL: f32 = 0.6;
const GALLERY_REACH_M: f32 = 15.0;
const GALLERY_VALLEY_CUT: f32 = 0.2;

fn cone_reach(h: f32, rule: Profile, texel: f32) -> f32 {
    return min(rule.reach, reach_limit(texel) - h);
}

// A path bench shows from one texel: the mesh cannot show it, the normals can.
fn path_shows(h: f32, rule: Profile, level: u32) -> bool {
    let texel = gate_texel(level);
    return 2.0 * h >= texel && h + rule.reach <= reach_limit(texel);
}

fn shore_band(texel: f32) -> f32 {
    return SHORE_CELLS * MESH_TEXELS * texel;
}

// Whether a level holds the terrain about a lake under its ramp.
fn shore_shows(level: u32) -> bool {
    let texel = gate_texel(level);
    return shore_band(texel) + SHORE_EASE <= reach_limit(texel);
}

// How far water lies above the terrain that it meets, on a level with texels of this size.
fn water_lift(texel: f32) -> f32 {
    return LIFT_M + LIFT * MESH_TEXELS * texel;
}

// The most that the terrain may rise d metres from a lake's shore.
fn shore_cap(d: f32, texel: f32) -> f32 {
    let band = shore_band(texel);
    return SHORE_SLOPE * d + widen(max(0.0, d - band), reach_limit(texel) - band, 1.0);
}

// How far a junction's cores reach, with the widest half-width of its roads: twice that is its reach.
fn junction_reach(h: f32, texel: f32) -> f32 {
    return min(min(JUNCTION_CAP, 2.0 * h), reach_limit(texel) * 0.5);
}

// A width that grows at `slope` and goes to infinity at `reach`, so an interval stops binding
// smoothly instead of with a step.
fn widen(x: f32, reach: f32, slope: f32) -> f32 {
    if (x >= reach) {
        return INF;
    }
    return slope * x * reach / (reach - x);
}

fn segment_distance(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - a - t * ab);
}

// Whether w + ε(1, ε) lies left of u. A point on a shore line takes the side of that tiny step,
// so its side does not depend on the origin that counts the crossings. The products are compared,
// not subtracted, so that no fused multiply-add makes a tie uneven.
fn left(u: vec2f, w: vec2f) -> bool {
    let l = u.x * w.y;
    let r = u.y * w.x;
    return l > r || (l == r && (u.y < 0.0 || (u.y == 0.0 && u.x > 0.0)));
}

// Whether segment c-p crosses edge a-b. The origin c lies off every edge.
fn crosses(c: vec2f, p: vec2f, a: vec2f, b: vec2f) -> bool {
    let one = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x) > 0.0;
    return left(a - c, p - c) != left(b - c, p - c) && one != left(b - a, p - a);
}

// The intervals at a texel, what paints it, the water height (NONE without water) and the flags.
struct Stamp {
    lo: f32,
    hi: f32,
    conflict: bool,
    water: f32,
    paint: Paint,
    // The nearest lake shore alone, negative in a lake.
    lake: f32,
    // Whether the cones of the roads hold the lower bound (a fill) and the upper bound (a cut).
    fill: bool,
    cut: bool,
    // Whether p lies in water or in a building, where nothing stands.
    taken: bool,
}

// The stamp of world point p, a texel centre of a node of `level` whose first texel is `first`, from
// the node's data at `data`.
fn stamp_at(level: u32, first: vec2i, data: u32, p: vec2i) -> Stamp {
    var out = Stamp(-INF, INF, false, -1e30, no_paint(), INF, false, false, false);
    if (data == NO_DATA) {
        return out;
    }
    let shift = 22u - level;
    let unit = unit_metres(p.y);
    let texel = unit * f32(1 << shift);
    let size = CELL_TEXELS << shift;
    let origin = (first << vec2u(shift)) - vec2i(size);
    let cell = clamp((p - origin) / size, vec2i(0), vec2i(GRID - 1));
    let c = cell.y * GRID + cell.x;
    let offset = word(data + HEAD + u32(c));
    // The cell's origin of the lake crossings: its centre, held inside the node, where the feature tile
    // is valid, moved off every shore line of the cell by the worker.
    let nudge = vec2i(i32((offset >> 24u) & 15u), i32(offset >> 28u)) - vec2i(8);
    let centre = clamp(origin + cell * size + vec2i(size / 2), first << vec2u(shift), ((first + vec2i(255)) << vec2u(shift)) + vec2i((1 << shift) - 1)) + nudge;
    let records = data + HEAD + OFFSETS + word(data + 1u);
    var lo: array<f32, BUCKETS>;
    var hi: array<f32, BUCKETS>;
    for (var b = 0u; b < BUCKETS; b++) {
        lo[b] = -INF;
        hi[b] = INF;
    }
    // Whether the texel is within the reach of a junction.
    var junction = false;
    // The lake of the edges so far: whether they cross the way from the cell's origin to p, and the shore distance.
    var crossed = false;
    var shore = INF;
    var water_in = -1e30;
    var in_lake = false;
    // The road line of the segments so far: whether p lies in its core band, and whether a segment
    // holds it there. Inside a bend tighter than the core, none does: a conflict, which the mask flags.
    var line = NO_DATA;
    var line_near = false;
    var line_held = false;
    // The metres from the road that holds each bound of the cones, and whether a road holds it, not a path.
    var cone_lo_x = INF;
    var cone_hi_x = INF;
    var cone_lo_road = false;
    var cone_hi_road = false;
    // Whether p lies under the road of a deck.
    var under_deck = false;
    var paint = no_paint();
    let first_entry = offset & 0xffffffu;
    let last_entry = word(data + HEAD + u32(c) + 1u) & 0xffffffu;
    for (var e = first_entry; e < last_entry; e++) {
        let entry = word(data + HEAD + OFFSETS + e);
        let r = records + RECORD * (entry & 0x3fffffffu);
        let kind = word(r + 4u);
        if (kind == JUNCTION) {
            junction = junction || length(vec2f(p - point(r))) * unit < 2.0 * junction_reach(real(r + 6u), texel);
        } else if (kind == CORNER) {
            // The corner paints its asphalt and the sidewalk round it and holds their ground: each road's
            // surface carried on from its end, as heightAt has it, and the slab with it. On a road's carriageway
            // the road paints and holds, but for its edge there, which the arc replaces; the corner takes only the
            // slivers beside it where a road bends inside it. Its record: the centre, the first end, the heights at
            // the ends, the sidewalks' widths in decimetres, the slopes per metre east and south of the first road
            // in words 9 and 13 and of the second in 14 and 15, the second end and the narrower half-width.
            let corner = in_corner(r, p, vec2f(0.0), unit, level);
            if (!corner.holds) {
                continue;
            }
            paint_corner(&paint, r, corner);
            let rule = profiles[(word(r + 5u) >> 16u) & 0xffu];
            let walks = word(r + 8u);
            let e = corner.e;
            let ea = corner.ea;
            let eb = corner.eb;
            if (corner.d <= select(corner.walk, max(corner.walk, CORNER_KERB_M), walks != 0u)) {
                // As heightAt of corners.ts: by the distances past each road's sidewalk from the first road's side,
                // the centre to its end and on to the roads' corner c, and from the second's.
                let c = ea + vec2f(-ea.y, ea.x) * dot(eb - ea, eb) / dot(vec2f(-ea.y, ea.x), eb);
                let da = max(0.0, min(segment_distance(e, vec2f(0.0), ea), segment_distance(e, ea, c)) * unit - f32(walks & 0xffu) * 0.1);
                let db = max(0.0, min(segment_distance(e, c, eb), segment_distance(e, eb, vec2f(0.0))) * unit - f32((walks >> 8u) & 0xffu) * 0.1);
                let za = real(r + 6u) + dot(vec2f(real(r + 9u), real(r + 13u)), vec2f(p - point(r + 2u))) * unit;
                let zb = real(r + 7u) + dot(vec2f(real(r + 14u), real(r + 15u)), vec2f(p - point(r + 10u))) * unit;
                let z = mix(za, zb, select(0.5, da / (da + db), da + db > 0.0));
                lo[CORNER_CORE] = max(lo[CORNER_CORE], z - rule.tol);
                hi[CORNER_CORE] = min(hi[CORNER_CORE], z + rule.tol);
            }
        } else if (kind == EDGE) {
            // Relative to the cell's origin and to the edge, so that every node that holds the edge gets the same bits.
            let a = point(r);
            let b = point(r + 2u);
            crossed = crossed != crosses(vec2f(0.0), vec2f(p - centre), vec2f(a - centre), vec2f(b - centre));
            shore = min(shore, segment_distance(vec2f(p - a), vec2f(0.0), vec2f(b - a)) * unit);
        } else if (kind == PAD) {
            // A platform holds the ground inside it under its top.
            if (((entry & INSIDE) != 0u) != crossed) {
                hi[BANKS] = min(hi[BANKS], real(r + 6u));
            }
            crossed = false;
            shore = INF;
        } else if (kind == AREA) {
            out.taken = out.taken || ((entry & INSIDE) != 0u) != crossed;
            crossed = false;
            shore = INF;
        } else if (kind == LAKE) {
            let water = real(r + 6u);
            if (((entry & INSIDE) != 0u) != crossed) {
                hi[WATER_CORE] = min(hi[WATER_CORE], water - min(BED_DEPTH, SHORE_SLOPE * shore));
                water_in = max(water_in, water);
                in_lake = true;
                out.lake = min(out.lake, -shore);
            } else {
                if (shore_shows(level)) {
                    hi[BANKS] = min(hi[BANKS], water + shore_cap(shore, texel));
                }
                out.lake = min(out.lake, shore);
            }
            crossed = false;
            shore = INF;
        } else if (kind == RIVER) {
            // A river holds its level over its width as a road holds its height, with banks for cones.
            // In a lake the lake's bed holds: its river line runs on through it. The cell lists its lakes
            // before its lines, so in_lake is final here.
            let h = real(r + 8u);
            let rule = profiles[(word(r + 5u) >> 16u) & 0xffu];
            let g = along(r, p, vec2f(0.0), unit);
            if (g.lateral >= INF || in_lake || !road_shows(h, rule, level)) {
                continue;
            }
            let x = g.lateral - h;
            if (g.core && x <= 0.0) {
                lo[WATER_CORE] = max(lo[WATER_CORE], g.z - rule.tol);
                hi[WATER_CORE] = min(hi[WATER_CORE], g.z + rule.tol);
                water_in = max(water_in, g.z);
            }
            let reach = cone_reach(h, rule, texel);
            if (g.cone && max(x, 0.0) < reach) {
                lo[BANKS] = max(lo[BANKS], g.z - (rule.tol + widen(max(x, 0.0), reach, rule.fill)));
                hi[BANKS] = min(hi[BANKS], g.z + (rule.tol + widen(max(x, 0.0), reach, rule.cut)));
            }
        } else {
            if ((word(r + 13u) & 0xffffu) != line) {
                out.conflict = out.conflict || (line_near && !line_held);
                line = word(r + 13u) & 0xffffu;
                line_near = false;
                line_held = false;
            }
            let rule = profiles[(word(r + 5u) >> 16u) & 0xffu];
            let g = along(r, p, vec2f(0.0), unit);
            let own = real(r + 8u);
            let h = line_half(r, g);
            // A bridge past its abutments holds the ground under its deck, and paints nothing: the deck draws its road.
            let bridge = kind == BRIDGE;
            if (bridge && off_ground(r, g, rule, level, texel, ABUTMENT_CELLS)) {
                let x = max(0.0, g.lateral - h - TROUGH_M);
                let reach = min(TROUGH_REACH_M, reach_limit(texel) - h - TROUGH_M);
                hi[TROUGH] = min(hi[TROUGH], g.z - f32(word(r + 5u) >> 24u) * 0.1 - CLEAR_M + widen(x, reach, 1.0));
                if (g.lateral <= h) {
                    hi[DECK] = min(hi[DECK], g.z - DECK_GAP_M);
                    under_deck = true;
                }
                continue;
            }
            // A tunnel past its portals holds the ground over its bore, and paints nothing.
            if (kind == TUNNEL && off_ground(r, g, rule, level, texel, PORTAL_CELLS)) {
                let x = max(0.0, g.lateral - h - BORE_WALL_M);
                let reach = min(COVER_REACH_M, reach_limit(texel) - h - BORE_WALL_M);
                lo[COVER] = max(lo[COVER], g.z + f32(word(r + 5u) >> 24u) * 0.1 + COVER_M - widen(x, reach, 1.0));
                continue;
            }
            let paved = kind == ROAD || kind == RAIL || bridge || kind == TUNNEL;
            // A town road's sidewalk lies at the height of its edge: its core reaches past it.
            let walk = line_walk(r, rule, g);
            if (paved && road_shows(own, rule, level) && (g.beside <= h + walk || (g.core && g.lateral <= h + walk))) {
                line_near = true;
                line_held = line_held || (g.core && g.lateral <= h + walk);
            }
            if (g.lateral >= INF) {
                continue;
            }
            paint_line(&paint, r, kind, g, h, walk);
            // A gallery's floor runs flat to a mesh cell past its walls and pillars, so the mesh's slope up to
            // the hill lies behind its wall. Past that, on the hill side, the hill holds the top of its roof
            // and no cut; on the valley side a shallow cut keeps it open.
            let roof = f32((word(r + 5u) >> 24u) & 0x7fu) * 0.1;
            let gallery = kind <= PATH && roof > 0.0;
            let wall = h + GALLERY_BACK_M + MESH_TEXELS * texel;
            let hill = gallery && (g.side > 0.0) == ((word(r + 5u) >> 31u) != 0u);
            if (hill && g.core && g.lateral > wall) {
                let reach = min(GALLERY_REACH_M, reach_limit(texel) - wall);
                let end = min(real(r + 14u) + g.da, real(r + 15u) + g.db);
                lo[COVER] = max(lo[COVER], g.z + min(roof, GALLERY_FILL * end) - widen(g.lateral - wall, reach, GALLERY_FILL));
                continue;
            }
            if (paved && road_shows(own, rule, level)) {
                let tol = rule.tol;
                let core_half = select(h, wall, gallery) + walk;
                if (g.core && g.lateral <= core_half) {
                    let bucket = select(ROAD_CORE, PORTAL_CORE, kind == TUNNEL);
                    lo[bucket] = max(lo[bucket], g.z - tol);
                    hi[bucket] = min(hi[bucket], g.z + tol);
                }
                let x = max(0.0, g.lateral - core_half);
                let reach = cone_reach(core_half, rule, texel);
                if (g.cone && x < reach) {
                    let l = g.z - (tol + widen(x, reach, rule.fill));
                    let u = g.z + (tol + widen(x, reach, select(rule.cut, GALLERY_VALLEY_CUT, gallery)));
                    if (l > lo[CONES] || (l == lo[CONES] && x < cone_lo_x)) {
                        lo[CONES] = l;
                        cone_lo_x = x;
                        cone_lo_road = true;
                    }
                    if (u < hi[CONES] || (u == hi[CONES] && x < cone_hi_x)) {
                        hi[CONES] = u;
                        cone_hi_x = x;
                        cone_hi_road = true;
                    }
                }
            } else if (kind == PATH && path_shows(own, rule, level)) {
                let x = max(0.0, g.lateral - h);
                let tol = rule.tol + rule.tilt * min(g.lateral, h);
                if (g.core && g.lateral <= h) {
                    lo[PATH_CORE] = max(lo[PATH_CORE], g.z - tol);
                    hi[PATH_CORE] = min(hi[PATH_CORE], g.z + tol);
                }
                if ((g.core || g.cone) && x < rule.reach) {
                    // A path's slopes meet the slopes of the roads beside it as two roads' slopes meet.
                    let l = g.z - tol - widen(x, rule.reach, rule.fill);
                    let u = g.z + tol + widen(x, rule.reach, rule.cut);
                    if (l > lo[CONES] || (l == lo[CONES] && x < cone_lo_x)) {
                        lo[CONES] = l;
                        cone_lo_x = x;
                        cone_lo_road = false;
                    }
                    if (u < hi[CONES] || (u == hi[CONES] && x < cone_hi_x)) {
                        hi[CONES] = u;
                        cone_hi_x = x;
                        cone_hi_road = false;
                    }
                }
            }
        }
    }
    out.conflict = out.conflict || (line_near && !line_held);
    out.paint = painted(paint);
    out.water = water_in;
    out.taken = out.taken || water_in > -1e30;
    // Under a deck the ground falls away: no fill of a road or a path holds it up. Their cuts hold, so a
    // road under a bridge keeps its cut slopes. Beside the deck the fills of the roads there hold: the
    // deck's face hides their step.
    if (under_deck) {
        lo[CONES] = -INF;
    }
    // Under water no bank holds: where a river meets a lake or another river.
    if (hi[WATER_CORE] < INF) {
        lo[BANKS] = -INF;
        hi[BANKS] = INF;
    }
    // A bank or a cut digs no deeper than a bore's cover: a lake's shore ramp, a river's bank and the cut
    // of a road or a path leave a tunnel buried.
    hi[BANKS] = max(hi[BANKS], lo[COVER]);
    if (lo[COVER] > hi[CONES]) {
        hi[CONES] = lo[COVER];
        cone_hi_road = false;
    }
    // Near a junction the cores of its roads hold the middle of their heights where they disagree by
    // the centimetres of the solver, and a core outranks the cones. Past the cores their cones ramp as
    // anywhere else, so a wall between two roads grows from nothing at their junction.
    if (junction) {
        if (lo[ROAD_CORE] > hi[ROAD_CORE]) {
            let middle = 0.5 * (lo[ROAD_CORE] + hi[ROAD_CORE]);
            lo[ROAD_CORE] = middle;
            hi[ROAD_CORE] = middle;
        }
        if (hi[ROAD_CORE] < INF) {
            lo[CONES] = -INF;
            hi[CONES] = INF;
        }
    }
    // A conflict, which the mask flags: an empty bucket holds the middle of its bounds, but for the
    // cones, and a bucket outside the interval pulls it to its nearest end. So the height has no step
    // where a conflict begins.
    for (var b = 0u; b < BUCKETS; b++) {
        if (lo[b] <= -INF && hi[b] >= INF) {
            continue;
        }
        var l = lo[b];
        var u = hi[b];
        // A corner, a portal, the ground under a deck and the cover of a tunnel yield to the buckets above them without a conflict.
        if ((b == CORNER_CORE || b == PORTAL_CORE || b == TROUGH || b == COVER) && (l > out.hi || u < out.lo)) {
            continue;
        }
        // The cones of two roads: the ground ramps from the fill of the one to the cut of the other by
        // the share of the way between them, so it has no step where the conflict begins nor inside it.
        // A ramp is ground, never a wall: a retaining wall holds the slope beside one road.
        let ramp = l > u && b == CONES;
        if (l > u) {
            out.conflict = true;
            l = select(0.5 * (l + u), mix(l, u, cone_lo_x / max(cone_lo_x + cone_hi_x, 1e-3)), ramp);
            u = l;
        }
        if (l > out.hi || u < out.lo) {
            out.conflict = true;
            let end = select(out.lo, out.hi, l > out.hi);
            out.lo = end;
            out.hi = end;
            continue;
        }
        // A road's slope may be a retaining wall; a path's never is.
        if (l > out.lo) {
            out.lo = l;
            out.fill = b == CONES && cone_lo_road && !ramp;
        }
        if (u < out.hi) {
            out.hi = u;
            out.cut = b == CONES && cone_hi_road && !ramp;
        }
    }
    return out;
}
