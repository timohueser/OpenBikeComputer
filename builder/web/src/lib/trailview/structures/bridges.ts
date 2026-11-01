// The bridges of a node: a deck along each bridge line at its solved heights, with parapets for a road,
// a railing for a path, cables for a bridge that hangs and walls and a roof for a covered one, and a pier
// at each `bridge-pier` point of the bake, down into the drawn surface.
import { EXTENT, unitScale, type Feature } from '../stream/features';
import { BRIDGE, isPath, LIFT_M, metresPerUnit, RAIL_CODE, type Line, type Node } from '../terrain/bins';
import { decksOf, isPaved, onDeck, PARAPET_M, teesOf, wider, type DeckPlan } from '../roads/decks';
import { KERB_RISE_M } from '../roads/section';
import type { Owns } from './node';
import { curveStops, GROUND, MeshBuilder, mix, PAINT, paintOf, posts, rgb, type Corner, type Station } from './mesh';

/** A road's parapet: width and height above the road. */
const PARAPET: Corner = [PARAPET_M, 1.0];
/** A deck's kerb and sidewalk where corridors draw their own near the eye: this far inside theirs, so theirs show and these only past their range. */
const WALK_INSIDE_M = 0.01;
/** A railing: its height, the side of its rail and its posts, and the posts' spacing; a cable and its hangers are thinner. */
const RAIL_M = 1.05, BAR_M = 0.08, CABLE_M = 0.04, POSTS_M = 2;
/** A covered bridge: the top of its wall, its eaves, the rise of its roof per metre of half-width, and the roof's overhang. */
const COVER_WALL_M = 1.0, EAVES_M = 2.6, PITCH = 0.5, OVERHANG_M = 0.4;
/** A pier: its thickness along the bridge for a road and a path, the share of the deck's width that it takes, and how deep it stands in the ground. */
const PIER_M = { road: 1.6, path: 0.5 }, PIER_WIDTH = 0.6, PIER_SINK_M = 1.5;
/** An abutment's thickness along the bridge. */
const ABUTMENT_M = 1.0;
/** A deck is tested every PIECE_M for a wider deck that carries it. */
const PIECE_M = 1.0;
/** A pier stands on the nearest bridge line within this many metres. */
const PIER_REACH_M = 3;

const CONCRETE = rgb('#9a978e'), CAP = rgb('#b3b0a7'), SOFFIT = rgb('#76746d'), WALKWAY = rgb('#a8a59c'), WOOD = rgb('#7d5c3d'), WOOD_DARK = rgb('#5d4430');
const STEEL = rgb('#4a4d50'), ROOF = rgb('#7a3f2c'), ROOF_UNDER = rgb('#3e2a1e');
const pierColour = (road: boolean) => (road ? CONCRETE : rgb('#8d8a82'));

/** A railway's bridge: no parapets, which would stand between the decks of two tracks side by side, but a steel railing. */
const isTrack = (line: Line) => line.code >= RAIL_CODE;

/** A bridge without a corridor's plan. */
const NO_DECK: DeckPlan = { walks: [0, 0], yields: [[0, 0], [0, 0]] };

