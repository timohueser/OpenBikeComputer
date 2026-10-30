// The bridges of a node: a deck along each bridge line at its solved heights, with parapets for a road,
// a railing for a path, cables for a bridge that hangs and walls and a roof for a covered one, and a pier
// at each `bridge-pier` point of the bake, down into the drawn surface.
import { EXTENT, unitScale, type Feature } from '../stream/features';
import { BRIDGE, isPath, metresPerUnit, PATH, RAIL_CODE, ROAD, walkOf, type Line, type Node } from '../terrain/bins';
import type { Owns } from './node';
import { deckSleepers, GROUND, MeshBuilder, mix, PAINT, posts, rgb, type Corner, type Station } from './mesh';
import { stationsOf as smoothStations, type Station as RoadStation } from '../roads/stations';
import { CF, CROWN, KERB_RISE_M, KERB_TOP_M, lookOf } from '../roads/section';
import { corridorColour } from '../roads/corridor';
import { JMAT } from '../roads/junctions';
import { LEFT, RIGHT } from '../terrain/town';

/** The deck's road lies this far above the abutment's ground, which holds the road's height under it. */
const LIFT_M = 0.03;
/** A road's parapet: width and height above the road. */
const PARAPET: Corner = [0.35, 1.0];
/** A railing: its height, the side of its rail and its posts, and the posts' spacing; a cable and its hangers are thinner. */
const RAIL_M = 1.05, BAR_M = 0.08, CABLE_M = 0.04, POSTS_M = 2;
/** A covered bridge: the top of its wall, its eaves, the rise of its roof per metre of half-width, and the roof's overhang. */
const COVER_WALL_M = 1.0, EAVES_M = 2.6, PITCH = 0.5, OVERHANG_M = 0.4;
/** A pier: its thickness along the bridge for a road and a path, the share of the deck's width that it takes, and how deep it stands in the ground. */
const PIER_M = { road: 1.6, path: 0.5 }, PIER_WIDTH = 0.6, PIER_SINK_M = 1.5;
/** An abutment's thickness along the bridge. */
const ABUTMENT_M = 1.0;
/** A deck that leaves a road or a deck at a T starts at its edge, if its two ends together lose at most this share of its length. */
const TEE_SHARE = 0.5;
/** A deck that runs on another deck's road within MEET_M of its height joins it there. */
const MEET_M = 1.0;
/** A pier stands on the nearest bridge line within this many metres. */
const PIER_REACH_M = 3;

const CONCRETE = rgb('#9a978e'), CAP = rgb('#b3b0a7'), SOFFIT = rgb('#76746d'), WOOD = rgb('#7d5c3d'), WOOD_DARK = rgb('#5d4430');
const WALKWAY = rgb('#a8a59c');
const STEEL = rgb('#4a4d50'), ROOF = rgb('#7a3f2c'), ROOF_UNDER = rgb('#3e2a1e');
const pierColour = (road: boolean) => (road ? CONCRETE : rgb('#8d8a82'));

/** The half-width of a deck to the outer face of a road's parapets; a step past a path's railing still stands on it. */
export const deckHalf = (line: Line) => line.h + PARAPET[0];

/** A paved road or path, by its surface, as `paved` of roads.wgsl: its deck is painted, not planked. */
const isPaved = (line: Line) => line.code <= 2 || line.code === 7;
/** A railway's bridge: no parapets, which would stand between the decks of two tracks side by side, but a steel railing. */
const isTrack = (line: Line) => line.code >= RAIL_CODE;

