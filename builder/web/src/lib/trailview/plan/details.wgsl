// Storey 0's track and step details (`mesh/details.rs` of obc-ground-plan): each part built here from its rings,
// corners of the cell's storey mesh at their heights from corners.wgsl, as culvert_vertex builds a mouth; an instance
// is a part (details.ts). They shade with the corridors' fragment stage without PIECE, as the parts of roads/parts.ts:
// by their facets, faded out from DETAIL_M to DETAIL_END_M (track-looks.wgsl) as the bed's look paints them.
// #include piece.wgsl

// As mesh/details.rs: the kinds of a part, and a sleeper's second seat on the ring where its axis leaves the cell.
const D_RAIL: u32 = 0u;
const D_RACK: u32 = 1u;
const D_CABLE: u32 = 2u;
const D_SLEEPER: u32 = 3u;
const D_ROLLER: u32 = 4u;
const D_TREAD: u32 = 5u;
const D_CAP: u32 = 6u;
const D_LOG: u32 = 7u;
const D_AXIS: u32 = 1u;
// As roads/parts.ts: a sleeper's top over the bed, its depth and half-width along the track; a roller's half-width
// across and along and its top; a rail 0.125 m high, 1.2 times on standard gauge.
const D_SLEEPER_UP_M: f32 = 0.05;
const D_SLEEPER_DEPTH_M: f32 = 0.12;
const D_SLEEPER_ALONG_M: f32 = 0.13;
const D_ROLLER_M: vec3f = vec3f(0.16, 0.05, 0.11);
const D_RAIL_SCALE: f32 = 1.2;
// A tread's foot under its face.
const D_TREAD_SINK_M: f32 = 0.05;
// As roads/parts.ts: a log's radius, its axis over the way's face and its foot under it, its half-lengths; its turn at
// most (roads/tracks.ts).
const D_LOG_R_M: f32 = 0.08;
const D_LOG_AXIS_M: f32 = 0.03;
const D_LOG_SUNK_M: f32 = 0.03;
const D_LOG_TURN: f32 = 0.07;

// As roads/tracks.ts: a rail's section from right to left (metres across from its middle, up), a rack's and a cable's.
fn d_section(kind: u32, k: u32) -> vec2f {
    var rail = array(vec2f(-0.05, 0.0), vec2f(-0.05, 0.012), vec2f(-0.009, 0.028), vec2f(-0.009, 0.088), vec2f(-0.03, 0.097), vec2f(-0.031, 0.125),
        vec2f(0.031, 0.125), vec2f(0.03, 0.097), vec2f(0.009, 0.088), vec2f(0.009, 0.028), vec2f(0.05, 0.012), vec2f(0.05, 0.0));
    var rack = array(vec2f(-0.08, 0.0), vec2f(-0.08, 0.09), vec2f(0.08, 0.09), vec2f(0.08, 0.0));
    var cable = array(vec2f(-0.015, 0.06), vec2f(-0.015, 0.09), vec2f(0.015, 0.09), vec2f(0.015, 0.06));
    switch kind {
        case D_RACK: {
            return rack[k % 4u];
        }
        case D_CABLE: {
            return cable[k % 4u];
        }
        default: {
            return rail[k % 12u];
        }
    }
}

fn d_points(kind: u32) -> u32 {
    return select(4u, 12u, kind == D_RAIL);
}

// A ring: its corner's place and height, its offset, its line's unit tangent in LOD space (x east, y north) and its
// stroke metres.
struct DRing {
    v: PieceVertex,
    offset: f32,
    t: vec2f,
    along: f32,
}

fn d_ring(i: u32) -> DRing {
    let w = params.rings + 3u * i;
    let k = piece[w];
    let t = unpack2x16snorm(piece[w + 1u]);
    return DRing(corner_at(k), bitcast<f32>(piece[params.offsets + k]), normalize(vec2f(t.x, -t.y) + vec2f(1e-9, 0.0)), bitcast<f32>(piece[w + 2u]));
}

fn d_left(t: vec2f) -> vec2f {
    return vec2f(-t.y, t.x);
}

