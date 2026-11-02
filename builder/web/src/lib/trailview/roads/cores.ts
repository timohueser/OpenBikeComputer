// Where a model at a point stands beside the ways: the cores of a tile's ways as their corridors draw them, and the place out of them.
import { insideRings, junctions, metresPerUnit, PATH, ringsDistance, RIVER, STREAM, type Line, type Shapes } from '../terrain/bins';
import { halfAt, partnersOf } from '../terrain/corners';
import { near, townOf, type Town } from '../terrain/town';
import { direction } from './arms';
import { isDeck } from './decks';
import { CF, corridorLine, sideOf, type Part } from './section';
import { between, stationsOf, type Station } from './stations';

/**
 * The ground of a tile that a model at a point keeps out of. `ways`: the roads, tracks, paths and railways
 * with a corridor on the ground; a deck stands over the ground, and the test is flat. `partners` set a
 * road's half-width at its ends. `meetings`: the points where three ways or more meet, decks included.
 * And the buildings and the water.
 */
export interface Cores {
    ways: Line[];
    partners: Map<Line, [Line?, Line?]>;
    meetings: { x: number; y: number; lines: Line[] }[];
    /** The points that two ways or more share, where a junction's corner can lie. */
    joints: { x: number; y: number }[];
    buildings: Shapes['buildings'];
    town: Town;
    water: { rings: number[][][]; lines: Line[] };
}

/**
 * How a model at a point keeps out of the ways: `half`, the metres that it reaches from its foot across the
 * way, `radius`, the most that it reaches any way (`half` without it), and whether it stands in a junction's
 * `corner`, as a guidepost does.
 */
export interface Stand { half: number; radius?: number; corner?: boolean }

/**
 * The way beside a model: the core's edge `out` metres from its foot, `turn` radians counter-clockwise from
 * east, whether the way is a path, whose band has its own rows, and whether the model stands on the band's
 * rows on the stamped ground or past them.
 */
export interface Beside { out: number; turn: number; path: boolean; band: boolean }

const coresByTile = new WeakMap<Shapes, Cores>(), stationsByLine = new WeakMap<Line, Station[]>();

/** The cores of a tile's shapes, made once per tile. */
export function coresOf(shapes: Shapes): Cores {
    let cores = coresByTile.get(shapes);
    if (cores) return cores;
    const { lines, buildings } = shapes, ways = lines.filter(line => corridorLine(line) && !isDeck(line) && line.z.length > 1);
    const meet = [...ways, ...lines.filter(isDeck)], at = new Map<string, Line[]>();
    for (const line of meet) line.z.forEach((_, v) => {
        const key = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
        (at.get(key) ?? at.set(key, []).get(key)!).push(line);
    });
    const meetings = [...junctions(meet)].filter(([, j]) => j.ways >= 3).map(([key, { x, y }]) => ({ x, y, lines: [...new Set(at.get(key))] }));
    const water = { rings: [...shapes.lakes, ...shapes.seas, ...shapes.areas].map(({ rings }) => rings), lines: lines.filter(({ kind }) => kind === STREAM || kind === RIVER) };
    const joints = [...at].filter(([, here]) => new Set(here).size > 1).map(([key]) => key.split(',').map(Number)).map(([x, y]) => ({ x, y }));
    coresByTile.set(shapes, (cores = { ways, partners: partnersOf(lines), meetings, joints, buildings, town: townOf(buildings), water }));
    return cores;
}

/** The stations of a way, as its corridor sweeps them. */
function stationsIn(cores: Cores, line: Line): Station[] {
    return stationsByLine.get(line) ?? stationsByLine.set(line, stationsOf(line, v => halfAt(line, v, cores.partners))).get(line)!;
}

/** A way's side reaches at most this many metres past its half-width: a sidewalk or a shoulder, then its band. */
const PAST_HALF_M = 10;
/** A guidepost inside a core this near to a meeting, past the widest core there, takes the junction's widest gap. */
const JUNCTION_M = 2;
/** A model keeps this much room to a building's walls and to a stream's water. */
const WALL_M = 0.5, WATER_M = 0.3;
/** The search out of the cores: its step, and the farthest a point moves. */
const STEP_M = 0.1, FARTHEST_M = 30;

