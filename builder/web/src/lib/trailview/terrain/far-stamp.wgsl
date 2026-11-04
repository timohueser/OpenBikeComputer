// The far stamp of the ground plan: one stamp for every terrain page on the plan's path. Every way is a
// stroke: per segment, its core plane, its cut and fill slopes and its paint measure the distance to the
// segment, so a bend has a round join on its outer side and a line end a round cap; a structure's end is
// flat. Overlapping cores take the core of the way that the paint ranks first. It has no junction, corner,
// sidewalk or snap logic, and reads only the node's data, so every page and every shot is the same.
//
// Footprints: a way changes the ground only within its core and slopes, `h + edge + reach` of a segment of
// its line. Water and structures keep the stamps of stamp-core.wgsl, which reach farther: a river's banks
// its profile's reach past its edge, the ground under a deck TROUGH_M + TROUGH_REACH_M past the way's
// edge, the cover over a bore BORE_WALL_M + COVER_REACH_M past it, a gallery's fill GALLERY_REACH_M past
// its wall; a lake's and the sea's ramp ends SHORE_REACH_M from the shore.
// #include stamp-core.wgsl
// #include model.wgsl
// #include heights.wgsl

// The stamp pipeline of the plan's path sets it; `stamp_of` then runs the far stamp.
override FAR_STAMP: bool = false;
// The greatest bank reach of the water near the eye: with the owned region's growth of 9.75 m it stays under
// the 40 m margin of the feature tiles.
override SHORE_REACH_M: f32 = 15.0;
// The one road model (prototype): the snap moves vertices onto the core's edge, past the floors.
override SNAP_CORE: bool = false;

const STREAM: u32 = 7u;
// The end of a line: round, or flat where a structure starts, as CAP_A to FLAT_B of bins.ts.
const CAP_A: u32 = 2u;
const CAP_B: u32 = 4u;
const FLAT_A: u32 = 8u;
const FLAT_B: u32 = 16u;
// A bore's approach yields to every carriageway on the ground over it, and outranks every edge.
const RANK_PORTAL: f32 = 1000.0;
// Where a texel lies about a segment: in its mitre quad, past a round end, past a flat end, or elsewhere.
const QUAD: u32 = 0u;
const CAP: u32 = 1u;
const FLAT: u32 = 2u;
const NONE_ZONE: u32 = 3u;

// The stamp, and the record index + 1 and the line index + 1 of the core's winner, 0 for none; the
// water's core, which bounds every road, and what bounds every slope: the cover over a bore from
// below, the underside of a deck from above.
struct FarStamp {
    stamp: Stamp,
    core: u32,
    line: u32,
    water: vec2f,
    slope: vec2f,
}

struct FarAlong {
    // Metres from the segment's line, and from the segment itself (a capsule).
    lateral: f32,
    seg: f32,
    // The height of the core plane, by the mitre-quad parameter, and the side of the line.
    z: f32,
    side: f32,
    zone: u32,
    // Outside the quad, the metres past the mitre line of the nearer end.
    past: f32,
    // Metres past the mitre lines of the start and of the end, along the segment.
    da: f32,
    db: f32,
    // Metres from the line where the foot of the perpendicular lies on the segment, else INF.
    beside: f32,
}

fn stamp_of(level: u32, first: vec2i, data: u32, p: vec2i) -> Stamp {
    if (FAR_STAMP) {
        return far_stamp_at(level, first, data, p, true).stamp;
    }
    return stamp_at(level, first, data, p);
}

fn shore_limit(texel: f32) -> f32 {
    return min(reach_limit(texel), SHORE_REACH_M);
}

fn far_shore_cap(d: f32, texel: f32) -> f32 {
    let band = shore_band(texel);
    return SHORE_SLOPE * d + widen(max(0.0, d - band), shore_limit(texel) - band, 1.0);
}

fn far_sea_band(texel: f32) -> f32 {
    return clamp(shore_limit(texel) - BED_DEPTH / SHORE_SLOPE, 0.0, shore_band(texel));
}

