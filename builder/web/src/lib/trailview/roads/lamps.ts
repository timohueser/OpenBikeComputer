// Street lamps from the stations of a lit road: where the corridor draws its sidewalk or behind its shoulder.
import { BRIDGE, cores, insideRings, isCore, junctions, linesNear, metresPerUnit, PATH, ringsDistance, ROAD, TUNNEL, walkOf, type Line, type Shapes } from '../terrain/bins';
import { halfAt, partnersOf } from '../terrain/corners';
import { add, along, BUILT_M, builtNear, gridOf, LEFT, near, RIGHT, townOf, type Grid, type Town } from '../terrain/town';
import { LANTERN, streetLamp } from '../models/amenities';
import { unitModel, type Build, type Kind, type Placement } from '../kinds/kind';
import { classOf, shoulderOf } from './section';
import { stationAt, stationsOf, type Station } from './stations';

/**
 * A lamp stands SPACING_M from the next, moved up to SEARCH_M along to a free place: on the sidewalk at
 * SIDEWALK_SHARE of its width, else BEHIND_M past the shoulder; CLEAR_M from a building's walls,
 * OFF_ROAD_M from another carriageway, MOUTH_M past the edge of a way that joins,
 * and APART_M from every other lamp of its tile: each way starts its own count of SPACING_M.
 */
const SPACING_M = 32, SEARCH_M = 10, SIDEWALK_SHARE = 0.72, BEHIND_M = 0.5, CLEAR_M = 1.5, OFF_ROAD_M = 0.4, MOUTH_M = 4, APART_M = SPACING_M / 2;

/** The roads that have lamps, and how far from the eye a lamp draws. */
const LIT = new Set(['trunk', 'trunk_link', 'primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link', 'unclassified', 'residential', 'living_street']);

const LAMP_REACH_M = 600;
const lamp = unitModel(streetLamp, LAMP_REACH_M);

/** The kind of the lit roads. `streetLamps` places their lamps, from the road's stations. */
export const lampKind: Kind = {
    build() {},
    models: { lamp: lamp.spec },
    lights: { lamp: { colour: '#ffc98a', radius: 16, at: LANTERN, housed: true } },
};

/** The lamps of each tile, made once: the stations of its lit roads are many, its lamps few. */
const planned = new WeakMap<Shapes, Placement[]>();

/** Points in a grid, in their order: a point that comes later takes its place with `add`. */
interface Spots<T extends { x: number; y: number }> { points: T[]; grid: Grid }

const spotsOf = <T extends { x: number; y: number }>(points: T[]): Spots<T> => ({ points, grid: gridOf(points.map(({ x, y }) => [x, y, x, y])) });

/** Whether `hit` holds for a point that lies within `metres` of world point x, y: `hit` takes no point farther off. */
function within<T extends { x: number; y: number }>({ points, grid }: Spots<T>, x: number, y: number, metres: number, unit: number, hit: (point: T) => boolean): boolean {
    let found = false;
    // One unit more for the rounding of a distance.
    near(grid, x, y, x, y, metres / unit + 1, k => (found ||= hit(points[k])));
    return found;
}

/**
 * What decides the place of a lamp: the tile's shapes, the widest half-width of its carriageways (`half`), its junctions
 * and the ends of its decks and bores with the widest among them (`wide`), and the lamps that stand.
 */
interface Places { out: Pick<Build, 'shapes' | 'cornered'>; town: Town; half: number; mouths: Spots<{ x: number; y: number; h: number }>; wide: number; lamps: Spots<Placement> }

/** Whether a point lies within OFF_ROAD_M of a carriageway other than `own`. */
function inTraffic(shapes: Shapes, half: number, own: Line, x: number, y: number, unit: number): boolean {
    return linesNear(shapes, x, y, (half + OFF_ROAD_M) / unit).some(k => {
        const line = shapes.lines[k];
        if (line === own || !isCore(line)) return false;
        const reach = (line.h + OFF_ROAD_M) / unit, { xy } = line;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const ax = xy[2 * v], ay = xy[2 * v + 1], bx = xy[2 * v + 2], by = xy[2 * v + 3];
            if (Math.min(ax, bx) > x + reach || Math.max(ax, bx) < x - reach || Math.min(ay, by) > y + reach || Math.max(ay, by) < y - reach) continue;
            const t = along(ax, ay, bx, by, x, y);
            if (Math.hypot(ax + t * (bx - ax) - x, ay + t * (by - ay) - y) < reach) return true;
        }
        return false;
    });
}