/** Of `ways`, those whose side may reach within `metres` of world point x, y. */
function waysNear(ways: Line[], x: number, y: number, metres: number): Line[] {
    const unit = metresPerUnit(y);
    return ways.filter(({ xy, h }) => {
        const r = (h + PAST_HALF_M + metres) / unit;
        for (let i = 0; i + 3 < xy.length; i += 2) {
            if (Math.min(xy[i], xy[i + 2]) - r < x && Math.max(xy[i], xy[i + 2]) + r > x && Math.min(xy[i + 1], xy[i + 3]) - r < y && Math.max(xy[i + 1], xy[i + 3]) + r > y) return true;
        }
        return false;
    });
}

/**
 * The side of a way, 1 left and -1 right, on the step to station t at `foot`: the step lies on t's segment, as
 * sweep.ts draws it, because a vertex's station holds the segment into it.
 */
export const stepSide = (line: Line, t: Station, foot: Station, side: 1 | -1) => sideOf(line, side, t.v, foot.h);

/** How far world point x, y lies ahead of station s's cross-section, along a step dx, dy; in world units, for its sign. */
function beyond(s: Station, dx: number, dy: number, x: number, y: number): number {
    // The station's way ahead: square to its cross-section (ax, ay), on the step's side.
    const sign = dx * -s.ay + dy * s.ax >= 0 ? 1 : -1;
    return sign * ((x - s.x) * -s.ay + (y - s.y) * s.ax);
}

/** A point's foot on a way: its station, its side (1 left, -1 right), its metres from the line, and that side's points and core's edge. */
export interface Foot { line: Line; foot: Station; side: 1 | -1; metres: number; parts: Part[]; edge: number }

/**
 * Of `ways`, the side that holds world point x, y most deeply, out to `reach` metres from the line, as the
 * corridors draw it: each step between two stations with its own width, and a round cap at each end.
 */
function deepest(cores: Cores, ways: Line[], x: number, y: number, reach: (parts: Part[], edge: number) => number): Foot | undefined {
    const unit = metresPerUnit(y);
    let most = 0, found: Foot | undefined;
    for (const line of ways) {
        const st = stationsIn(cores, line);
        for (let k = 0; k + 1 < st.length; k++) {
            const s = st[k], t = st[k + 1], dx = t.x - s.x, dy = t.y - s.y, l = dx * dx + dy * dy;
            if (!l) continue;
            // A step's faces lie between the cross-sections of its stations, so steps meet with no wedge in a
            // bend. A way's end has a round cap (mesh.ts), or a joint where another way goes on.
            if ((beyond(s, dx, dy, x, y) < 0 && k > 0) || (beyond(t, dx, dy, x, y) > 0 && k + 2 < st.length)) continue;
            const u = Math.min(1, Math.max(0, ((x - s.x) * dx + (y - s.y) * dy) / l));
            const metres = Math.hypot(s.x + u * dx - x, s.y + u * dy - y) * unit;
            if (metres - s.h > PAST_HALF_M) continue;
            const foot = between(s, st[k + 1], u), side = (x - foot.x) * foot.ax + (y - foot.y) * foot.ay >= 0 ? 1 : -1;
            const { parts, edge } = stepSide(line, t, foot, side), depth = reach(parts, edge) - metres;
            if (depth > most) [most, found] = [depth, { line, foot, side, metres, parts, edge }];
        }
    }
    return found;
}

/** The core of the ways that holds world point x, y most deeply, widened by `past` metres; undefined outside every core. */
export function coreAt(cores: Cores, x: number, y: number, past = 0, ways = cores.ways): Foot | undefined {
    return deepest(cores, ways, x, y, (_, edge) => edge + past);
}

/** The way whose band reaches world point x, y past its core, out to the row that sinks under the mesh. */
function besideAt(cores: Cores, x: number, y: number, ways: Line[]): Beside | undefined {
    const hit = deepest(cores, ways, x, y, parts => parts[parts.length - 1].a);
    if (!hit || hit.metres < hit.edge) return undefined;
    const { line, foot, side, metres, parts, edge } = hit, sampled = Math.max(edge, ...parts.filter(({ flags }) => flags === CF.SAMPLE).map(({ a }) => a));
    // Toward the line, in metres east and north.
    return { out: metres - edge, turn: Math.atan2(side * foot.ay, -side * foot.ax), path: line.kind === PATH, band: metres <= sampled };
}