fn far_shore_shows(level: u32) -> bool {
    let texel = gate_texel(level);
    return shore_band(texel) + SHORE_EASE <= shore_limit(texel);
}

fn far_along(r: u32, p: vec2i, unit: f32) -> FarAlong {
    let a = point(r);
    let b = point(r + 2u);
    let flags = (word(r + 5u) >> 8u) & 0xffu;
    let ab = vec2f(b - a);
    let ap = vec2f(p - a);
    let bp = vec2f(p - b);
    let len = length(ab);
    let dir = ab / max(len, 1e-6);
    let ma = unpack2x16snorm(word(r + 9u));
    let mb = unpack2x16snorm(word(r + 10u));
    let da = dot(ap, ma) / max(dot(dir, ma), 0.1);
    let db = -dot(bp, mb) / max(dot(dir, mb), 0.1);
    let cross = dir.x * ap.y - dir.y * ap.x;
    let t = clamp(da / max(da + db, 1e-6), 0.0, 1.0);
    // The rise to the right of the bank at the start and the end.
    let bank = unpack2x16snorm(word(r + 11u));
    let z = mix(real(r + 6u), real(r + 7u), t) + cross * unit * mix(bank.x, bank.y, t);
    let foot = dot(ap, dir);
    let on = foot >= 0.0 && foot <= len;
    let seg = select(select(length(bp), length(ap), foot < 0.0), abs(cross), on) * unit;
    var zone = QUAD;
    var past = 0.0;
    if (da < 0.0 || db < 0.0) {
        let start = da < db;
        past = -select(db, da, start) * unit;
        let end_bits = flags & select(CAP_B | FLAT_B, CAP_A | FLAT_A, start);
        zone = select(select(NONE_ZONE, FLAT, (end_bits & (FLAT_A | FLAT_B)) != 0u), CAP, (end_bits & (CAP_A | CAP_B)) != 0u);
    }
    return FarAlong(abs(cross) * unit, seg, z, sign(cross), zone, past, da * unit, db * unit, select(INF, abs(cross) * unit, on));
}

// A segment's distance to its core of half-width w: negative inside, never negative past a flat end.
fn far_distance(g: FarAlong, w: f32) -> f32 {
    if (g.zone == FLAT) {
        return length(vec2f(max(0.0, g.lateral - w), g.past));
    }
    return g.seg - w;
}

// The rank of a way x metres past its carriageway's edge, as way_rank without sidewalks or yields.
fn far_rank(x: f32, paved: bool, tunnel: bool) -> f32 {
    return way_rank(x, 0.0, paved, false, 0.0) + select(0.0, RANK_PORTAL, tunnel && x < 0.0);
}

fn far_sleepers(r: u32, kind: u32, g: FarAlong) -> f32 {
    if (kind == RAIL) {
        return mix(real(r + 14u), real(r + 15u), clamp(g.da / max(g.da + g.db, 1e-6), 0.0, 1.0));
    }
    let start = real(r + 14u);
    return select(-(real(r + 15u) + g.db), start + g.da, start < 1e29) / SLEEPER_M;
}