/** Whether a lamp may stand at world point x, y beside `line`, whose sidewalk is `walk` metres wide. */
function free({ out, town, half, mouths, wide, lamps }: Places, line: Line, walk: number, x: number, y: number, unit: number): boolean {
    if (out.cornered(x, y) || within(mouths, x, y, wide + walk + MOUTH_M, unit, j => Math.hypot(j.x - x, j.y - y) * unit < j.h + walk + MOUTH_M)) return false;
    if (within(lamps, x, y, APART_M, unit, lamp => Math.hypot(lamp.x - x, lamp.y - y) * unit < APART_M)) return false;
    if (inTraffic(out.shapes, half, line, x, y, unit)) return false;
    const { buildings } = out.shapes;
    let walled = false;
    near(town, x, y, x, y, CLEAR_M / unit, b => (walled ||= insideRings(buildings[b].rings, x, y) || ringsDistance(buildings[b].rings, x, y) * unit < CLEAR_M));
    return !walled;
}

/** The place of a lamp `metres` along stations `st` of `line`, on its first free side, or undefined. */
function spot(places: Places, line: Line, st: Station[], metres: number): Placement | undefined {
    if (metres < 0 || metres > st[st.length - 1].along) return undefined;
    const s = stationAt(st, metres), unit = metresPerUnit(s.y), walk = walkOf(line);
    for (const side of [-1, 1]) {
        const walked = (line.sidewalks?.[s.v] ?? 0) & (side > 0 ? LEFT : RIGHT), a = s.h + (walked ? walk * SIDEWALK_SHARE : shoulderOf(line) + BEHIND_M);
        const x = s.x + side * s.ax * a, y = s.y + side * s.ay * a;
        if ((!walked && !builtNear(places.town, s.x, s.y, BUILT_M / unit)) || !free(places, line, walk, x, y, unit)) continue;
        // The arm reaches toward −y of the model: over the road, toward the line's middle.
        return { x, y, yaw: Math.atan2(-side * s.ax, -side * s.ay), height: lamp.height, model: 'lamp' };
    }
    return undefined;
}

/** The lamps of a tile's lit roads. */
function lampsOf(out: Pick<Build, 'shapes' | 'cornered'>): Placement[] {
    const { lines, buildings } = out.shapes;
    const partners = partnersOf(lines), ways = cores(lines), roads = lines.filter(line => line.kind === ROAD && !line.gallery && LIT.has(classOf(line)) && line.z.length > 1);
    // A road can run straight into a bridge or a tunnel, a node of two ways: each end of one is a mouth.
    const ends = lines.filter(({ kind }) => kind === BRIDGE || kind === TUNNEL).flatMap(({ xy, h }) => [{ x: xy[0], y: xy[1], h }, { x: xy[xy.length - 2], y: xy[xy.length - 1], h }]);
    const joins = [...junctions([...ways, ...lines.filter(({ kind }) => kind === PATH)]).values()].filter(j => j.ways > 2), mouths = [...joins, ...ends], lamps: Placement[] = [];
    const widest = (list: { h: number }[]) => list.reduce((most, { h }) => Math.max(most, h), 0);
    const places: Places = { out, town: townOf(buildings), half: widest(ways), mouths: spotsOf(mouths), wide: widest(mouths), lamps: spotsOf(lamps) };
    for (const line of roads) {
        const st = stationsOf(line, v => halfAt(line, v, partners)), total = st[st.length - 1].along;
        for (let metres = SPACING_M / 2; metres < total; ) {
            let placed: Placement | undefined;
            for (let d = 0; d <= SEARCH_M && !placed; d += 2) {
                for (const m of d ? [metres + d, metres - d] : [metres]) {
                    placed = spot(places, line, st, m);
                    if (placed) {
                        lamps.push(placed);
                        add(places.lamps.grid, [placed.x, placed.y, placed.x, placed.y]);
                        metres = m;
                        break;
                    }
                }
            }
            metres += SPACING_M;
        }
    }
    return lamps;
}

/** Stands the lamps of a tile that a node owns. */
export function streetLamps(out: Pick<Build, 'owns' | 'place' | 'shapes' | 'cornered'>): void {
    const lamps = planned.get(out.shapes) ?? planned.set(out.shapes, lampsOf(out)).get(out.shapes)!;
    for (const lamp of lamps) if (out.owns(lamp.x, lamp.y)) out.place(lamp);
}