/** The metres from world point x, y to an open line. */
function lineMetres({ xy }: Line, x: number, y: number): number {
    let best = Infinity;
    for (let i = 0; i + 3 < xy.length; i += 2) {
        const dx = xy[i + 2] - xy[i], dy = xy[i + 3] - xy[i + 1], t = Math.min(1, Math.max(0, ((x - xy[i]) * dx + (y - xy[i + 1]) * dy) / (dx * dx + dy * dy || 1)));
        best = Math.min(best, Math.hypot(x - xy[i] - t * dx, y - xy[i + 1] - t * dy));
    }
    return best * metresPerUnit(y);
}

/** Whether the box of a ring comes within `reach` world units of world point x, y. */
function boxNear(ring: number[], x: number, y: number, reach: number): boolean {
    const xs = ring.filter((_, i) => i % 2 === 0), ys = ring.filter((_, i) => i % 2 === 1);
    return Math.min(...xs) - reach < x && Math.max(...xs) + reach > x && Math.min(...ys) - reach < y && Math.max(...ys) + reach > y;
}

/** What lies near a point that a model keeps out of: the ways, and the water. */
interface Near { ways: Line[]; rings: number[][][]; streams: Line[] }

/**
 * A model keeps out to the band's first row: from the core's edge to there the corridor's face falls from a
 * kerb or a shoulder to the stamped ground.
 */
const firstRow = (parts: Part[], edge: number) => parts.find(({ flags }) => flags === CF.SAMPLE)?.a ?? edge;

/** What a model meets at a world point: nothing, a way's core out to its first row or a junction's corner, which it may pass, or water or a building, which it may not. */
type Ground = 'free' | 'way' | 'blocked';

function groundAt(cores: Cores, { ways, rings, streams }: Near, cornered: (x: number, y: number) => boolean, x: number, y: number): Ground {
    const unit = metresPerUnit(y);
    if (rings.some(ring => insideRings(ring, x, y)) || streams.some(line => lineMetres(line, x, y) < line.h + WATER_M)) return 'blocked';
    let walled = false;
    near(cores.town, x, y, x, y, WALL_M / unit, b => (walled ||= insideRings(cores.buildings[b].rings, x, y) || ringsDistance(cores.buildings[b].rings, x, y) * unit < WALL_M));
    if (walled) return 'blocked';
    return deepest(cores, ways, x, y, firstRow) || cornered(x, y) ? 'way' : 'free';
}

/** A way out of the cores: from where and which way, in world units per metre, and the front (east, south) of the model at its end. */
interface Leave { from: [number, number]; way: [number, number]; front: [number, number] }

/** A model placed before: its foot in world units and the metres that it reaches from it. */
export type Taken = [number, number, number];

/**
 * The first place along `leave` where a model with `half` metres of room each way meets no way and no model
 * placed before; undefined where water or a building comes first.
 */
function placeAlong(cores: Cores, nearby: Near, cornered: (x: number, y: number) => boolean, { from, way }: Leave, { half, radius = half }: Stand, taken: Taken[]): [number, number] | undefined {
    for (let d = half; d < FARTHEST_M; d += STEP_M) {
        const grounds = [-half, 0, half].map(k => groundAt(cores, nearby, cornered, from[0] + way[0] * (d + k), from[1] + way[1] * (d + k)));
        if (grounds.includes('blocked')) return undefined;
        const x = from[0] + way[0] * d, y = from[1] + way[1] * d, unit = metresPerUnit(y);
        if (grounds.every(ground => ground === 'free') && !taken.some(([tx, ty, reach]) => Math.hypot(tx - x, ty - y) * unit < reach + radius)) return [x, y];
    }
    return undefined;
}

/**
 * World point x, y moved out of the cores of the ways, out to their band's first row, and out of the
 * junctions' corners, with `half` metres of room on each side along the way that it leaves, and clear of
 * the models in `taken`. It never passes water or a building. A `corner` model near a meeting goes into its
 * widest gap, on the bisector of the widest angle between its ways, and faces the wider of the two ways at
 * the gap. Another model in a core goes to the core's nearer edge, else to its far edge, and faces the way;
 * one in a corner only goes away from the corner's shared point and faces it. A point in no core stays, as
 * does one with no free place. `yaw` turns the model's front (−y) toward its way; `beside` is the way whose
 * band reaches it.
 */
