// The tunnels of a node: the bore along each tunnel line, with its road, its walls, its arch and its
// lamps; and where a tunnel meets the ground, a portal: a facade with an arched opening across the road
// and a box of the bore into the hill. The stamps hold the road on the ground into the portal and the
// hill over the bore behind it, and the terrain draws no ground in the box: the box hides the slope
// between. Where the terrain draws coarser than level 17, that slope reaches past the box, and a dark
// void fills the opening.
import { metresPerUnit, RAIL_CODE, RULES, TUNNEL, type Line } from '../terrain/bins';
import { BORE, LAMP, MeshBuilder, PAINT, posts, rgb, stationsOf, VOID, type Corner, type Station } from './mesh';
import type { Owns } from './node';

/** A portal of a road and of a path: the bore's margin past the road and the facade's wings past the opening. */
const ROAD_PORTAL = { margin: 0.6, wings: 3 }, PATH_PORTAL = { margin: 0.3, wings: 1.2 };
/**
 * The depth of a portal's box at world row y: on level 17, from which the bore shows (VOID_LOD of
 * structures.wgsl), the ground's slope up over the bore ends within PORTAL_CELLS and a cell's diagonal
 * of mesh cells from the facade. Level 17's texel is 32 world units.
 */
const boxDepth = (y: number) => (RULES.PORTAL_CELLS + Math.SQRT2) * RULES.MESH_TEXELS * 32 * metresPerUnit(y) + 1;
/** A road's bore: the kerb's height, and its lamps' length and spacing. */
const KERB_M = 0.15, LAMP_M = 1.2, LAMPS_M = 10;
/** The arch springs at this share of the bore's height; the void stands this far in. The bore's road lies this far over the ground that runs into the portal. */
const SPRING = 0.6, VOID_M = 0.7, LIFT_M = 0.03;
/**
 * The facade's thickness, the box's walls and roof, and how deep the facade stands in the ground. The
 * facade's top is flush with the roof, under COVER_M of the road solver: a road over a tunnel hides both.
 */
const FACADE_M = 0.6, WALL_M = 0.6, SINK_M = 1;
/** Segments of the arch. */
const ARCH = 8;

const FACADE = rgb('#a5a299'), BOX = rgb('#8f8c84'), DARK = rgb('#0a0a0b', VOID);
const ROCK_FACADE = rgb('#77716a'), ROCK_BOX = rgb('#6a655e');
const TILES = rgb('#dcdad3', BORE), ARCH_LINING = rgb('#7f7e7a', BORE), WALK = rgb('#a3a19b', BORE), LIGHT = rgb('#fff3d6', LAMP);
const ROCK = rgb('#6f6a62', BORE), ROCK_FLOOR = rgb('#7d7262', BORE), LINING = rgb('#8a877f', BORE);

/** The half-width of a tunnel's bore, to its walls. */
export const boreHalf = (line: Line) => line.h + (line.deck! > 3 ? ROAD_PORTAL : PATH_PORTAL).margin;

/** Two tracks in one bore: their tunnels' ends that meet the ground lie at most this far apart, side by side. */
const TWIN_M = 8;

/**
 * A portal over two tracks: the move of its middle to the left of the way into the tunnel and the widening
 * of each side, in metres; null for the end whose twin draws the portal.
 */
export type Twin = { shift: number; widen: number } | null;

/**
 * Per tunnel line of a track, the twins of its two ends that meet the ground: two such ends of two tracks'
 * tunnels that lie side by side within TWIN_M and lead the same way are one portal, which the end that
 * comes first by its point draws. The nearest pairs pair first, and an end has at most one twin.
 */
export function twins(lines: Line[]): Map<Line, [Twin?, Twin?]> {
    const ends = lines.filter(line => line.kind === TUNNEL && line.abut && line.code >= RAIL_CODE).flatMap(line => [0, 1].filter(end => line.abut![end]).map(end => {
        const n = line.z.length, v = end ? n - 1 : 0, w = end ? n - 2 : 1, x = line.xy[2 * v], y = line.xy[2 * v + 1];
        const dx = line.xy[2 * w] - x, dy = line.xy[2 * w + 1] - y, l = Math.hypot(dx, dy) || 1;
        return { line, end, x, y, into: [dx / l, dy / l] };
    }));
    const pairs: { a: (typeof ends)[number]; b: (typeof ends)[number]; d: number; across: number }[] = [];
    for (const a of ends) for (const b of ends) {
        if (a.line === b.line || a.x > b.x || (a.x === b.x && a.y >= b.y)) continue;
        const unit = metresPerUnit(a.y), ox = (b.x - a.x) * unit, oy = (b.y - a.y) * unit, d = Math.hypot(ox, oy);
        if (d <= TWIN_M && a.into[0] * b.into[0] + a.into[1] * b.into[1] >= 0.9) pairs.push({ a, b, d, across: ox * a.into[1] - oy * a.into[0] });
    }
    const out = new Map<Line, [Twin?, Twin?]>(), taken = new Set<(typeof ends)[number]>();
    for (const { a, b, across } of pairs.sort((p, q) => p.d - q.d)) {
        if (taken.has(a) || taken.has(b)) continue;
        taken.add(a).add(b);
        (out.get(a.line) ?? out.set(a.line, [undefined, undefined]).get(a.line)!)[a.end] = { shift: across / 2, widen: Math.abs(across) / 2 };
        (out.get(b.line) ?? out.set(b.line, [undefined, undefined]).get(b.line)!)[b.end] = null;
    }
    return out;
}