/** The bridges of a feature tile's `list` and `lines` whose pieces a square `owns`. Corridors draw the decks of road bridges, and these hand over to them. */
export function bridges(b: MeshBuilder, list: Feature[], tile: Node, all: Line[], owns: Owns): void {
    const lines = all.filter(line => line.kind === BRIDGE);
    if (!lines.length) return;
    // Ends that two bridge lines share are joints: no cap there.
    const ends = new Map<string, number>();
    const endKey = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    for (const line of lines) for (const v of [0, line.z.length - 1]) ends.set(endKey(line, v), (ends.get(endKey(line, v)) ?? 0) + 1);
    const tees = teesOf(all), plans = decksOf(all);
    for (const line of lines) {
        const n = line.z.length, path = isPath(line), t = line.deck ?? 0.6, h = line.h, curve = curveStops(line), at = (k: number) => curve[k], metres = curve.map(({ along }) => along), last = metres.length - 1;
        // The deck runs from `from` to `to`, short of an end that leaves a road at a T.
        const total = metres[last], [from, to] = tees(line, total), deck = plans.get(line) ?? NO_DECK, { walks } = deck, corridor = plans.has(line);
        const point = (d: number) => {
            let v = 0;
            while (v + 2 <= last && metres[v + 1] < d) v++;
            return mix(at(v), at(v + 1), (d - metres[v]) / (metres[v + 1] - metres[v] || 1));
        };
        const end = (v: number) => (v ? (to < total ? point(to) : at(last)) : from > 0 ? point(from) : at(0));
        const decks = wider(line, lines);
        // Metres along the line where each side's walk and parapet start and stop: past a road that leaves an end on that side.
        const starts = deck.yields[0].map(d => (from > 0 ? from : from + d)), stops = deck.yields[1].map(d => (to < total ? to : to - d));
        const given = (d: number): [boolean, boolean] => [d < starts[0] || d > stops[0], d < starts[1] || d > stops[1]];
        for (let v = 0; v < last; v++) {
            const lo = Math.max(metres[v], from), hi = Math.min(metres[v + 1], to);
            if (lo >= hi || !owns(at(v).x, at(v).y)) continue;
            const stop = (d: number) => (d === metres[v] ? at(v) : d === metres[v + 1] ? at(v + 1) : point(d));
            const breaks = [lo, ...[...starts, ...stops].filter(d => d > lo && d < hi).sort((p, q) => p - q), hi];
            for (let i = 0; i + 1 < breaks.length; i++) {
                const [a0, a1] = [breaks[i], breaks[i + 1]];
                if (a1 <= a0) continue;
                const span = { walks, given: given((a0 + a1) / 2), corridor };
                // Where the deck runs on a wider deck's road at its height, that deck carries it: it draws nothing there.
                const pieces = decks.length ? Math.ceil((a1 - a0) / PIECE_M) : 0;
                let run = a0;
                for (let k = 1; k <= pieces; k++) {
                    const a = a0 + (a1 - a0) * (k - 1) / pieces, c = a0 + (a1 - a0) * k / pieces, carried = onDeck(point((a + c) / 2), decks);
                    if (carried && run < a) segment(b, stop(run), stop(a), line, t, a - run, span);
                    if (carried) run = c;
                }
                if (run < a1) segment(b, stop(run), stop(a1), line, t, a1 - run, span);
            }
        }
        const parapets = line.style === 'beam' && !path && !isTrack(line);
        // Each side's own end where it starts past a road that leaves the deck.
        if (parapets) for (const [i, d, ahead] of [[0, starts[0], 0.1], [1, starts[1], 0.1], [0, stops[0], -0.1], [1, stops[1], -0.1]]) {
            if (d <= from || d >= to || !owns(point(d).x, point(d).y)) continue;
            const st = point(d), next = point(d - ahead), side = i ? -1 : 1, walk = walks[i], gx = next.x - st.x, gy = next.y - st.y, l = Math.hypot(gx, gy) || 1;
            const cap = (a: number, c: number, bottom: number, top: number) => b.quad([b.at(st, [a, bottom], CONCRETE), b.at(st, [c, bottom], CONCRETE), b.at(st, [c, top], CONCRETE), b.at(st, [a, top], CONCRETE)], [gx / l, -gy / l, 0]);
            if (walk) cap(side * h, side * (h + walk), LIFT_M, LIFT_M + KERB_RISE_M);
            cap(side * (h + walk), side * (h + walk + PARAPET[0]), LIFT_M, PARAPET[1]);
        }
        for (const v of [0, n - 1]) {
            if (ends.get(endKey(line, v)) !== 1 || !owns(line.xy[2 * v], line.xy[2 * v + 1])) continue;
            // The end of the deck under its road, and of a road's parapets.
            const s = end(v), [wl, wr] = parapets ? [h + walks[0] + PARAPET[0], h + walks[1] + PARAPET[0]] : [h, h];
            const [ex, ey] = [line.xy[2 * (v ? v : 1)] - line.xy[2 * (v ? v - 1 : 0)], line.xy[2 * (v ? v : 1) + 1] - line.xy[2 * (v ? v - 1 : 0) + 1]];
            const l = Math.hypot(ex, ey) || 1, sign = v ? 1 : -1, out: [number, number, number] = [(sign * ex) / l, (-sign * ey) / l, 0];
            const cap = (a: number, c: number, bottom: number, top: number) => b.quad([b.at(s, [a, bottom], CONCRETE), b.at(s, [c, bottom], CONCRETE), b.at(s, [c, top], CONCRETE), b.at(s, [a, top], CONCRETE)], out);
            // A corridor's mouth runs over the end of a side that gives way: the cap stays under it.
            const top = corridor ? 0 : LIFT_M;
            cap(-wr, wl, -t, top);
            if (parapets) for (const [i, side, edge] of [[0, 1, wl], [1, -1, wr]]) {
                if (v ? stops[i] < to : starts[i] > from) continue;
                if (walks[i]) cap(side * h, side * (h + walks[i]), top, LIFT_M + KERB_RISE_M);
                cap(side * (h + walks[i]), side * edge, top, PARAPET[1]);
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

/** What a stretch of a deck holds: its sidewalks' widths left and right, whether each side gives way to a road, and whether a corridor draws its road near the eye. */
interface Span { walks: [number, number]; given: [boolean, boolean]; corridor: boolean }

/** The deck from station s to t, `length` metres long, in the line's style. */
function segment(b: MeshBuilder, s: Station, t: Station, line: Line, depth: number, length: number, span: Span): void {
    const h = line.h, road = !isPath(line), track = isTrack(line);
    if (line.style === 'beam' && road && !track) return roadBridge(b, s, t, line, depth, span);
    // A plank deck, or a road deck, its edges and its underside.
    const edge = line.style === 'cable' ? STEEL : track ? CONCRETE : WOOD_DARK;
    if (isPaved(line) || track) roadway(b, s, t, line, span.corridor);
    else b.strip(s, t, [-h, LIFT_M], [h, LIFT_M], WOOD, [0, 1]);
    for (const side of [1, -1]) b.strip(s, t, [side * h, LIFT_M], [side * h, -depth], edge, [side, 0]);
    b.strip(s, t, [-h, -depth], [h, -depth], edge, [0, -1]);
    if (line.style === 'covered') return covered(b, s, t, h, length);
    if (line.style === 'cable' || track) return railing(b, s, t, h, length, STEEL, track ? BAR_M : CABLE_M);
    railing(b, s, t, h, length, WOOD, BAR_M);
}

/** The paint's height over a deck: under a corridor it shows until the corridor's road arrives, then lies hidden. */
const paintLift = (corridor: boolean) => (corridor ? LIFT_M - WALK_INSIDE_M : LIFT_M);

/** The road on a deck, painted as the road on the ground. */
function roadway(b: MeshBuilder, s: Station, t: Station, line: Line, corridor: boolean): void {
    const h = line.h, paint = paintOf(line, PAINT), up = paintLift(corridor);
    b.quad([b.at(s, [-h, up], paint, -h, h), b.at(s, [h, up], paint, h, h), b.at(t, [h, up], paint, h, h), b.at(t, [-h, up], paint, -h, h)], b.outward(s, t, [0, 1]));
}

/**
 * A road's deck from station s to t: the road, a sidewalk behind a kerb where the road on the ground has one, a
 * parapet on each side, the deck's faces and its underside. A side that gives way has neither, only a level top.
 */
function roadBridge(b: MeshBuilder, s: Station, t: Station, line: Line, depth: number, { walks, given, corridor }: Span): void {
    const h = line.h, [pw, ph] = PARAPET, kerb = LIFT_M + KERB_RISE_M - WALK_INSIDE_M, foot = h + WALK_INSIDE_M, paint = paintOf(line, PAINT), up = paintLift(corridor);
    roadway(b, s, t, line, corridor);
    for (const [side, walk, gives] of [[1, walks[0], given[0]], [-1, walks[1], given[1]]] as [number, number, boolean][]) {
        const inner = h + walk, outer = side * (inner + pw);
        if (gives) {
            b.quad([b.at(s, [side * h, up], paint, side * h, h), b.at(s, [outer, up], paint, outer, h), b.at(t, [outer, up], paint, outer, h), b.at(t, [side * h, up], paint, side * h, h)], b.outward(s, t, [0, 1]));
            b.strip(s, t, [outer, LIFT_M], [outer, -depth], CONCRETE, [side, 0]);
            continue;
        }
        if (walk) {
            b.strip(s, t, [side * foot, LIFT_M], [side * foot, kerb], CAP, [-side, 0]);
            b.strip(s, t, [side * foot, kerb], [side * inner, kerb], WALKWAY, [0, 1]);
        }
        b.strip(s, t, [side * inner, walk ? kerb : LIFT_M], [side * inner, ph], CAP, [-side, 0]);
        b.strip(s, t, [side * inner, ph], [outer, ph], CAP, [0, 1]);
        b.strip(s, t, [outer, ph], [outer, -depth], CONCRETE, [side, 0]);
    }
    b.strip(s, t, [-(h + walks[1] + pw), -depth], [h + walks[0] + pw, -depth], SOFFIT, [0, -1]);
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