/** The bridges of a feature tile's `list` and `lines` whose pieces a square `owns`. */
export function bridges(b: MeshBuilder, list: Feature[], tile: Node, all: Line[], owns: Owns): void {
    const lines = all.filter(line => line.kind === BRIDGE);
    if (!lines.length) return;
    // Ends that two bridge lines share are joints: no cap there.
    const ends = new Map<string, number>();
    const endKey = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    for (const line of lines) for (const v of [0, line.z.length - 1]) ends.set(endKey(line, v), (ends.get(endKey(line, v)) ?? 0) + 1);
    // The inner vertices of the roads on the ground and of the decks, with the half-width that a deck leaving them starts past.
    const through = new Map<string, number>();
    for (const line of all) {
        const half = line.kind === BRIDGE ? deckHalf(line) : line.kind === ROAD || line.kind === PATH ? line.h : 0;
        for (let v = 1; half && v + 1 < line.z.length; v++) through.set(endKey(line, v), Math.max(through.get(endKey(line, v)) ?? 0, half));
    }
    for (const line of lines) {
        const n = line.z.length, path = isPath(line), t = line.deck ?? 0.6, h = line.h;
        // Metres along the line at each vertex; the deck runs from `from` to `to`, short of an end that leaves a road at a T.
        const along = [0];
        for (let v = 1; v < n; v++) along.push(along[v - 1] + Math.hypot(line.xy[2 * v] - line.xy[2 * v - 2], line.xy[2 * v + 1] - line.xy[2 * v - 1]) * metresPerUnit(line.xy[2 * v + 1]));
        const total = along[n - 1], cuts = [through.get(endKey(line, 0)) ?? 0, through.get(endKey(line, n - 1)) ?? 0];
        const [from, to] = cuts[0] + cuts[1] < TEE_SHARE * total ? [cuts[0], total - cuts[1]] : [0, total];
        // PROTOTYPE (road study): the deck follows the smooth curve of the corridors (roads/stations.ts), so it runs on
        // from the road on the ground without a kink; `along` of a station is metres on that curve, scaled to the chords'.
        const smooth = smoothStations(line), scale = total / (smooth[smooth.length - 1].along || 1);
        const ds = smooth.map(st => st.along * scale), track = line.code >= RAIL_CODE;
        const at = (k: number): Station => ({ ...smooth[k], sleepers: track ? deckSleepers(line, ds[k], total) : undefined });
        const point = (d: number) => {
            let k = 0;
            while (k + 2 < smooth.length && ds[k + 1] < d) k++;
            return mix(at(k), at(k + 1), (d - ds[k]) / (ds[k + 1] - ds[k] || 1));
        };
        const end = (v: number) => (v ? (to < total ? point(to) : at(smooth.length - 1)) : from > 0 ? point(from) : at(0));
        const decks = wider(line, lines, endKey), walks = deckWalks(line, all);
        // PROTOTYPE (road study): each side's walk and parapet start past a road that leaves an end on that side.
        const give = yields(line, smooth, all).map((pair, v) => pair.map(m => (v ? to - (to < total ? 0 : m * scale) : from + (from > 0 ? 0 : m * scale))));
        const given = (d: number) => [0, 1].map(i => d < give[0][i] || d > give[1][i]) as [boolean, boolean];
        for (let k = 0; k + 1 < smooth.length; k++) {
            const lo = Math.max(ds[k], from), hi = Math.min(ds[k + 1], to);
            if (lo >= hi || !owns(smooth[k].x, smooth[k].y)) continue;
            const cuts = [lo, ...give.flat().filter(d => d > lo && d < hi).sort((p, q) => p - q), hi];
            for (let i = 0; i + 1 < cuts.length; i++) {
                const [a, c] = [cuts[i], cuts[i + 1]];
                if (c - a < 1e-3) continue;
                const s = a === ds[k] ? at(k) : point(a), e = c === ds[k + 1] ? at(k + 1) : point(c);
                // Where the deck runs on a wider deck's road at its height, that deck carries it: it draws nothing there.
                if (decks.length && onDeck(mix(s, e, 0.5), decks)) continue;
                segment(b, s, e, line, t, c - a, walks, given((a + c) / 2), [a / scale, c / scale]);
            }
        }
        // The walk's and the parapet's own ends where a side starts past a road that leaves the deck.
        if (line.style === 'beam' && !path && !isTrack(line)) for (const v of [0, 1]) for (const i of [0, 1]) {
            const d = give[v][i];
            if (d === (v ? to : from) || d <= from || d >= to || !owns(point(d).x, point(d).y)) continue;
            const st = point(d), next = point(d + (v ? 0.1 : -0.1)), side = i ? -1 : 1, walk = walks[i], gx = next.x - st.x, gy = next.y - st.y, l = Math.hypot(gx, gy) || 1;
            const out: [number, number, number] = [gx / l, -gy / l, 0];
            const cap = (a: number, c: number, bottom: number, top: number) => b.quad([b.at(st, [a, bottom], CONCRETE), b.at(st, [c, bottom], CONCRETE), b.at(st, [c, top], CONCRETE), b.at(st, [a, top], CONCRETE)], out);
            if (walk) cap(side * h, side * (h + walk), LIFT_M, LIFT_M + KERB_RISE_M);
            cap(side * (h + walk), side * (h + walk + PARAPET[0]), LIFT_M, PARAPET[1]);
        }
        for (const v of [0, n - 1]) {
            if (ends.get(endKey(line, v)) !== 1 || !owns(line.xy[2 * v], line.xy[2 * v + 1])) continue;
            // The end of the deck under its road, and of a road's parapets.
            const s = end(v), parapets = line.style === 'beam' && !path && !isTrack(line), w = parapets ? h + PARAPET[0] : h;
            const [wl, wr] = parapets ? [w + walks[0], w + walks[1]] : [w, w];
            const [ex, ey] = [line.xy[2 * (v ? v : 1)] - line.xy[2 * (v ? v - 1 : 0)], line.xy[2 * (v ? v : 1) + 1] - line.xy[2 * (v ? v - 1 : 0) + 1]];
            const l = Math.hypot(ex, ey) || 1, sign = v ? 1 : -1, out: [number, number, number] = [(sign * ex) / l, (-sign * ey) / l, 0];
            const cap = (a: number, c: number, bottom: number, top: number) => b.quad([b.at(s, [a, bottom], CONCRETE), b.at(s, [c, bottom], CONCRETE), b.at(s, [c, top], CONCRETE), b.at(s, [a, top], CONCRETE)], out);
            cap(-wr, wl, -t, LIFT_M);
            if (parapets) for (const [side, walk, edge] of [[1, walks[0], wl], [-1, walks[1], wr]]) {
                if (v ? give[1][side > 0 ? 0 : 1] < to : give[0][side > 0 ? 0 : 1] > from) continue;
                if (walk) cap(side * h, side * (h + walk), LIFT_M, LIFT_M + KERB_RISE_M);
                cap(side * (h + walk), side * edge, LIFT_M, PARAPET[1]);
            }
            if (line.abut?.[v ? 1 : 0]) {
                const depth = Math.min(ABUTMENT_M, (to - from) / 2);
                abutment(b, s, point(v ? to - depth : from + depth), Math.max(wl, wr), t, out);
            }
        }
    }
    piers(b, list, tile, lines, owns);
}