/** The length of segment `v` of a line in metres. */
const segment = (line: Line, v: number) => Math.hypot(line.xy[2 * v + 2] - line.xy[2 * v], line.xy[2 * v + 3] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]);

/**
 * The box of a tunnel's portal at vertex `from` toward vertex `to`, in which the terrain draws nothing:
 * its depth, its half-width to the middle of its walls, the heights of its roof's top and of the bore's
 * floor at the facade, and the rise of both per metre into the tunnel. The ground under the floor draws,
 * so no gap opens under the threshold. The rise is the least from the facade to a vertex in the box, so
 * that the box's foot stays under the floor where the tunnel steepens, and the roof rises at the facade
 * until it clears the crown at each vertex: the vault hides the ground over it. A box ends at the other
 * end of a shorter tunnel if that end is a portal.
 */
export function portalBox(line: Line, from: number, to: number, widen = 0): [number, number, number, number, number] {
    const step = Math.sign(to - from), z = line.z[from], depth = boxDepth(line.xy[2 * from + 1]), places: [number, number][] = [];
    for (let v = from, s = 0; v !== to && s < depth; v += step) {
        s = Math.min(depth, s + segment(line, Math.min(v, v + step)));
        places.push([s, along(line, from, to, s).z - z]);
    }
    const rise = Math.min(...places.map(([s, dz]) => dz / s)), lift = Math.max(0, ...places.map(([s, dz]) => dz - rise * s - WALL_M));
    const end = places[places.length - 1][0], portal = line.abut![step > 0 ? 1 : 0];
    return [portal ? end : depth, boreHalf(line) + widen + WALL_M / 2, z + line.deck! + WALL_M + lift, z + LIFT_M, rise];
}

/** The outline of a bore from its left foot over the arch to its right foot. */
function outline(half: number, height: number): Corner[] {
    const spring = SPRING * height, arch: Corner[] = [];
    for (let k = 0; k <= ARCH; k++) {
        const a = Math.PI * (1 - k / ARCH);
        arch.push([half * Math.cos(a), spring + (height - spring) * Math.sin(a)]);
    }
    return arch;
}

/** The station `s` metres along a line from vertex `from` toward vertex `to`, across to the left of that way. */
function along(line: Line, from: number, to: number, s: number): Station {
    const step = Math.sign(to - from);
    let v = from, rest = s;
    for (;;) {
        const w = v + step, dx = line.xy[2 * w] - line.xy[2 * v], dy = line.xy[2 * w + 1] - line.xy[2 * v + 1];
        const unit = metresPerUnit(line.xy[2 * v + 1]), length = Math.hypot(dx, dy) * unit;
        if (rest <= length || w === to) {
            const u = Math.min(1, rest / (length || 1)), l = Math.hypot(dx, dy) || 1;
            // Left of the way into the tunnel.
            return { x: line.xy[2 * v] + dx * u, y: line.xy[2 * v + 1] + dy * u, z: line.z[v] + (line.z[w] - line.z[v]) * u, ax: dy / l / unit, ay: -dx / l / unit };
        }
        rest -= length;
        v = w;
    }
}

export function tunnels(b: MeshBuilder, lines: Line[], owns: Owns): void {
    const twin = twins(lines);
    for (const line of lines) {
        if (line.kind !== TUNNEL || !line.abut) continue;
        const n = line.z.length, road = line.deck! > 3, size = road ? ROAD_PORTAL : PATH_PORTAL;
        const at = stationsOf(line);
        for (let v = 0; v + 1 < n; v++) {
            if (!owns(line.xy[2 * v], line.xy[2 * v + 1])) continue;
            bore(b, at(v), at(v + 1), line, boreHalf(line), road, segment(line, v));
        }
        for (const [end, from, to] of [[0, 0, n - 1], [1, n - 1, 0]]) {
            const x = line.xy[2 * from], y = line.xy[2 * from + 1];
            const pair = twin.get(line)?.[end];
            if (!line.abut[end] || !owns(x, y) || pair === null) continue;
            portal(b, line, from, to, size, road, pair);
        }
    }
}