// A vertex of a part: its place in LOD space, its height, its look word and frame; `keep` false draws nothing.
struct DVertex {
    s: vec2f,
    h: f32,
    word: u32,
    across: f32,
    along: f32,
    keep: bool,
}

// A seat of a part on a span (a ring and the next, 16 bits each and a share in 12): its place, height, the span's unit
// direction and grade.
struct DSeat {
    s: vec2f,
    h: f32,
    dir: vec2f,
    grade: f32,
}

fn d_seat(word: u32) -> DSeat {
    let a = d_ring(word & 0xffffu);
    let b = d_ring((word & 0xffffu) + 1u);
    let f = f32((word >> 16u) & 0xfffu) / 4095.0;
    let d = b.v.s - a.v.s;
    let len = max(length(d), 1e-6);
    return DSeat(mix(a.v.s, b.v.s, f), mix(a.v.h, b.v.h, f), d / len, (b.v.h - a.v.h) / len);
}

// A box's corner c (bit 2 its left side, bit 1 its front end, bit 0 its top) from its plan's corners (2 · side + end):
// their places, foot and top heights and frames.
struct DBox {
    p: array<vec2f, 4>,
    a: array<f32, 4>,
    lo: array<f32, 4>,
    hi: array<f32, 4>,
    y: array<f32, 4>,
}

// A box's 6 faces of 4 corners, counter-clockwise from outside: top, right, left, front, back, bottom (roads/parts.ts).
fn d_box_corner(face: u32, k: u32) -> u32 {
    var faces = array(5u, 1u, 3u, 7u, 0u, 2u, 3u, 1u, 6u, 4u, 5u, 7u, 2u, 6u, 7u, 3u, 4u, 0u, 1u, 5u, 6u, 2u, 0u, 4u);
    return faces[4u * face + k];
}

fn d_box(b: DBox, index: u32, mats: vec2u, code: u32) -> DVertex {
    let face = index / 6u;
    let c = d_box_corner(face, array(0u, 1u, 2u, 0u, 2u, 3u)[index % 6u]);
    let at = c >> 1u;
    let up = (c & 1u) == 1u;
    let mat = select(mats.y, mats.x, face == 0u);
    return DVertex(b.p[at], select(b.lo[at], b.hi[at], up), look_word(mat, code, 0.0), b.a[at], b.y[at], true);
}

// A centred box: its middle, unit left and ahead, half-width across and along, its base height at the middle and
// its rise per metre across and along, and its foot and top over that.
fn d_centred(m: vec2f, left: vec2f, ahead: vec2f, half: vec2f, base: f32, rise: vec2f, foot: f32, top: f32) -> DBox {
    var b: DBox;
    for (var k = 0u; k < 4u; k++) {
        let a = select(-half.x, half.x, k >= 2u);
        let y = select(-half.y, half.y, (k & 1u) == 1u);
        let h = base + a * rise.x + y * rise.y;
        b.p[k] = m + left * a + ahead * y;
        b.a[k] = a;
        b.lo[k] = h + foot;
        b.hi[k] = h + top;
        b.y[k] = y;
    }
    return b;
}