/**
 * The abutment under a deck's end at station s: a wall from the deck's underside down into the drawn
 * ground, `half` metres to each side and back to station `inner`, so the deck never ends in the air
 * where the ground falls away under it. Where the ground holds the road's height, it hides the wall.
 */
function abutment(b: MeshBuilder, s: Station, inner: Station, half: number, depth: number, out: [number, number, number]): void {
    const down = (st: Station, a: number) => b.vertex(st.x + st.ax * a, st.y + st.ay * a, -PIER_SINK_M, CONCRETE | (GROUND << 24));
    const up = (st: Station, a: number) => b.at(st, [a, -depth], CONCRETE);
    b.quad([down(s, -half), down(s, half), up(s, half), up(s, -half)], out);
    b.quad([down(inner, half), down(inner, -half), up(inner, -half), up(inner, half)], [-out[0], -out[1], 0]);
    for (const side of [1, -1]) b.quad([down(s, side * half), down(inner, side * half), up(inner, side * half), up(s, side * half)], b.outward(s, inner, [side, 0]));
}

/** The decks wider than `line` that it may run on: not those that it runs on into at a joint, and only those whose road comes within its box. */
function wider(line: Line, lines: Line[], endKey: (line: Line, v: number) => string): Line[] {
    const joints = new Set([endKey(line, 0), endKey(line, line.z.length - 1)]), [x0, y0, x1, y1] = box(line.xy);
    const unit = Math.min(metresPerUnit(y0), metresPerUnit(y1));
    return lines.filter(other => {
        if (other.h <= line.h || joints.has(endKey(other, 0)) || joints.has(endKey(other, other.z.length - 1))) return false;
        const [a0, b0, a1, b1] = box(other.xy), grow = other.h / unit;
        return a0 - grow <= x1 && a1 + grow >= x0 && b0 - grow <= y1 && b1 + grow >= y0;
    });
}