export function besideWays(cores: Cores, cornered: (x: number, y: number) => boolean, x: number, y: number, stand: Stand, taken: Taken[] = []): { x: number; y: number; yaw?: number; beside?: Beside } {
    const { corner = false } = stand;
    const ways = waysNear(cores.ways, x, y, FARTHEST_M), unit = metresPerUnit(y), core = deepest(cores, ways, x, y, firstRow);
    const stay = { x, y, beside: besideAt(cores, x, y, ways) };
    if (!core && !cornered(x, y)) return stay;
    const meeting = nearest(cores.meetings, x, y), leaves: Leave[] = [];
    const away = (way: [number, number]): Leave => ({ from: [x, y], way, front: [-way[0], -way[1]] });
    if (corner && meeting && Math.hypot(meeting.x - x, meeting.y - y) * unit < widest(cores, meeting) + JUNCTION_M) {
        const gap = widestGap(meeting, unit);
        if (gap) leaves.push({ from: [meeting.x, meeting.y], way: gap.bisector, front: gap.front });
    }
    if (core) {
        for (const side of [core.side, -core.side]) leaves.push(away([side * core.foot.ax, side * core.foot.ay]));
    } else {
        const joint = nearest(cores.joints, x, y);
        if (joint && (joint.x !== x || joint.y !== y)) {
            const l = Math.hypot(x - joint.x, y - joint.y) * unit;
            leaves.push(away([(x - joint.x) / l, (y - joint.y) / l]));
        }
    }
    const reach = FARTHEST_M / unit, nearby: Near = {
        ways, streams: waysNear(cores.water.lines, x, y, FARTHEST_M),
        rings: cores.water.rings.filter(rings => rings.some(ring => boxNear(ring, x, y, reach))),
    };
    for (const leave of leaves) {
        const at = placeAlong(cores, nearby, cornered, leave, stand, taken);
        if (at) return { x: at[0], y: at[1], yaw: Math.atan2(leave.front[0], leave.front[1]), beside: besideAt(cores, at[0], at[1], ways) };
    }
    return stay;
}

/** Of `points`, the one nearest to world point x, y. */
function nearest<T extends { x: number; y: number }>(points: T[], x: number, y: number): T | undefined {
    let best = Infinity, found: T | undefined;
    for (const j of points) {
        const d = (j.x - x) ** 2 + (j.y - y) ** 2;
        if (d < best) [best, found] = [d, j];
    }
    return found;
}

/** The widest core of the ways at a meeting, in metres from their middle lines. A deck has none here: the test is flat. */
function widest(cores: Cores, { x, y, lines }: Cores['meetings'][number]): number {
    let most = 0;
    for (const line of lines) {
        if (isDeck(line)) continue;
        const s = stationsIn(cores, line).reduce((p, q) => (Math.hypot(q.x - x, q.y - y) < Math.hypot(p.x - x, p.y - y) ? q : p));
        most = Math.max(most, sideOf(line, 1, s.v, s.h).edge, sideOf(line, -1, s.v, s.h).edge);
    }
    return most;
}

/**
 * The widest gap between the ways out of a meeting: its bisector in world units per metre, and the front
 * (east, south) of a model in it that faces the wider of the two ways at the gap, its +x along that way.
 */
export function widestGap({ x, y, lines }: Cores['meetings'][number], unit: number): { bisector: [number, number]; front: [number, number] } | undefined {
    const arms = lines.flatMap(line => line.z.map((_, v) => v).filter(v => line.xy[2 * v] === x && line.xy[2 * v + 1] === y).flatMap(v =>
        ([-1, 1] as const).filter(step => v + step >= 0 && v + step < line.z.length).map(step => {
            const [east, south] = direction(line, v, step, unit);
            return { angle: Math.atan2(south, east), h: line.h };
        }))).sort((a, b) => a.angle - b.angle);
    if (arms.length < 2) return undefined;
    let gap = -1, at = 0;
    arms.forEach((arm, i) => {
        const next = i + 1 < arms.length ? arms[i + 1].angle : arms[0].angle + 2 * Math.PI;
        if (next - arm.angle > gap) [gap, at] = [next - arm.angle, i];
    });
    const a = arms[at], b = arms[(at + 1) % arms.length], middle = a.angle + gap / 2, along = b.h > a.h ? b.angle : a.angle;
    // The front is square to the wider way, on the side toward the meeting.
    const side = Math.sin(middle - along) > 0 ? 1 : -1, faces = along - side * Math.PI / 2;
    return { bisector: [Math.cos(middle) / unit, Math.sin(middle) / unit], front: [Math.cos(faces), Math.sin(faces)] };
}
