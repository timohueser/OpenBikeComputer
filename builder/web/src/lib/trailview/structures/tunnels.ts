// The tunnels of a node: the bore along each tunnel line, with its road, its walls, its arch and its
// lamps; and where a tunnel meets the ground, a portal: a facade with an arched opening across the road
// and a box of the bore into the hill. The stamps hold the road on the ground into the portal and the
// hill over the bore behind it, and the terrain draws no ground in the box: the box hides the slope
// between. Beyond the reach of the masks the terrain has no hole, and a dark void fills the opening.
import { metresPerUnit, TUNNEL, type Line } from '../terrain/bins';
import { INSIDE, LAMP, MeshBuilder, PAINT, posts, rgb, station, VOID, type Corner, type Station } from './mesh';
import type { Owns } from './node';

/** A portal of a road and of a path: the bore's margin past the road, the depth of the box, the facade's wings past the opening and its height over the bore. */
const ROAD_PORTAL = { margin: 0.6, depth: 12, wings: 3, crown: 1.5 }, PATH_PORTAL = { margin: 0.3, depth: 6, wings: 1.2, crown: 0.8 };
/** A road's bore: the kerb's height, and its lamps' length and spacing. */
const KERB_M = 0.15, LAMP_M = 1.2, LAMPS_M = 10;
/** The arch springs at this share of the bore's height; the void stands this far in. The bore's road lies this far over the ground that runs into the portal. */
const SPRING = 0.6, VOID_M = 0.7, LIFT_M = 0.03;
/** The facade's thickness, the box's walls and roof, and how deep the facade stands in the ground. */
const FACADE_M = 0.6, WALL_M = 0.6, SINK_M = 1;
/** Segments of the arch. */
const ARCH = 8;

const FACADE = rgb('#a5a299'), BOX = rgb('#8f8c84'), DARK = rgb('#0a0a0b', VOID);
const ROCK_FACADE = rgb('#77716a'), ROCK_BOX = rgb('#6a655e');
const TILES = rgb('#dcdad3', INSIDE), ARCH_LINING = rgb('#7f7e7a', INSIDE), WALK = rgb('#a3a19b', INSIDE), LIGHT = rgb('#fff3d6', LAMP);
const ROCK = rgb('#6f6a62', INSIDE), ROCK_FLOOR = rgb('#7d7262', INSIDE);

/** The half-width of a tunnel's bore, to its walls. */
export const boreHalf = (line: Line) => line.h + (line.deck! > 3 ? ROAD_PORTAL : PATH_PORTAL).margin;

/**
 * The box of a tunnel's portal at vertex `from` toward vertex `to`, in which the terrain draws nothing:
 * its depth, its half-width to the middle of its walls, the height of its roof's top at the facade and
 * the rise of that roof per metre into the tunnel.
 */
export function portalBox(line: Line, from: number, to: number): [number, number, number, number] {
    const depth = (line.deck! > 3 ? ROAD_PORTAL : PATH_PORTAL).depth, z = line.z[from];
    return [depth, boreHalf(line) + WALL_M / 2, z + line.deck! + WALL_M, (along(line, from, to, depth).z - z) / depth];
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
    for (const line of lines) {
        if (line.kind !== TUNNEL || !line.abut) continue;
        const n = line.z.length, road = line.deck! > 3, size = road ? ROAD_PORTAL : PATH_PORTAL;
        const stations = new Map<number, Station>(), at = (v: number) => stations.get(v) ?? stations.set(v, station(line, v)).get(v)!;
        for (let v = 0; v + 1 < n; v++) {
            if (!owns(line.xy[2 * v], line.xy[2 * v + 1])) continue;
            const length = Math.hypot(line.xy[2 * v + 2] - line.xy[2 * v], line.xy[2 * v + 3] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]);
            bore(b, at(v), at(v + 1), line, boreHalf(line), road, length);
        }
        for (const [end, from, to] of [[0, 0, n - 1], [1, n - 1, 0]]) {
            const x = line.xy[2 * from], y = line.xy[2 * from + 1];
            if (!line.abut[end] || !owns(x, y)) continue;
            portal(b, line, from, to, size, road);
        }
    }
}

function portal(b: MeshBuilder, line: Line, from: number, to: number, size: typeof ROAD_PORTAL, road: boolean): void {
    const at = (s: number) => along(line, from, to, s);
    const front = at(0), face = at(FACADE_M), inside = at(VOID_M), back = at(size.depth);
    const half = boreHalf(line), height = line.deck!, spring = SPRING * height, wing = half + size.wings, top = height + size.crown;
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
    const outer = half + WALL_M, roof = height + WALL_M;
    for (const side of [1, -1]) b.strip(face, back, [side * outer, -SINK_M], [side * outer, roof], body, [side, 0]);
    b.strip(face, back, [-outer, roof], [outer, roof], body, [0, 1]);
}

/** The bore from station s to t, `length` metres long, `half` metres wide on each side, seen from inside. */
function bore(b: MeshBuilder, s: Station, t: Station, line: Line, half: number, road: boolean, length: number): void {
    const h = line.h, height = line.deck!, spring = SPRING * height, arch = outline(half, height);
    if (road) {
        const paint = rgb('#000000', PAINT | INSIDE) | line.code;
        b.quad([b.at(s, [-h, LIFT_M], paint, -h, h), b.at(s, [h, LIFT_M], paint, h, h), b.at(t, [h, LIFT_M], paint, h, h), b.at(t, [-h, LIFT_M], paint, -h, h)], b.outward(s, t, [0, 1]));
        for (const side of [1, -1]) {
            b.strip(s, t, [side * h, LIFT_M], [side * h, KERB_M], WALK, [-side, 0]);
            b.strip(s, t, [side * h, KERB_M], [side * half, KERB_M], WALK, [0, 1]);
        }
    } else {
        b.strip(s, t, [half, LIFT_M], [-half, LIFT_M], ROCK_FLOOR, [0, 1]);
    }
    const wall = road ? TILES : ROCK, vault = road ? ARCH_LINING : ROCK;
    for (const side of [1, -1]) b.strip(s, t, [side * half, 0], [side * half, spring], wall, [-side, 0]);
    for (let k = 0; k < ARCH; k++) b.strip(s, t, arch[k], arch[k + 1], vault, [-(arch[k][0] + arch[k + 1][0]), -(arch[k][1] + arch[k + 1][1] - 2 * spring)]);
    if (!road) return;
    // A lamp under the crown about every LAMPS_M.
    posts(s, t, length, (p, q) => b.quad([b.at(p, [-0.15, height - 0.05], LIGHT), b.at(p, [0.15, height - 0.05], LIGHT), b.at(q, [0.15, height - 0.05], LIGHT), b.at(q, [-0.15, height - 0.05], LIGHT)], b.outward(p, q, [0, -1])), LAMP_M, LAMPS_M);
}