/** The box of a line's points: west, north, east, south in world units. */
function box(xy: ArrayLike<number>): [number, number, number, number] {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (let i = 0; i < xy.length; i += 2) {
        x0 = Math.min(x0, xy[i]);
        x1 = Math.max(x1, xy[i]);
        y0 = Math.min(y0, xy[i + 1]);
        y1 = Math.max(y1, xy[i + 1]);
    }
    return [x0, y0, x1, y1];
}

/** Whether station s of a deck lies on the road of one of the wider `decks` within MEET_M of its height. */
function onDeck(s: Station, decks: Line[]): boolean {
    const unit = metresPerUnit(s.y);
    for (const { xy, z, h } of decks) {
        for (let v = 0; v + 1 < z.length; v++) {
            const ax = xy[2 * v], ay = xy[2 * v + 1], dx = xy[2 * v + 2] - ax, dy = xy[2 * v + 3] - ay;
            const u = Math.min(1, Math.max(0, ((s.x - ax) * dx + (s.y - ay) * dy) / (dx * dx + dy * dy || 1)));
            if (Math.hypot(s.x - ax - u * dx, s.y - ay - u * dy) * unit <= h && Math.abs(s.z - z[v] - (z[v + 1] - z[v]) * u) < MEET_M) return true;
        }
    }
    return false;
}

/** The deck from station s to t, `length` metres long, in the line's style; `given` sides start past a road that leaves the deck there, `along` is metres on its curve. */
function segment(b: MeshBuilder, s: Station, t: Station, line: Line, depth: number, length: number, walks: [number, number], given: [boolean, boolean], along: [number, number]): void {
    const h = line.h, road = !isPath(line), track = isTrack(line);
    if (line.style === 'beam' && road && !track) return roadBridge(b, s, t, line, depth, walks, given, along);
    // A plank deck, or a road deck, its edges and its underside.
    const edge = line.style === 'cable' ? STEEL : track ? CONCRETE : WOOD_DARK;
    if (isPaved(line) || track) roadway(b, s, t, line);
    else b.strip(s, t, [-h, LIFT_M], [h, LIFT_M], WOOD, [0, 1]);
    for (const side of [1, -1]) b.strip(s, t, [side * h, LIFT_M], [side * h, -depth], edge, [side, 0]);
    b.strip(s, t, [-h, -depth], [h, -depth], edge, [0, -1]);
    if (line.style === 'covered') return covered(b, s, t, h, length);
    if (line.style === 'cable' || track) return railing(b, s, t, h, length, STEEL, track ? BAR_M : CABLE_M);
    railing(b, s, t, h, length, WOOD, BAR_M);
}