// Vertex `index` of the part of item `item`.
fn d_vertex(index: u32, item: u32) -> DVertex {
    let w0 = piece[params.items + 2u * item];
    let w1 = piece[params.items + 2u * item + 1u];
    let kind = w0 >> 28u;
    let bits = (w0 >> 16u) & 0xfffu;
    var out = DVertex(vec2f(0.0), NONE, 0u, 0.0, 0.0, false);
    // A sleeper's flags lie in its second word's top bits; a tread's and a log's second word is no code.
    let narrow = (select(bits, w1 >> 28u, kind == D_SLEEPER) & 2u) != 0u;
    let code = select(select(w1 & 15u, 0u, kind == D_TREAD || kind == D_LOG), RAIL_CODE + u32(narrow), kind == D_SLEEPER);
    switch kind {
        case D_RAIL, D_RACK, D_CABLE: {
            // A span's quad between its rings: an edge of the section, swept.
            let q = index / 6u;
            let corner = array(0u, 1u, 1u, 0u, 1u, 0u)[index % 6u];
            let k = (q + array(0u, 0u, 1u, 0u, 1u, 1u)[index % 6u]) % d_points(kind);
            if (q >= d_points(kind)) {
                return out;
            }
            out = d_swept(kind, bits, d_ring((w0 & 0xffffu) + corner), k, code);
            out.word = look_word(select(select(LOOK_CABLE, LOOK_RACK, kind == D_RACK), select(LOOK_RAIL, LOOK_RAIL_HEAD, q == 5u), kind == D_RAIL), code, 0.0);
        }
        case D_CAP: {
            // The section's polygon at a run's end, in convex fans: its foot, web and head; facing back at a run's
            // start, on at its end.
            var rail = array(0u, 1u, 2u, 0u, 2u, 9u, 0u, 9u, 10u, 0u, 10u, 11u, 2u, 3u, 8u, 2u, 8u, 9u, 3u, 4u, 5u, 3u, 5u, 6u, 3u, 6u, 7u, 3u, 7u, 8u);
            let section = (bits >> 4u) & 3u;
            let tri = index / 3u;
            if (index >= select(6u, 30u, section == D_RAIL)) {
                return out;
            }
            var k = index % 3u;
            if ((bits & 8u) != 0u && k != 0u) {
                k = 3u - k;
            }
            let at = 3u * tri + k;
            out = d_swept(section, bits, d_ring(w0 & 0xffffu), select(array(0u, 1u, 2u, 0u, 2u, 3u)[at % 6u], rail[at], section == D_RAIL), code);
            out.word = look_word(select(select(LOOK_CABLE, LOOK_RACK, section == D_RACK), LOOK_RAIL, section == D_RAIL), code, 0.0);
        }
        case D_SLEEPER: {
            // Between its seats, which take their rails' heights; a second seat past the cell line lies on the axis to
            // the ring where it leaves the cell.
            let a = d_seat(w0);
            var b = DSeat(vec2f(0.0), 0.0, a.dir, a.grade);
            let gauge = 2.0 * rail_gauge(code);
            if (((w1 >> 28u) & D_AXIS) != 0u) {
                let x = d_ring(w1 & 0xffffu);
                let d = max(length(x.v.s - a.s), 1e-6);
                b = DSeat(a.s + (x.v.s - a.s) * gauge / d, a.h + (x.v.h - a.h) * gauge / d, a.dir, a.grade);
            } else {
                b = d_seat(w1 & 0x0fffffffu);
            }
            let left = d_left(a.dir);
            let across = dot(a.s - b.s, left);
            let rise = vec2f((a.h - b.h) / select(across, 1e-6, abs(across) < 1e-6), a.grade);
            let half = vec2f(sleeper_half(code), D_SLEEPER_ALONG_M);
            let bx = d_centred(0.5 * (a.s + b.s), left, a.dir, half, 0.5 * (a.h + b.h), rise, D_SLEEPER_UP_M - D_SLEEPER_DEPTH_M, D_SLEEPER_UP_M);
            let mat = select(LOOK_SLEEPER, LOOK_TIMBER, ((w1 >> 28u) & 4u) != 0u);
            out = d_box(bx, index, vec2u(mat), code);
        }
        case D_ROLLER: {
            let a = d_seat(w0);
            let bx = d_centred(a.s, d_left(a.dir), a.dir, D_ROLLER_M.xy, a.h, vec2f(0.0, a.grade), 0.0, D_ROLLER_M.z);
            out = d_box(bx, index, vec2u(LOOK_RAIL), code);
        }
        case D_TREAD: {
            // From under its face to its level (its rings' offsets), between its rings; open at an end on the cell line.
            let half = bitcast<f32>(w1);
            var b: DBox;
            for (var e = 0u; e < 2u; e++) {
                let r = d_ring((w0 & 0xffffu) + e);
                for (var side = 0u; side < 2u; side++) {
                    let a = select(-half, half, side == 1u);
                    let k = 2u * side + e;
                    b.p[k] = r.v.s + d_left(r.t) * a;
                    b.a[k] = a;
                    b.lo[k] = r.v.h - r.offset - D_TREAD_SINK_M;
                    b.hi[k] = r.v.h;
                    b.y[k] = r.along;
                }
            }
            let face = index / 6u;
            if ((face == 4u && (bits & 2u) != 0u) || (face == 3u && (bits & 4u) != 0u)) {
                return out;
            }
            out = d_box(b, index, vec2u(LOOK_TREAD, LOOK_RISER), code);
        }
        default: {
            out = d_log(w0, w1, index);
        }
    }
    return out;
}

