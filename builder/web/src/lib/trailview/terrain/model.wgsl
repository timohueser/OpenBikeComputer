// The one road model (prototype): GP3's candidates of each zoom-16 cell, evaluated at a point with the
// overlay's own decision, as `faces::records::find` of the crate does it line for line. The look calls it
// per fragment, so every boundary is the zero set of a distance that the fragment computes itself:
// no texel lies between the records and the screen. The words of `model.bin` (the crate's
// `debug model`) lie in a texture of MODEL_ROW words per row. features.wgsl includes it.
// #include drawn-height.wgsl
// #include model-words.wgsl

// The look reads the model in place of the mask's paint; MODEL_VIEW 1 draws the labels flat.
override MODEL: bool = false;
override MODEL_VIEW: u32 = 0u;
// A level of detail for the device: the look evaluates the roads past the masks' reach per fragment
// (develop draws none there). Off for a low tier.
override MODEL_FAR_ROADS: bool = true;
// Probe bits (scratch): 1 no wall look, 2 no cliff light from facets, 4 no cut-face look.
override MODEL_PROBE: u32 = 0u;

const MODEL_ROW: u32 = 4096u;
const M_BINS: i32 = 64;
const M_BIN: f32 = 4096.0;
const M_CAND: u32 = 12u;
const M_LABEL: u32 = 12u;
const M_LINE: u32 = 16u;
const M_SAMPLE: u32 = 14u;
const M_DISC: u32 = 11u;
const M_HEADER: u32 = 3u;
const M_NONE: u32 = 0xffffffffu;
// The candidates that a point keeps: those that cover it, and those near enough to change its label.
const M_COVER: u32 = 24u;
const M_NEAR: u32 = 8u;
// Candidate kinds, as records.rs.
const M_WAY: u32 = 0u;
const M_POINT: u32 = 1u;
const M_REGION: u32 = 2u;
const M_BAND: u32 = 3u;
const M_FILL: u32 = 4u;
const M_OUTLINE: u32 = 5u;
// Surfaces and ranks, as faces::Surface and plan.json's ranks.
const S_CARRIAGEWAY: u32 = 2u;
const S_CROSSING: u32 = 6u;
const S_RAILBED: u32 = 7u;
const RANK_SHOULDER_PAVED: u32 = 2u;
const M_FAR: f32 = 1e9;
// A round's chords leave its arc by at most this share of its radius (the overlay's 10° steps, with room).
const M_SAGITTA: f32 = 0.0055;
// As MARGIN_M of records.rs: a bin lists every candidate edge within this of its square, so every point
// finds the paved label this near. The paint's anti-aliased rim ends here: a far pixel stretched across a
// way smears it no farther.
const M_PAINT_REACH: f32 = 3.0;

fn mw(i: u32) -> u32 {
    return textureLoad(model_words, vec2i(i32(i % MODEL_ROW), i32(i / MODEL_ROW)), 0).r;
}

fn mf(i: u32) -> f32 {
    return bitcast<f32>(mw(i));
}

fn mi(i: u32) -> f32 {
    return f32(bitcast<i32>(mw(i)));
}

struct ModelPoint {
    // The cell block's first word; 0 where no record holds the point.
    block: u32,
    // The label (an index of the block's labels), M_NONE for terrain.
    label: u32,
    // The label across the nearest change of look, and the metres to it.
    alt: u32,
    d_label: f32,
    // Metres to the paved outline (negative inside) and to the outer edges of the band layers: paved
    // shoulder, shoulder, verge, slope.
    d_paved: f32,
    d_band: vec4f,
    // Metres to the nearest point's fans (a junction's corners, a joint's or a bend's rounds; negative
    // inside): where the paved outline is one of their arcs.
    d_corner: f32,
    // Whether the nearest round joins two arms that both wear lines: then its arc wears one too.
    corner_lined: bool,
    // The point in frame units from the cell's origin.
    q: vec2f,
    // The label's material line: metres across from its middle (signed), along, its half-width, and the
    // unit direction across it in frame axes (x east, y south).
    line: u32,
    across: f32,
    along: f32,
    half: f32,
    axis: vec2f,
    // Probe 32768 (a mouth of the lesser way's material): metres past the carriageways of the label's own
    // lines (a junction's hosts, a way's own line), and the nearest of its other lines (a junction's
    // other arms, a way's partners at its joints).
    own_out: f32,
    mouth: u32,
}