/**
 * The road on a deck, painted as the road on the ground. PROTOTYPE (road study): with the crown of a paved
 * road and the lift of its class, as the corridor's carriageway (roads/section.ts, roads/corridor.ts), so the
 * deck's road runs on from the road on the ground without a step.
 */
const roadLift = (line: Line) => LIFT_M + Math.max(0, 30 - line.profile) * 0.001;

function roadway(b: MeshBuilder, s: Station, t: Station, line: Line): void {
    const h = line.h, paint = rgb('#000000', PAINT) | line.code, lift = roadLift(line);
    const crown = isPaved(line) && !isTrack(line) ? CROWN * h : 0;
    for (const [a, c] of line.h >= 3 ? [[-h, -h / 2], [-h / 2, 0], [0, h / 2], [h / 2, h]] : [[-h, 0], [0, h]]) {
        const up = (x: number) => lift + crown * (1 - (x / h) ** 2);
        b.quad([b.at(s, [a, up(a)], paint, a, h), b.at(s, [c, up(c)], paint, c, h), b.at(t, [c, up(c)], paint, c, h), b.at(t, [a, up(a)], paint, a, h)], b.outward(s, t, [0, 1]));
    }
}

/**
 * A road's deck from station s to t: the road, a sidewalk behind a kerb where the town roads at its ends have
 * one (`walks`, left and right), a parapet on each side, the deck's faces and its underside. A `given` side has
 * neither: its top runs level from the road to the deck's outer face, where a road leaves the deck's end.
 * PROTOTYPE (road study): near the eye the road and that top take the corridors' looks (roads/looks.wgsl), as
 * corridor faces that sink past the corridors' range, where this paint shows.
 */
function roadBridge(b: MeshBuilder, s: Station, t: Station, line: Line, depth: number, walks: [number, number], given: [boolean, boolean], along: [number, number]): void {
    const h = line.h, [pw, ph] = PARAPET, lift = roadLift(line), kerb = lift + KERB_RISE_M;
    roadway(b, s, t, line);
    const look = lookOf(line), crown = isPaved(line) ? CROWN * h : 0, up = (x: number) => lift + crown * (1 - Math.min(1, (x / h) ** 2));
    const cv = (st: Station, a: number, z: number, mat: number, flags: number, m: number) => b.vertex(st.x + st.ax * a, st.y + st.ay * a, st.z + z, corridorColour(mat, line.code, flags, 0), a, h, m);
    const lookQuad = (a: number, c: number, mat: number, flags = 0) => b.quad([cv(s, a, up(a), mat, flags, along[0]), cv(s, c, up(c), mat, flags, along[0]), cv(t, c, up(c), mat, flags, along[1]), cv(t, a, up(a), mat, flags, along[1])], b.outward(s, t, [0, 1]));
    const lines = (given[0] ? CF.NO_LEFT_LINE : 0) | (given[1] ? CF.NO_RIGHT_LINE : 0);
    for (const [a, c] of h >= 3 ? [[-h, -h / 2], [-h / 2, 0], [0, h / 2], [h / 2, h]] : [[-h, 0], [0, h]]) lookQuad(a, c, look, lines);
    for (const [side, walk, give] of [[1, walks[0], given[0]], [-1, walks[1], given[1]]] as [number, number, boolean][]) {
        const inner = h + walk;
        if (give) {
            const paint = rgb('#000000', PAINT) | line.code, edge = side * (inner + pw);
            b.quad([b.at(s, [side * h, lift], paint, side * h, h), b.at(s, [edge, lift], paint, edge, h), b.at(t, [edge, lift], paint, edge, h), b.at(t, [side * h, lift], paint, side * h, h)], b.outward(s, t, [0, 1]));
            lookQuad(side * h, edge, JMAT.FILLET);
            b.strip(s, t, [edge, lift], [edge, -depth], CONCRETE, [side, 0]);
            continue;
        }
        if (walk) {
            b.strip(s, t, [side * h, lift], [side * h, kerb], CAP, [-side, 0]);
            b.strip(s, t, [side * h, kerb], [side * (h + KERB_TOP_M), kerb], CAP, [0, 1]);
            b.strip(s, t, [side * (h + KERB_TOP_M), kerb], [side * inner, kerb], WALKWAY, [0, 1]);
        }
        b.strip(s, t, [side * inner, walk ? kerb : lift], [side * inner, ph], CAP, [-side, 0]);
        b.strip(s, t, [side * inner, ph], [side * (inner + pw), ph], CAP, [0, 1]);
        b.strip(s, t, [side * (inner + pw), ph], [side * (inner + pw), -depth], CONCRETE, [side, 0]);
    }
    b.strip(s, t, [-(h + walks[1] + pw), -depth], [h + walks[0] + pw, -depth], SOFFIT, [0, -1]);
}

