// The one road model at a texel of a page: the crate's `faces::records::find` on the records of the
// cell that holds the point, with the distances that the stamp writes into a page's textures. Every
// side test is exact on integers, with find's (ε, ε²) rule at a tie, so the GPU and the CPU give a
// point the same label. The records lie in model_words: a table of the cells about the eye, then the
// blocks (model.ts).
// #include heights.wgsl
// #include mouth.wgsl

// The stamp shapes and paints the ground from the records (`ground=plan`).
override MODEL: bool = false;

// The level of the nodes that are cells, CELL_LEVEL of model.ts: the records shape the pages of it
// and finer. A coarser page is not stamped again when a block arrives or leaves.
const M_CELL_LEVEL: u32 = 16u;
// The table's first word, as TABLE_MAGIC of model.ts.
const M_TABLE: u32 = 0x4f42540au;
// Candidate kinds and surfaces, as records.rs and faces::Surface.
const M_WAY: u32 = 0u;
const M_POINT: u32 = 1u;
const M_REGION: u32 = 2u;
const M_OUTLINE: u32 = 5u;
const S_CARRIAGEWAY: u32 = 2u;
const S_CROSSING: u32 = 6u;
const S_RAILBED: u32 = 7u;
const M_BIN_UNITS: i32 = 4096;
// The candidates that cover a point at most, and those whose edge lies near enough to change its paint.
const M_COVER: u32 = 24u;
const M_NEAR: u32 = 8u;
// A round's chords leave its arc by at most this share of its radius (the overlay's 10° steps, with room).
const M_SAGITTA: f32 = 0.0055;

struct ModelPoint {
    // The block of the cell that holds the point; 0 where no records hold it.
    block: u32,
    // The point in frame units from the cell's origin.
    q: vec2i,
    // The label (an index of the block's labels), M_NONE for terrain.
    label: u32,
    // The paved label that the paint draws: the point's own where it is paved, else the paved one across
    // the nearest change within the reach; M_NONE for none.
    painted: u32,
    // Metres to the paved outline and the rail beds (negative inside) and to the core's outline: those
    // and the ways' band layers but the slope.
    d_paved: f32,
    core: f32,
    // Metres across from the middle of the painted label's host line (signed by side), along it, and its
    // half-width; across is M_FAR where no host line lies in the bin.
    across: f32,
    along: f32,
    half: f32,
    // The first word of the bin's height lists, for height_at.
    heights: u32,
    // The stop arm line whose mouth holds the point in a junction's face (m_mouth), M_NONE for none.
    mouth: u32,
}

// The product of two i32 as a signed 64-bit number: (low word, high word).
fn m_mul(a: i32, b: i32) -> vec2u {
    let x = u32(abs(a));
    let y = u32(abs(b));
    let x0 = x & 0xffffu;
    let x1 = x >> 16u;
    let y0 = y & 0xffffu;
    let y1 = y >> 16u;
    let p00 = x0 * y0;
    let p01 = x0 * y1;
    let p10 = x1 * y0;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    var lo = (p00 & 0xffffu) | (mid << 16u);
    var hi = x1 * y1 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u);
    if ((a < 0) != (b < 0)) {
        lo = ~lo + 1u;
        hi = ~hi + select(0u, 1u, lo == 0u);
    }
    return vec2u(lo, hi);
}

// The sign of cross(a, b), exact for components of up to 31 bits.
fn m_orient(a: vec2i, b: vec2i) -> i32 {
    let l = m_mul(a.x, b.y);
    let r = m_mul(a.y, b.x);
    if (l.y != r.y) {
        return select(-1, 1, bitcast<i32>(l.y) > bitcast<i32>(r.y));
    }
    if (l.x != r.x) {
        return select(-1, 1, l.x > r.x);
    }
    return 0;
}

// Whether point p moved by (ε, ε²) lies left of the line through a along e, as records::left.
fn m_left(e: vec2i, a: vec2i, p: vec2i) -> bool {
    let s = m_orient(e, p - a);
    if (s != 0) {
        return s > 0;
    }
    return e.y < 0 || (e.y == 0 && e.x > 0);
}