// The far stamp of world point p, a texel centre of a node of `level` whose first texel is `first`, from
// the node's data at `data`. Without `roads` the ways shape no core and no slope (the one road model's
// heights do), and keep the ground of their structures.
fn far_stamp_at(level: u32, first: vec2i, data: u32, p: vec2i, roads: bool) -> FarStamp {
    var out = Stamp(-INF, INF, false, -1e30, INF, 0.0, 0u, 0.0, false, 0.0, INF, false, false, INF, INF, false, false, false, false);
    if (data == NO_DATA) {
        return FarStamp(out, 0u, 0u, vec2f(-INF, INF), vec2f(-INF, INF));
    }
    let shift = 22u - level;
    let unit = unit_metres(p.y);
    let texel = unit * f32(1 << shift);
    let size = CELL_TEXELS << shift;
    let origin = (first << vec2u(shift)) - vec2i(size);
    let cell = clamp((p - origin) / size, vec2i(0), vec2i(GRID - 1));
    let c = cell.y * GRID + cell.x;
    let offset = word(data + HEAD + u32(c));
    let nudge = vec2i(i32((offset >> 24u) & 15u), i32(offset >> 28u)) - vec2i(8);
    let centre = clamp(origin + cell * size + vec2i(size / 2), first << vec2u(shift), ((first + vec2i(255)) << vec2u(shift)) + vec2i((1 << shift) - 1)) + nudge;
    let records = data + HEAD + OFFSETS + word(data + 1u);
    var lo: array<f32, BUCKETS>;
    var hi: array<f32, BUCKETS>;
    for (var b = 0u; b < BUCKETS; b++) {
        lo[b] = -INF;
        hi[b] = INF;
    }
    var crossed = false;
    var shore = INF;
    var water_in = -1e30;
    var in_lake = false;
    var edge = vec2f(-INF, INF);
    var abutment = false;
    var line = NO_DATA;
    var line_near = false;
    var line_held = false;
    var cone_lo_x = INF;
    var cone_hi_x = INF;
    var cone_lo_road = false;
    var cone_hi_road = false;
    var under_deck = false;
    // The core's winner: its rank, height, tolerance, record and line.
    var core_rank = INF;
    var core_z = 0.0;
    var core_tol = 0.0;
    var core_record = 0u;
    var core_line = 0u;
    var paint_rank = INF;
    var paint_z = INF;
    let first_entry = offset & 0xffffffu;
    let last_entry = word(data + HEAD + u32(c) + 1u) & 0xffffffu;
    for (var e = first_entry; e < last_entry; e++) {
        let entry = word(data + HEAD + OFFSETS + e);
        let index = entry & 0x3fffffffu;
        let r = records + RECORD * index;
        let kind = word(r + 4u);
        if (kind == EDGE) {
            let a = point(r);
            let b = point(r + 2u);
            crossed = crossed != crosses(vec2f(0.0), vec2f(p - centre), vec2f(a - centre), vec2f(b - centre));
            shore = min(shore, segment_distance(vec2f(p - a), vec2f(0.0), vec2f(b - a)) * unit);
        } else if (kind == PAD) {
            if (((entry & INSIDE) != 0u) != crossed) {
                hi[BANKS] = min(hi[BANKS], real(r + 6u));
            }
            crossed = false;
            shore = INF;
        } else if (kind == AREA) {
            out.taken = out.taken || ((entry & INSIDE) != 0u) != crossed;
            crossed = false;
            shore = INF;
        } else if (kind == LAKE || kind == SEA) {
            let water = real(r + 6u);
            let lake = kind == LAKE;
            if (((entry & INSIDE) != 0u) != crossed) {
                let bed = select(max(0.0, shore - far_sea_band(texel)), shore, lake);
                let bucket = select(SEA_BED, WATER, lake);
                hi[bucket] = min(hi[bucket], water - min(BED_DEPTH, SHORE_SLOPE * bed));
                water_in = max(water_in, water);
                in_lake = true;
                out.lake = select(out.lake, min(out.lake, -shore), lake);
            } else if (lake) {
                if (far_shore_shows(level)) {
                    hi[BANKS] = min(hi[BANKS], water + far_shore_cap(shore, texel));
                    edge.y = min(edge.y, water + edge_cap(shore, texel));
                }
                out.lake = min(out.lake, shore);
            }
            crossed = false;
            shore = INF;
        } else if (kind == RIVER) {
            let h = real(r + 8u);
            let rule = stamp_rules.profiles[(word(r + 5u) >> 16u) & 0xffu];
            let g = along(r, p, unit);
            if (g.lateral >= INF || in_lake || !road_shows(h, rule, level)) {
                continue;
            }
            let x = g.lateral - h;
            if (g.core && x <= 0.0) {
                lo[WATER] = max(lo[WATER], g.z - rule.tol);
                hi[WATER] = min(hi[WATER], g.z + rule.tol);
                water_in = max(water_in, g.z);
            }
            let reach = cone_reach(h, rule, texel);
            if (g.cone && max(x, 0.0) < reach) {
                lo[BANKS] = max(lo[BANKS], g.z - (rule.tol + widen(max(x, 0.0), reach, rule.fill)));
                hi[BANKS] = min(hi[BANKS], g.z + (rule.tol + widen(max(x, 0.0), reach, rule.cut)));
            }
            if (g.cone && x > 0.0 && shore_shows(level)) {
                edge = vec2f(max(edge.x, g.z - rule.tol - edge_cap(x, texel)), min(edge.y, g.z + rule.tol + edge_cap(x, texel)));
            }
        } else if (kind == STREAM) {
            // A stream shapes no ground: it keeps plants off its water, at least one texel wide.
            out.taken = out.taken || far_along(r, p, unit).seg <= max(real(r + 8u), 0.5 * texel);
        } else {
            if ((word(r + 13u) & 0xffffu) != line) {
                out.conflict = out.conflict || (line_near && !line_held);
                line = word(r + 13u) & 0xffffu;
                line_near = false;
                line_held = false;
            }
            let rule = stamp_rules.profiles[(word(r + 5u) >> 16u) & 0xffu];
            let g = far_along(r, p, unit);
            let h = real(r + 8u);
            let shows = road_shows(h, rule, level);
            // A bridge past its abutments holds the ground under its deck, and paints nothing: the deck draws its road.
            let bridge = kind == BRIDGE;
            // Without the roads' cores no abutment holds a deck's ends: the deck bounds all its span.
            if (bridge && !roads && g.zone == QUAD && shows && g.lateral <= h) {
                hi[DECK] = min(hi[DECK], g.z - DECK_GAP_M);
            }
            if (bridge && g.zone == QUAD && shows && min(real(r + 14u) + g.da, real(r + 15u) + g.db) >= ABUTMENT_CELLS * MESH_TEXELS * texel) {
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
            let tunnel = kind == TUNNEL;
            if (tunnel && g.zone == QUAD && shows && min(real(r + 14u) + g.da, real(r + 15u) + g.db) >= PORTAL_CELLS * MESH_TEXELS * texel) {
                let x = max(0.0, g.lateral - h - BORE_WALL_M);
                let reach = min(COVER_REACH_M, reach_limit(texel) - h - BORE_WALL_M);
                lo[COVER] = max(lo[COVER], g.z + f32(word(r + 5u) >> 24u) * 0.1 + COVER_M - widen(x, reach, 1.0));
                continue;
            }
            let paved = kind != PATH;
            let held = g.zone <= CAP && g.seg <= h;
            if (paved && shows && (g.beside <= h || held)) {
                line_near = true;
                line_held = line_held || held;
            }
            if (g.zone == NONE_ZONE) {
                continue;
            }
            let roof = f32((word(r + 5u) >> 24u) & 0x7fu) * 0.1;
            let gallery = kind <= PATH && roof > 0.0;
            // A gallery's floor runs flat to a mesh cell past its walls and pillars; on the hill side the hill
            // holds the top of its roof, on the valley side a shallow cut keeps it open.
            let wall = h + GALLERY_BACK_M + MESH_TEXELS * texel;
            let hill = gallery && (g.side > 0.0) == ((word(r + 5u) >> 31u) != 0u);
            if (hill && g.zone == QUAD && g.lateral > wall) {
                let reach = min(GALLERY_REACH_M, reach_limit(texel) - wall);
                let end = min(real(r + 14u) + g.da, real(r + 15u) + g.db);
                lo[COVER] = max(lo[COVER], g.z + min(roof, GALLERY_FILL * end) - widen(g.lateral - wall, reach, GALLERY_FILL));
                continue;
            }
            let code = word(r + 5u) & SURFACE_BITS;
            let x_paint = far_distance(g, h);
            let rank = far_rank(x_paint, paved, tunnel);
            if (rank < paint_rank || (rank == paint_rank && g.z < paint_z)) {
                paint_rank = rank;
                paint_z = g.z;
                out.road = x_paint;
                out.across = g.side * g.seg;
                out.surface = code;
                out.walk = 0.0;
                out.ford = ((word(r + 5u) >> 8u) & FORD) != 0u;
                out.sleepers = select(0.0, far_sleepers(r, kind, g), code >= TRACK_CODE);
                out.shoulder = SNAP_CORE;
                out.trodden = kind == PATH && code >= DIRT_CODE && code <= PATH_CODE;
                out.marked = ((word(r + 5u) >> 8u) & MARKED) != 0u;
            }
            if (!shows || !roads) {
                continue;
            }
            let core = select(h + real(r + 12u), wall, gallery);
            let d = far_distance(g, core);
            if (g.zone != FLAT && d <= 0.0) {
                let ranked = far_rank(d + core - h, paved, tunnel);
                if (ranked < core_rank || (ranked == core_rank && g.z < core_z)) {
                    core_rank = ranked;
                    core_z = g.z;
                    core_tol = rule.tol;
                    core_record = index + 1u;
                    core_line = (word(r + 13u) & 0xffffu) + 1u;
                }
                abutment = abutment || bridge;
            }
            let x = max(0.0, d);
            let reach = cone_reach(core, rule, texel);
            if (x < reach) {
                let l = g.z - rule.tol - widen(x, reach, rule.fill);
                let u = g.z + rule.tol + widen(x, reach, select(rule.cut, GALLERY_VALLEY_CUT, gallery));
                if (l > lo[CONES] || (l == lo[CONES] && x < cone_lo_x)) {
                    lo[CONES] = l;
                    cone_lo_x = x;
                    cone_lo_road = paved;
                }
                if (u < hi[CONES] || (u == hi[CONES] && x < cone_hi_x)) {
                    hi[CONES] = u;
                    cone_hi_x = x;
                    cone_hi_road = paved;
                }
            }
        }
    }
    out.conflict = out.conflict || (line_near && !line_held);
    let water = vec2f(select(-INF, lo[WATER], hi[WATER] < INF), hi[WATER]);
    if (core_rank < INF) {
        lo[ROAD_CORE] = core_z - core_tol;
        hi[ROAD_CORE] = core_z + core_tol;
    }
    out.water = water_in;
    out.taken = out.taken || water_in > -1e30;
    if (under_deck) {
        lo[CONES] = -INF;
    }
    if (hi[WATER] < INF) {
        lo[BANKS] = -INF;
        hi[BANKS] = INF;
    } else if (!abutment) {
        lo[WATER] = min(edge.x, hi[ROAD_CORE]);
        hi[WATER] = edge.y;
    }
    hi[BANKS] = max(hi[BANKS], lo[COVER]);
    if (lo[COVER] > hi[CONES]) {
        hi[CONES] = lo[COVER];
        cone_hi_road = false;
    }
    // As stamp_at: an empty bucket holds nothing, a bucket outside the interval pulls it to its nearest end,
    // and the cones of two ways ramp from the fill of the one to the cut of the other.
    for (var b = 0u; b < BUCKETS; b++) {
        if (lo[b] <= -INF && hi[b] >= INF) {
            continue;
        }
        var l = lo[b];
        var u = hi[b];
        if ((b == TROUGH || b == COVER) && (l > out.hi || u < out.lo)) {
            continue;
        }
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
        if (l > out.lo) {
            out.lo = l;
            out.fill = b == CONES && cone_lo_road && !ramp;
            out.fill_x = cone_lo_x;
        }
        if (u < out.hi) {
            out.hi = u;
            out.cut = b == CONES && cone_hi_road && !ramp;
            out.cut_x = cone_hi_x;
        }
    }
    return FarStamp(out, core_record, core_line, water, vec2f(min(lo[COVER], hi[DECK]), hi[DECK]));
}
