// The portals of a node: where a tunnel meets the ground, a facade with an arched opening across the
// road, a dark void behind it, and a short box of the bore into the hill. The stamps hold the road on
// the ground into the portal and the hill over the bore behind it; the box hides the slope between.
import { metresPerUnit, TUNNEL, type Line } from '../terrain/bins';
import { MeshBuilder, rgb, VOID, type Corner, type Station } from './mesh';

/** A portal of a road and of a path: the opening's margin past the road, the depth of the box, the facade's wings past the opening and its height over the bore. */
const ROAD_PORTAL = { margin: 0.6, depth: 10, wings: 3, crown: 1.5 }, PATH_PORTAL = { margin: 0.3, depth: 6, wings: 1.2, crown: 0.8 };
/** The arch springs at this share of the bore's height; the void stands this far in. */
const SPRING = 0.6, VOID_M = 0.7;
/** The facade's thickness, the box's walls and roof, and how deep the facade stands in the ground. */
const FACADE_M = 0.6, WALL_M = 0.6, SINK_M = 1;
/** Segments of the arch. */
const ARCH = 8;

const FACADE = rgb('#a5a299'), BOX = rgb('#8f8c84'), LINING = rgb('#4b4945'), DARK = rgb('#0a0a0b', VOID);
const ROCK_FACADE = rgb('#77716a'), ROCK_BOX = rgb('#6a655e');

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

export function portals(b: MeshBuilder, lines: Line[], box: [number, number, number, number]): void {
    for (const line of lines) {
        if (line.kind !== TUNNEL || !line.abut) continue;
        const n = line.z.length, road = line.deck! > 3, size = road ? ROAD_PORTAL : PATH_PORTAL;
        for (const [end, from, to] of [[0, 0, n - 1], [1, n - 1, 0]]) {
            const x = line.xy[2 * from], y = line.xy[2 * from + 1];
            if (!line.abut[end] || x < box[0] || x > box[2] || y < box[1] || y > box[3]) continue;
            portal(b, line, from, to, size, road);
        }
    }
}

function portal(b: MeshBuilder, line: Line, from: number, to: number, size: typeof ROAD_PORTAL, road: boolean): void {
    const at = (s: number) => along(line, from, to, s);
    const front = at(0), face = at(FACADE_M), inside = at(VOID_M), back = at(size.depth);
    const half = line.h + size.margin, height = line.deck!, spring = SPRING * height, wing = half + size.wings, top = height + size.crown;
    const facade = road ? FACADE : ROCK_FACADE, body = road ? BOX : ROCK_BOX;
    // The opening's outline from its left foot over the arch to its right foot.
    const arch: Corner[] = [];
    for (let k = 0; k <= ARCH; k++) {
        const a = Math.PI * (1 - k / ARCH);
        arch.push([half * Math.cos(a), spring + (height - spring) * Math.sin(a)]);
    }
    // Out of the tunnel, east, north and up.
    const ex = front.x - inside.x, ey = front.y - inside.y, el = Math.hypot(ex, ey) || 1, away: [number, number, number] = [ex / el, -ey / el, 0];
    const quad = (s: Station, corners: Corner[], colour: number, dir: [number, number, number]) => b.quad(corners.map(c => b.at(s, c, colour)) as [number, number, number, number], dir);
    // The facade: the wings beside the opening, the wall over the arch, its top and its ends.
    for (const side of [1, -1]) quad(front, [[side * half, -SINK_M], [side * wing, -SINK_M], [side * wing, top], [side * half, top]], facade, away);
    for (let k = 0; k < ARCH; k++) quad(front, [arch[k], arch[k + 1], [arch[k + 1][0], top], [arch[k][0], top]], facade, away);
    b.strip(front, face, [-wing, top], [wing, top], facade, [0, 1]);
    for (const side of [1, -1]) b.strip(front, face, [side * wing, top], [side * wing, -SINK_M], facade, [side, 0]);
    b.quad([b.at(face, [-wing, top], facade), b.at(face, [wing, top], facade), b.at(face, [wing, -SINK_M], facade), b.at(face, [-wing, -SINK_M], facade)], [-away[0], -away[1], 0]);
    // The lining between the facade and the void: the walls up to the arch, and the arch.
    for (const side of [1, -1]) b.strip(front, inside, [side * half, 0], [side * half, spring], LINING, [-side, 0]);
    for (let k = 0; k < ARCH; k++) b.strip(front, inside, arch[k], arch[k + 1], LINING, [-(arch[k][0] + arch[k + 1][0]), -(arch[k][1] + arch[k + 1][1] - 2 * spring)]);
    // The void: a fan over the opening's outline, facing out.
    const centre = b.at(inside, [0, spring / 2], DARK);
    const rim = [[half, 0] as Corner, ...arch.slice().reverse(), [-half, 0] as Corner].map(c => b.at(inside, c, DARK));
    for (let k = 0; k + 1 < rim.length; k++) b.triangle(centre, rim[k], rim[k + 1], away);
    // The box of the bore behind the facade: its walls and roof, and its end.
    const outer = half + WALL_M, roof = height + WALL_M;
    for (const side of [1, -1]) b.strip(face, back, [side * outer, -SINK_M], [side * outer, roof], body, [side, 0]);
    b.strip(face, back, [-outer, roof], [outer, roof], body, [0, 1]);
    b.quad([b.at(back, [-outer, roof], body), b.at(back, [outer, roof], body), b.at(back, [outer, -SINK_M], body), b.at(back, [-outer, -SINK_M], body)], [-away[0], -away[1], 0]);
}