// Point k of the section of a swept part at ring r, on its line: the ring's offset sinks it toward a crossing.
fn d_swept(kind: u32, bits: u32, r: DRing, k: u32, code: u32) -> DVertex {
    let p = d_section(kind, k);
    let scale = select(1.0, D_RAIL_SCALE, kind == D_RAIL && (bits & 2u) == 0u);
    let up = D_SLEEPER_UP_M + p.y * scale;
    // A rail's frame across is the track's: its middle lies a gauge to the side of the rail's.
    let middle = select(0.0, select(-1.0, 1.0, (bits & 1u) != 0u) * rail_gauge(code), kind == D_RAIL);
    return DVertex(r.v.s + d_left(r.t) * p.x, r.v.h + up, 0u, middle + p.x, r.along / SLEEPER_SPACING_M, true);
}

// A log across a trail's steps at its ring: round over the face, flat below, its ends closed; its length, turn and
// grade from its words (roads/parts.ts).
fn d_log(w0: u32, w1: u32, index: u32) -> DVertex {
    let r = d_ring(w0 & 0xffffu);
    var halves = array(0.3, 0.35, 0.45, 0.55, 0.7, 0.85);
    let half = halves[min((w0 >> 16u) & 7u, 5u)];
    let pose = unpack2x16snorm(w1);
    let turn = pose.x * D_LOG_TURN;
    let ahead = vec2f(r.t.x * cos(turn) - r.t.y * sin(turn), r.t.x * sin(turn) + r.t.y * cos(turn));
    let low = asin((D_LOG_AXIS_M + D_LOG_SUNK_M) / D_LOG_R_M);
    // 8 quads about its round (the last its flat foot), then 3 quads at each end: points of the arc at a side.
    let q = index / 6u;
    let c = array(0u, 1u, 2u, 0u, 2u, 3u)[index % 6u];
    var arc: u32;
    var side: f32;
    if (q < 8u) {
        arc = (q + select(0u, 1u, c >= 2u)) % 8u;
        side = select(-half, half, c == 1u || c == 2u);
    } else {
        let k = 2u * ((q - 8u) % 3u) + 1u;
        let right = q < 11u;
        var fan = array(0u, k, k + 1u, k + 2u);
        if (!right) {
            fan = array(k + 2u, k + 1u, k, 0u);
        }
        arc = fan[c];
        side = select(half, -half, right);
    }
    let angle = -low + f32(arc) * (3.14159265 + 2.0 * low) / 7.0;
    let y = cos(angle) * D_LOG_R_M;
    let up = D_LOG_AXIS_M + sin(angle) * D_LOG_R_M + 4.0 * pose.y * y;
    return DVertex(r.v.s + d_left(ahead) * side + ahead * y, r.v.h + up, look_word(LOOK_LOG, 0u, 0.0), side, y, true);
}

@vertex
fn detail_vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) item: u32) -> CorridorOut {
    let d = d_vertex(index, item);
    var out = piece_out(PieceVertex(units_of(d.s), d.s, d.h), d.word, CornerPlace(vec2f(d.across, 0.0), d.along), vec3f(0.0, 0.0, 1.0));
    if (!d.keep || !known(d.h)) {
        out.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    }
    return out;
}

@vertex
fn detail_caster(@builtin(vertex_index) index: u32, @builtin(instance_index) item: u32) -> @builtin(position) vec4f {
    let d = d_vertex(index, item);
    let p = frame.view_proj * vec4f(corner_point(PieceVertex(units_of(d.s), d.s, d.h)), 1.0);
    return select(vec4f(0.0, 0.0, 2.0, 1.0), p, d.keep && known(d.h));
}