// The change of the winding number from c to q, both moved by (ε, ε²), across edge ab, as
// records::crossing. In doubled units.
fn m_crossing(a: vec2i, b: vec2i, c: vec2i, q: vec2i) -> i32 {
    // Segments whose boxes lie a unit or more apart do not cross, moved by (ε, ε²) or not.
    if (any(max(a, b) < min(c, q)) || any(min(a, b) > max(c, q))) {
        return 0;
    }
    let e = b - a;
    let sq = m_left(e, a, q);
    if (m_left(e, a, c) == sq) {
        return 0;
    }
    let d = q - c;
    let sa = m_orient(d, a - c);
    let sb = m_orient(d, b - c);
    let tie = d.y > 0 || (d.y == 0 && d.x < 0);
    if (select(sa > 0, tie, sa == 0) == select(sb > 0, tie, sb == 0)) {
        return 0;
    }
    return select(-1, 1, sq);
}

fn m_segment(a: vec2f, b: vec2f, q: vec2f) -> f32 {
    let u = b - a;
    let t = clamp(dot(q - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
    return length(q - a - t * u);
}

// Whether q lies past the cut at word `at`: on its right, within its length, as the overlay's `past`.
fn m_past(at: u32, q: vec2i) -> bool {
    let a = m_vertex(at);
    let u = m_vertex(at + 2u) - a;
    let v = q - a;
    let along = dot(vec2f(u), vec2f(v));
    return m_orient(u, v) < 0 && along >= 0.0 && along <= dot(vec2f(u), vec2f(u));
}

fn m_surface(block: u32, id: u32) -> u32 {
    return select(mw(m_label(block, id)) & 0xffu, 0u, id == M_NONE);
}

// Whether a label is paved: a carriageway, a crossing or a rail bed, which the paint draws.
fn m_paved(block: u32, id: u32) -> bool {
    let s = m_surface(block, id);
    return s == S_CARRIAGEWAY || s == S_CROSSING || s == S_RAILBED;
}

// The finish code (SURFACES of bins.ts) and the flags of a label's material line, 0 for none.
fn model_finish(block: u32, id: u32) -> u32 {
    if (id == M_NONE) {
        return 0u;
    }
    let line = mw(m_label(block, id) + 2u);
    return select(mw(block + mw(block + 6u) + M_LINE * line + 1u), 0u, line == M_NONE);
}

// The glob order of a line at q: of its sample nearest q among the bin's segments, as records::way_glob.
fn m_way_glob(block: u32, line: u32, q: vec2i, segs: u32, n: u32) -> u32 {
    let samples = block + mw(block + 7u);
    let first = mw(block + mw(block + 6u) + M_LINE * line + 2u);
    var best = M_FAR * M_FAR;
    var glob = M_NONE;
    for (var k = 0u; k < n; k++) {
        let s = mw(segs + k);
        if ((s & 0xffffu) != line) {
            continue;
        }
        for (var e = 0u; e < 2u; e++) {
            let w = samples + M_SAMPLE * (first + (s >> 16u) + e);
            let d = vec2f(q - m_vertex(w));
            if (dot(d, d) < best) {
                best = dot(d, d);
                glob = mw(w + 4u);
            }
        }
    }
    return glob;
}

fn m_glob(block: u32, ci: u32, q: vec2i, segs: u32, n: u32) -> u32 {
    let k = m_cand(block, ci);
    if (mw(k) == M_WAY) {
        return m_way_glob(block, mw(k + 4u), q, segs, n);
    }
    return mw(k + 2u);
}

// Whether a candidate's label is of line water: its material line has M_WATER.
fn m_of_water(block: u32, k: u32) -> bool {
    let line = mw(m_label(block, mw(k + 3u)) + 2u);
    return (m_line_finish(block, line) & M_WATER) != 0u;
}

// The overlay's decision at q over the candidates that cover it, as records::decide: the least key,
// then the least glob; a fan's label past a seam; a junction's region over its arms; a carriageway
// over a rail bed is a crossing. Where a zone covers q, line water does not count.
fn m_decide(block: u32, cover: array<u32, M_COVER>, count: u32, q: vec2i, segs: u32, n: u32) -> u32 {
    var zone = false;
    for (var i = 0u; i < count; i++) {
        let k = m_cand(block, cover[i]);
        zone = zone || (mw(k) != M_REGION && mw(k) != M_OUTLINE && m_surface(block, mw(k + 3u)) == M_SURFACE_ZONE);
    }
    var best = M_NONE;
    var rail = M_NONE;
    for (var i = 0u; i < count; i++) {
        let k = m_cand(block, cover[i]);
        if (mw(k) == M_REGION || mw(k) == M_OUTLINE || (zone && m_of_water(block, k))) {
            continue;
        }
        best = min(best, mw(k + 1u));
        if (rail == M_NONE && m_surface(block, mw(k + 3u)) == S_RAILBED) {
            rail = cover[i];
        }
    }
    if (best == M_NONE) {
        return M_NONE;
    }
    var win = M_NONE;
    var win_glob = M_NONE;
    for (var i = 0u; i < count; i++) {
        let k = m_cand(block, cover[i]);
        if (mw(k) == M_REGION || mw(k) == M_OUTLINE || mw(k + 1u) != best || (zone && m_of_water(block, k))) {
            continue;
        }
        let g = m_glob(block, cover[i], q, segs, n);
        if (win == M_NONE || g < win_glob) {
            win = cover[i];
            win_glob = g;
        }
    }
    let k = m_cand(block, win);
    if (mw(k) == M_FILL) {
        return mw(k + 3u);
    }
    let extras = m_extras(block);
    var id = mw(k + 3u);
    var across = false;
    for (var b = 0u; b < mw(k + 8u); b++) {
        let e = extras + mw(k + 7u) + 5u * b;
        if (m_past(e, q)) {
            id = mw(e + 4u);
            across = true;
            break;
        }
    }
    let l = m_label(block, id);
    // A zone keeps itself in a region of its owner way, as records::decide.
    if (((mw(l) >> 8u) & 0xffu) == 1u && !across && (mw(l) & 0xffu) != M_SURFACE_ZONE) {
        let w = mw(l + 1u);
        var region = M_NONE;
        var region_key = vec2u(M_NONE);
        for (var i = 0u; i < count; i++) {
            let r = m_cand(block, cover[i]);
            if (mw(r) != M_REGION) {
                continue;
            }
            var mine = 0u;
            var clear = true;
            for (var p = 0u; p < mw(r + 10u); p++) {
                let part = extras + mw(r + 9u) + 11u * p;
                if (mw(part) != w) {
                    continue;
                }
                mine++;
                if ((mw(part + 1u) == 1u && m_past(part + 2u, q)) || (mw(part + 6u) == 1u && m_past(part + 7u, q))) {
                    clear = false;
                }
            }
            let key = vec2u(mw(r + 1u), mw(r + 2u));
            if (mine > 0u && clear && (key.x < region_key.x || (key.x == region_key.x && key.y < region_key.y))) {
                region = cover[i];
                region_key = key;
            }
        }
        if (region != M_NONE) {
            id = mw(m_cand(block, region) + 3u);
        }
    }
    let l2 = m_label(block, id);
    if ((mw(l2) & 0xffu) == S_CARRIAGEWAY && rail != M_NONE) {
        let rail_line = mw(m_label(block, mw(m_cand(block, rail) + 3u)) + 2u);
        let want = vec4u(S_CROSSING | (mw(l2) & ~0xffu), mw(l2 + 1u), mw(l2 + 2u), rail_line);
        for (var x = 0u; x < mw(block + 2u); x++) {
            let lx = m_label(block, x);
            if (all(vec4u(mw(lx), mw(lx + 1u), mw(lx + 2u), mw(lx + 3u)) == want)) {
                id = x;
                break;
            }
        }
    }
    return id;
}

// The block of the cell that holds world point p (units of 2^30 per world side), 0 for none.
fn model_block(p: vec2i) -> u32 {
    if (mw(0u) != M_TABLE) {
        return 0u;
    }
    let c = (p >> vec2u(14u)) - vec2i(i32(mw(1u)), i32(mw(2u)));
    let size = vec2i(i32(mw(3u)), i32(mw(4u)));
    if (any(c < vec2i(0)) || any(c >= size)) {
        return 0u;
    }
    return mw(5u + u32(c.y * size.x + c.x));
}

// The model at world point p, a texel centre: its label, its distances and the paved label within `reach`
// metres for the paint.
fn model_at(p: vec2i, reach: f32) -> ModelPoint {
    let block = model_block(p);
    if (block == 0u) {
        return ModelPoint(0u, vec2i(0), M_NONE, M_NONE, M_FAR, M_FAR, M_FAR, 0.0, 0.0, 0u, M_NONE);
    }
    return model_point(block, (p & vec2i(16383)) * 16, reach);
}

// The model of a block at point q (frame units from the cell's origin).
fn model_point(block: u32, q: vec2i, reach: f32) -> ModelPoint {
    var out = ModelPoint(0u, vec2i(0), M_NONE, M_NONE, M_FAR, M_FAR, M_FAR, 0.0, 0.0, 0u, M_NONE);
    out.block = block;
    out.q = q;
    let head = m_bin_head(block, vec2f(q));
    if (head == 0u) {
        return out;
    }
    let mpu = h_scale(block, vec2f(q));
    let units = reach / mpu;
    let qf = vec2f(q);
    let bin = min(q / M_BIN_UNITS, vec2i(M_BINS - 1));
    let c = 2 * (bin * M_BIN_UNITS + M_BIN_UNITS / 2) + 1;
    let q2 = 2 * q;
    let extras = m_extras(block);
    var cover: array<u32, M_COVER>;
    var count = 0u;
    var near: array<u32, M_NEAR>;
    var near_d: array<f32, M_NEAR>;
    var nears = 0u;
    var outline = M_NONE;
    var rail = M_FAR;
    var at = block + mw(head);
    for (var item = 0u; item < (mw(head + 1u) >> 16u); item++) {
        let ci = mw(at) & 0xffffu;
        var wind = i32((mw(at) >> 16u) & 0xffu) - 128;
        let k = m_cand(block, ci);
        let kind = mw(k);
        var d = M_FAR;
        var run = at + 2u;
        for (var r = 0u; r < mw(at + 1u); r++) {
            let n = mw(run);
            for (var e = 0u; e + 1u < n; e++) {
                let a = m_vertex(run + 1u + 2u * e);
                let b = m_vertex(run + 3u + 2u * e);
                wind += m_crossing(2 * a, 2 * b, c, q2);
                d = min(d, m_segment(vec2f(a), vec2f(b), qf));
            }
            run += 1u + 2u * n;
        }
        at = run;
        var sd = select(d, -d, wind != 0);
        if (kind == M_OUTLINE) {
            outline = k;
            out.d_paved = sd * mpu;
            continue;
        }
        // A fan or round leaves out the disc of its circle, as `find`.
        var in_disc = false;
        for (var j = 0u; j < mw(k + 6u); j++) {
            let disc = extras + mw(k + 5u) + M_DISC * j;
            let r = mf(disc + 2u) - length(qf - vec2f(m_vertex(disc)));
            in_disc = in_disc || r > 0.0;
            sd = max(sd, r);
        }
        if (wind != 0 && !in_disc && count < M_COVER) {
            cover[count] = ci;
            count++;
        }
        let label = mw(k + 3u);
        // The core is a way's: line water is no way, so its face and its bank stay out of the mask.
        let surface = m_surface(block, label);
        if (kind == M_BAND && surface != M_SURFACE_SLOPE && surface != M_SURFACE_WATER && surface != M_SURFACE_BANK) {
            out.core = min(out.core, sd * mpu);
        }
        // The paved outline holds the carriageways; the paint draws the rail beds too.
        if (kind != M_BAND && kind != M_REGION && m_surface(block, label) == S_RAILBED) {
            rail = min(rail, sd * mpu);
        }
        // A paved candidate's edge near the point can change what the paint draws there.
        if ((wind == 0 || in_disc) && sd < units && nears < M_NEAR && kind != M_BAND && kind != M_REGION && m_paved(block, label)) {
            near[nears] = ci;
            near_d[nears] = sd;
            nears++;
        }
    }
    // The outline polygon cuts each round's arc in chords; its disc moves the distance onto the arc, by no
    // more than the chords' sagitta.
    if (outline != M_NONE) {
        let poly = out.d_paved;
        for (var j = 0u; j < mw(outline + 6u); j++) {
            let disc = extras + mw(outline + 5u) + M_DISC * j;
            let r = mf(disc + 2u);
            let arc = (r - length(qf - vec2f(m_vertex(disc)))) * mpu;
            out.d_paved = max(out.d_paved, min(arc, poly + (r * M_SAGITTA + 1.0) * mpu));
        }
    }
    out.d_paved = min(out.d_paved, rail);
    out.core = min(out.core, out.d_paved);
    let segs = at;
    let n = mw(head + 1u) & 0xffffu;
    out.heights = segs + n;
    out.label = m_decide(block, cover, count, q, segs, n);
    out.painted = select(M_NONE, out.label, m_paved(block, out.label));
    // Outside the paved labels, the nearest paved candidate that the point would join across its edge.
    if (out.painted == M_NONE) {
        var nearest = M_FAR;
        for (var i = 0u; i < nears; i++) {
            if (near_d[i] >= nearest) {
                continue;
            }
            var other = cover;
            var m = count;
            if (m < M_COVER) {
                other[m] = near[i];
                m++;
            }
            let id = m_decide(block, other, m, q, segs, n);
            if (m_paved(block, id)) {
                nearest = near_d[i];
                out.painted = id;
            }
        }
    }
    // The metres across and along of the painted label's host lines: a junction's through pair (from its record), a
    // way's own line (from its nearest segment in the bin), a stop arm's own line in its mouth. Their middles run on
    // through it. A texel takes the look at its centre.
    out.across = M_FAR;
    let jr = m_junction(block, out.painted);
    let mouth = m_mouth(block, jr, qf, mpu);
    out.mouth = mouth.line;
    var frame = mouth.place;
    if (mouth.line == M_NONE && jr != M_NONE) {
        frame = m_junction_frame(block, jr, qf, m_arms_with(jr, M_ARM_HOST), M_NONE, mpu).place;
    }
    if (jr != M_NONE) {
        out.across = frame.x;
        out.along = frame.y;
        out.half = frame.z;
    } else if (out.painted != M_NONE) {
        let line = mw(m_label(block, out.painted) + 2u);
        let samples = block + mw(block + 7u);
        let lines = block + mw(block + 6u);
        var best = M_FAR;
        for (var k = 0u; k < n; k++) {
            let s = mw(segs + k);
            if ((s & 0xffffu) != line) {
                continue;
            }
            let wa = samples + M_SAMPLE * (mw(lines + M_LINE * line + 2u) + (s >> 16u));
            let wb = wa + M_SAMPLE;
            let a = vec2f(m_vertex(wa));
            let u = vec2f(m_vertex(wb)) - a;
            let t = clamp(dot(qf - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
            let d = length(qf - a - t * u);
            if (d < best) {
                best = d;
                out.across = sign(m_cross(u, qf - a)) * d * mpu;
                out.along = mix(mf(wa + 3u), mf(wb + 3u), t);
                out.half = mix(mf(wa + 2u), mf(wb + 2u), t) * mpu;
            }
        }
    }
    return out;
}