/**
 * PROTOTYPE (road study): the sidewalks that a road's deck carries, left and right of its line: those that the
 * town roads meeting its ends have there (roads/section.ts draws them), so the walk runs on over the bridge.
 */
function deckWalks(line: Line, all: Line[]): [number, number] {
    const out: [number, number] = [0, 0], n = line.z.length;
    for (const end of [0, n - 1]) {
        const x = line.xy[2 * end], y = line.xy[2 * end + 1];
        for (const road of all) {
            if (road.kind !== ROAD || !road.sidewalks) continue;
            const m = road.z.length;
            for (const w of [0, m - 1]) {
                if (road.xy[2 * w] !== x || road.xy[2 * w + 1] !== y) continue;
                // A road that runs into the deck's start, or out of its end, has the deck's left on its left.
                const flags = road.sidewalks[w ? m - 2 : 0] ?? 0, same = (w === m - 1) === (end === 0), width = walkOf(road);
                if (flags & (same ? LEFT : RIGHT)) out[0] = Math.max(out[0], width);
                if (flags & (same ? RIGHT : LEFT)) out[1] = Math.max(out[1], width);
            }
        }
    }
    return out;
}

/** A railing from station s to t on each side: a rail of `bar` metres square at the top, and a post about every POSTS_M. */
function railing(b: MeshBuilder, s: Station, t: Station, h: number, length: number, colour: number, bar: number, top = RAIL_M, bottom = LIFT_M): void {
    for (const side of [1, -1]) {
        const inner = side * (h - bar);
        for (const [a, c, out] of [[[inner, top - bar], [inner, top], [-side, 0]], [[inner, top], [side * h, top], [0, 1]], [[side * h, top], [side * h, top - bar], [side, 0]], [[side * h, top - bar], [inner, top - bar], [0, -1]]] as [Corner, Corner, Corner][]) {
            b.strip(s, t, a, c, colour, out);
        }
        posts(s, t, length, (p, q) => {
            for (const [a, c, out] of [[[inner, bottom], [inner, top], [-side, 0]], [[side * h, bottom], [side * h, top], [side, 0]]] as [Corner, Corner, Corner][]) {
                b.strip(p, q, a, c, colour, out);
            }
            for (const [st, sign] of [[p, -1], [q, 1]] as [Station, number][]) {
                const dx = t.x - s.x, dy = t.y - s.y, l = Math.hypot(dx, dy) || 1;
                b.quad([b.at(st, [inner, bottom], colour), b.at(st, [side * h, bottom], colour), b.at(st, [side * h, top], colour), b.at(st, [inner, top], colour)], [sign * dx / l, -sign * dy / l, 0]);
            }
        }, bar, POSTS_M);
    }
}