function portal(b: MeshBuilder, line: Line, from: number, to: number, size: typeof ROAD_PORTAL, road: boolean, pair = { shift: 0, widen: 0 }): void {
    const at = (s: number) => {
        const st = along(line, from, to, s);
        return { ...st, x: st.x + st.ax * pair.shift, y: st.y + st.ay * pair.shift };
    };
    const front = at(0), face = at(FACADE_M), inside = at(VOID_M), back = at(boxDepth(line.xy[2 * from + 1]));
    const half = boreHalf(line) + pair.widen, height = line.deck!, spring = SPRING * height, wing = half + size.wings, top = height + WALL_M;
    const facade = road ? FACADE : ROCK_FACADE, body = road ? BOX : ROCK_BOX, arch = outline(half, height);
    // Out of the tunnel, east, north and up.
    const ex = front.x - inside.x, ey = front.y - inside.y, el = Math.hypot(ex, ey) || 1, away: [number, number, number] = [ex / el, -ey / el, 0];
    const quad = (s: Station, corners: Corner[], colour: number, dir: [number, number, number]) => b.quad(corners.map(c => b.at(s, c, colour)) as [number, number, number, number], dir);
    // The facade, out and in: the wings beside the opening and the wall over the arch; then its top and its ends.
    for (const [st, dir] of [[front, away], [face, [-away[0], -away[1], 0]]] as [Station, [number, number, number]][]) {
        for (const side of [1, -1]) quad(st, [[side * half, -SINK_M], [side * wing, -SINK_M], [side * wing, top], [side * half, top]], facade, dir);
        for (let k = 0; k < ARCH; k++) quad(st, [arch[k], arch[k + 1], [arch[k + 1][0], top], [arch[k][0], top]], facade, dir);
    }
    b.strip(front, face, [-wing, top], [wing, top], facade, [0, 1]);
    for (const side of [1, -1]) b.strip(front, face, [side * wing, top], [side * wing, -SINK_M], facade, [side, 0]);
    // The void: a fan over the opening's outline, facing out.
    const centre = b.at(inside, [0, spring / 2], DARK);
    const rim = [[half, 0] as Corner, ...arch.slice().reverse(), [-half, 0] as Corner].map(c => b.at(inside, c, DARK));
    rim.forEach((v, k) => b.triangle(centre, v, rim[(k + 1) % rim.length], away));
    // The box of the bore behind the facade: its walls and its roof. The bore runs on through it.
    const outer = half + WALL_M;
    for (const side of [1, -1]) b.strip(face, back, [side * outer, -SINK_M], [side * outer, top], body, [side, 0]);
    b.strip(face, back, [-outer, top], [outer, top], body, [0, 1]);
}

/** The bore from station s to t, `length` metres long, `half` metres wide on each side, seen from inside. */
function bore(b: MeshBuilder, s: Station, t: Station, line: Line, half: number, road: boolean, length: number): void {
    const h = line.h, height = line.deck!, spring = SPRING * height, arch = outline(half, height);
    if (road) {
        const paint = rgb('#000000', PAINT | BORE) | line.code;
        b.quad([b.at(s, [-h, LIFT_M], paint, -h, h), b.at(s, [h, LIFT_M], paint, h, h), b.at(t, [h, LIFT_M], paint, h, h), b.at(t, [-h, LIFT_M], paint, -h, h)], b.outward(s, t, [0, 1]));
        for (const side of [1, -1]) {
            b.strip(s, t, [side * h, LIFT_M], [side * h, KERB_M], WALK, [-side, 0]);
            b.strip(s, t, [side * h, KERB_M], [side * half, KERB_M], WALK, [0, 1]);
        }
    } else {
        b.strip(s, t, [half, LIFT_M], [-half, LIFT_M], ROCK_FLOOR, [0, 1]);
    }
    // A track's bore is lined in concrete and dark.
    const track = line.code >= RAIL_CODE, wall = track ? LINING : road ? TILES : ROCK, vault = track ? LINING : road ? ARCH_LINING : ROCK;
    for (const side of [1, -1]) b.strip(s, t, [side * half, 0], [side * half, spring], wall, [-side, 0]);
    for (let k = 0; k < ARCH; k++) b.strip(s, t, arch[k], arch[k + 1], vault, [-(arch[k][0] + arch[k + 1][0]), -(arch[k][1] + arch[k + 1][1] - 2 * spring)]);
    if (!road || track) return;
    // A lamp under the crown about every LAMPS_M.
    posts(s, t, length, (p, q) => b.quad([b.at(p, [-0.15, height - 0.05], LIGHT), b.at(p, [0.15, height - 0.05], LIGHT), b.at(q, [0.15, height - 0.05], LIGHT), b.at(q, [-0.15, height - 0.05], LIGHT)], b.outward(p, q, [0, -1])), LAMP_M, LAMPS_M);
}
