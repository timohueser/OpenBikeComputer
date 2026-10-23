// The bridges of a node: a deck along each bridge line at its solved heights, with parapets for a road,
// a railing for a path, cables for a bridge that hangs and walls and a roof for a covered one, and a pier
// at each `bridge-pier` point of the bake, down into the drawn surface.
import { EXTENT, type Feature } from '../stream/features';
import { BRIDGE, metresPerUnit, type Line, type Node } from '../terrain/bins';
import type { Owns } from './node';
import { GROUND, MeshBuilder, mix, PAINT, posts, rgb, station, type Corner, type Station } from './mesh';

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
/** A pier stands on the nearest bridge line within this many metres. */
const PIER_REACH_M = 3;

const CONCRETE = rgb('#9a978e'), CAP = rgb('#b3b0a7'), SOFFIT = rgb('#76746d'), WOOD = rgb('#7d5c3d'), WOOD_DARK = rgb('#5d4430');
const STEEL = rgb('#4a4d50'), ROOF = rgb('#7a3f2c'), ROOF_UNDER = rgb('#3e2a1e');
const pierColour = (road: boolean) => (road ? CONCRETE : rgb('#8d8a82'));

/** The half-width of a deck to the outer face of a road's parapets; a step past a path's railing still stands on it. */
export const deckHalf = (line: Line) => line.h + PARAPET[0];

/** Paths, tracks of grass or dirt: a footbridge. */
const isPath = (line: Line) => line.code >= 3 && line.code <= 6;

/** The bridges of a feature tile's `list` and `lines` whose pieces a square `owns`. */
export function bridges(b: MeshBuilder, list: Feature[], tile: Node, all: Line[], owns: Owns): void {
    const lines = all.filter(line => line.kind === BRIDGE);
    if (!lines.length) return;
    // Ends that two bridge lines share are joints: no cap there.
    const ends = new Map<string, number>();
    const endKey = (line: Line, v: number) => `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
    for (const line of lines) for (const v of [0, line.z.length - 1]) ends.set(endKey(line, v), (ends.get(endKey(line, v)) ?? 0) + 1);
    for (const line of lines) {
        const n = line.z.length, path = isPath(line), t = line.deck ?? 0.6, h = line.h;
        const stations = new Map<number, Station>(), at = (v: number) => stations.get(v) ?? stations.set(v, station(line, v)).get(v)!;
        for (let v = 0; v + 1 < n; v++) {
            if (!owns(line.xy[2 * v], line.xy[2 * v + 1])) continue;
            segment(b, at(v), at(v + 1), line, t, Math.hypot(line.xy[2 * v + 2] - line.xy[2 * v], line.xy[2 * v + 3] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]));
        }
        for (const v of [0, n - 1]) {
            if (ends.get(endKey(line, v)) !== 1 || !owns(line.xy[2 * v], line.xy[2 * v + 1])) continue;
            // The end of the deck under its road, and of a road's parapets.
            const s = at(v), parapets = line.style === 'beam' && !path, w = parapets ? h + PARAPET[0] : h;
            const [ex, ey] = [line.xy[2 * (v ? v : 1)] - line.xy[2 * (v ? v - 1 : 0)], line.xy[2 * (v ? v : 1) + 1] - line.xy[2 * (v ? v - 1 : 0) + 1]];
            const l = Math.hypot(ex, ey) || 1, sign = v ? 1 : -1, out: [number, number, number] = [(sign * ex) / l, (-sign * ey) / l, 0];
            const cap = (a: number, c: number, bottom: number, top: number) => b.quad([b.at(s, [a, bottom], CONCRETE), b.at(s, [c, bottom], CONCRETE), b.at(s, [c, top], CONCRETE), b.at(s, [a, top], CONCRETE)], out);
            cap(-w, w, -t, LIFT_M);
            if (parapets) for (const side of [1, -1]) cap(side * h, side * w, LIFT_M, PARAPET[1]);
            if (line.abut?.[v ? 1 : 0]) {
                const w2 = v ? v - 1 : 1, length = Math.hypot(line.xy[2 * w2] - line.xy[2 * v], line.xy[2 * w2 + 1] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]);
                abutment(b, s, mix(s, at(w2), Math.min(0.5, ABUTMENT_M / Math.max(length, 1e-3))), w, t, out);
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

/** The deck from station s to t, `length` metres long, in the line's style. */
function segment(b: MeshBuilder, s: Station, t: Station, line: Line, depth: number, length: number): void {
    const h = line.h, road = !isPath(line);
    if (line.style === 'beam' && road) return roadBridge(b, s, t, line, depth);
    // A plank deck, or a road deck, its edges and its underside.
    const edge = line.style === 'cable' ? STEEL : WOOD_DARK;
    if (road) roadway(b, s, t, line);
    else b.strip(s, t, [-h, LIFT_M], [h, LIFT_M], WOOD, [0, 1]);
    for (const side of [1, -1]) b.strip(s, t, [side * h, LIFT_M], [side * h, -depth], edge, [side, 0]);
    b.strip(s, t, [-h, -depth], [h, -depth], edge, [0, -1]);
    if (line.style === 'covered') return covered(b, s, t, h, length);
    if (line.style === 'cable') return railing(b, s, t, h, length, STEEL, CABLE_M);
    railing(b, s, t, h, length, WOOD, BAR_M);
}

/** The road on a deck, painted as the road on the ground. */
function roadway(b: MeshBuilder, s: Station, t: Station, line: Line): void {
    const h = line.h, paint = rgb('#000000', PAINT) | line.code;
    b.quad([b.at(s, [-h, LIFT_M], paint, -h, h), b.at(s, [h, LIFT_M], paint, h, h), b.at(t, [h, LIFT_M], paint, h, h), b.at(t, [-h, LIFT_M], paint, -h, h)], b.outward(s, t, [0, 1]));
}

/** A road's deck from station s to t: the road, a parapet on each side, the deck's faces and its underside. */
function roadBridge(b: MeshBuilder, s: Station, t: Station, line: Line, depth: number): void {
    const h = line.h, [pw, ph] = PARAPET;
    roadway(b, s, t, line);
    for (const side of [1, -1]) {
        b.strip(s, t, [side * h, LIFT_M], [side * h, ph], CAP, [-side, 0]);
        b.strip(s, t, [side * h, ph], [side * (h + pw), ph], CAP, [0, 1]);
        b.strip(s, t, [side * (h + pw), ph], [side * (h + pw), -depth], CONCRETE, [side, 0]);
    }
    b.strip(s, t, [-(h + pw), -depth], [h + pw, -depth], SOFFIT, [0, -1]);
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
    const scale = 2 ** (17 - tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT;
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