/** A covered bridge from station s to t: a wooden wall on each side, posts up to the eaves, and a pitched roof with its underside. */
function covered(b: MeshBuilder, s: Station, t: Station, h: number, length: number): void {
    const w = h + 0.1, eave = w + OVERHANG_M, ridge = EAVES_M + PITCH * eave;
    for (const side of [1, -1]) {
        b.strip(s, t, [side * w, LIFT_M], [side * w, COVER_WALL_M], WOOD_DARK, [-side, 0]);
        b.strip(s, t, [side * w, COVER_WALL_M], [side * w, LIFT_M], WOOD_DARK, [side, 0]);
        b.strip(s, t, [0, ridge], [side * eave, EAVES_M], ROOF, [0, 1]);
        b.strip(s, t, [side * eave, EAVES_M], [0, ridge], ROOF_UNDER, [0, -1]);
    }
    railing(b, s, t, w, length, WOOD_DARK, 0.15, EAVES_M, COVER_WALL_M);
}

/** A pier at each `bridge-pier` point near the node, across its bridge line, from under the deck down into the drawn surface. */
function piers(b: MeshBuilder, list: Feature[], tile: Node, lines: Line[], owns: Owns): void {
    const scale = unitScale(tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT;
    for (const { kind, parts } of list) {
        if (kind !== 'bridge-pier') continue;
        for (const { xy } of parts) {
            const px = (ox + xy[0]) * scale, py = (oy + xy[1]) * scale;
            if (!owns(px, py)) continue;
            const unit = metresPerUnit(py);
            let best: { line: Line; v: number; u: number; d: number } | undefined;
            for (const line of lines) for (let v = 0; v + 1 < line.z.length; v++) {
                const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), dx = bx - ax, dy = by - ay;
                const u = Math.min(1, Math.max(0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy || 1)));
                const d = Math.hypot(px - ax - u * dx, py - ay - u * dy) * unit;
                if (d < PIER_REACH_M && (!best || d < best.d)) best = { line, v, u, d };
            }
            if (!best) continue;
            const { line, v, u } = best, road = !isPath(line);
            const dx = line.xy[2 * v + 2] - line.xy[2 * v], dy = line.xy[2 * v + 3] - line.xy[2 * v + 1], l = Math.hypot(dx, dy) || 1;
            const top = line.z[v] + (line.z[v + 1] - line.z[v]) * u - (line.deck ?? 0.6) + 0.1;
            const half = road ? PIER_WIDTH * (line.h + PARAPET[0]) : line.h, thick = (road ? PIER_M.road : PIER_M.path) / 2;
            // Along and across the line, in world units per metre.
            const [gx, gy] = [dx / l / unit, dy / l / unit], [cx, cy] = [dy / l / unit, -dx / l / unit];
            const corner = (a: number, c: number): [number, number] => [px + gx * a + cx * c, py + gy * a + cy * c];
            const colour = pierColour(road);
            const faces: [[number, number], [number, number], [number, number, number]][] = [
                [[thick, half], [thick, -half], [dx / l, -dy / l, 0]], [[-thick, -half], [-thick, half], [-dx / l, dy / l, 0]],
                [[-thick, half], [thick, half], [dy / l, dx / l, 0]], [[thick, -half], [-thick, -half], [-dy / l, -dx / l, 0]],
            ];
            for (const [[a0, c0], [a1, c1], out] of faces) {
                const [p0, p1] = [corner(a0, c0), corner(a1, c1)];
                b.quad([b.vertex(...p0, top, colour), b.vertex(...p1, top, colour), b.vertex(...p1, -PIER_SINK_M, colour | (GROUND << 24)), b.vertex(...p0, -PIER_SINK_M, colour | (GROUND << 24))], out);
            }
        }
    }
}

/**
 * PROTOTYPE (road study): the half-widths of a road's deck to its outer faces, left and right: its road, the
 * walks that it carries and its parapets.
 */
export function deckOuter(line: Line, all: Line[]): [number, number] {
    if (line.style !== 'beam' || isPath(line) || isTrack(line)) return [line.h, line.h];
    const walks = deckWalks(line, all);
    return [line.h + walks[0] + PARAPET[0], line.h + walks[1] + PARAPET[0]];
}

/** A road that leaves a deck's end counts with its way out this far. */
const LEAVES_M = 20;