fn m_cross(a: vec2f, b: vec2f) -> f32 {
    return a.x * b.y - a.y * b.x;
}

// The change of the winding number from c to q across edge ab: +1 where q enters the edge's left.
fn m_crossing(a: vec2f, b: vec2f, c: vec2f, q: vec2f) -> i32 {
    let e = b - a;
    let sc = m_cross(e, c - a) > 0.0;
    let sq = m_cross(e, q - a) > 0.0;
    if (sc == sq) {
        return 0;
    }
    let s = q - c;
    if ((m_cross(s, a - c) > 0.0) == (m_cross(s, b - c) > 0.0)) {
        return 0;
    }
    return select(-1, 1, sq);
}

fn m_segment(a: vec2f, b: vec2f, q: vec2f) -> f32 {
    let u = b - a;
    let t = clamp(dot(q - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
    return length(q - a - t * u);
}

fn m_pair(w: u32) -> vec2f {
    return vec2f(f32(bitcast<i32>(w << 16u) >> 16u), f32(bitcast<i32>(w) >> 16u));
}

fn m_cand(block: u32, ci: u32) -> u32 {
    return block + mw(block + 4u) + ci * M_CAND;
}

fn m_label(block: u32, id: u32) -> u32 {
    return block + mw(block + 5u) + id * M_LABEL;
}

fn m_extras(block: u32) -> u32 {
    return block + mw(block + 8u);
}

// Whether q lies past the cut at word `at`: on its right, within its length.
fn m_past(at: u32, q: vec2f) -> bool {
    let a = vec2f(mi(at), mi(at + 1u));
    let u = vec2f(mi(at + 2u), mi(at + 3u)) - a;
    let v = q - a;
    let along = dot(u, v);
    return m_cross(u, v) < 0.0 && along >= 0.0 && along <= dot(u, u);
}

// The glob of a line at q: its sample nearest q among the bin's segments.
fn m_way_glob(block: u32, line: u32, q: vec2f, segs: u32, n: u32) -> u32 {
    let samples = block + mw(block + 7u);
    let first = mw(block + mw(block + 6u) + M_LINE * line + 2u);
    var best = M_FAR;
    var glob = M_NONE;
    for (var k = 0u; k < n; k++) {
        let s = mw(segs + k);
        if ((s & 0xffffu) != line) {
            continue;
        }
        for (var e = 0u; e < 2u; e++) {
            let w = samples + M_SAMPLE * (first + (s >> 16u) + e);
            let d = q - vec2f(mi(w), mi(w + 1u));
            if (dot(d, d) < best) {
                best = dot(d, d);
                glob = mw(w + 4u);
            }
        }
    }
    return glob;
}

fn m_glob(block: u32, ci: u32, q: vec2f, segs: u32, n: u32) -> u32 {
    let k = m_cand(block, ci);
    if (mw(k) == M_WAY) {
        return m_way_glob(block, mw(k + 4u), q, segs, n);
    }
    return mw(k + 2u);
}

// The overlay's decision at q over the candidates that cover it: the least key, then the least glob;
// an outline round's own label; a fan's label past a seam; a junction's region over its arms; a
// carriageway over a rail bed is a crossing.
fn m_decide(block: u32, cover: array<u32, M_COVER>, count: u32, q: vec2f, segs: u32, n: u32) -> u32 {
    var best = M_NONE;
    var rail = M_NONE;
    for (var i = 0u; i < count; i++) {
        let k = m_cand(block, cover[i]);
        if (mw(k) == M_REGION || mw(k) == M_OUTLINE) {
            continue;
        }
        best = min(best, mw(k + 1u));
        if ((mw(m_label(block, mw(k + 3u))) & 0xffu) == S_RAILBED) {
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
        if (mw(k) == M_REGION || mw(k) == M_OUTLINE || mw(k + 1u) != best) {
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
    var id = mw(k + 3u);
    var across = false;
    let extras = m_extras(block);
    for (var b = 0u; b < mw(k + 8u); b++) {
        let e = extras + mw(k + 7u) + 5u * b;
        if (m_past(e, q)) {
            id = mw(e + 4u);
            across = true;
            break;
        }
    }
    let l = m_label(block, id);
    if (((mw(l) >> 8u) & 0xffu) == 1u && !across) {
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

// What makes a label look different: its surface, its material's finish and lines. Junction faces and
// their arms of one finish look alike.
fn m_look(block: u32, id: u32) -> u32 {
    if (id == M_NONE) {
        return 0u;
    }
    let l = m_label(block, id);
    let surface = mw(l) & 0xffu;
    let line = mw(l + 2u);
    let finish = select(mw(block + mw(block + 6u) + M_LINE * line + 1u) & 0x3ffu, 0u, line == M_NONE);
    return surface | (finish << 8u) | (select(0u, (mw(l) >> 16u) & 0xffu, surface == 3u) << 20u);
}

// The cut of words at `at` reflected: q mirrored across its line.
fn m_mirror(at: u32, q: vec2f) -> vec2f {
    let a = vec2f(mi(at), mi(at + 1u));
    let u = normalize(vec2f(mi(at + 2u), mi(at + 3u)) - a);
    let v = q - a;
    return a + 2.0 * dot(v, u) * u - v;
}

fn m_cut_distance(at: u32, q: vec2f) -> f32 {
    return m_segment(vec2f(mi(at), mi(at + 1u)), vec2f(mi(at + 2u), mi(at + 3u)), q);
}

// The model at a world point (whole world units and a fraction), with the anti-aliasing reach `aa`
// in metres: how far a change of label still blends. A point farther than `stop` metres off the paved
// outline's polygon gets that distance only, no label: a disc moves the distance out, never in.
fn model_at(whole: vec2i, part: vec2f, aa: f32, stop: f32) -> ModelPoint {
    var out = ModelPoint(0u, M_NONE, M_NONE, M_FAR, M_FAR, vec4f(M_FAR), M_FAR, false, vec2f(0.0), M_NONE, 0.0, 0.0, 0.0, vec2f(1.0, 0.0), M_FAR, M_NONE);
    if (!MODEL || mw(0u) != 0x4f424d46u) {
        return out;
    }
    let cell = whole >> vec2u(14u);
    let c = cell - vec2i(i32(mw(1u)), i32(mw(2u)));
    let size = vec2i(i32(mw(3u)), i32(mw(4u)));
    if (any(c < vec2i(0)) || any(c >= size)) {
        return out;
    }
    let block = mw(5u + u32(c.y * size.x + c.x));
    if (block == 0u) {
        return out;
    }
    let q = vec2f((whole & vec2i(16383)) * 16) + part * 16.0;
    out.q = q;
    let bin = clamp(vec2i(floor(q / M_BIN)), vec2i(0), vec2i(M_BINS - 1));
    let head = block + mw(block + 9u) + M_HEADER * u32(bin.y * M_BINS + bin.x);
    let counts = mw(head + 1u);
    out.block = block;
    if ((counts >> 16u) == 0u) {
        return out;
    }
    let mpu = mf(block + 10u);
    let reach = aa / mpu;
    let rq = q - (vec2f(bin) * M_BIN + 0.5 * M_BIN);
    let reference = vec2f(0.5);
    var cover: array<u32, M_COVER>;
    var count = 0u;
    var near: array<u32, M_NEAR>;
    var near_d: array<f32, M_NEAR>;
    var near_look: array<u32, M_NEAR>;
    // Far from the paved area only the band layers matter (the look's shaped ground): the paved outline
    // and the bands come first in a bin, and the other candidates' edges are skipped.
    let skip_from = select(1.4 * aa + 1.0, M_FAR, aa <= 0.0);
    // Far off a pixel spans more than the bands' detail: a far point reads the paved outline, and the
    // rest only where it is paved or about to be.
    let far = aa > 0.3;
    var nears = 0u;
    var outline = M_NONE;
    var at = block + mw(head);
    let extras = m_extras(block);
    for (var item = 0u; item < (counts >> 16u); item++) {
        let w0 = mw(at);
        let ci = w0 & 0xffffu;
        var wind = i32((w0 >> 16u) & 0xffu) - 128;
        let edges = mw(at + 1u);
        let k = m_cand(block, ci);
        let kind = mw(k);
        // The outline and the bands come first in a bin: past them the paved distance is known.
        if (kind != M_OUTLINE && kind != M_BAND && out.d_paved > stop) {
            return out;
        }
        // Deep inside the paved area the bands do not show either.
        if ((kind != M_OUTLINE && kind != M_BAND && out.d_paved > skip_from) || (kind == M_BAND && (far || out.d_paved < -skip_from))) {
            at += 2u + 2u * edges;
            continue;
        }
        var d = M_FAR;
        for (var e = 0u; e < edges; e++) {
            let a = m_pair(mw(at + 2u + 2u * e));
            let b = m_pair(mw(at + 3u + 2u * e));
            wind += m_crossing(a, b, reference, rq);
            d = min(d, m_segment(a, b, rq));
        }
        at += 2u + 2u * edges;
        var sd = select(d, -d, wind != 0);
        // The outline's discs cut it only where no paving candidate covers the point: after the loop.
        if (kind == M_OUTLINE) {
            outline = k;
            out.d_paved = sd * mpu;
            continue;
        }
        for (var j = 0u; j < mw(k + 6u); j++) {
            let disc = extras + mw(k + 5u) + M_DISC * j;
            sd = max(sd, mf(disc + 2u) - length(q - vec2f(mi(disc), mi(disc + 1u))));
        }
        if (sd < 0.0 && count < M_COVER) {
            cover[count] = ci;
            count++;
        }
        let l = m_label(block, min(mw(k + 3u), mw(block + 2u) - 1u));
        if (kind == M_POINT && sd * mpu < out.d_corner) {
            out.d_corner = sd * mpu;
            // The round whose arc lies nearest: its two arms.
            var nearest = M_FAR;
            for (var j = 0u; j < mw(k + 6u); j++) {
                let disc = extras + mw(k + 5u) + M_DISC * j;
                let arc = abs(length(q - vec2f(mi(disc), mi(disc + 1u))) - mf(disc + 2u));
                if (arc < nearest) {
                    nearest = arc;
                    let lines = block + mw(block + 6u);
                    let a = mw(disc + 9u);
                    let b = mw(disc + 10u);
                    let lined_a = a != M_NONE && (mw(lines + M_LINE * a + 1u) & 256u) != 0u;
                    let lined_b = b == M_NONE || (mw(lines + M_LINE * b + 1u) & 256u) != 0u;
                    out.corner_lined = lined_a && lined_b;
                }
            }
        }
        if (kind == M_BAND) {
            let band = ((mw(l) >> 16u) & 0xffu) - RANK_SHOULDER_PAVED;
            if (band < 4u) {
                out.d_band[band] = min(out.d_band[band], sd * mpu);
            }
        }
        // Only a paved candidate's edge can change the look inside the paved area; the paved outline
        // itself blends by its own distance.
        if (abs(sd) < reach && nears < M_NEAR && (kind == M_WAY || kind == M_POINT || kind == M_FILL)) {
            near[nears] = ci | select(0u, 0x80000000u, sd < 0.0);
            near_d[nears] = abs(sd) * mpu;
            near_look[nears] = m_look(block, mw(k + 3u));
            nears++;
        }
    }
    // The outline polygon cuts each round's arc in chords; its disc moves the distance onto the arc, by no
    // more than the chords' sagitta. So where a disc lies over another way the road keeps its ground.
    if (outline != M_NONE) {
        let d_poly = out.d_paved;
        for (var j = 0u; j < mw(outline + 6u); j++) {
            let disc = extras + mw(outline + 5u) + M_DISC * j;
            let r = mf(disc + 2u);
            let arc = (r - length(q - vec2f(mi(disc), mi(disc + 1u)))) * mpu;
            out.d_paved = max(out.d_paved, min(arc, d_poly + (r * M_SAGITTA + 1.0) * mpu));
        }
    }
    let segs = at;
    let n = counts & 0xffffu;
    if ((MODEL_PROBE & 8u) != 0u) {
        out.d_label = f32(counts >> 16u);
        out.across = f32(at - block - mw(head));
        return out;
    }
    out.label = m_decide(block, cover, count, q, segs, n);
    let look = m_look(block, out.label);
    // The nearest change of look: across a candidate's edge, with that candidate's cover turned over,
    // or across a seam or a region's cut, at the mirrored point.
    for (var i = 0u; i < select(nears, 0u, (MODEL_PROBE & 16u) != 0u); i++) {
        // A candidate whose own look is the point's cannot change the look by its edge alone.
        if (near_look[i] == look) {
            continue;
        }
        var other: array<u32, M_COVER>;
        var m = 0u;
        let ci = near[i] & 0xffffu;
        for (var j = 0u; j < count; j++) {
            if (cover[j] != ci) {
                other[m] = cover[j];
                m++;
            }
        }
        if ((near[i] & 0x80000000u) == 0u && m < M_COVER) {
            other[m] = ci;
            m++;
        }
        let id = m_decide(block, other, m, q, segs, n);
        if (m_look(block, id) != look && near_d[i] < out.d_label) {
            out.d_label = near_d[i];
            out.alt = id;
        }
    }
    for (var i = 0u; i < select(count, 0u, (MODEL_PROBE & 16u) != 0u); i++) {
        let k = m_cand(block, cover[i]);
        let cuts = select(mw(k + 8u), mw(k + 10u), mw(k) == M_REGION);
        for (var j = 0u; j < cuts; j++) {
            for (var side = 0u; side < select(1u, 2u, mw(k) == M_REGION); side++) {
                var cut = extras + mw(k + 7u) + 5u * j;
                if (mw(k) == M_REGION) {
                    let part = extras + mw(k + 9u) + 11u * j;
                    if (mw(part + 1u + 5u * side) != 1u) {
                        continue;
                    }
                    cut = part + 2u + 5u * side;
                }
                let d = m_cut_distance(cut, q) * mpu;
                if (d < aa && d < out.d_label) {
                    // Across a fan's seam lies the beyond label; across a region's cut, its way's own.
                    var id = M_NONE;
                    if (mw(k) != M_REGION) {
                        id = mw(cut + 4u);
                    } else {
                        let way = mw(extras + mw(k + 9u) + 11u * j);
                        for (var c = 0u; c < count; c++) {
                            let wk = m_cand(block, cover[c]);
                            if (mw(wk) == M_WAY && mw(m_label(block, mw(wk + 3u)) + 1u) == way) {
                                id = mw(wk + 3u);
                            }
                        }
                    }
                    if (id != M_NONE && m_look(block, id) != look) {
                        out.d_label = d;
                        out.alt = id;
                    }
                }
            }
        }
    }
    // The metres across and along of the host lines (a junction's through pair, a way and the ways it
    // meets at a joint) of the label that the paint draws: the point's own where it is paved, else the
    // paved one across the nearest change; from their nearest segment in the bin: their middles run on
    // through it. So the anti-aliased rim outside the outline reads its road's half-width.
    let painted = select(select(out.label, out.alt, m_paved(block, out.alt)), out.label, m_paved(block, out.label));
    if (painted != M_NONE) {
        let l = m_label(block, painted);
        out.line = mw(l + 2u);
        let samples = block + mw(block + 7u);
        let lines = block + mw(block + 6u);
        var best = M_FAR;
        for (var k = 0u; k < n; k++) {
            let s = mw(segs + k);
            let line = s & 0xffffu;
            if (line != mw(l + 4u) && line != mw(l + 5u) && line != mw(l + 6u) && line != mw(l + 7u)) {
                continue;
            }
            let wa = samples + M_SAMPLE * (mw(lines + M_LINE * line + 2u) + (s >> 16u));
            let wb = wa + M_SAMPLE;
            let a = vec2f(mi(wa), mi(wa + 1u));
            let b = vec2f(mi(wb), mi(wb + 1u));
            let u = b - a;
            let t = clamp(dot(q - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
            let d = length(q - a - t * u);
            if (d < best) {
                best = d;
                let dir = normalize(u);
                out.axis = vec2f(-dir.y, dir.x);
                out.across = sign(m_cross(dir, q - a)) * d * mpu;
                out.along = mix(mf(wa + 3u), mf(wb + 3u), t);
                out.half = mix(mf(wa + 2u), mf(wb + 2u), t) * mpu;
            }
        }
        if (best == M_FAR) {
            out.across = M_FAR;
        }
        if ((MODEL_PROBE & 32768u) != 0u) {
            let junction = ((mw(l) >> 8u) & 0xffu) == 2u;
            var other = M_FAR;
            for (var k = 0u; k < n; k++) {
                let s = mw(segs + k);
                let line = s & 0xffffu;
                let own = select(line == mw(l + 2u), line == mw(l + 4u) || line == mw(l + 5u) || line == mw(l + 6u) || line == mw(l + 7u), junction);
                let others = line == mw(l + 8u) || line == mw(l + 9u) || line == mw(l + 10u) || line == mw(l + 11u);
                if (!own && !others) {
                    continue;
                }
                let wa = samples + M_SAMPLE * (mw(lines + M_LINE * line + 2u) + (s >> 16u));
                let wb = wa + M_SAMPLE;
                let a = vec2f(mi(wa), mi(wa + 1u));
                let u = vec2f(mi(wb), mi(wb + 1u)) - a;
                let t = clamp(dot(q - a, u) / max(dot(u, u), 1e-9), 0.0, 1.0);
                let d = length(q - a - t * u);
                if (own) {
                    out.own_out = min(out.own_out, (d - mix(mf(wa + 2u), mf(wb + 2u), t)) * mpu);
                }
                if (others && d < other) {
                    other = d;
                    out.mouth = line;
                }
            }
        }
    }
    return out;
}

// Whether a label is paved: a carriageway, a crossing or a rail bed, which the paint draws.
fn m_paved(block: u32, id: u32) -> bool {
    let surface = select(mw(m_label(block, id)) & 0xffu, 0u, id == M_NONE);
    return surface == S_CARRIAGEWAY || surface == S_CROSSING || surface == S_RAILBED;
}

// The finish code (SURFACES of bins.ts) and the flags of a label's material line.
fn model_finish(block: u32, id: u32) -> u32 {
    if (id == M_NONE) {
        return 0u;
    }
    let line = mw(m_label(block, id) + 2u);
    if (line == M_NONE) {
        return 0u;
    }
    return mw(block + mw(block + 6u) + M_LINE * line + 1u);
}

fn model_surface(block: u32, id: u32) -> u32 {
    if (id == M_NONE) {
        return 0u;
    }
    return mw(m_label(block, id)) & 0xffu;
}

fn model_owner_kind(block: u32, id: u32) -> u32 {
    if (id == M_NONE) {
        return 0u;
    }
    return (mw(m_label(block, id)) >> 8u) & 0xffu;
}

// The model at a point of LOD space.
fn model_at_s(s: vec2f, aa: f32, stop: f32) -> ModelPoint {
    let u = units_of(s);
    let whole = vec2i(floor(u));
    return model_at(surface.anchor + whole, u - floor(u), aa, stop);
}

// The look's cache of a page texel (probe 2097152): the paved distance and the host's metres across, ±64 m in
// steps of 1/512 m about 32768; the metres along modulo CACHE_ALONG_M in 11 bits, the shoulder's width past the
// paved outline in 4 (CACHE_SHOULDER_STEP_M) and whether it is paved; then the painted label's finish code
// (4 bits, one asphalt code), its lines (1), whether a paved label is near (1), the edge line's share (4) and
// the host's half-width in quarter metres (6). A paved label is the point's own, else the one across the
// nearest change within two texels.
const CACHE_ALONG_M: f32 = 72.0;
const CACHE_SHOULDER_STEP_M: f32 = 0.05;
const CACHE_RAIL_CODE: u32 = 9u;

fn cache_metres(m: f32) -> u32 {
    return u32(clamp(round(m * 512.0) + 32768.0, 0.0, 65535.0));
}

// The shoulder of a point outside the paved area: the metres from the paved outline to the outer edge of both
// shoulder layers (the band layers of the point at `bands`, with no reach, so they hold every layer), and whether
// the paved shoulder layer fills it. A way whose shoulder is all paved has no shoulder layer of its own, but a
// round or a junction's fan beside it has both.
fn model_shoulder_of(d_paved: f32, bands: vec4f) -> vec2f {
    let outer = min(bands.x, bands.y);
    let width = clamp(d_paved - outer, 0.0, 15.0 * CACHE_SHOULDER_STEP_M);
    return vec2f(width, select(0.0, 1.0, width > 0.0 && bands.x <= outer + 0.01));
}

fn model_cache_texel(p: vec2i, texel_m: f32, bands: vec4f) -> vec4u {
    // The paved label reaches past the outline as far as a fragment's outside test looks: up to two
    // pixels of at most a texel each, and the noise of an unpaved edge; and the paint's whole reach.
    let cm = model_at(p, vec2f(0.0), max(3.0 * texel_m + 0.3, M_PAINT_REACH), M_FAR);
    let id = select(select(M_NONE, cm.alt, m_paved(cm.block, cm.alt)), cm.label, m_paved(cm.block, cm.label));
    let flags = model_finish(cm.block, id);
    var code = select(flags & 0xffu, 1u, (flags & 0xffu) == 2u);
    if (model_surface(cm.block, id) == S_RAILBED && code < CACHE_RAIL_CODE) {
        code = CACHE_RAIL_CODE;
    }
    let near = select(0u, 1u, id != M_NONE);
    let marked = select(0u, 1u, (flags & 256u) != 0u);
    // The edge line's share, as model_surface_look draws it, carried on past the outline unclamped: a
    // texel outside holds the share of the edge beside it, so four texels across an edge filter to it.
    let on_round = 1.0 - smoothstep(0.03, 0.12, cm.d_corner - cm.d_paved);
    let on_edge = 1.0 - smoothstep(0.05, 0.25, abs(-cm.d_paved - (cm.half - abs(cm.across))));
    let edged = max(on_round * select(0.0, 1.0, cm.corner_lined), on_edge * (1.0 - on_round));
    let along = cm.along - CACHE_ALONG_M * floor(cm.along / CACHE_ALONG_M);
    let w = min(code, 15u) | (marked << 4u) | (near << 5u) | (u32(round(edged * 15.0)) << 6u) | (min(u32(round(cm.half * 4.0)), 63u) << 10u);
    let shoulder = model_shoulder_of(cm.d_paved, bands);
    let z = (u32(round(along / CACHE_ALONG_M * 2048.0)) & 2047u) | (u32(round(shoulder.x / CACHE_SHOULDER_STEP_M)) << 11u) | (u32(shoulder.y) << 15u);
    return vec4u(cache_metres(cm.d_paved), cache_metres(cm.across), z, w);
}