/**
 * PROTOTYPE (road study): the metres from each end of a road's deck (start, end) that each side (left, right)
 * gives to a road that leaves that end at an angle: its walk and parapet start past that road's core.
 */
function yields(line: Line, smooth: RoadStation[], all: Line[]): [[number, number], [number, number]] {
    const out: [[number, number], [number, number]] = [[0, 0], [0, 0]];
    if (line.style !== 'beam' || isPath(line) || isTrack(line) || smooth.length < 2) return out;
    const n = line.z.length, total = smooth[smooth.length - 1].along, outer = deckOuter(line, all);
    // A point's metres along the deck's curve and to its left.
    const frame = (x: number, y: number) => {
        let best: [number, number] = [0, 0], least = Infinity;
        for (let k = 0; k + 1 < smooth.length; k++) {
            const s = smooth[k], t = smooth[k + 1], unit = metresPerUnit(s.y), dx = (t.x - s.x) * unit, dy = (t.y - s.y) * unit, l2 = dx * dx + dy * dy || 1;
            const ex = (x - s.x) * unit, ey = (y - s.y) * unit, u = Math.max(0, Math.min(1, (ex * dx + ey * dy) / l2)), d = Math.hypot(ex - u * dx, ey - u * dy);
            if (d < least) [least, best] = [d, [s.along + u * (t.along - s.along), (ex * dy - ey * dx) / Math.sqrt(l2)]];
        }
        return best;
    };
    for (const v of [0, 1] as const) {
        const x = line.xy[v ? 2 * n - 2 : 0], y = line.xy[v ? 2 * n - 1 : 1];
        const k0 = v ? smooth.length - 1 : 0, k1 = v ? smooth.length - 2 : 1, inward = [smooth[k1].x - smooth[k0].x, smooth[k1].y - smooth[k0].y];
        const ways: { other: Line; list: RoadStation[]; from: number; dir: number; cos: number }[] = [];
        for (const other of all) {
            if (other.kind !== ROAD) continue;
            for (let w = 0; w < other.z.length; w++) {
                if (other.xy[2 * w] !== x || other.xy[2 * w + 1] !== y) continue;
                const list = smoothStations(other), j = list.findIndex(st => st.x === x && st.y === y);
                if (j < 0) continue;
                for (const dir of [-1, 1]) {
                    const nb = list[j + dir];
                    if (!nb) continue;
                    const ux = nb.x - x, uy = nb.y - y, cos = (ux * inward[0] + uy * inward[1]) / (Math.hypot(ux, uy) * Math.hypot(inward[0], inward[1]) || 1);
                    ways.push({ other, list, from: j, dir, cos });
                }
            }
        }
        // The way that runs on from the deck is the road it carries; the others leave it.
        const onward = ways.reduce<(typeof ways)[number] | undefined>((a, w) => (w.cos < -0.5 && (!a || w.cos < a.cos) ? w : a), undefined);
        for (const w of ways) {
            if (w === onward) continue;
            const core = w.other.h + 0.6;
            for (let k = w.from; k >= 0 && k < w.list.length && Math.abs(w.list[k].along - w.list[w.from].along) <= LEAVES_M; k += w.dir) {
                const st = w.list[k];
                for (const a of [-core, 0, core]) {
                    const [m, lat] = frame(st.x + st.ax * a, st.y + st.ay * a), d = v ? total - m : m;
                    if (d <= 0 || d > 0.45 * total) continue;
                    if (lat > line.h && lat <= outer[0] + 0.5) out[v][0] = Math.max(out[v][0], d);
                    if (-lat > line.h && -lat <= outer[1] + 0.5) out[v][1] = Math.max(out[v][1], d);
                }
            }
        }
        for (const i of [0, 1]) if (out[v][i] > 0) out[v][i] = Math.min(0.45 * total, out[v][i] + 0.3);
    }
    return out;
}
